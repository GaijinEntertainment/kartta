//! [`Inspection`] — the per-entry facts the walk does not collect, read on first ask and kept.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// A slot nothing has read yet.
const UNREAD: u64 = u64::MAX;

/// A slot whose file would not stat. No filesystem addresses this many bytes, so it cannot collide with a real size.
const UNREADABLE: u64 = u64::MAX - 1;

/// What the index learns by touching a file, one slot per entry.
///
/// Slots align positionally with [`Index::entries`](crate::Index::entries), so reaching one is an index rather than a
/// lookup, and the whole table is allocated once with the index.
///
/// Every slot is read through `&self`. A query holds no exclusive borrow — two of them may run on one index at the
/// same time — so filling a slot cannot take one either. Two threads that race on the same entry both read the
/// filesystem and both store what they read, which costs a syscall and settles on the same answer, where a lock would
/// cost every reader an atomic round trip to save it.
pub(crate) struct Inspection {
    sizes: Vec<AtomicU64>,
}

impl Inspection {
    /// A table of unread slots, one per entry.
    pub(crate) fn new(entries: usize) -> Self {
        Self {
            sizes: (0..entries).map(|_| AtomicU64::new(UNREAD)).collect(),
        }
    }

    /// The size of the regular file at `path`, stat'ing it on the first ask and answering from the slot after that.
    ///
    /// `None` when the file would not stat, which is remembered too: a caller looping over a listing pays for one
    /// failed stat per entry rather than one per pass.
    ///
    /// The caller settles that the entry is a regular file. A directory or a symlink has no size worth reading, and
    /// deciding that here would spend a syscall to learn what the walk already recorded.
    pub(crate) fn size(&self, at: usize, path: &Path) -> Option<u64> {
        let slot = self.sizes.get(at)?;

        match slot.load(Ordering::Relaxed) {
            UNREAD => {
                let size = read_size(path);
                slot.store(size.unwrap_or(UNREADABLE), Ordering::Relaxed);
                size
            }
            UNREADABLE => None,
            size => Some(size),
        }
    }
}

/// The size a regular file reports, without following a symlink that took its place since the walk.
fn read_size(path: &Path) -> Option<u64> {
    Some(std::fs::symlink_metadata(path).ok()?.len())
}

impl Clone for Inspection {
    fn clone(&self) -> Self {
        Self {
            sizes: self
                .sizes
                .iter()
                .map(|slot| AtomicU64::new(slot.load(Ordering::Relaxed)))
                .collect(),
        }
    }
}

impl fmt::Debug for Inspection {
    /// Reports the table's shape rather than its slots, which run to one per indexed entry.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Inspection")
            .field("slots", &self.sizes.len())
            .finish()
    }
}
