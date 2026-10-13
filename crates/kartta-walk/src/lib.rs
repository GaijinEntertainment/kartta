//! Parallel filesystem tree walk with caller-controlled descent and cancellation.
//!
//! The walk reads a directory, hands its listing to a [`Policy`], and records whatever the policy keeps. A directory's
//! entries arrive together, its subtrees are walked in parallel, and the result is the tree itself — so parent links
//! never need reconstructing and children are already in order.
//!
//! # Using it
//!
//! Implement [`Policy`] to say what to record and where to descend, then call [`walk`]. It returns a [`WalkOutcome`]:
//! read [`errors`](WalkOutcome::errors) and [`is_cancelled`](WalkOutcome::is_cancelled) to learn whether the answer is
//! complete, then take [`into_tree`](WalkOutcome::into_tree) and descend from [`Tree::children`], following
//! [`Node::children`] down. See the example at the end.
//!
//! # The policy
//!
//! [`Policy`] answers two questions. [`Policy::enter`] derives the state that applies to one directory's entries from
//! its parent's state — this is where an inherited rule set (a `.gitignore` stack, a depth budget, an allow-list)
//! accumulates on the way down. [`Policy::visit`] then classifies each entry: record it and descend
//! ([`Visit::Keep`]), record it but treat it as opaque ([`Visit::Prune`]), or drop it ([`Visit::Skip`]).
//!
//! The walk itself knows nothing about ignore files, hidden entries or name conventions. It reads directories, keeps
//! the answers ordered, and stops when told to.
//!
//! # Paths are forward-slash on every platform
//!
//! The tree this crate returns is an abstraction over the filesystem, not a view of one platform's spelling of it.
//! Every path it reports — [`Entry::path`], [`Dir::path`], [`Node::name`] — is forward-slash separated on Windows
//! exactly as on unix, so a consumer stores, compares, matches and serializes one path form and never branches on the
//! host.
//!
//! That holds by construction rather than by convention. The only string the walk takes from the OS is
//! [`std::fs::DirEntry::file_name`], a single path component, which contains no separator on any platform; the only
//! separator it ever writes is a literal `/`. No full path is ever stringified, so a native separator has no route into
//! a reported path.
//!
//! Native paths stay on the I/O side of that line: [`Entry::native_path`] and [`Dir::native_path`] return an OS
//! [`std::path::Path`], because opening a file needs the platform's own spelling. The rule is one form for identity,
//! the other for access.
//!
//! Names are decoded lossily, so a filename that is not valid UTF-8 reports replacement characters in `path` and `name`
//! while [`Entry::native_path`] still opens it faithfully.
//!
//! # Determinism
//!
//! Entries are sorted by name within each directory before the policy sees them, and subtree results are collected in
//! that same order. Two walks of an unchanged tree therefore produce identical output, whatever the thread count — the
//! parallelism is in the traversal, never in the result.
//!
//! A cancelled walk is the one exception: the flag is noticed wherever the threads happen to be, so which entries it
//! recorded depends on scheduling.
//!
//! # Symlinks are never followed
//!
//! A symlink reports [`EntryKind::Symlink`] and is never descended, even when it points at a directory. The walk stays
//! inside the tree it was given, so symlink loops cannot occur and no cycle detection is needed. A policy that wants
//! the target's type or size reads it itself.
//!
//! # Controls
//!
//! [`WalkOptions::cancel_token`] is the only way to end a walk before it runs out of entries; the result is the partial
//! tree, which [`WalkOutcome::is_cancelled`] marks as a truncated answer rather than an error. There is deliberately no
//! cap on entries recorded — [`WalkOptions`] explains why.
//!
//! [`WalkOptions::threads`] fixes how many threads walk the tree, for a caller that does not own the machine. Left
//! `None`, the walk uses the ambient rayon pool.
//!
//! # Errors
//!
//! Two failures are fatal: a root that cannot be listed, and a thread pool that cannot be started. Everything deeper —
//! a directory that cannot be read, an entry that cannot be read, an entry whose file type cannot be read — is
//! collected into [`WalkOutcome::errors`] and the walk continues, so one unreadable subdirectory costs its subtree
//! rather than the whole answer.
//!
//! # Scope
//!
//! The crate walks and reports structure; it does not model what it finds. The payload on each node is whatever the
//! [`Policy`] chose, so the entry model — kinds, sizes, symlink targets — belongs to the consumer, as do a path lookup,
//! any flat projection of the tree, ignore-file semantics, and every query.
//!
//! It exists to feed a filesystem index. A tree is what this returns because a tree is what a traversal discovers,
//! and reconstructing one costs a second owned copy of every path. What the consumer keeps is its own affair: the
//! index this crate feeds flattens the tree into a depth-first list in a single pass and answers even its directory
//! listings from that, on its own side of the boundary.
//!
//! # Examples
//!
//! Record every entry, descending everywhere:
//!
//! ```
//! use kartta_walk::{Dir, Entry, EntryKind, Node, Policy, Visit, WalkOptions, walk};
//!
//! struct All;
//!
//! impl Policy for All {
//!     type State = ();
//!     type Value = EntryKind;
//!
//!     fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}
//!
//!     fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
//!         Visit::Keep(entry.kind())
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let dir = tempfile::tempdir()?;
//! std::fs::write(dir.path().join("a.txt"), "hello")?;
//!
//! // The report is checked, then the tree is what gets kept.
//! let tree = walk(dir.path(), &All, &(), &WalkOptions::default())?.into_tree();
//! let names: Vec<&str> = tree.children().map(Node::name).collect();
//! assert_eq!(names, ["a.txt"]);
//! # Ok(())
//! # }
//! ```

mod cancel;
mod error;
mod policy;
mod tree;
mod walk;

pub use crate::cancel::CancelToken;
pub use crate::error::WalkError;
pub use crate::policy::{Dir, Entry, EntryKind, Policy, Visit};
pub use crate::tree::{Children, Node, NodeId, Tree};
pub use crate::walk::{WalkOptions, WalkOutcome, walk};
