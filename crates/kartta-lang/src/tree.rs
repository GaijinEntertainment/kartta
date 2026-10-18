//! Parsing and walking a tree-sitter tree, for a language that extracts through tree-sitter.

use std::cell::RefCell;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::fingerprint::{OFFSET, fnv};

/// The tree of `source` in `grammar`, or `None` when this tree-sitter cannot load the grammar.
///
/// The parse runs on the parser this thread keeps for the grammar, which the thread makes the first time it parses
/// with it.
#[must_use]
pub fn parse(grammar: &Language, source: &[u8]) -> Option<Tree> {
    thread_local! {
        static PARSERS: RefCell<Vec<Parser>> = const { RefCell::new(Vec::new()) };
    }

    PARSERS.with_borrow_mut(|parsers| {
        let kept = parsers.iter().position(|parser| {
            parser
                .language()
                .is_some_and(|language| *language == *grammar)
        });
        let at = if let Some(at) = kept {
            at
        } else {
            let mut parser = Parser::new();
            parser.set_language(grammar).ok()?;
            parsers.push(parser);
            parsers.len() - 1
        };
        parsers[at].parse(source, None)
    })
}

/// The [fingerprint](crate::Language::fingerprint) of a language that extracts through `grammar`.
///
/// It holds `source_id`, the identity of the language crate's own code that
/// [`set_source_id`](crate::fingerprint::set_source_id) hands its build, then the grammar's version, a digest of its
/// `node_types` and its counts of ABI version, node kinds, parse states and fields. It cannot see a grammar built from
/// changed sources that keeps all of those, nor a change to the tree-sitter runtime that keeps its version: a crate
/// that updates either bumps its own version, which `source_id` holds.
#[must_use]
pub fn fingerprint(source_id: &str, grammar: &Language, node_types: &str) -> Vec<u8> {
    let version = grammar.metadata().map_or([0; 3], |version| {
        [
            version.major_version,
            version.minor_version,
            version.patch_version,
        ]
    });
    let counts = [
        grammar.abi_version(),
        grammar.node_kind_count(),
        grammar.parse_state_count(),
        grammar.field_count(),
    ];

    let mut fingerprint = source_id.as_bytes().to_vec();
    fingerprint.extend_from_slice(&version);
    fingerprint.extend_from_slice(&fnv(OFFSET, node_types.as_bytes()).to_le_bytes());
    for count in counts {
        fingerprint.extend_from_slice(&u64::try_from(count).unwrap_or(u64::MAX).to_le_bytes());
    }
    fingerprint
}

/// Calls `visit` with `node` and every node under it in pre-order, and descends into a node's children only when
/// `visit` returns `true` for it.
///
/// A cursor yields nodes already in pre-order, which is the order a walk that keeps a stack of enclosing scopes
/// needs: a scope is open while the nodes visited start before its end. A node lives as long as the tree, so such a
/// stack can hold the enclosing nodes themselves.
pub fn preorder<'t>(node: Node<'t>, mut visit: impl FnMut(Node<'t>) -> bool) {
    let mut cursor = node.walk();

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

/// The one-based line of `row`, a zero-based row of a tree-sitter point, and `u32::MAX` for a row past it.
#[must_use]
pub fn line(row: usize) -> u32 {
    u32::try_from(row).map_or(u32::MAX, |row| row.saturating_add(1))
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

/// `node`'s children, in order.
pub fn child_nodes(node: Node<'_>) -> impl DoubleEndedIterator<Item = Node<'_>> {
    (0..node.child_count()).filter_map(move |index| node.child(index))
}

/// `node`'s named children, in order.
#[expect(
    clippy::cast_possible_truncation,
    reason = "tree-sitter counts a node's children in a uint32_t, which the binding only widens to usize"
)]
pub fn named_child_nodes(node: Node<'_>) -> impl DoubleEndedIterator<Item = Node<'_>> {
    (0..node.named_child_count() as u32).filter_map(move |index| node.named_child(index))
}
