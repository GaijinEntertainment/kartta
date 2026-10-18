use kartta_lang::tree::text;
use tree_sitter::Node;

// constraint: an owner is the whole `::` chain of its type from the outermost one, `Geo::Shape`, so two types of one
// name keep two owners; a `class << self` block is transparent, and a method ends the walk, so a type a method holds
// is no member of the type around the method
pub(crate) fn enclosing_type(node: Node<'_>, source: &[u8]) -> Option<String> {
    let chain = type_chain(node, source);
    chain.first().filter(|name| !name.is_empty())?;

    Some(chain_qualified(&chain))
}

// constraint: the `Const` of `def Const.x` names the nearest enclosing type of that name, as Ruby's lexical constant
// lookup finds it; a constant that names no enclosing type keeps its spelling
pub(crate) fn constant_type(node: Node<'_>, constant: String, source: &[u8]) -> String {
    let chain = type_chain(node, source);
    chain
        .iter()
        .position(|name| *name == constant)
        .map_or(constant, |at| chain_qualified(&chain[at..]))
}

pub(crate) fn type_qualification(
    node: Node<'_>,
    name: &str,
    source: &[u8],
) -> (Option<String>, String) {
    enclosing_type(node, source).map_or_else(
        || (None, name.to_owned()),
        |enclosing| {
            let qualified = format!("{enclosing}::{name}");
            (Some(enclosing), qualified)
        },
    )
}

pub(crate) fn in_singleton_class(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            "singleton_class" => return true,
            "class" | "module" | "method" | "singleton_method" => return false,
            _ => ancestor = parent.parent(),
        }
    }

    false
}

fn type_chain(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            "class" | "module" => names.extend(
                parent
                    .child_by_field_name("name")
                    .map(|name| text(name, source)),
            ),
            "method" | "singleton_method" => break,
            _ => {}
        }
        ancestor = parent.parent();
    }

    names
}

fn chain_qualified(nearest_first: &[String]) -> String {
    let outermost_first: Vec<&str> = nearest_first.iter().rev().map(String::as_str).collect();
    outermost_first.join("::")
}
