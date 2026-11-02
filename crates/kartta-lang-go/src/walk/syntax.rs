use std::borrow::Cow;

use kartta_lang::tree::{child_nodes, named_child_nodes, text};
use tree_sitter::Node;

use crate::resolve::{is_major_version, package_segments};

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

// constraint: `encoding/json`, `encoding/xml`, `encoding/gob`, the yaml, toml and bson packages and `mapstructure`
// marshal and unmarshal a value, and their encoders and decoders encode and decode one, through calls of these names,
// each skipping a field whose tag says so
pub(super) const SERIALIZING_CALLS: &[&str] = &[
    "Decode",
    "Encode",
    "Marshal",
    "MarshalIndent",
    "MarshalWrite",
    "Unmarshal",
    "UnmarshalRead",
];

// constraint: `reflect` takes a value through calls of these names and sees every field, whatever its tags say
pub(super) const INSPECTING_CALLS: &[&str] = &["TypeOf", "ValueOf"];

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

// constraint: `reflect.StructTag` reads a tag as space-separated `key:"value"` pairs, and a value of `-` makes
// `encoding/json`, `encoding/xml` and their kin skip the field, so a tag hands its field to some library where one
// pair's value is anything else; a tag that holds no such pair hands it over too, since a library may read it whole
pub(super) fn tag_hands_over_its_field(literal: &str) -> bool {
    let tag = tag_text(literal);
    let mut rest = tag.trim_start();
    let mut paired = false;
    while let Some((key, after)) = rest.split_once(":\"") {
        let Some(end) = after.find('"') else {
            break;
        };
        if key.is_empty() || key.contains(|c: char| c.is_whitespace() || c == '"') {
            break;
        }
        if &after[..end] != "-" {
            return true;
        }
        paired = true;
        rest = after[end + 1..].trim_start();
    }
    !paired
}

// constraint: a tag is a raw string literal, read as written, or an interpreted one, whose escapes stand for the
// quotes and backslashes of its pairs
fn tag_text(literal: &str) -> Cow<'_, str> {
    if let Some(raw) = literal
        .strip_prefix('`')
        .and_then(|tag| tag.strip_suffix('`'))
    {
        return Cow::Borrowed(raw);
    }
    let Some(interpreted) = literal
        .strip_prefix('"')
        .and_then(|tag| tag.strip_suffix('"'))
    else {
        return Cow::Borrowed(literal);
    };
    let mut text = String::with_capacity(interpreted.len());
    let mut chars = interpreted.chars();
    while let Some(c) = chars.next() {
        text.push(if c == '\\' {
            chars.next().unwrap_or(c)
        } else {
            c
        });
    }
    Cow::Owned(text)
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
    let (last, base) = package_segments(path);

    let mut names = vec![last.to_owned()];
    let versionless = match base.rsplit_once('.') {
        Some((head, tail)) if is_major_version(tail) => head,
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
    named_child_nodes(node).find(|child| child.kind() != "comment")
}

pub(super) fn parents_of(type_node: Node<'_>, source: &[u8]) -> Vec<String> {
    match type_node.kind() {
        "struct_type" => embedded_fields(type_node, source),
        "interface_type" => embedded_interfaces(type_node, source),
        _ => Vec::new(),
    }
}

fn embedded_fields(struct_type: Node<'_>, source: &[u8]) -> Vec<String> {
    child_nodes(struct_type)
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
    for mut child in child_nodes(interface_type) {
        if child.kind() == "type_elem"
            && let Some(inner) = child_nodes(child).find(Node::is_named)
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
    child_nodes(receiver)
        .filter(|child| child.kind() == "parameter_declaration")
        .find_map(|child| {
            child
                .child_by_field_name("type")
                .and_then(unwrap_pointer)
                .and_then(|node| base_type_name(node, source))
        })
        .unwrap_or_default()
}

// constraint: only `T{}`, `&T{}`, `new(T)` and a conversion name the implementing type statically; a constructor call's
// return type is unknown, so it names none. A conversion takes one operand (doc/go_spec.html, Conversions), and a call
// of one operand cannot be told from it here: the name it records is then a function's, which no type of that package
// can share, so it reaches no type
pub(super) fn assertion_value_type(expression: Node<'_>, source: &[u8]) -> Option<String> {
    let mut expression = unparenthesized(expression);
    while expression.kind() == "unary_expression" {
        expression = unparenthesized(expression.child_by_field_name("operand")?);
    }
    match expression.kind() {
        "composite_literal" => base_type_name(expression.child_by_field_name("type")?, source),
        "call_expression" => {
            let function = unparenthesized(expression.child_by_field_name("function")?);
            let arguments = expression.child_by_field_name("arguments")?;
            if function.kind() == "identifier" && text(function, source) == "new" {
                return operand_type_name(first_operand(arguments)?, source);
            }
            let mut operands =
                named_child_nodes(arguments).filter(|child| child.kind() != "comment");
            if operands.next().is_none() || operands.next().is_some() {
                return None;
            }

            let converted = match function.kind() {
                "unary_expression" => function.child_by_field_name("operand")?,
                _ => function,
            };
            operand_type_name(converted, source)
        }
        _ => None,
    }
}

pub(super) fn unparenthesized(mut node: Node<'_>) -> Node<'_> {
    while matches!(
        node.kind(),
        "parenthesized_expression" | "parenthesized_type"
    ) && let Some(inner) = first_operand(node)
    {
        node = inner;
    }

    node
}

fn operand_type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut node = unparenthesized(node);
    while node.kind() == "index_expression" {
        node = unparenthesized(node.child_by_field_name("operand")?);
    }
    match node.kind() {
        "identifier" => Some(text(node, source)),
        "selector_expression" => node
            .child_by_field_name("operand")
            .is_some_and(|operand| operand.kind() == "identifier")
            .then(|| text(node, source)),
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

    child_nodes(node).find(Node::is_named)
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
