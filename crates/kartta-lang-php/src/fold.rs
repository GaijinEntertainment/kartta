use std::num::NonZeroU8;

use kartta_lang::path::dir;
use kartta_lang::tree::{named_child_nodes, text};
use tree_sitter::Node;

// constraint: `__DIR__` and `__FILE__` depend on the path, which the bytes do not know, so the fold writes them as
// markers that the resolver substitutes; the marker byte never comes from the source, because a literal that holds it
// folds to nothing
pub(crate) const MARKER: char = '\u{1}';
const DIR_MARKER: &str = "\u{1}D";
const FILE_MARKER: &str = "\u{1}F";
// constraint: a `dirname` the fold cannot apply climbs one level at resolution; it is a marker of its own rather than
// a written `/..`, because PHP's `dirname` strips a written `..` while a deferred climb composes with the next one
const UP_MARKER: &str = "\u{1}U";

// constraint: a string, `__DIR__`, `__FILE__`, a `.` concatenation of those and `dirname` of one fold to a path; any
// other operand, a variable or an interpolated string among them, folds to nothing; PHP's scanner matches both
// constants in any letter case and lowers a function's name before its lookup, so those names match in any ASCII case,
// while it compares a named argument's name exactly
pub(crate) fn fold(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut steps = vec![Step::Fold(node)];
    let mut folded = Vec::new();
    while let Some(step) = steps.pop() {
        match step {
            Step::Fold(node) => match node.kind() {
                "parenthesized_expression" => steps.push(Step::Fold(node.named_child(0)?)),
                "binary_expression" => {
                    let operator = node.child_by_field_name("operator")?;
                    if &source[operator.byte_range()] != b"." {
                        return None;
                    }
                    steps.push(Step::Join);
                    steps.push(Step::Fold(node.child_by_field_name("right")?));
                    steps.push(Step::Fold(node.child_by_field_name("left")?));
                }
                "function_call_expression" => {
                    let (path, levels) = dirname(node, source)?;
                    steps.push(Step::Climb(levels));
                    steps.push(Step::Fold(path));
                }
                _ => folded.push(leaf(node, source)?),
            },
            Step::Join => {
                let right = folded.pop()?;
                folded.last_mut()?.push_str(&right);
            }
            Step::Climb(levels) => {
                let path = folded.last_mut()?;
                for _ in 0..levels.get() {
                    *path = dirname_once(path);
                }
            }
        }
    }
    folded.pop()
}

/// What [`fold`] does next: fold a node onto the folded values, join the last two, or climb from the last.
enum Step<'t> {
    Fold(Node<'t>),
    Join,
    Climb(NonZeroU8),
}

fn leaf(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "string" | "encapsed_string" => string_literal(node, source),
        "name" => match &source[node.byte_range()] {
            name if name.eq_ignore_ascii_case(b"__DIR__") => Some(DIR_MARKER.to_owned()),
            name if name.eq_ignore_ascii_case(b"__FILE__") => Some(FILE_MARKER.to_owned()),
            _ => None,
        },
        _ => None,
    }
}

// constraint: PHP's `__DIR__` and `__FILE__` are absolute, so a path marker names a file only at the start of the
// path; the climb marker follows one
pub(crate) fn marker_leads(spec: &str) -> bool {
    !spec.contains(MARKER)
        || spec
            .match_indices(DIR_MARKER)
            .chain(spec.match_indices(FILE_MARKER))
            .all(|(at, _)| at == 0)
}

pub(crate) fn substitute_markers(spec: &str, path: &str) -> String {
    let mut out = String::with_capacity(spec.len() + path.len());
    let mut rest = spec;

    while let Some(at) = rest.find(MARKER) {
        out.push_str(&rest[..at]);
        let marked = &rest[at..];
        rest = if let Some(tail) = marked.strip_prefix(DIR_MARKER) {
            out.push_str(dir(path));
            tail
        } else if let Some(tail) = marked.strip_prefix(FILE_MARKER) {
            out.push_str(path);
            tail
        } else if let Some(tail) = marked.strip_prefix(UP_MARKER) {
            out.push_str("/..");
            tail
        } else {
            out.push(MARKER);
            &marked[MARKER.len_utf8()..]
        };
    }
    out.push_str(rest);

    out
}

fn string_literal(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut content = String::new();
    for part in named_child_nodes(node) {
        if part.kind() != "string_content" {
            return None;
        }
        content.push_str(&text(part, source));
    }

    (!content.contains(MARKER)).then_some(content)
}

// constraint: PHP's `dirname` is lexical: it strips trailing separators and cuts at the last `/` as written, so the
// fold cuts itself wherever that `/` follows the last marker, and defers a climb only where the cut would fall inside
// a marker; `dirname(__FILE__)` is `__DIR__`, and `levels` applies it that many times
fn dirname<'t>(call: Node<'t>, source: &[u8]) -> Option<(Node<'t>, NonZeroU8)> {
    let function = call.child_by_field_name("function")?;
    // constraint: PHP resolves `\dirname` to the global function in any namespace, and `Foo\dirname` into `Foo`; the
    // fold does not read the namespace `namespace\dirname` depends on, so it folds that to nothing
    let name = &source[function.byte_range()];
    let name = name.strip_prefix(b"\\").unwrap_or(name);
    if !name.eq_ignore_ascii_case(b"dirname") {
        return None;
    }
    let (path, levels) = dirname_arguments(call.child_by_field_name("arguments")?, source)?;
    let levels = match levels {
        Some(levels) => level_count(levels, source)?,
        None => NonZeroU8::MIN,
    };

    Some((path, levels))
}

// constraint: PHP binds `dirname`'s arguments by position or by the names `path` and `levels`, and refuses a third
fn dirname_arguments<'tree>(
    arguments: Node<'tree>,
    source: &[u8],
) -> Option<(Node<'tree>, Option<Node<'tree>>)> {
    let mut slots = [None, None];
    let arguments = named_child_nodes(arguments).filter(|argument| argument.kind() == "argument");
    for (position, argument) in arguments.enumerate() {
        let slot = match argument.child_by_field_name("name") {
            None => position,
            Some(name) => match &source[name.byte_range()] {
                b"path" => 0,
                b"levels" => 1,
                _ => return None,
            },
        };
        let value = named_child_nodes(argument).next_back()?;
        if slots.get_mut(slot)?.replace(value).is_some() {
            return None;
        }
    }
    let [path, levels] = slots;

    Some((path?, levels))
}

// constraint: PHP refuses a `levels` below 1, and reads a literal with a leading `0` as octal, hexadecimal or binary,
// so the fold takes plain decimals only; a count past 255 folds to nothing, since no checkout is that deep and the cap
// bounds the climb markers one call writes
fn level_count(levels: Node<'_>, source: &[u8]) -> Option<NonZeroU8> {
    let literal = &source[levels.byte_range()];
    if levels.kind() != "integer" || literal.starts_with(b"0") {
        return None;
    }
    str::from_utf8(literal).ok()?.parse().ok()
}

fn dirname_once(folded: &str) -> String {
    let trimmed = folded.trim_end_matches('/');
    let folded = if trimmed.is_empty() { folded } else { trimmed };

    if folded == FILE_MARKER {
        DIR_MARKER.to_owned()
    } else if cuts_in_literal_tail(folded) {
        lexical_dirname(folded).to_owned()
    } else {
        format!("{folded}{UP_MARKER}")
    }
}

fn cuts_in_literal_tail(folded: &str) -> bool {
    folded
        .rfind(MARKER)
        .is_none_or(|marker| folded.rfind('/').is_some_and(|slash| slash > marker))
}

fn lexical_dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(at) => &path[..at],
        None => ".",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution_replaces_each_marker_once_and_keeps_an_unknown_one() {
        assert_eq!(
            substitute_markers("\u{1}D\u{1}U/x.php", "src/a/page.php"),
            "src/a/../x.php"
        );
        assert_eq!(substitute_markers("\u{1}F", "src/page.php"), "src/page.php");
        assert_eq!(substitute_markers("a\u{1}Xb", "page.php"), "a\u{1}Xb");
    }

    #[test]
    fn only_a_leading_path_marker_names_a_file() {
        assert!(marker_leads("config/app.php"));
        assert!(marker_leads("\u{1}D\u{1}U/x.php"));
        assert!(!marker_leads("inc/\u{1}D/x.php"));
    }
}
