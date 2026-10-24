use kartta_lang::tree::{child_nodes, line, named_child_nodes, text};
use kartta_lang::{Symbol, SymbolKind};
use tree_sitter::Node;

// constraint: `Symbol::new` copies its name into the qualified name, so the symbol starts unnamed, which allocates
// nothing, and then takes both names
pub(super) fn symbol(
    name: String,
    qualified_name: String,
    kind: SymbolKind,
    node: Node<'_>,
) -> Symbol {
    let mut symbol = Symbol::new(
        String::new(),
        kind,
        line(node.start_position().row),
        line(node.end_position().row),
    );
    symbol.name = name;
    symbol.qualified_name = qualified_name;
    symbol
}

pub(super) fn is_exported(node: Node<'_>, source: &[u8]) -> bool {
    child_nodes(node)
        .find(|child| child.kind() == "visibility_modifier")
        .is_some_and(|visibility| {
            text(visibility, source)
                .strip_prefix("pub")
                .is_some_and(|scope| !is_own_module(scope))
        })
}

// constraint: `pub(self)` and `pub(in self)` keep an item inside its module, as no modifier does
fn is_own_module(scope: &str) -> bool {
    let Some(scope) = scope
        .trim_start()
        .strip_prefix('(')
        .and_then(|scope| scope.strip_suffix(')'))
    else {
        return false;
    };
    let scope = scope.trim();
    scope.strip_prefix("in").map_or(scope, str::trim_start) == "self"
}

pub(super) fn named_child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    named_child_nodes(node).find(|child| child.kind() == kind)
}

// constraint: the item kinds that carry the outer attributes gating test code, which bounds the scan for them
pub(super) fn is_attributable_item(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_signature_item"
            | "impl_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "trait_item"
            | "mod_item"
            | "type_item"
            | "associated_type"
            | "macro_definition"
            | "const_item"
            | "static_item"
            | "foreign_mod_item"
    )
}

// constraint: the `Self` type of an impl is the nominal base of its target; a tuple, `dyn` or function type has no
// single name, so it opens no scope outside a function body and its functions stay free rather than taking an owner
// spelled `(u8, u8)`; a scoped target stays whole, as written, for the module chain to resolve
pub(super) fn impl_type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let base = type_base(node.child_by_field_name("type")?);
    if !matches!(
        base.kind(),
        "type_identifier" | "scoped_type_identifier" | "primitive_type"
    ) {
        return None;
    }
    let name = text(base, source);
    (!name.is_empty()).then_some(name)
}

// constraint: a generic type reduces to its base and a reference, pointer or array to its element, so `&[T]` names
// `T` rather than carrying the wrapper syntax
pub(super) fn type_base(node: Node<'_>) -> Node<'_> {
    let inner = match node.kind() {
        "generic_type" | "reference_type" | "pointer_type" => node.child_by_field_name("type"),
        "array_type" => node.child_by_field_name("element"),
        _ => None,
    };
    inner.map_or(node, type_base)
}

// constraint: a function that returns `Result<T, E>` constructs `T`, as a wasm-bindgen `catch` constructor does
pub(super) fn constructed_type<'tree>(returned: Node<'tree>, source: &[u8]) -> Node<'tree> {
    let ok = Some(returned)
        .filter(|returned| returned.kind() == "generic_type")
        .filter(|generic| {
            generic
                .child_by_field_name("type")
                .is_some_and(|base| text(base, source) == "Result")
        })
        .and_then(|generic| {
            generic
                .child_by_field_name("type_arguments")?
                .named_child(0)
        });
    type_base(ok.unwrap_or(returned))
}

pub(super) fn last_segment(node: Node<'_>, source: &[u8]) -> String {
    if node.kind() == "scoped_identifier"
        && let Some(name) = node.child_by_field_name("name")
    {
        return text(name, source);
    }
    text(node, source)
}

// constraint: a global path, `::a::B`, starts with an empty segment, and generic arguments are dropped
pub(super) fn path_text(path: Node<'_>, source: &[u8]) -> Option<String> {
    match path.kind() {
        "identifier" | "type_identifier" | "self" | "super" | "crate" => Some(text(path, source)),
        "scoped_identifier" | "scoped_type_identifier" => {
            let prefix = match path.child_by_field_name("path") {
                Some(prefix) => path_text(prefix, source)?,
                None => String::new(),
            };
            let name = path_text(path.child_by_field_name("name")?, source)?;
            Some(format!("{prefix}::{name}"))
        }
        "generic_type" => path_text(path.child_by_field_name("type")?, source),
        _ => None,
    }
}
