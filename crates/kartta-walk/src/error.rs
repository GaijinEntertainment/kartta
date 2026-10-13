//! [`WalkError`] — a failure that cost a walk, or part of one.

use std::error::Error;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// A failure that stopped a walk, or cost it one entry or subtree.
///
/// Two of these are fatal and returned from [`walk`](crate::walk): an unreadable root, and a thread pool that could not
/// be started. Every other failure is collected into [`WalkOutcome::errors`](crate::WalkOutcome::errors) and costs only
/// the affected entry or subtree, so one unreadable directory yields a partial tree instead of nothing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WalkError {
    /// A directory could not be listed, so its entire subtree is missing from the result.
    #[error("cannot read directory {}: {source}", native_path.display())]
    ReadDir {
        /// The directory that could not be listed.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// An entry's type could not be resolved, so the entry was not recorded.
    #[error("cannot read the entry type of {}: {source}", native_path.display())]
    FileType {
        /// The entry whose type could not be read.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// The thread pool [`WalkOptions::threads`](crate::WalkOptions::threads) asked for could not be started, so the
    /// walk never began.
    ///
    /// A restricted environment is where this happens: a process or cgroup thread limit, or a sandbox that refuses to
    /// spawn. The walk fails rather than quietly running at a parallelism the caller did not ask for.
    #[error("cannot start a thread pool of {threads} for the walk: {source}")]
    ThreadPool {
        /// The thread count that was requested.
        threads: NonZeroUsize,
        /// The underlying failure, boxed so no dependency's error type reaches this crate's public API.
        source: Box<dyn Error + Send + Sync>,
    },
}

impl WalkError {
    /// The native path the failed operation was applied to, or `None` for a failure that names no path.
    ///
    /// Every error in [`WalkOutcome::errors`](crate::WalkOutcome::errors) names a path; only
    /// [`WalkError::ThreadPool`], which can be returned but never collected, does not.
    #[must_use]
    pub fn native_path(&self) -> Option<&Path> {
        match self {
            Self::ReadDir { native_path, .. } | Self::FileType { native_path, .. } => {
                Some(native_path)
            }
            Self::ThreadPool { .. } => None,
        }
    }
}
