use kartta_lang::SymbolKind;
pub(crate) use kartta_lang::tree::{child_nodes, named_child_nodes};
use kartta_lang::tree::{line, text};
use tree_sitter::Node;

const FUNCTION_KINDS: [&str; 4] = [
    "arrow_function",
    "function_expression",
    "function",
    "generator_function",
];

const HOC_WRAPPERS: [&str; 6] = [
    "memo",
    "forwardRef",
    "useCallback",
    "useMemo",
    "observer",
    "debounce",
];

const QUOTES: [char; 3] = ['"', '\'', '`'];

pub(crate) fn start_line(node: Node<'_>) -> u32 {
    line(node.start_position().row)
}

// constraint: the grammar keeps a member's decorators, and those written before `export`, beside the declaration
// rather than inside it, so a declaration starts at the first decorator in the run directly before it
pub(crate) fn declaration_start_line(node: Node<'_>) -> u32 {
    let mut first = node;
    while let Some(decorator) = first
        .prev_named_sibling()
        .filter(|previous| previous.kind() == "decorator")
    {
        first = decorator;
    }
    start_line(first)
}

pub(crate) fn end_line(node: Node<'_>) -> u32 {
    line(node.end_position().row)
}

pub(crate) fn trim_quotes(literal: &str) -> &str {
    let mut chars = literal.chars();
    match (chars.next(), chars.next_back()) {
        (Some(open), Some(close)) if open == close && QUOTES.contains(&open) => chars.as_str(),
        _ => literal,
    }
}

pub(crate) fn unquote(mut literal: String) -> String {
    let inner = trim_quotes(&literal).len();
    if inner < literal.len() {
        literal.truncate(literal.len() - 1);
        literal.remove(0);
    }
    literal
}

fn is_function(node: Node<'_>) -> bool {
    FUNCTION_KINDS.contains(&node.kind())
}

pub(crate) fn function_value<'t>(value: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    if is_function(value) {
        return Some(value);
    }
    if value.kind() != "call_expression" {
        return None;
    }

    let function = value.child_by_field_name("function")?;
    let wrapper = match function.kind() {
        "identifier" => function,
        "member_expression" => function.child_by_field_name("property")?,
        _ => return None,
    };
    if !HOC_WRAPPERS.contains(&text(wrapper, source).as_str()) {
        return None;
    }

    child_nodes(value.child_by_field_name("arguments")?).find(|argument| is_function(*argument))
}

pub(crate) fn type_name(name: Node<'_>, source: &[u8]) -> String {
    let name_text = text(name, source);
    if name.kind() == "string" {
        trim_quotes(&name_text).to_owned()
    } else {
        name_text
    }
}

pub(crate) fn member_name(member: Node<'_>) -> Option<Node<'_>> {
    (member.child_by_field_name("name")).or_else(|| member.child_by_field_name("property"))
}

pub(crate) fn is_member_exported(member: Node<'_>, source: &[u8]) -> bool {
    if member_name(member).is_some_and(|name| name.kind() == "private_property_identifier") {
        return false;
    }
    !child_nodes(member)
        .any(|child| child.kind() == "accessibility_modifier" && text(child, source) == "private")
}

pub(crate) fn is_function_type(annotation: Node<'_>) -> bool {
    named_child_nodes(annotation).any(|annotated| annotated.kind() == "function_type")
}

pub(crate) fn first_string_argument(call: Node<'_>, source: &[u8]) -> Option<String> {
    named_child_nodes(call.child_by_field_name("arguments")?)
        .find(|argument| argument.kind() == "string")
        .map(|argument| text(argument, source))
}

/// The statements outside every body whose blocks can declare a `var`, each holding its blocks among its named
/// children.
pub(crate) const BLOCK_STATEMENTS: [&str; 16] = [
    "statement_block",
    "if_statement",
    "else_clause",
    "try_statement",
    "catch_clause",
    "finally_clause",
    "for_statement",
    "for_in_statement",
    "while_statement",
    "do_statement",
    "labeled_statement",
    "with_statement",
    "switch_statement",
    "switch_body",
    "switch_case",
    "switch_default",
];

/// The left side of a `for (var … in …)` or `for (var … of …)`.
pub(crate) fn declared_var(for_in: Node<'_>) -> Option<Node<'_>> {
    for_in
        .child_by_field_name("kind")
        .filter(|kind| kind.kind() == "var")
        .and_then(|_| for_in.child_by_field_name("left"))
}

// constraint: a module is strict code, and so is a script whose directive prologue, the string statements it opens
// with, holds `"use strict"`
pub(crate) fn is_strict(program: Node<'_>, source: &[u8]) -> bool {
    let mut prologue = named_child_nodes(program)
        .filter(|statement| !matches!(statement.kind(), "comment" | "hash_bang_line"))
        .map_while(|statement| {
            (statement.kind() == "expression_statement")
                .then(|| statement.named_child(0))
                .flatten()
                .filter(|directive| directive.kind() == "string")
        });
    let use_strict = prologue.any(|directive| {
        let literal = &source[directive.byte_range()];
        literal.get(1..literal.len().saturating_sub(1)) == Some(b"use strict".as_slice())
    });
    use_strict
        || named_child_nodes(program)
            .any(|statement| matches!(statement.kind(), "import_statement" | "export_statement"))
}

pub(crate) fn declarator_kind(declaration: Node<'_>) -> SymbolKind {
    if declaration.kind() == "lexical_declaration"
        && child_nodes(declaration).any(|child| child.kind() == "const")
    {
        SymbolKind::Constant
    } else {
        SymbolKind::Variable
    }
}

pub(crate) fn parents(declaration: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut parents = Vec::new();

    for child in child_nodes(declaration) {
        match child.kind() {
            // constraint: JavaScript's grammar holds the expression `extends` takes in the heritage itself, and
            // TypeScript's wraps it in a clause, which `parent_name` reads as no name
            "class_heritage" => parents.extend(
                child_nodes(child)
                    .filter(|clause| {
                        matches!(clause.kind(), "extends_clause" | "implements_clause")
                    })
                    .flat_map(child_nodes)
                    .chain(child_nodes(child))
                    .filter_map(|entry| parent_name(entry, source)),
            ),
            "extends_type_clause" => {
                parents.extend(child_nodes(child).filter_map(|entry| parent_name(entry, source)));
            }
            _ => {}
        }
    }

    parents
}

fn parent_name(entry: Node<'_>, source: &[u8]) -> Option<String> {
    match entry.kind() {
        "identifier" | "type_identifier" => Some(text(entry, source)),
        "generic_type" => parent_name(entry.child_by_field_name("name")?, source),
        "member_expression" | "nested_type_identifier" => {
            let last = named_child_nodes(entry).last()?;
            matches!(
                last.kind(),
                "identifier" | "type_identifier" | "property_identifier"
            )
            .then(|| text(last, source))
        }
        _ => None,
    }
}

pub(crate) fn call_target(call: Node<'_>, source: &[u8]) -> (String, Option<String>) {
    let Some(function) = call.child_by_field_name("function") else {
        return (String::new(), None);
    };

    match function.kind() {
        "identifier" => (text(function, source), None),
        "import" => ("import".to_owned(), None),
        "member_expression" => {
            let receiver = (function.child_by_field_name("object")).map(|object| {
                if object.kind() == "identifier" {
                    text(object, source)
                } else {
                    String::new()
                }
            });
            (function.child_by_field_name("property")).map_or_else(
                || (String::new(), None),
                |property| (text(property, source), receiver),
            )
        }
        _ => (String::new(), None),
    }
}
