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
// other operand, a variable or an interpolated string among them, folds to nothing
pub(crate) fn fold(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "parenthesized_expression" => fold(node.named_child(0)?, source),
        "string" | "encapsed_string" => string_literal(node, source),
        "name" => match &source[node.byte_range()] {
            b"__DIR__" => Some(DIR_MARKER.to_owned()),
            b"__FILE__" => Some(FILE_MARKER.to_owned()),
            _ => None,
        },
        "binary_expression" => {
            let operator = node.child_by_field_name("operator")?;
            if &source[operator.byte_range()] != b"." {
                return None;
            }
            let left = fold(node.child_by_field_name("left")?, source)?;
            let right = fold(node.child_by_field_name("right")?, source)?;
            Some(left + &right)
        }
        "function_call_expression" => dirname(node, source),
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
// a marker; `dirname(__FILE__)` is `__DIR__`
fn dirname(call: Node<'_>, source: &[u8]) -> Option<String> {
    let function = call.child_by_field_name("function")?;
    if function.kind() != "name" || &source[function.byte_range()] != b"dirname" {
        return None;
    }
    let arguments = call.child_by_field_name("arguments")?;
    let argument = named_child_nodes(arguments).find(|argument| argument.kind() == "argument")?;
    let value = named_child_nodes(argument).next_back()?;

    let folded = fold(value, source)?;
    let trimmed = folded.trim_end_matches('/');
    let folded = if trimmed.is_empty() {
        folded.as_str()
    } else {
        trimmed
    };

    Some(if folded == FILE_MARKER {
        DIR_MARKER.to_owned()
    } else if cuts_in_literal_tail(folded) {
        lexical_dirname(folded).to_owned()
    } else {
        format!("{folded}{UP_MARKER}")
    })
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
