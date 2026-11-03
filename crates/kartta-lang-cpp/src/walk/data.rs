use kartta_lang::tree::{signature, text};
use tree_sitter::Node;

use super::child_nodes;
use super::name::type_base_name;

const DATA_SIGNATURE_MAX: usize = 160;
const POINTERS_A_CALL_READS: &str = "**";

pub(super) fn has_storage_class(node: Node<'_>, class: &str, source: &[u8]) -> bool {
    has_child_spelled(node, "storage_class_specifier", class, source)
}

pub(super) fn has_qualifier(node: Node<'_>, qualifier: &str, source: &[u8]) -> bool {
    has_child_spelled(node, "type_qualifier", qualifier, source)
}

fn has_child_spelled(node: Node<'_>, kind: &str, spelling: &str, source: &[u8]) -> bool {
    child_nodes(node)
        .any(|child| child.kind() == kind && &source[child.byte_range()] == spelling.as_bytes())
}

pub(super) fn alias_declarator_name(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "type_identifier" => return Some(node),
            "pointer_declarator" | "array_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            _ => return None,
        }
    }
}

/// Whether the type a typedef writes defines the class or enum tag `name` spells, as C's `typedef struct S { } S;` or
/// `typedef enum E { } E;`: the definition already holds the name, so an alias would be a second type of it.
pub(super) fn defines_class_tag(
    written: Option<Node<'_>>,
    name: Node<'_>,
    declarator: Node<'_>,
    source: &[u8],
) -> bool {
    let spells_name = |tag: Node<'_>| source[tag.byte_range()] == source[name.byte_range()];
    name == declarator
        && written.is_some_and(|written| {
            matches!(
                written.kind(),
                "class_specifier" | "struct_specifier" | "union_specifier" | "enum_specifier"
            ) && written.child_by_field_name("body").is_some()
                && written.child_by_field_name("name").is_some_and(spells_name)
        })
}

pub(super) fn field_owner(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut member = node;
    loop {
        let mut list = member.parent()?;
        while list.kind().starts_with("preproc_") {
            list = list.parent()?;
        }
        let class = list.parent()?;
        if !matches!(
            class.kind(),
            "class_specifier" | "struct_specifier" | "union_specifier"
        ) {
            return None;
        }
        if let Some(name) = class.child_by_field_name("name") {
            return Some(type_base_name(name, source));
        }

        let holder = class.parent()?;
        match holder.kind() {
            "field_declaration" if holder.child_by_field_name("declarator").is_none() => {
                member = holder;
            }
            "type_definition" => {
                let mut cursor = holder.walk();
                let alias = holder
                    .children_by_field_name("declarator", &mut cursor)
                    .find_map(alias_declarator_name)?;
                return Some(text(alias, source));
            }
            _ => return None,
        }
    }
}

/// The name a field declarator declares, and how many pointers the field holds to a value of its type: each `*` and
/// each array dimension counts one, a reference none, and a function makes it `None`, since no member call names its
/// operand.
pub(super) fn field_declarator(mut node: Node<'_>) -> Option<(Node<'_>, Option<usize>)> {
    let mut pointers = Some(0);
    loop {
        match node.kind() {
            "field_identifier" => return Some((node, pointers)),
            "pointer_declarator" | "array_declarator" => {
                pointers = pointers.map(|held| held + 1);
                node = node.child_by_field_name("declarator")?;
            }
            "function_declarator" => {
                pointers = None;
                node = parenthesized_declarator(node)?;
            }
            "reference_declarator" | "parenthesized_declarator" => node = node.named_child(0)?,
            _ => return None,
        }
    }
}

/// The value types of a field of `class`, each after a `*` per pointer: the class, and for a class template the class
/// its first argument names, `reached`, behind the pointers it takes. `->` goes through one pointer and an element's
/// `[]` through one more, so a type behind more than [`POINTERS_A_CALL_READS`] is left out. A field of a builtin type,
/// of a template parameter or of a function has no `class` or no `pointers`, and records none.
pub(super) fn field_value_types(
    class: Option<&str>,
    pointers: Option<usize>,
    reached: Option<(&str, usize)>,
) -> Vec<String> {
    let (Some(class), Some(pointers)) = (class, pointers) else {
        return Vec::new();
    };
    let reached = reached.map(|(class, taken)| (class, pointers + taken));
    std::iter::once((class, pointers))
        .chain(reached)
        .filter_map(|(class, held)| {
            POINTERS_A_CALL_READS
                .get(..held)
                .map(|stars| format!("{stars}{class}"))
        })
        .collect()
}

pub(super) fn data_declarator<'t>(
    mut node: Node<'t>,
    source: &[u8],
) -> Option<(Node<'t>, Option<bool>)> {
    let mut const_pointer = None;
    loop {
        match node.kind() {
            "identifier" => return Some((node, const_pointer)),
            "pointer_declarator" => {
                const_pointer = Some(has_qualifier(node, "const", source));
                node = node.child_by_field_name("declarator")?;
            }
            "init_declarator" | "array_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            "function_declarator" => node = parenthesized_declarator(node)?,
            "reference_declarator" | "parenthesized_declarator" => node = node.named_child(0)?,
            _ => return None,
        }
    }
}

fn parenthesized_declarator(function: Node<'_>) -> Option<Node<'_>> {
    function
        .child_by_field_name("declarator")
        .filter(|declarator| declarator.kind() == "parenthesized_declarator")
}

pub(super) fn data_signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    if node.start_position().row == node.end_position().row
        && node.byte_range().len() <= DATA_SIGNATURE_MAX
    {
        return signature(node, source);
    }

    let mut cursor = node.walk();
    let Some(value) = node
        .children_by_field_name("declarator", &mut cursor)
        .find_map(|declarator| declarator.child_by_field_name("value"))
    else {
        return signature(node, source);
    };
    let text = String::from_utf8_lossy(&source[node.start_byte()..value.start_byte()]);
    let text = text.trim_end().trim_end_matches('=').trim_end();

    (!text.is_empty()).then(|| text.to_owned())
}

pub(super) fn at_namespace_scope(node: Node<'_>) -> bool {
    std::iter::successors(node.parent(), Node::parent).all(|ancestor| {
        let kind = ancestor.kind();
        (kind.starts_with("preproc_") && !ancestor.has_error())
            || matches!(
                kind,
                "translation_unit"
                    | "namespace_definition"
                    | "declaration_list"
                    | "linkage_specification"
                    | "template_declaration"
            )
    })
}
