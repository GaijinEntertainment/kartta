//! Walking a tree-sitter tree, for a language that extracts through tree-sitter.

use std::ops::Range;

use tree_sitter::{Node, Tree};

/// Calls `visit` with every node of `tree` in pre-order, and descends into a node's children only when `visit`
/// returns `true` for it.
///
/// A cursor yields nodes already in pre-order, which is the order a walk that keeps a stack of enclosing scopes
/// needs: a scope is open while the nodes visited start before its end. A node lives as long as the tree, so such a
/// stack can hold the enclosing nodes themselves.
pub fn preorder<'t>(tree: &'t Tree, mut visit: impl FnMut(Node<'t>) -> bool) {
    let mut cursor = tree.walk();

    loop {
        if visit(cursor.node()) && cursor.goto_first_child() {
            continue;
        }

        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// The text of `node` in `source`, decoded lossily where it is not UTF-8, so a name with a stray byte keeps its
/// symbol rather than losing it.
#[must_use]
pub fn text(node: Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.byte_range()]).into_owned()
}

/// The declaration's text up to its `body` field, or to its end without one, a trailing `;` dropped: the signature
/// of a symbol. `None` when nothing is left.
#[must_use]
pub fn signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    signature_from(node.start_byte(), node, source)
}

/// [`signature`] starting at the byte `start` rather than at `node`, while `node`'s body still ends it: the signature
/// of a function bound to a name, which starts at the name rather than at the anonymous function.
#[must_use]
pub fn signature_from(start: usize, node: Node<'_>, source: &[u8]) -> Option<String> {
    let end = node
        .child_by_field_name("body")
        .map_or_else(|| node.end_byte(), |body| body.start_byte());
    let text = String::from_utf8_lossy(&source[start..end]);
    let text = text.trim_end().trim_end_matches(';').trim_end();

    (!text.is_empty()).then(|| text.to_owned())
}

/// The indices of `node`'s children, in the type [`Node::child`] takes.
#[must_use]
pub fn children(node: Node<'_>) -> Range<u32> {
    0..node.child_count()
}

/// The indices of `node`'s named children, in the type [`Node::named_child`] takes.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "tree-sitter counts a node's children in a uint32_t, which the binding only widens to usize"
)]
pub fn named_children(node: Node<'_>) -> Range<u32> {
    0..node.named_child_count() as u32
}
