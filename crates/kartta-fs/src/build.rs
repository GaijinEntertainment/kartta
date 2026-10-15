use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use kartta_walk::WalkError;

use crate::binary::BinaryExtensions;
use crate::content_index::ContentIndexStorage;

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
    /// How many threads the index runs on — the walk that builds it, the reads during that walk, and every parallel
    /// search after. `None` runs on the ambient rayon pool, which takes every core the machine reports unless the
    /// process configured it otherwise.
    ///
    /// Set it wherever the reported count is not the count the process may use: a container runtime that misreports
    /// its CPU quota reports the host's cores, and an index sized by that would oversubscribe the quota it actually has.
    /// A named count gets the index a pool of exactly that size, kept for its lifetime, so concurrent searches share
    /// those threads rather than each starting more; [`Index::threads`](crate::Index::threads) reports the count.
    ///
    /// The result does not depend on this: entries are name-sorted within each directory whatever the thread count, so
    /// two builds of an unchanged tree produce identical indexes.
    pub threads: Option<NonZeroUsize>,
    /// Which extensions settle a file as binary without it being read. Defaults to [`BinaryExtensions::builtin`].
    pub binary_extensions: BinaryExtensions,
    /// Whether the build also indexes file content by trigram, and where it keeps that index. `None`, the default,
    /// builds none.
    ///
    /// With one, the build reads every regular file the ignore rules keep whose extension is not in
    /// [`BuildOptions::binary_extensions`], once, while the walk is in its directory, and records the trigrams of
    /// each one that is text. That is the read a [`ContentVisitor`](crate::ContentVisitor) is handed when it wants the
    /// file, so a consumer that reads content anyway pays for one read either way. A build without a visitor pays for
    /// all of them: on a tree of sources, reading every text file costs many times what the walk alone does.
    /// [`Index::content_index_stats`](crate::Index::content_index_stats) reports what the index holds.
    pub content_index: Option<ContentIndexStorage>,
}

/// A failure that stopped a build, or cost it one entry, one subtree, one directory's rules or one file's content.
///
/// [`BuildError::Walk`] is returned for the two failures the walk itself treats as fatal — a root that cannot be listed,
/// and a thread pool that cannot be started — [`BuildError::ContentIndex`] when the content index cannot be kept where
/// it was asked to be, [`BuildError::Cancelled`] by [`Index::build_with`](crate::Index::build_with) when its cancel
/// token stopped the build, and [`BuildError::ContentIdsExhausted`] by [`Index::refresh`](crate::Index::refresh) when
/// the index has no ids left; [`Index::build`](crate::Index::build) takes no token and returns only the first two.
/// Everything else is collected into [`BuildOutcome::errors`](crate::BuildOutcome::errors) and costs only what it names,
/// so one unreadable directory yields a partial index rather than none.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The walk failed — outright when returned, or for one directory or entry when collected.
    #[error(transparent)]
    Walk(#[from] WalkError),
    /// A `.gitignore` exists but could not be read, so its rules were not applied.
    ///
    /// This is the one collected failure that leaves the index holding *more* than it should rather than less: every
    /// entry the unread file would have hidden is indexed instead. Reporting it matters more than the rest — a missing
    /// subtree is a visible gap, whereas files git would hide look exactly like files git would keep.
    #[error("cannot read the ignore file {}: {source}", native_path.display())]
    Ignore {
        /// The ignore file that could not be read.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// An entry's name is not valid UTF-8, so it was not indexed. A directory takes its subtree with it.
    ///
    /// Such a name has no faithful path: it decodes to replacement characters, which two different names can share, so
    /// indexing it would give two entries one lookup key and a path that opens neither. Dropping it trades a file the
    /// index cannot address for a gap the caller can see.
    #[error("cannot index {}: the name is not valid UTF-8", native_path.display())]
    Name {
        /// The entry that could not be named.
        native_path: PathBuf,
    },
    /// A file the build read for a [`ContentVisitor`](crate::ContentVisitor) or for the content index could not be
    /// read, so neither holds its content.
    ///
    /// The entry is still indexed; only its content is missing from what the visitor was handed and from the content
    /// index.
    #[error("cannot read {}: {source}", native_path.display())]
    Read {
        /// The file that could not be read.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// The content index could not be kept where [`BuildOptions::content_index`] asked: a scratch file in that
    /// directory could not be created, written or mapped.
    ///
    /// Returned rather than collected, because the build was asked for an index it cannot hand back. The directory has
    /// to exist and have room for the index and what the build spills while it makes it.
    #[error("cannot keep the content index in {}: {source}", native_path.display())]
    ContentIndex {
        /// The directory the scratch files were to be created in.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// The cancel token stopped the build before the walk saw every entry.
    ///
    /// Returned rather than collected: a cancelled walk holds whichever entries its threads had reached, which depends
    /// on scheduling, so there is no partial index worth handing back. A visitor may have been handed some files'
    /// content by then.
    #[error("the build was cancelled")]
    Cancelled,
    /// A refresh needed a [`ContentId`](crate::ContentId) past the last one the index can hand out.
    ///
    /// Returned rather than collected, and only by [`Index::refresh`](crate::Index::refresh). An id never returns to
    /// use within one index, so an index runs out once its build and refreshes have read about four billion files in
    /// all, and every refresh that reads a file fails from then on. A new build numbers its reads from the start again.
    #[error("the index has no content ids left to number the files a refresh reads")]
    ContentIdsExhausted,
}

impl BuildError {
    /// The native path the failed operation was applied to, or `None` for a failure that names no path.
    ///
    /// Native, and spelled relative to the root exactly as the root itself was given — not the root-relative
    /// forward-slash form the index reports elsewhere. A failure names a place on disk for an operator to go and look
    /// at, and the entry it concerns may have no indexed path at all: [`BuildError::Name`] exists precisely because one
    /// could not be formed.
    ///
    /// Every collected error names a path. Only a returned [`BuildError::Walk`] wrapping
    /// [`WalkError::ThreadPool`](kartta_walk::WalkError::ThreadPool), [`BuildError::Cancelled`] and
    /// [`BuildError::ContentIdsExhausted`] do not.
    #[must_use]
    pub fn native_path(&self) -> Option<&Path> {
        match self {
            Self::Walk(source) => source.native_path(),
            Self::Ignore { native_path, .. }
            | Self::Name { native_path }
            | Self::Read { native_path, .. }
            | Self::ContentIndex { native_path, .. } => Some(native_path),
            Self::Cancelled | Self::ContentIdsExhausted => None,
        }
    }

    pub(crate) const fn variant_rank(&self) -> u8 {
        match self {
            Self::Walk(_) => 0,
            Self::Ignore { .. } => 1,
            Self::Name { .. } => 2,
            Self::Read { .. } => 3,
            Self::ContentIndex { .. } => 4,
            Self::Cancelled => 5,
            Self::ContentIdsExhausted => 6,
        }
    }
}
