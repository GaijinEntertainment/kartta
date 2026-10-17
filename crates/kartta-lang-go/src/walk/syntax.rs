use kartta_lang::tree::{children, named_children, text};
use tree_sitter::{Node, TreeCursor};

pub(super) const PREDECLARED: &[&str] = &[
    "any",
    "bool",
    "byte",
    "comparable",
    "complex64",
    "complex128",
    "error",
    "false",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "iota",
    "nil",
    "rune",
    "string",
    "true",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
];

pub(super) const TYPE_ONLY_KINDS: &[&str] = &[
    "array_type",
    "channel_type",
    "function_type",
    "generic_type",
    "implicit_length_array_type",
    "interface_type",
    "map_type",
    "parenthesized_type",
    "pointer_type",
    "qualified_identifier",
    "slice_type",
    "struct_type",
    "type_identifier",
];

pub(super) fn line(row: usize) -> u32 {
    u32::try_from(row).map_or(u32::MAX, |row| row.saturating_add(1))
}

// constraint: Go exports a name whose first rune is a Unicode upper-case letter, so `Ä` exports; `char::is_uppercase`
// agrees with Go's `unicode.IsUpper` on every rune that can start an identifier
pub(super) fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

// constraint: `go help generate` fixes the banner as a line matching `^// Code generated .* DO NOT EDIT\.$` before the
// package clause, so the scan stops at the clause and an indented comment is not the banner; a byte order mark the
// go command accepts before the first line is not part of it
pub(super) fn is_generated_source(source: &[u8]) -> bool {
    let source = source.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(source);
    for raw in source.split(|&byte| byte == b'\n') {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        if line.starts_with(b"// Code generated ") && line.ends_with(b" DO NOT EDIT.") {
            return true;
        }
        if let Some(rest) = line.trim_ascii_start().strip_prefix(b"package")
            && rest.first().is_some_and(u8::is_ascii_whitespace)
        {
            break;
        }
    }

    false
}

pub(super) fn trim_quotes(mut spec: String) -> String {
    const QUOTES: [char; 2] = ['"', '`'];

    spec.truncate(spec.trim_end_matches(QUOTES).len());
    let lead = spec.len() - spec.trim_start_matches(QUOTES).len();
    spec.drain(..lead);
    spec
}

// constraint: the graph's import table decides which name a package binds, so an unaliased import offers every usual
// spelling of its package name; a spelling outside this set loses the file's selector uses of that package
pub(super) fn default_name_candidates(path: &str) -> Vec<String> {
    let mut segments = path.rsplit('/');
    let last = segments.next().unwrap_or(path);
    let is_major = |segment: &str| {
        segment.len() > 1
            && segment.starts_with('v')
            && segment[1..].bytes().all(|b| b.is_ascii_digit())
    };
    let base = match segments.next() {
        Some(previous) if is_major(last) => previous,
        _ => last,
    };

    let mut names = vec![last.to_owned()];
    let versionless = match base.rsplit_once(".v") {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => base,
    };
    let trimmed = (versionless.strip_prefix("go-"))
        .or_else(|| versionless.strip_suffix("-go"))
        .or_else(|| versionless.strip_suffix(".go"))
        .unwrap_or(versionless);
    let head = trimmed.split('-').next().unwrap_or(trimmed);
    for name in [base, versionless, trimmed, head] {
        for spelling in [name.to_owned(), name.replace('-', "")] {
            if !names.contains(&spelling) {
                names.push(spelling);
            }
        }
    }

    names
}

pub(super) fn is_pointer_indirection(node: Node<'_>) -> bool {
    node.kind() == "unary_expression"
        && (node.child_by_field_name("operator")).is_some_and(|operator| operator.kind() == "*")
}

pub(super) fn first_operand(node: Node<'_>) -> Option<Node<'_>> {
    named_children(node)
        .filter_map(|i| node.named_child(i))
        .find(|child| child.kind() != "comment")
}

pub(super) fn parents_of(type_node: Node<'_>, source: &[u8]) -> Vec<String> {
    match type_node.kind() {
        "struct_type" => embedded_fields(type_node, source),
        "interface_type" => embedded_interfaces(type_node, source),
        _ => Vec::new(),
    }
}

fn embedded_fields(struct_type: Node<'_>, source: &[u8]) -> Vec<String> {
    children(struct_type)
        .filter_map(|i| struct_type.child(i))
        .filter(|field| {
            field.kind() == "field_declaration" && field.child_by_field_name("name").is_none()
        })
        .filter_map(|field| field.child_by_field_name("type"))
        .filter_map(|type_field| {
            unwrap_pointer(type_field).and_then(|node| base_type_name(node, source))
        })
        .collect()
}

fn embedded_interfaces(interface_type: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut parents = Vec::new();
    for i in children(interface_type) {
        let Some(mut child) = interface_type.child(i) else {
            continue;
        };
        if child.kind() == "type_elem"
            && let Some(inner) = children(child)
                .filter_map(|j| child.child(j))
                .find(Node::is_named)
        {
            child = inner;
        }
        if let Some(name) = base_type_name(child, source) {
            parents.push(name);
        }
    }

    parents
}

pub(super) fn receiver_type(receiver: Node<'_>, source: &[u8]) -> String {
    children(receiver)
        .filter_map(|i| receiver.child(i))
        .filter(|child| child.kind() == "parameter_declaration")
        .find_map(|child| {
            child
                .child_by_field_name("type")
                .and_then(unwrap_pointer)
                .and_then(|node| base_type_name(node, source))
        })
        .unwrap_or_default()
}

// constraint: only `T{}`, `&T{}`, `new(T)` and `(*T)(nil)` name the implementing type statically; a constructor call's
// return type is unknown, so it names none
pub(super) fn assertion_value_type(expression: Node<'_>, source: &[u8]) -> Option<String> {
    match expression.kind() {
        "composite_literal" => base_type_name(expression.child_by_field_name("type")?, source),
        "unary_expression" => {
            assertion_value_type(expression.child_by_field_name("operand")?, source)
        }
        "call_expression" => {
            let function = expression.child_by_field_name("function")?;
            if function.kind() == "identifier" && text(function, source) == "new" {
                let operand = first_operand(expression.child_by_field_name("arguments")?)?;
                return operand_type_name(operand, source);
            }

            let pointer = (function.kind() == "parenthesized_expression")
                .then(|| first_operand(function))
                .flatten()
                .filter(|inner| inner.kind() == "unary_expression")?;
            operand_type_name(pointer.child_by_field_name("operand")?, source)
        }
        _ => None,
    }
}

fn operand_type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text(node, source)),
        "selector_expression" => node
            .child_by_field_name("operand")
            .is_some_and(|operand| operand.kind() == "identifier")
            .then(|| text(node, source)),
        "index_expression" => operand_type_name(node.child_by_field_name("operand")?, source),
        _ => base_type_name(node, source),
    }
}

pub(super) fn base_type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let base = match node.kind() {
        "generic_type" => node.child_by_field_name("type")?,
        _ => node,
    };

    matches!(base.kind(), "type_identifier" | "qualified_identifier").then(|| text(base, source))
}

fn unwrap_pointer(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() != "pointer_type" {
        return Some(node);
    }

    children(node)
        .filter_map(|i| node.child(i))
        .find(Node::is_named)
}

// constraint: a receiver is spelled the way Go spells owners, a bare type name, so only an identifier operand names
// one; any other operand rounds the receiver to the empty string, a member call of no known type
pub(super) fn call_target(function: Node<'_>, source: &[u8]) -> (String, Option<String>) {
    match function.kind() {
        "identifier" => (text(function, source), None),
        "selector_expression" => {
            let receiver = function.child_by_field_name("operand").map(|operand| {
                if operand.kind() == "identifier" {
                    text(operand, source)
                } else {
                    String::new()
                }
            });

            function.child_by_field_name("field").map_or_else(
                || (String::new(), None),
                |field| (text(field, source), receiver),
            )
        }
        _ => (String::new(), None),
    }
}

pub(super) fn next_in_preorder(cursor: &mut TreeCursor<'_>, descend: bool) -> bool {
    if descend && cursor.goto_first_child() {
        return true;
    }
    loop {
        if cursor.goto_next_sibling() {
            return true;
        }
        if !cursor.goto_parent() {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unaliased_import_offers_its_package_name_spellings() {
        for (path, name) in [
            ("github.com/pkg/errors", "errors"),
            ("gopkg.in/yaml.v3", "yaml"),
            ("github.com/mattn/go-sqlite3", "sqlite3"),
            ("example.com/api/v2", "api"),
            ("example.com/kit-go", "kit"),
            ("example.com/some-pkg", "somepkg"),
            ("github.com/anthropics/anthropic-sdk-go", "anthropic"),
        ] {
            let candidates = default_name_candidates(path);

            assert!(
                candidates.iter().any(|candidate| candidate == name),
                "{path} offers {name}: {candidates:?}"
            );
        }
    }
}
