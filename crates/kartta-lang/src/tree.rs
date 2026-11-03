//! Parsing and walking a tree-sitter tree, for a language that extracts through tree-sitter.

use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Range;

use tree_sitter::{Language, Node, Parser, Tree};

use crate::RawExtraction;
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

/// What `walk` extracts from the tree of `source` in `grammar`, or nothing when this tree-sitter cannot load the
/// grammar.
pub fn extract(
    grammar: &Language,
    source: &[u8],
    walk: impl FnOnce(&Tree, &[u8]) -> RawExtraction,
) -> RawExtraction {
    parse(grammar, source).map_or_else(RawExtraction::default, |tree| walk(&tree, source))
}

/// The [fingerprint](crate::Language::fingerprint) of the calling language crate, built once per process.
///
/// It is [`tree::fingerprint`](crate::tree::fingerprint) over the crate's `SOURCE_ID`, the grammar `$grammar` and the
/// node types `$node_types`. The crate's build script sets `SOURCE_ID` through
/// [`set_source_id`](crate::fingerprint::set_source_id).
#[macro_export]
macro_rules! tree_sitter_fingerprint {
    ($grammar:expr, $node_types:expr) => {{
        static FINGERPRINT: ::std::sync::OnceLock<::std::vec::Vec<u8>> =
            ::std::sync::OnceLock::new();
        FINGERPRINT
            .get_or_init(|| $crate::tree::fingerprint(env!("SOURCE_ID"), &$grammar, $node_types))
    }};
}

/// The [fingerprint](crate::Language::fingerprint) of a language that extracts through `grammar`.
///
/// It holds `source_id`, the identity of the language crate's code and dependency versions that
/// [`set_source_id`](crate::fingerprint::set_source_id) hands its build, then the grammar's version, a digest of its
/// `node_types` and its counts of ABI version, node kinds, parse states and fields. `source_id` follows the versions of
/// the grammar crate and of tree-sitter that the build's `Cargo.lock` resolves, so an update of either changes it. A
/// build that finds no lock follows only the manifest's version requirements, and the rest covers a later release it
/// resolves as far as that release changes its version metadata, node types or counts: many grammars of ABI 14 carry
/// no version metadata.
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

/// The declaration's text up to its `body` field, or to its end without one, its comments and a trailing `;`
/// dropped: the signature of a symbol. `None` when nothing is left.
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
    let text = uncommented(node, start..end, source);
    let text = text.trim_end().trim_end_matches(';').trim_end();

    (!text.is_empty()).then(|| text.to_owned())
}

/// The text of `source[range]`, decoded lossily, without the comments the tree holds in it up to `node`'s end.
///
/// The spaces and tabs before a comment go with it, and a space stays where it parted two words, so `float /*dt*/)`
/// keeps `float)` and `int/*w*/x` keeps `int x`. A comment is an extra node, which the grammar lets stand between any
/// two tokens, whose kind ends in `comment`; another extra, such as a heredoc body, stays.
#[must_use]
pub fn uncommented<'s>(node: Node<'_>, range: Range<usize>, source: &'s [u8]) -> Cow<'s, str> {
    // constraint: the walk below costs 1.2% of a whole tree's extraction, and every shipped grammar's comment token
    // starts with `//`, `/*`, `#`, `--` (Lua, and JavaScript's `<!--` and `-->`) or `=begin` (Ruby), so text that holds
    // none of them holds no comment; a language whose comments open otherwise adds its opener here
    let bytes = &source[range.clone()];
    if !bytes.contains(&b'#')
        && !bytes
            .windows(2)
            .any(|pair| matches!(pair, b"//" | b"/*" | b"--" | b"=b"))
    {
        return String::from_utf8_lossy(bytes);
    }
    let mut covering = node;
    while covering.start_byte() > range.start {
        let Some(parent) = covering.parent() else {
            break;
        };
        covering = parent;
    }

    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii();
    let mut kept: Option<String> = None;
    let mut at = range.start;
    preorder(covering, |inner| {
        if inner.end_byte() <= range.start || inner.start_byte() >= range.end {
            return false;
        }
        let comment = inner.is_extra() && inner.kind().ends_with("comment");
        if comment {
            let text = kept.get_or_insert_with(|| String::with_capacity(range.len()));
            text.push_str(&String::from_utf8_lossy(
                &source[at..inner.start_byte().max(at)],
            ));
            text.truncate(text.trim_end_matches([' ', '\t']).len());
            at = inner.end_byte().min(range.end);
            if text.bytes().next_back().is_some_and(word)
                && source[at..range.end]
                    .first()
                    .is_some_and(|&byte| word(byte))
            {
                text.push(' ');
            }
        }
        !comment
    });
    let Some(mut text) = kept else {
        return String::from_utf8_lossy(&source[range]);
    };
    text.push_str(&String::from_utf8_lossy(&source[at..range.end]));
    Cow::Owned(text)
}

/// `node`'s children, in order.
pub fn child_nodes(node: Node<'_>) -> impl DoubleEndedIterator<Item = Node<'_>> {
    let count = node.child_count();
    let (indexed, listed) = if count > INDEXED_WIDTH {
        (0..0, node.children(&mut node.walk()).collect())
    } else {
        (0..count, Vec::new())
    };
    indexed
        .filter_map(move |index| node.child(index))
        .chain(listed)
}

/// `node`'s named children, in order.
#[expect(
    clippy::cast_possible_truncation,
    reason = "tree-sitter counts a node's children in a uint32_t, which the binding only widens to usize"
)]
pub fn named_child_nodes(node: Node<'_>) -> impl DoubleEndedIterator<Item = Node<'_>> {
    let count = node.named_child_count() as u32;
    let (indexed, listed) = if count > INDEXED_WIDTH {
        (0..0, node.named_children(&mut node.walk()).collect())
    } else {
        (0..count, Vec::new())
    };
    indexed
        .filter_map(move |index| node.named_child(index))
        .chain(listed)
}

// constraint: `ts_node_child` restarts from the first child on each index, and a cursor allocates a stack per call
const INDEXED_WIDTH: u32 = 16;
