//! [`WalkError`] — a failure that cost a walk, or part of one.

use std::io;
use std::path::{Path, PathBuf};

/// A failure that stopped a walk, or cost it one entry or subtree.
///
/// Two of these are fatal and returned from [`walk`](crate::walk): an unreadable root, and a subtree that leaves the
/// root. Every other failure is collected into [`WalkOutcome::errors`](crate::WalkOutcome::errors) and costs only the
/// affected entry or subtree, so one unreadable directory yields a partial tree instead of nothing.
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
    /// An entry of a listed directory could not be read, so it is missing from the result. The rest of the listing
    /// stands.
    #[error("cannot read an entry of directory {}: {source}", native_path.display())]
    ReadEntry {
        /// The directory whose entry could not be read. The entry itself has no name to report.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// An entry's file type could not be read, so the entry was not recorded.
    #[error("cannot read the file type of {}: {source}", native_path.display())]
    FileType {
        /// The entry whose type could not be read.
        native_path: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },
    /// [`WalkOptions::subtree`](crate::WalkOptions::subtree) names a path with a `..` component, which may lead outside
    /// the root, so the walk never began.
    #[error("cannot walk the subtree {subtree:?}: a `..` component may lead outside the root")]
    Subtree {
        /// The path as it was given.
        subtree: String,
    },
}

impl WalkError {
    /// The native path the failed operation was applied to, or `None` for a failure that names no path.
    ///
    /// Every error in [`WalkOutcome::errors`](crate::WalkOutcome::errors) names a path; only [`WalkError::Subtree`],
    /// which can be returned but never collected, does not.
    #[must_use]
    pub fn native_path(&self) -> Option<&Path> {
        match self {
            Self::ReadDir { native_path, .. }
            | Self::ReadEntry { native_path, .. }
            | Self::FileType { native_path, .. } => Some(native_path),
            Self::Subtree { .. } => None,
        }
    }
}
