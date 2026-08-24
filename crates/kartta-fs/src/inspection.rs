//! [`Inspection`] — the per-entry facts the walk does not collect, read on first ask and kept.

use std::fmt;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::binary::named_binary;
use crate::open::open_regular;

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
/// same time — so filling a slot cannot take one either, and two threads racing on one entry both read the filesystem.
/// Only the first of them publishes: the loser answers with the value already in the slot rather than its own read.
/// The two reads can disagree, because a file rewritten between them has two truthful sizes, and keeping both would
/// give one entry two answers and leave the slot holding whichever call finished last.
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
                let read = read_size(path).unwrap_or(UNREADABLE);

                decode_size(
                    slot.compare_exchange(UNREAD, read, Ordering::Relaxed, Ordering::Relaxed)
                        .map_or_else(|published| published, |_| read),
                )
            }
            stored => decode_size(stored),
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
                let read = classify(indexed, path);

                decode_class(
                    slot.compare_exchange(UNCLASSIFIED, read, Ordering::Relaxed, Ordering::Relaxed)
                        .map_or_else(|published| published, |_| read),
                )
            }
            stored => decode_class(stored),
        }
    }
}

/// What a stored size means to a caller.
const fn decode_size(stored: u64) -> Option<u64> {
    match stored {
        UNREAD | UNREADABLE => None,
        size => Some(size),
    }
}

/// What a stored classification means to a caller.
const fn decode_class(class: u8) -> Option<bool> {
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
/// Reads until the probe is full or the file ends, because one `read` may return early for reasons that say nothing
/// about the file — a short read that stopped at 4 KiB would report a file as text on the strength of a sixteenth of
/// the promised prefix, and the answer is then cached.
///
/// The handle is opened through [`open_regular`], so this cannot wait on a fifo left under a name the walk recorded
/// as a regular file, and never reads a directory or a device node as content.
fn holds_nul(path: &Path) -> Option<bool> {
    let mut file = open_regular(path).ok()?;
    let mut head = vec![0u8; PROBE];
    let mut filled = 0;

    while filled < PROBE {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }

    Some(head[..filled].contains(&0))
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
