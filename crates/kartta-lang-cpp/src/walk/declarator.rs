use kartta_lang::tree::{children, signature, text};
use kartta_lang::{Symbol, SymbolKind};
use tree_sitter::Node;

use super::exported_symbol;
use super::name::{function_base_name, is_simple_identifier, sanitize_name, type_base_name};
use crate::repair::is_macro_name;

struct DeclaredName {
    name: String,
    scope: Option<String>,
}

pub(super) fn function_symbol(node: Node<'_>, source: &[u8]) -> Option<Symbol> {
    let DeclaredName { name, scope } =
        declared_name(real_function_declarator(node, source)?, source)?;

    let mut symbol = exported_symbol(name, SymbolKind::Function, node);
    if let Some(scope) = scope {
        symbol.kind = SymbolKind::Method;
        symbol.qualified_name = format!("{scope}::{}", symbol.name);
        symbol.owner = Some(scope);
    }
    symbol.signature = signature(node, source);
    Some(symbol)
}

fn declared_name(function: Node<'_>, source: &[u8]) -> Option<DeclaredName> {
    declared_name_of(function.child_by_field_name("declarator")?, source)
}

fn declared_name_of(declarator: Node<'_>, source: &[u8]) -> Option<DeclaredName> {
    let declared = match declarator.kind() {
        "identifier" | "field_identifier" | "operator_name" | "destructor_name"
        | "template_function" | "template_method" => DeclaredName {
            name: function_base_name(declarator, source),
            scope: None,
        },
        "qualified_identifier" if joins_a_macro_as_scope(declarator) => {
            return declared_name_of(declarator.child_by_field_name("name")?, source);
        }
        "qualified_identifier" => {
            let (terminal, scope) = qualified_terminal(declarator);
            let name = terminal
                .map(|terminal| function_base_name(terminal, source))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| sanitize_name(&text(declarator, source)));
            let scope = scope
                .map(|scope| type_base_name(scope, source))
                .filter(|scope| is_simple_identifier(scope));
            DeclaredName { name, scope }
        }
        "pointer_declarator" | "reference_declarator" => {
            let inner = declarator
                .child_by_field_name("declarator")
                .filter(|inner| inner.kind() == "function_declarator")?;
            return declared_name(inner, source);
        }
        _ => return None,
    };

    (!declared.name.is_empty()).then_some(declared)
}

pub(super) fn real_function_declarator<'t>(node: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    match misparse_real_declarator(node, source) {
        Some(real) => unwrap_to_function_declarator(real),
        None => unwrap_to_function_declarator(node.child_by_field_name("declarator")?),
    }
}

fn misparse_real_declarator<'t>(node: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    let field = node.child_by_field_name("declarator")?;
    if leads_with_a_macro(node, field, source) {
        return None;
    }
    let evicted = children(node)
        .filter_map(|index| node.child(index))
        .filter(|child| child.is_error() && child.end_byte() <= field.start_byte())
        .find_map(declarator_in_error);
    if let Some(inner) = evicted {
        return Some(first_of_declarator_run(inner));
    }

    let previous = node.prev_sibling()?;
    if node.child_by_field_name("type").is_some()
        || previous.end_position().row != node.start_position().row
    {
        return None;
    }
    if previous.is_error() {
        return declarator_in_error(previous);
    }
    (previous.kind() == "function_declarator"
        && node.parent().is_some_and(|parent| parent.is_error()))
    .then_some(previous)
}

// constraint: a macro before the return type, `JPH_INLINE float Tan(float x)`, parses as the type and pushes the real
// type into an error, where an annotation after the name, `int size_ GUARDED_BY(mtx)`, parses as the declarator and
// pushes the real name there; which of the two is spelled as a macro tells them apart
fn leads_with_a_macro(node: Node<'_>, field: Node<'_>, source: &[u8]) -> bool {
    let macro_type = node.child_by_field_name("type").is_some_and(|written| {
        written.kind() == "type_identifier" && is_macro_name(&source[written.byte_range()])
    });
    macro_type
        && unwrap_to_function_declarator(field)
            .and_then(|function| function.child_by_field_name("declarator"))
            .is_some_and(|name| !is_macro_name(&source[name.byte_range()]))
}

fn declarator_in_error(error: Node<'_>) -> Option<Node<'_>> {
    children(error)
        .filter_map(|index| error.child(index))
        .rfind(|inner| {
            matches!(
                inner.kind(),
                "function_declarator" | "init_declarator" | "identifier" | "field_identifier"
            )
        })
}

fn first_of_declarator_run(last: Node<'_>) -> Node<'_> {
    let mut first = last;
    while first.kind() == "function_declarator"
        && let Some(previous) = first.prev_sibling()
        && previous.kind() == "function_declarator"
    {
        first = previous;
    }
    first
}

fn unwrap_to_function_declarator(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "function_declarator" => return Some(node),
            "pointer_declarator" | "reference_declarator" => node = inner_declarator(node)?,
            _ => return None,
        }
    }
}

fn inner_declarator(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("declarator").or_else(|| {
        children(node)
            .filter_map(|index| node.child(index))
            .find(|child| {
                matches!(
                    child.kind(),
                    "function_declarator" | "pointer_declarator" | "reference_declarator"
                )
            })
    })
}

// constraint: a macro between the return type and the name, `XMMATRIX XM_CALLCONV XMMatrixRotationZ()`, parses as a
// scope that the parser joins to the name with a `::` the source does not hold
pub(super) fn joins_a_macro_as_scope(qualified: Node<'_>) -> bool {
    children(qualified)
        .filter_map(|index| qualified.child(index))
        .any(|child| child.is_missing())
}

pub(super) fn qualified_terminal(qualified: Node<'_>) -> (Option<Node<'_>>, Option<Node<'_>>) {
    let mut name = None;
    let mut scope = None;
    let mut current = Some(qualified);

    while let Some(node) = current.filter(|node| node.kind() == "qualified_identifier") {
        scope = node.child_by_field_name("scope");
        name = node.child_by_field_name("name");
        current = name;
    }

    (name, scope)
}

pub(super) fn is_pure_virtual(node: Node<'_>) -> bool {
    node.child_by_field_name("declarator")
        .is_some_and(|declarator| declarator.kind() == "function_declarator")
        && node
            .child_by_field_name("default_value")
            .is_some_and(|value| value.kind() == "number_literal")
}

pub(super) fn is_macro_invocation(node: Node<'_>, symbol: &Symbol) -> bool {
    if node.child_by_field_name("type").is_some() {
        return false;
    }

    symbol.owner.as_deref().is_none_or(|owner| {
        !(symbol.name == owner
            || symbol.name.strip_prefix('~') == Some(owner)
            || symbol.name.starts_with("operator"))
    })
}

/// The scope a function's declarator writes before its name where it holds more than one name, each by its base
/// name: `A::N` of `void A::N::f() {}`.
pub(super) fn nested_scope(node: Node<'_>, source: &[u8]) -> Option<String> {
    let name = real_function_declarator(node, source)?.child_by_field_name("declarator")?;
    let scopes = std::iter::successors(Some(name), |qualified| {
        qualified.child_by_field_name("name")
    })
    .take_while(|qualified| qualified.kind() == "qualified_identifier")
    .filter_map(|qualified| qualified.child_by_field_name("scope"));
    scopes.clone().nth(1)?;
    Some(
        scopes
            .map(|scope| type_base_name(scope, source))
            .collect::<Vec<_>>()
            .join("::"),
    )
}
