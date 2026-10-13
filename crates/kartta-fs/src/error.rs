//! [`BuildError`] — a failure that cost a build, or part of one.

use std::io;
use std::path::{Path, PathBuf};

use kartta_walk::WalkError;

/// A failure that stopped a build, or cost it one entry, one subtree or one directory's rules.
///
/// Only [`BuildError::Walk`] is ever returned from [`Index::build`](crate::Index::build), and only for the two
/// failures the walk itself treats as fatal: a root that cannot be listed, and a thread pool that cannot be started.
/// Everything else is collected into [`BuildOutcome::errors`](crate::BuildOutcome::errors) and costs only what it
/// names, so one unreadable directory yields a partial index rather than none.
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
    #[error("cannot read the ignore file {}: {source}", path.display())]
    Ignore {
        /// The ignore file that could not be read.
        path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// An entry's name is not valid UTF-8, so it was not indexed. A directory takes its subtree with it.
    ///
    /// Such a name has no faithful path: it decodes to replacement characters, which two different names can share, so
    /// indexing it would give two entries one lookup key and a path that opens neither. Dropping it trades a file the
    /// index cannot address for a gap the caller can see.
    #[error("cannot index {}: the name is not valid UTF-8", path.display())]
    Name {
        /// The entry that could not be named.
        path: PathBuf,
    },
}

impl BuildError {
    /// The path the failed operation was applied to, or `None` for a failure that names no path.
    ///
    /// Native, and spelled relative to the root exactly as the root itself was given — not the root-relative
    /// forward-slash form the index reports elsewhere. A failure names a place on disk for an operator to go and look
    /// at, and the entry it concerns may have no indexed path at all: [`BuildError::Name`] exists precisely because one
    /// could not be formed.
    ///
    /// Every collected error names a path. Only a returned [`BuildError::Walk`] wrapping
    /// [`WalkError::ThreadPool`](kartta_walk::WalkError::ThreadPool) does not.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Walk(source) => source.native_path(),
            Self::Ignore { path, .. } | Self::Name { path } => Some(path),
        }
    }

    /// Orders one kind of failure against another, so a report built by racing threads still reads the same twice.
    pub(crate) const fn kind_order(&self) -> u8 {
        match self {
            Self::Walk(_) => 0,
            Self::Ignore { .. } => 1,
            Self::Name { .. } => 2,
        }
    }
}
