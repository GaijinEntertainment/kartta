use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::open::{Symlinks, open_regular};

const UNREAD: u64 = 0;
// constraint: no filesystem addresses `u64::MAX - 1` bytes, so this cannot collide with a size stored plus one
const UNREADABLE: u64 = u64::MAX;

pub(crate) const PROBE: usize = 64 * 1024;

const UNCLASSIFIED: u8 = 0;
const TEXT: u8 = 1;
const BINARY: u8 = 2;
const UNOPENABLE: u8 = 3;

pub(crate) struct Inspection {
    sizes: Box<[AtomicU64]>,
    classes: Box<[AtomicU8]>,
}

impl Inspection {
    pub(crate) fn new(entries: usize) -> Self {
        Self {
            // SAFETY: an `AtomicU64` has the in-memory representation of a `u64`, so all-zero bytes are a valid one
            // holding `UNREAD`.
            sizes: unsafe { Box::new_zeroed_slice(entries).assume_init() },
            // SAFETY: an `AtomicU8` has the in-memory representation of a `u8`, so a zero byte is a valid one holding
            // `UNCLASSIFIED`.
            classes: unsafe { Box::new_zeroed_slice(entries).assume_init() },
        }
    }

    pub(crate) fn size(&self, at: usize, native_path: impl FnOnce() -> PathBuf) -> Option<u64> {
        let slot = self.sizes.get(at)?;

        match slot.load(Ordering::Relaxed) {
            UNREAD => {
                let read = read_size(&native_path()).map_or(UNREADABLE, |size| size + 1);

                decode_size(
                    slot.compare_exchange(UNREAD, read, Ordering::Relaxed, Ordering::Relaxed)
                        .map_or_else(|published| published, |_| read),
                )
            }
            stored => decode_size(stored),
        }
    }

    pub(crate) fn is_binary(
        &self,
        at: usize,
        named: impl FnOnce() -> bool,
        native_path: impl FnOnce() -> PathBuf,
    ) -> Option<bool> {
        let slot = self.classes.get(at)?;

        match slot.load(Ordering::Relaxed) {
            UNCLASSIFIED => {
                let read = if named() {
                    BINARY
                } else {
                    classify(&native_path())
                };

                decode_class(
                    slot.compare_exchange(UNCLASSIFIED, read, Ordering::Relaxed, Ordering::Relaxed)
                        .map_or_else(|published| published, |_| read),
                )
            }
            stored => decode_class(stored),
        }
    }

    pub(crate) fn known_size(&self, at: usize) -> Option<u64> {
        decode_size(self.sizes.get(at)?.load(Ordering::Relaxed))
    }

    pub(crate) fn known_binary(&self, at: usize) -> Option<bool> {
        decode_class(self.classes.get(at)?.load(Ordering::Relaxed))
    }

    pub(crate) fn record_size(&self, at: usize, size: u64) {
        if let Some(slot) = self.sizes.get(at) {
            _ = slot.compare_exchange(UNREAD, size + 1, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_binary(&self, at: usize, binary: bool) {
        if let Some(slot) = self.classes.get(at) {
            let class = if binary { BINARY } else { TEXT };
            _ = slot.compare_exchange(UNCLASSIFIED, class, Ordering::Relaxed, Ordering::Relaxed);
        }
    }
}

const fn decode_size(stored: u64) -> Option<u64> {
    match stored {
        UNREAD | UNREADABLE => None,
        stored => Some(stored - 1),
    }
}

const fn decode_class(class: u8) -> Option<bool> {
    match class {
        TEXT => Some(false),
        BINARY => Some(true),
        _ => None,
    }
}

fn classify(native_path: &Path) -> u8 {
    match holds_nul(native_path) {
        Some(true) => BINARY,
        Some(false) => TEXT,
        None => UNOPENABLE,
    }
}

fn holds_nul(native_path: &Path) -> Option<bool> {
    let (mut file, _) = open_regular(native_path, Symlinks::Refuse).ok()?;
    let mut head = vec![0u8; PROBE];
    let filled = read_prefix(&mut file, &mut head).ok()?;

    Some(head[..filled].contains(&0))
}

// constraint: one `read` may return short for reasons that say nothing about the file, and a verdict taken on a short
// prefix is kept, so this reads until the file ends or `head` is full
pub(crate) fn read_prefix(file: &mut impl Read, head: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;

    while filled < head.len() {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }

    Ok(filled)
}

// constraint: `len()` answers for whatever the path names now, so the kind is settled on this stat rather than taken
// from the walk — a directory that replaced the file would otherwise publish its own length as the file's size
fn read_size(native_path: &Path) -> Option<u64> {
    let metadata = std::fs::symlink_metadata(native_path).ok()?;

    metadata.is_file().then_some(metadata.len())
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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Inspection")
            .field("sizes", &self.sizes.len())
            .field("classes", &self.classes.len())
            .finish()
    }
}
