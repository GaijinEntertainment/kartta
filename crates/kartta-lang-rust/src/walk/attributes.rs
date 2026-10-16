use kartta_lang::tree::{named_children, text};
use tree_sitter::Node;

use super::syntax::{last_segment, named_child_of_kind};

// constraint: an item's outer attributes are the preceding sibling `attribute_item` nodes up to the first other item,
// and a comment between them does not break the binding
fn outer_attributes(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(node.prev_sibling(), Node::prev_sibling)
        .filter(|sibling| !matches!(sibling.kind(), "line_comment" | "block_comment"))
        .take_while(|sibling| sibling.kind() == "attribute_item")
}

pub(super) fn opens_test_scope(node: Node<'_>, source: &[u8]) -> bool {
    outer_attributes(node).any(|attribute| attribute_marks_test(attribute, source))
}

pub(super) fn mod_body_marks_test(module: Node<'_>, source: &[u8]) -> bool {
    let Some(body) = module.child_by_field_name("body") else {
        return false;
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .any(|child| child.kind() == "inner_attribute_item" && attribute_marks_test(child, source))
}

// constraint: a test marker is an attribute whose path ends in `test` or `bench`, which covers `#[tokio::test]`, or
// a `cfg` that holds only under `test`
pub(super) fn attribute_marks_test(item: Node<'_>, source: &[u8]) -> bool {
    let Some(attribute) = named_child_of_kind(item, "attribute") else {
        return false;
    };
    let Some(path) = attribute.named_child(0) else {
        return false;
    };
    match last_segment(path, source).as_str() {
        "test" | "bench" => true,
        "cfg" => cfg_gates_test(attribute, source),
        _ => false,
    }
}

// constraint: a `cfg` gates test code when its predicate is false in every configuration without `test` and can
// still hold with it, so `cfg(all(test, not(miri)))` gates, while `cfg(any(test, feature = "x"))`, `cfg(not(test))`
// and the never-true `cfg(any())` do not; a configuration gives each distinct atom one truth however often the
// predicate repeats it. The check tries every assignment of the repeated atoms and leaves each atom written once
// unknown, which tells a constant predicate exactly, because such an atom reaches the result through one path only;
// past `MAX_REPEATED` repeated atoms the rest stay unknown too, which bounds the assignments at 2^`MAX_REPEATED` and
// misses only a gate that holds through their repetition
fn cfg_gates_test(attribute: Node<'_>, source: &[u8]) -> bool {
    const MAX_REPEATED: usize = 12;

    let mut atoms = Vec::new();
    let Some(predicate) = attribute
        .child_by_field_name("arguments")
        .and_then(|arguments| operands(arguments, source, &mut atoms).into_iter().next())
    else {
        return false;
    };
    let mut repeated = 0;
    for atom in &mut atoms {
        atom.bit = (atom.uses > 1 && repeated < MAX_REPEATED).then_some(repeated);
        repeated += usize::from(atom.bit.is_some());
    }

    let value = |test, assignment| {
        predicate.value(&Configuration {
            test,
            assignment,
            atoms: &atoms,
        })
    };
    let mut assignments = 0..1_usize << repeated;
    assignments
        .clone()
        .all(|assignment| value(false, assignment) == Truth::False)
        && assignments.any(|assignment| value(true, assignment) != Truth::False)
}

struct Configuration<'b> {
    test: bool,
    assignment: usize,
    atoms: &'b [Atom<'b>],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Truth {
    False,
    True,
    Unknown,
}

impl Truth {
    const fn of(holds: bool) -> Self {
        if holds { Self::True } else { Self::False }
    }

    const fn negated(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }
}

enum Cfg {
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    Test,
    Atom(usize),
}

impl Cfg {
    fn value(&self, at: &Configuration<'_>) -> Truth {
        match self {
            Self::All(operands) => Self::combine(operands, Truth::False, at),
            Self::Any(operands) => Self::combine(operands, Truth::True, at),
            Self::Not(operand) => operand.value(at).negated(),
            Self::Test => Truth::of(at.test),
            Self::Atom(atom) => at.atoms[*atom].bit.map_or(Truth::Unknown, |bit| {
                Truth::of((at.assignment >> bit) & 1 == 1)
            }),
        }
    }

    // constraint: one operand of the absorbing truth decides `all` or `any`, and an unknown operand leaves the result
    // unknown only when none does
    fn combine(operands: &[Self], absorbing: Truth, at: &Configuration<'_>) -> Truth {
        let mut combined = absorbing.negated();
        for operand in operands {
            match operand.value(at) {
                Truth::Unknown => combined = Truth::Unknown,
                truth if truth == absorbing => return absorbing,
                _ => {}
            }
        }
        combined
    }
}

// constraint: `bit` is the atom's bit of a configuration's assignment, or none when the atom stays unknown
struct Atom<'s> {
    key: (&'s [u8], &'s [u8]),
    uses: usize,
    bit: Option<usize>,
}

// constraint: an operand is an identifier, followed by its argument tree when it is an operator or by its string
// value when it is a `key = "value"` atom; an operator other than `all`, `any` and a one-operand `not` is an atom
// keyed by its name and argument text
fn operands<'s>(tree: Node<'_>, source: &'s [u8], atoms: &mut Vec<Atom<'s>>) -> Vec<Cfg> {
    let mut cursor = tree.walk();
    let mut children = tree.named_children(&mut cursor).peekable();

    let mut predicates = Vec::new();
    while let Some(lead) = children.next() {
        if lead.kind() != "identifier" {
            continue;
        }
        let name = &source[lead.byte_range()];
        let predicate =
            if let Some(arguments) = children.next_if(|next| next.kind() == "token_tree") {
                let mut inner = operands(arguments, source, atoms);
                match name {
                    b"all" => Cfg::All(inner),
                    b"any" => Cfg::Any(inner),
                    b"not" if inner.len() == 1 => Cfg::Not(Box::new(inner.swap_remove(0))),
                    _ => atom(atoms, (name, &source[arguments.byte_range()])),
                }
            } else {
                let value = children
                    .next_if(|next| next.kind() == "string_literal")
                    .map_or(&[][..], |value| &source[value.byte_range()]);
                if name == b"test" && value.is_empty() {
                    Cfg::Test
                } else {
                    atom(atoms, (name, value))
                }
            };
        predicates.push(predicate);
    }
    predicates
}

fn atom<'s>(atoms: &mut Vec<Atom<'s>>, key: (&'s [u8], &'s [u8])) -> Cfg {
    if let Some(at) = atoms.iter().position(|seen| seen.key == key) {
        atoms[at].uses += 1;
        return Cfg::Atom(at);
    }
    atoms.push(Atom {
        key,
        uses: 1,
        bit: None,
    });
    Cfg::Atom(atoms.len() - 1)
}

pub(super) fn path_attribute(module: Node<'_>, source: &[u8]) -> Option<String> {
    outer_attributes(module).find_map(|attribute| path_attribute_value(attribute, source))
}

// constraint: only the `path = "<string literal>"` shape names a file, and its text is taken as written: an escape is
// not decoded, which none of the 1,601 `#[path]` attributes of a 447-crate registry writes, and a raw string names
// nothing
fn path_attribute_value(item: Node<'_>, source: &[u8]) -> Option<String> {
    let attribute = named_child_of_kind(item, "attribute")?;
    let value = attribute.child_by_field_name("value")?;
    if value.kind() != "string_literal" {
        return None;
    }
    let name = named_children(attribute)
        .filter_map(|at| attribute.named_child(at))
        .find(|child| child.kind() == "identifier")?;
    if text(name, source) != "path" {
        return None;
    }

    let literal = text(value, source);
    let spec = literal.trim_matches('"');
    (!spec.is_empty()).then(|| spec.to_owned())
}
