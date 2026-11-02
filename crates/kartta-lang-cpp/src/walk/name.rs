use kartta_lang::tree::text;
use kartta_lang::{Symbol, SymbolKind};
use tree_sitter::Node;

use super::data::data_declarator;
use super::{child_nodes, exported_symbol};
use crate::{BRACKET_INCLUDE, QUOTED_INCLUDE};

pub(super) fn function_base_name(node: Node<'_>, source: &[u8]) -> String {
    let mut node = node;
    loop {
        let inner = match node.kind() {
            "template_function" | "template_method" => node.child_by_field_name("name"),
            "dependent_name" => node.named_child(0),
            _ => None,
        };
        let Some(inner) = inner else {
            return sanitize_name(&text(node, source));
        };
        node = inner;
    }
}

pub(super) fn type_base_name(node: Node<'_>, source: &[u8]) -> String {
    let mut node = node;
    while matches!(node.kind(), "template_type" | "qualified_identifier")
        && let Some(name) = node.child_by_field_name("name")
    {
        node = name;
    }
    match node.kind() {
        "type_identifier" | "identifier" | "field_identifier" | "namespace_identifier" => {
            text(node, source)
        }
        _ => sanitize_name(&text(node, source)),
    }
}

pub(super) fn sanitize_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn is_simple_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first == b'_' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

pub(super) fn class_symbol(node: Node<'_>, source: &[u8]) -> Option<Symbol> {
    let name = node.child_by_field_name("name")?;
    node.child_by_field_name("body")?;
    let kind = if node.kind() == "class_specifier" {
        SymbolKind::Class
    } else {
        SymbolKind::Struct
    };

    let mut symbol = exported_symbol(type_base_name(name, source), kind, node);
    symbol.parents = child_nodes(node)
        .filter(|child| child.kind() == "base_class_clause")
        .flat_map(child_nodes)
        .filter_map(|base| base_parent_name(base, source))
        .collect();
    Some(symbol)
}

fn base_parent_name(base: Node<'_>, source: &[u8]) -> Option<String> {
    let mut named = base;
    let mut scoped = String::new();
    while named.kind() == "qualified_identifier" {
        if let Some(scope) = named.child_by_field_name("scope") {
            scoped.push_str(&String::from_utf8_lossy(&source[scope.byte_range()]));
        }
        scoped.push_str("::");
        named = named.child_by_field_name("name")?;
    }
    let name = match named.kind() {
        "type_identifier" => named,
        "template_type" => named.child_by_field_name("name")?,
        _ => return None,
    };
    scoped.push_str(&text(name, source));
    Some(scoped)
}

/// Whether a class's head names a specialization of a template, `Box<bool>` or `ns::Box<T *>`.
pub(super) fn specializes(class: Node<'_>) -> bool {
    let mut name = class.child_by_field_name("name");
    while let Some(scoped) = name.filter(|name| name.kind() == "qualified_identifier") {
        name = scoped.child_by_field_name("name");
    }
    name.is_some_and(|name| name.kind() == "template_type")
}

pub(super) fn head_scope(class: Node<'_>, source: &[u8]) -> Option<String> {
    let name = class.child_by_field_name("name")?;
    if name.kind() != "qualified_identifier" {
        return None;
    }

    let written = base_parent_name(name, source)?;
    let (scope, _) = written.trim_start_matches("::").rsplit_once("::")?;
    Some(scope.to_owned())
}

// constraint: the operand of `.` or `->` is a value, never a type, so from the syntax alone a named one records the
// empty receiver like any other expression, and only `this` names an owner, the caller's own
pub(super) fn call_target(call: Node<'_>, source: &[u8]) -> Option<(String, Option<String>)> {
    let function = call.child_by_field_name("function")?;
    let target = match function.kind() {
        "identifier" => (unless_cast(text(function, source))?, None),
        "field_expression" => {
            let field = function.child_by_field_name("field")?;
            let receiver = function.child_by_field_name("argument").map(|argument| {
                if argument.kind() == "this" {
                    "this".to_owned()
                } else {
                    String::new()
                }
            });
            (function_base_name(field, source), receiver)
        }
        "qualified_identifier" if function.child_by_field_name("scope").is_some() => {
            (text(function, source), None)
        }
        "qualified_identifier" => {
            let name = function.child_by_field_name("name")?;
            (format!("::{}", function_base_name(name, source)), None)
        }
        "template_function" => (
            unless_cast(text(function.child_by_field_name("name")?, source))?,
            None,
        ),
        _ => return None,
    };

    Some(target).filter(|(callee, _)| !callee.is_empty())
}

fn unless_cast(name: String) -> Option<String> {
    let cast_or_keyword = matches!(
        name.as_str(),
        "static_cast"
            | "dynamic_cast"
            | "reinterpret_cast"
            | "const_cast"
            | "sizeof"
            | "alignof"
            | "decltype"
            | "typeid"
            | "noexcept"
            | "char"
            | "int"
            | "float"
            | "double"
            | "void"
            | "bool"
            | "short"
            | "long"
            | "unsigned"
            | "signed"
    );

    (!cast_or_keyword).then_some(name)
}

/// The names a template's parameter list declares, types and values alike.
pub(super) fn template_parameters(template: Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(list) = template.child_by_field_name("parameters") else {
        return Vec::new();
    };
    child_nodes(list)
        .filter_map(|parameter| template_parameter_name(parameter, source))
        .map(|name| text(name, source))
        .collect()
}

fn template_parameter_name<'t>(parameter: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    let mut parameter = parameter;
    while parameter.kind() == "template_template_parameter_declaration" {
        parameter = child_nodes(parameter)
            .find(|child| child.kind().ends_with("type_parameter_declaration"))?;
    }
    match parameter.kind() {
        "type_parameter_declaration" | "variadic_type_parameter_declaration" => {
            child_nodes(parameter).find(|child| child.kind() == "type_identifier")
        }
        "optional_type_parameter_declaration" => parameter.child_by_field_name("name"),
        "parameter_declaration"
        | "optional_parameter_declaration"
        | "variadic_parameter_declaration" => {
            let declarator = parameter.child_by_field_name("declarator")?;
            let declarator = match declarator.kind() {
                "variadic_declarator" => declarator.named_child(0)?,
                _ => declarator,
            };
            data_declarator(declarator, source).map(|(name, _)| name)
        }
        _ => None,
    }
}

pub(super) fn include_spec(include: Node<'_>, source: &[u8]) -> Option<(String, u32)> {
    let path = include.child_by_field_name("path")?;
    let form = match path.kind() {
        "string_literal" => QUOTED_INCLUDE,
        "system_lib_string" => BRACKET_INCLUDE,
        _ => return None,
    };

    let written = String::from_utf8_lossy(&source[path.byte_range()]);
    let spec = written.trim_matches(|c| matches!(c, '"' | '<' | '>'));
    (!spec.is_empty()).then(|| (spec.to_owned(), form))
}
