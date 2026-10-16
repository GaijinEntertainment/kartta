use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::key::{ContentHash, Namespace};
use crate::pack::{self, Lookup, Pack};
use crate::writer::PackWriter;

/// A cache directory: every pack published into it, mapped read-only, answering lookups by key.
///
/// [`Store::open`] maps the packs present at that moment; a pack another process publishes later is seen by the next
/// open, never by a store already open, so the entries a store answers with stay fixed for its life. A lookup is a
/// binary search over each pack's mapped index — no file is opened per entry — and the payload it returns is checked
/// against the 64-bit hash stored with it, so a damaged entry is passed over: another pack's sound copy answers
/// instead, or the lookup misses.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    packs: Vec<Pack>,
    rejected: u64,
    counters: Counters,
}

#[derive(Debug, Default)]
struct Counters {
    hits: AtomicU64,
    misses: AtomicU64,
    damaged: AtomicU64,
}

/// What a [`Store`] holds and what its lookups have found so far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Stats {
    /// Packs mapped when the store opened.
    pub packs: u64,
    /// Entries across those packs.
    pub entries: u64,
    /// Files with the pack extension that were not packs this store can read — truncated, foreign, or of another
    /// format version — and so answer nothing.
    pub rejected_packs: u64,
    /// Lookups that found their entry.
    pub hits: u64,
    /// Lookups that found no entry.
    pub misses: u64,
    /// Lookups that found their key only with payloads that failed their check, and so were answered as misses. A
    /// changed byte in a key makes its entry look absent, and its lookups count in [`Stats::misses`].
    pub damaged: u64,
}

impl Store {
    /// Opens the cache directory `dir`, creating it when absent, and maps every pack in it.
    ///
    /// # Errors
    ///
    /// Returns the error of creating or listing the directory. A pack that cannot be opened or read is not an error:
    /// it is left in place and counted in [`Stats::rejected_packs`].
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut paths = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension() == Some(OsStr::new(pack::EXTENSION)) {
                paths.push(path);
            }
        }
        paths.sort_unstable();

        let mut packs = Vec::with_capacity(paths.len());
        let mut rejected = 0;
        for path in &paths {
            match Pack::open(path) {
                Ok(Some(pack)) => packs.push(pack),
                Ok(None) | Err(_) => rejected += 1,
            }
        }

        Ok(Self {
            dir,
            packs,
            rejected,
            counters: Counters::default(),
        })
    }

    /// The payload cached under (`namespace`, `content`), borrowed from the mapped pack that holds it.
    #[must_use]
    pub fn get(&self, namespace: &Namespace, content: &ContentHash) -> Option<&[u8]> {
        let mut damaged = false;
        for pack in &self.packs {
            match pack.get(namespace, content) {
                Lookup::Found(payload) => {
                    self.counters.hits.fetch_add(1, Ordering::Relaxed);
                    return Some(payload);
                }
                Lookup::Damaged => damaged = true,
                Lookup::Absent => {}
            }
        }
        let counter = if damaged {
            &self.counters.damaged
        } else {
            &self.counters.misses
        };
        counter.fetch_add(1, Ordering::Relaxed);

        None
    }

    /// A writer whose pack will be published into this store's directory, for the entries this process computes.
    #[must_use]
    pub fn writer(&self) -> PackWriter {
        PackWriter::new(&self.dir)
    }

    /// The directory the store reads.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What the store holds and what its lookups have found since it opened.
    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            packs: u64::try_from(self.packs.len()).unwrap_or(u64::MAX),
            entries: self
                .packs
                .iter()
                .map(|pack| u64::try_from(pack.entry_count()).unwrap_or(u64::MAX))
                .sum(),
            rejected_packs: self.rejected,
            hits: self.counters.hits.load(Ordering::Relaxed),
            misses: self.counters.misses.load(Ordering::Relaxed),
            damaged: self.counters.damaged.load(Ordering::Relaxed),
        }
    }
}
