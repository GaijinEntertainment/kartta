//! Repository file index with glob, fuzzy and regex queries over a single filesystem walk.
//!
//! One walk builds one snapshot, and every surface answers from it — the path queries, the content query, and the
//! directory listing never re-stat the tree. Construction takes an OS path and reads the real filesystem; there is no
//! virtual-filesystem abstraction to mock.
//!
//! # Conventions
//!
//! - Paths are forward-slash and repository-relative on every platform. Windows support is a hard requirement, so no
//!   surface returns an OS-native separator.
//! - Sizes are read at walk time. `Some(0)` is a genuinely empty file and `None` is a size that could not be read;
//!   the two are different index states and are never conflated.
//! - Gitignore handling belongs to the `ignore` crate, fuzzy ranking to `kartta-fuzz`, and regex content matching to
//!   ripgrep's engine crates. Same dialects and diagnostics as those tools, by delegation rather than reimplementation.
//! - Import resolution is not this crate's concern: a specifier-to-file mapping is language semantics and lives with
//!   the language extractors. What lives here are the mechanical primitives a resolver queries.
//!
//! # Symlinks
//!
//! One rule on every surface: a symlink is **reported, never read through**. Its content is either already indexed
//! under the target's own path (an in-tree target) or is not the repository's at all (an out-of-tree target), so
//! following one either duplicates a file under a second path or reads outside the tree.
//!
//! In the tree a symlink is a leaf: the raw target is captured by readlink, one following stat captures the target's
//! type and size, a directory link is never descended, and a dangling link is dropped outright. Listed is therefore not
//! the same as readable — a symlink found by a path query produces no content match, by rule rather than by accident.
//!
//! Because the index is a snapshot, a regular file replaced by a symlink after the walk is still followed when the
//! content query opens it. That race is accepted, not mitigated: no portable open-without-follow exists, and it takes a
//! writer mutating the tree mid-query, who could put the same content in a regular file instead.
