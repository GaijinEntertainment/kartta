use std::error::Error;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fmt, io};

use kartta_walk::WalkError;

use crate::binary::BinaryExtensions;
use crate::content_index::ContentIndexStorage;
use crate::index::Stopped;
use crate::pruned::PrunedNames;

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
    /// `.git` itself is still skipped — that is a repository boundary rather than an ignore rule — and so is every
    /// name in [`BuildOptions::pruned_names`], which the walk prunes before any rule is consulted.
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
    /// The entry names the build neither indexes nor descends. Defaults to [`PrunedNames::builtin`], the clutter
    /// editors and the operating system leave in a tree.
    ///
    /// A name is pruned before any ignore rule is consulted, so no `.gitignore` negation brings it back and
    /// [`IgnoreSources::None`] prunes it too. While ignore rules apply, a pruned `.gitignore` still applies its rules,
    /// as an ignored one does. `.git` is pruned whatever the set holds. A refresh prunes the names the build was given,
    /// and so does a `Watcher` on Linux, which watches no directory they name.
    pub pruned_names: PrunedNames,
    /// How many threads the index runs on — the walk that builds it, the reads during that walk, and every parallel
    /// search after. `None` runs on the ambient rayon pool, which takes every core the machine reports unless the
    /// process configured it otherwise.
    ///
    /// Set it wherever the reported count is not the count the process may use: a container runtime that misreports
    /// its CPU quota reports the host's cores, and an index sized by that would oversubscribe the quota it actually has.
    /// A named count gets the index a pool of exactly that size, kept for its lifetime, so concurrent searches share
    /// those threads rather than each starting more; [`Index::threads`](crate::Index::threads) reports the count. The
    /// count bounds the threads that compute: a refresh of named paths on a mount that answers in round trips also
    /// stats them on up to 32 threads of the call's own, which wait for the filesystem, as
    /// [`Index::refresh_paths`](crate::Index::refresh_paths) states.
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
    ///
    /// A failure to create, write or map a scratch file costs only this index: the build collects it as
    /// [`BuildError::ContentIndex`] and answers as one that was asked for none, with
    /// [`Index::content_index_failure`](crate::Index::content_index_failure) saying why.
    pub content_index: Option<ContentIndexStorage>,
    /// A [`kartta_cache`] store that per-file data is shared through, with every build that uses the same directory —
    /// in this process or any other, of this tree or another. The content index takes each text file's trigrams from
    /// it, and a [`ContentVisitor`](crate::ContentVisitor) is lent it through
    /// [`Content::cached`](crate::Content::cached) and [`Content::cache`](crate::Content::cache). `None`, the default,
    /// shares nothing. A build with neither a content index nor a visitor does not open it. The directory is created
    /// when absent, and must allow hard links.
    ///
    /// An entry is keyed by the blake3 hash of a file's content and by a namespace naming what computed it, never by
    /// its path, so checkouts a few files apart share every other file's entry, and a version of this crate, or of a
    /// visitor, that computes differently keeps entries of its own. A file whose trigrams the store holds is not
    /// scanned. The build computes the rest and publishes them, with what the visitor cached, into the directory as one
    /// pack once its walk is done, then merges the directory's packs when they pass eight and removes the entries older
    /// than [`BuildOptions::content_cache_ttl`]; a refresh does the same for the files it reads.
    ///
    /// The cache is a trade. Each file the build reads for the content index or a visitor is hashed, and a file only
    /// the content index reads that runs past one 64 KiB chunk is read twice when the store lacks it — once to hash it,
    /// and once to scan and hash it again — so the read still holds one chunk at a time. A build that finds nothing in
    /// the store pays for hashing, encoding and writing every entry; one that finds everything pays for hashing and the
    /// lookups in place of scanning. Where the build is bound by its CPUs, as under a container's quota, that saves
    /// what scanning cost; where opening the files costs most of a build, as on macOS, it changes little.
    ///
    /// A failure to open, write or publish to the directory costs only the sharing: the build computes what it could
    /// not take from the store, and reports the failure as [`BuildError::Cache`].
    pub content_cache: Option<PathBuf>,
    /// The age past which an entry of [`BuildOptions::content_cache`] expires, counted from when the build that
    /// computed it began. Zero, the default, is [`kartta_cache::DEFAULT_TTL`], a week, and never "expire everything":
    /// zero is what an unset number decodes to, and that reading would empty a cache other builds share.
    ///
    /// The merge every build ends with expires what is older. A build that finds an entry does not renew it, so an
    /// entry every build finds is computed again at most once per time to live; a shorter one holds the directory
    /// closer to what the trees sharing it contain, and recomputes more often. A pack that holds expired entries is
    /// rewritten without them once they make up half of its payload bytes, and until then they are still found. The
    /// same merge removes the temporary files of builds that never published, once an hour has passed since their last
    /// write and no writer holds their lock, and the packs of another cache format once both the time to live and an
    /// hour have passed since their last write.
    pub content_cache_ttl: Duration,
    /// Whether the index keeps a stamp of each file the build reads — the time it last changed, eight bytes a file — so
    /// that [`Index::refresh`](crate::Index::refresh) reads again only the files that changed since. `false`, the
    /// default, keeps none, and a refresh then reads every wanted file again, as the build did.
    ///
    /// On unix the time is the inode change time, which no write can set back: a tool that keeps modification times,
    /// such as `cp -p` or `tar x`, still changes it. Elsewhere it is the modification time, and an edit that keeps the
    /// file's size and sets its modification time back is carried over unread.
    ///
    /// A build reads files only for a [`ContentVisitor`](crate::ContentVisitor) and for
    /// [`BuildOptions::content_index`], so without either this keeps nothing. A file changed within two seconds before
    /// the build began is read again by the next refresh whatever its stamp says: a filesystem whose clock ticks
    /// coarsely can give a later write the same time.
    pub stamps: bool,
}

/// A failure that stopped a build, or cost it one entry, one subtree, one directory's rules or one file's content.
///
/// [`BuildError::Walk`] is returned for a root that cannot be listed, [`BuildError::ThreadPool`] for a thread pool that
/// cannot be started, [`BuildError::Stopped`] by [`Index::build_with`](crate::Index::build_with) when its cancel token
/// stopped it, [`BuildError::Cancelled`] by [`Index::refresh`](crate::Index::refresh) and
/// [`Index::refresh_paths`](crate::Index::refresh_paths) when theirs stopped them, [`BuildError::ContentIndex`] by both
/// refreshes when the content index cannot be kept where it was asked to be, [`BuildError::ContentIdsExhausted`] by both
/// refreshes when the index has no ids left, and
/// [`BuildError::Misspelled`] by [`Index::refresh_paths`](crate::Index::refresh_paths) for a path not spelled as the
/// index spells one; [`Index::build`](crate::Index::build) takes no token and returns only the first two.
/// Everything else is collected into [`BuildOutcome::errors`](crate::BuildOutcome::errors) and costs only what it names,
/// so one unreadable directory yields a partial index rather than none.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The walk failed — outright when returned, or for one directory or entry when collected.
    #[error(transparent)]
    Walk(#[from] WalkError),
    /// The thread pool [`BuildOptions::threads`] asked for could not be started, so the build never began.
    ///
    /// A restricted environment is where this happens: a process or cgroup thread limit, or a sandbox that refuses to
    /// spawn. The build fails rather than quietly running at a parallelism the caller did not ask for.
    #[error("cannot start the index's thread pool of {threads}: {source}")]
    ThreadPool {
        /// The thread count that was requested.
        threads: NonZeroUsize,
        /// The underlying failure, boxed so no dependency's error type reaches this crate's public API.
        source: Box<dyn Error + Send + Sync>,
    },
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
    /// Collected by a build, which goes on to hand back every other thing it made, so a caller can use what it has and
    /// build again for the index. Returned by a refresh, which leaves the index answering as it did. The directory has
    /// to exist and have room for the index and what the build spills while it makes it.
    #[error("cannot keep the content index in {}: {source}", native_path.display())]
    ContentIndex {
        /// The directory the scratch files were to be created in.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// The cancel token stopped a build, which hands back what it had made.
    ///
    /// Returned rather than collected, because the index it holds is not the one the caller asked for: [`Stopped`]
    /// says how much it lacks. A visitor may have been handed some files' content by then.
    #[error("the build was cancelled")]
    Stopped(Box<Stopped>),
    /// The cancel token stopped a refresh, which leaves the index answering as it did.
    #[error("the refresh was cancelled")]
    Cancelled,
    /// A refresh needed a [`ContentId`](crate::ContentId) past the last one the index can hand out.
    ///
    /// Returned rather than collected, and only by [`Index::refresh`](crate::Index::refresh) and
    /// [`Index::refresh_paths`](crate::Index::refresh_paths). An id never returns to use within one index, so an index
    /// runs out once its build and refreshes have read about four billion files in all, and every refresh that reads a
    /// file fails from then on. A new build numbers its reads from the start again.
    #[error("the index has no content ids left to number the files a refresh reads")]
    ContentIdsExhausted,
    /// A path given to [`Index::refresh_paths`](crate::Index::refresh_paths) is not spelled as the index spells a
    /// path, from the root with `/` between names, so it names no part of the tree or may lead out of it. Returned
    /// before anything is walked, so the index is left as it was.
    #[error("cannot refresh {path:?}: the path {fault}")]
    Misspelled {
        /// The path as it was given.
        path: String,
        /// What is wrong with its spelling.
        fault: PathFault,
    },
    /// The directory [`BuildOptions::content_cache`] names could not be opened, written, published to or merged, or its
    /// merge could not remove files it should have: merged packs, which then hold their entries twice, temporary files
    /// of killed builds, or packs it does not map.
    ///
    /// Collected rather than returned: a content index is whole, because the build computed every file's trigrams it
    /// could not take from the cache. What it lost is the sharing — what it computed, and what a visitor cached, is
    /// not in the directory for the next build, unless the failure came after publishing, in the merge.
    #[error("cannot share per-file data through {}: {source}", native_path.display())]
    Cache {
        /// The cache directory.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
}

impl BuildError {
    /// The word for the failure in an answer: `walk`, `thread_pool`, `ignore`, `name`, `read`, `content_index`,
    /// `cancelled`, `content_ids_exhausted`, `misspelled` or `cache`.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Walk(_) => "walk",
            Self::ThreadPool { .. } => "thread_pool",
            Self::Ignore { .. } => "ignore",
            Self::Name { .. } => "name",
            Self::Read { .. } => "read",
            Self::ContentIndex { .. } => "content_index",
            Self::Stopped(_) | Self::Cancelled => "cancelled",
            Self::ContentIdsExhausted => "content_ids_exhausted",
            Self::Misspelled { .. } => "misspelled",
            Self::Cache { .. } => "cache",
        }
    }

    /// The native path the failed operation was applied to, or `None` for a failure that names no path.
    ///
    /// Native, and spelled relative to the root exactly as the root itself was given — not the root-relative
    /// forward-slash form the index reports elsewhere. A failure names a place on disk for an operator to go and look
    /// at, and the entry it concerns may have no indexed path at all: [`BuildError::Name`] exists precisely because one
    /// could not be formed.
    ///
    /// Every collected error names a path. Only the returned [`BuildError::ThreadPool`], [`BuildError::Stopped`],
    /// [`BuildError::Cancelled`], [`BuildError::ContentIdsExhausted`] and [`BuildError::Misspelled`] do not.
    #[must_use]
    pub fn native_path(&self) -> Option<&Path> {
        match self {
            Self::Walk(source) => source.native_path(),
            Self::Ignore { native_path, .. }
            | Self::Name { native_path }
            | Self::Read { native_path, .. }
            | Self::ContentIndex { native_path, .. }
            | Self::Cache { native_path, .. } => Some(native_path),
            Self::ThreadPool { .. }
            | Self::Stopped(_)
            | Self::Cancelled
            | Self::ContentIdsExhausted
            | Self::Misspelled { .. } => None,
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
            Self::Misspelled { .. } => 7,
            Self::Cache { .. } => 8,
            Self::ThreadPool { .. } => 9,
            Self::Stopped(_) => 10,
        }
    }
}

/// What keeps a path from naming a part of the tree, as [`BuildError::Misspelled`] names it.
///
/// The index spells a path from the root with `/` between names. The fault's text reads after the path, as in
/// `"C:\a" starts with the drive C:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathFault {
    /// The path starts with `/` or `\`.
    Absolute,
    /// The path starts with a drive, such as `C:`, here its letter.
    Drive(char),
    /// A `\` separates the path's names, where the index reads `/` alone, so the path names one entry at the root.
    Backslash,
    /// A name of the path is `..`, which may lead out of the tree.
    ParentName,
}

impl fmt::Display for PathFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute => f.write_str("is absolute"),
            Self::Drive(letter) => write!(f, "starts with the drive {letter}:"),
            Self::Backslash => f.write_str("separates its names with \\"),
            Self::ParentName => f.write_str("holds a .. name, which may leave the root"),
        }
    }
}

impl PathFault {
    /// The first fault of `path`, in the order the variants are declared, or `None` for a path spelled as the index
    /// spells one. A name of `.` or an empty name is no fault: the index reads `./a` and `a//b` as `a` and `a/b`.
    #[must_use]
    pub fn of(path: &str) -> Option<Self> {
        if path.starts_with(['/', '\\']) {
            return Some(Self::Absolute);
        }
        if let [drive, b':', ..] = path.as_bytes()
            && drive.is_ascii_alphabetic()
        {
            return Some(Self::Drive(char::from(*drive)));
        }
        if path.contains('\\') {
            return Some(Self::Backslash);
        }
        path.split('/')
            .any(|name| name == "..")
            .then_some(Self::ParentName)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_spelled_from_the_root_with_slashes_has_no_fault() {
        for path in ["pkg/a.go", "pkg/..a.go", "./pkg//a.go", "", "pkg/C:x"] {
            assert_eq!(PathFault::of(path), None, "{path:?}");
        }
    }

    #[test]
    fn a_misspelled_path_has_its_first_fault() {
        for (path, fault) in [
            ("/r/pkg", PathFault::Absolute),
            ("\\r\\a.go", PathFault::Absolute),
            ("C:\\r", PathFault::Drive('C')),
            ("c:src\\a.rs", PathFault::Drive('c')),
            ("pkg\\a.go", PathFault::Backslash),
            ("../a.go", PathFault::ParentName),
            ("pkg/../../a.go", PathFault::ParentName),
        ] {
            assert_eq!(PathFault::of(path), Some(fault), "{path:?}");
        }
    }

    #[test]
    fn a_misspelled_path_is_refused_by_its_fault() {
        let refused = BuildError::Misspelled {
            path: "C:\\r".to_owned(),
            fault: PathFault::Drive('C'),
        };

        assert_eq!(
            refused.to_string(),
            r#"cannot refresh "C:\\r": the path starts with the drive C:"#
        );
    }

    #[test]
    fn a_pool_that_cannot_start_is_named_as_the_index_s_whole_pool() {
        let error = BuildError::ThreadPool {
            threads: NonZeroUsize::new(4).expect("four"),
            source: "the process thread limit".into(),
        };

        assert_eq!(
            error.to_string(),
            "cannot start the index's thread pool of 4: the process thread limit"
        );
        assert_eq!(error.label(), "thread_pool");
    }

    #[test]
    fn a_failure_that_names_no_path_has_a_label_of_its_own() {
        let misspelled = BuildError::Misspelled {
            path: "../x".to_owned(),
            fault: PathFault::ParentName,
        };

        assert_eq!(misspelled.label(), "misspelled");
        assert_eq!(BuildError::Cancelled.label(), "cancelled");
        assert_eq!(
            BuildError::ContentIdsExhausted.label(),
            "content_ids_exhausted"
        );
    }

    #[test]
    fn a_failure_at_a_path_has_a_label_of_its_own() {
        let native_path = || PathBuf::from("a");
        let source = || io::Error::other("failed");

        let name = BuildError::Name {
            native_path: native_path(),
        };
        assert_eq!(name.label(), "name");
        for (error, label) in [
            (
                BuildError::Ignore {
                    native_path: native_path(),
                    source: source(),
                },
                "ignore",
            ),
            (
                BuildError::Read {
                    native_path: native_path(),
                    source: source(),
                },
                "read",
            ),
            (
                BuildError::ContentIndex {
                    native_path: native_path(),
                    source: source(),
                },
                "content_index",
            ),
            (
                BuildError::Cache {
                    native_path: native_path(),
                    source: source(),
                },
                "cache",
            ),
        ] {
            assert_eq!(error.label(), label);
        }
    }
}
