//! [`BuildOptions`] — what a build indexes, and which rules decide it.

use std::num::NonZeroUsize;

use crate::binary::BinaryExtensions;

/// Which of git's ignore sources a build honours.
///
/// The variants form a ladder: each one adds a source to the one before it, so they compare in order and
/// `sources >= IgnoreSources::InTree` reads as "at least the in-tree rules apply".
///
/// # Reproducibility is the line the ladder draws
///
/// Everything up to and including [`IgnoreSources::InTree`] is content of the tree itself, so **the same tree indexes
/// identically wherever it sits**. Every source above it — `$GIT_DIR/info/exclude`, `core.excludesFile`, the XDG
/// fallback — is machine-local, so honouring one makes that stop being true: the same tree indexes differently on a
/// laptop and in CI.
///
/// An ignore file that is a symlink out of the tree is refused rather than followed, and reported, so no rule can
/// enter this level from bytes the tree does not carry. A symlink to a target *inside* the tree is followed, as git
/// follows it.
///
/// Two limits on "identically". The guarantee is over the tree on disk, never over a commit: a build reads the working
/// tree and not git's index, so two checkouts of one commit differ wherever their untracked, unignored content does.
/// And it holds across machines that agree on filename case — a control file spelled `.GITIGNORE` governs a subtree on
/// a case-insensitive Windows filesystem and is an ordinary file on unix, which is git's own behaviour and so is this
/// crate's.
///
/// A cache keyed on an index must therefore cover the active source set *and* the content of every source outside the
/// tree. An index built under one set and served under another is a wrong answer that nothing else will catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum IgnoreSources {
    /// No ignore rules at all: every entry under the root is indexed, `.gitignore` files included.
    ///
    /// `.git` itself is still skipped — that is a repository boundary rather than an ignore rule.
    None,
    /// The `.gitignore` files found inside the tree, and nothing else. The same tree indexes identically on any
    /// machine.
    #[default]
    InTree,
}

/// What a build indexes and how it runs.
///
/// Construct from [`BuildOptions::default`] and adjust the fields you need; the type is `#[non_exhaustive]`, so it
/// gains fields without breaking callers.
///
/// # Examples
///
/// ```
/// use kartta_fs::{BuildOptions, IgnoreSources};
///
/// let mut options = BuildOptions::default();
/// options.ignore_sources = IgnoreSources::None;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct BuildOptions {
    /// Which ignore sources apply. Defaults to the reproducible [`IgnoreSources::InTree`].
    pub ignore_sources: IgnoreSources,
    /// How many threads walk the tree. `None` uses the ambient rayon pool.
    ///
    /// The result does not depend on this: entries are name-sorted within each directory whatever the thread count, so
    /// two builds of an unchanged tree produce identical indexes.
    pub threads: Option<NonZeroUsize>,
    /// Which extensions settle a file as binary without it being read. Defaults to [`BinaryExtensions::builtin`].
    pub binary_extensions: BinaryExtensions,
}
