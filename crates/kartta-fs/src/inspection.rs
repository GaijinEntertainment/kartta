//! [`Inspection`] — the per-entry facts the walk does not collect, read on first ask and kept.

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::binary::named_binary;

/// A slot nothing has read yet.
const UNREAD: u64 = u64::MAX;

/// A slot whose file would not stat. No filesystem addresses this many bytes, so it cannot collide with a real size.
const UNREADABLE: u64 = u64::MAX - 1;

/// How much of a file decides whether it is binary.
///
/// A NUL anywhere in the first block is the rule git and ripgrep both use; what differs is how much they look at.
/// Reading more costs nothing measurable, because the open dominates and a read returns only what the file holds, so
/// the size is chosen for what it catches rather than for what it costs. This is ripgrep's buffer size, which matters
/// because a searcher applying its own rule to the same file must not disagree with this one.
const PROBE: usize = 64 * 1024;

/// A classification slot nothing has read yet.
const UNCLASSIFIED: u8 = 0;

/// The file holds no NUL in its first [`PROBE`] bytes.
const TEXT: u8 = 1;

/// The file holds a NUL, or its name is on the binary extension list.
const BINARY: u8 = 2;

/// The file would not open or would not read.
const UNOPENABLE: u8 = 3;

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
    classes: Vec<AtomicU8>,
}

impl Inspection {
    /// A table of unread slots, one per entry.
    pub(crate) fn new(entries: usize) -> Self {
        Self {
            sizes: (0..entries).map(|_| AtomicU64::new(UNREAD)).collect(),
            classes: (0..entries).map(|_| AtomicU8::new(UNCLASSIFIED)).collect(),
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

    /// Whether the regular file at `path` is binary, settling it on the first ask and answering from the slot after
    /// that.
    ///
    /// `None` when the file would not open or would not read, which is remembered too.
    ///
    /// The caller settles that the entry is a regular file, and passes the indexed path so the extension can decide
    /// without any file being opened.
    pub(crate) fn is_binary(&self, at: usize, indexed: &str, path: &Path) -> Option<bool> {
        let slot = self.classes.get(at)?;

        match slot.load(Ordering::Relaxed) {
            UNCLASSIFIED => {
                let class = classify(indexed, path);
                slot.store(class, Ordering::Relaxed);
                decode(class)
            }
            class => decode(class),
        }
    }
}

/// What a stored classification means to a caller.
const fn decode(class: u8) -> Option<bool> {
    match class {
        TEXT => Some(false),
        BINARY => Some(true),
        _ => None,
    }
}

/// Settles a file's class, by its name where the name is enough and by its first bytes otherwise.
fn classify(indexed: &str, path: &Path) -> u8 {
    if named_binary(indexed) {
        return BINARY;
    }

    let Some(holds_nul) = holds_nul(path) else {
        return UNOPENABLE;
    };

    if holds_nul { BINARY } else { TEXT }
}

/// Whether the first [`PROBE`] bytes of `path` hold a NUL, or `None` when the file would not be read.
///
/// One read, not a loop. A short read leaves the rest of the file unexamined, which is the same bound the constant
/// already imposes and is why this answers about a prefix rather than about a file.
fn holds_nul(path: &Path) -> Option<bool> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; PROBE];
    let read = file.read(&mut head).ok()?;

    Some(head[..read].contains(&0))
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
            classes: self
                .classes
                .iter()
                .map(|slot| AtomicU8::new(slot.load(Ordering::Relaxed)))
                .collect(),
        }
    }
}

impl fmt::Debug for Inspection {
    /// Reports how many slots each table holds rather than the slots, which run to one per indexed entry.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Inspection")
            .field("sizes", &self.sizes.len())
            .field("classes", &self.classes.len())
            .finish()
    }
}
