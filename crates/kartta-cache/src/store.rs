use std::collections::HashSet;
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

const MERGE_PAST: usize = 8;
const LISTINGS: u64 = 4;

/// What a [`Store::merge`] did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Merged {
    /// Packs merged, `0` when the store held too few to merge.
    pub packs: u64,
    /// Entries the merged pack holds.
    pub entries: u64,
    /// The merged pack, `None` when nothing was merged or every merged entry failed its check.
    pub published: Option<PathBuf>,
    /// Merged packs that could not be removed, and so still sit beside the merged pack, holding the same entries.
    pub left_in_place: u64,
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
    /// A pack that vanishes between the listing and its opening was removed by another process's merge, which
    /// publishes the merged pack before it removes anything, so the store lists the directory again and maps what it
    /// finds there, up to four listings. Only a merge finishing between every two of them leaves a pack the store does
    /// not hold, and that pack is counted as rejected.
    ///
    /// # Errors
    ///
    /// Returns the error of creating or listing the directory. A pack that cannot be opened or read is not an error:
    /// it is left in place and counted in [`Stats::rejected_packs`], and so is one still vanishing after four listings.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let listed = dir.clone();

        Self::open_listed(dir, || pack_paths(&listed))
    }

    fn open_listed(
        dir: PathBuf,
        mut list: impl FnMut() -> io::Result<Vec<PathBuf>>,
    ) -> io::Result<Self> {
        let mut opening = Opening::default();
        for listing in 1..=LISTINGS {
            let vanished = opening.open_all(list()?);
            if vanished == 0 {
                break;
            }
            if listing == LISTINGS {
                opening.rejected += vanished;
            }
        }
        let Opening {
            mut packs,
            rejected,
            ..
        } = opening;
        // constraint: the largest pack answers first, so a hit in a store whose packs a merge keeps few and uneven
        // costs one search rather than one per pack
        packs.sort_by_key(|pack| std::cmp::Reverse(pack.entry_count()));

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

    /// Merges this store's packs into fewer when it holds more than eight, and removes the packs it merged.
    ///
    /// Each publish of a new pack adds one and every lookup searches the packs until one answers, so a store left alone
    /// answers more slowly with every build. While the largest pack holds more entries than all the others together,
    /// the others merge into one and the largest stays as it is, so a merge copies what builds added since; otherwise
    /// every pack merges into one. An entry that fails its check is dropped, and a key held by several packs is written
    /// once.
    ///
    /// The merge reads the packs this store mapped, so a pack published after it opened is left for a later merge, and
    /// a store that mapped a merged pack keeps answering from it after it is removed. Two processes that merge the same
    /// packs at once each publish a merged pack, which is wasted work but no lost entry: each removes only packs it
    /// merged, and the next merge folds the duplicates.
    ///
    /// # Errors
    ///
    /// Returns the error of writing or publishing the merged pack, in which case no pack is removed. A merged pack that
    /// cannot be removed is not an error: it stays in place, answers the same entries, and is counted in
    /// [`Merged::left_in_place`].
    pub fn merge(&self) -> io::Result<Merged> {
        if self.packs.len() <= MERGE_PAST {
            return Ok(Merged::default());
        }
        let inputs = self.merge_inputs();
        let writer = self.writer();
        for pack in inputs {
            for (namespace, content, payload) in pack.entries() {
                writer.insert(&namespace, &content, payload)?;
            }
        }
        let entries = u64::try_from(writer.entry_count()).unwrap_or(u64::MAX);
        let published = writer.publish()?;

        let mut merged = Merged {
            packs: u64::try_from(inputs.len()).unwrap_or(u64::MAX),
            entries,
            published,
            left_in_place: 0,
        };
        for pack in inputs {
            if merged.published.as_deref() == Some(pack.path()) {
                continue;
            }
            match fs::remove_file(pack.path()) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => merged.left_in_place += 1,
                _ => {}
            }
        }

        Ok(merged)
    }

    fn merge_inputs(&self) -> &[Pack] {
        let largest = self.packs.first().map_or(0, Pack::entry_count);
        let rest: usize = self.packs.iter().skip(1).map(Pack::entry_count).sum();

        if largest > rest {
            &self.packs[1..]
        } else {
            &self.packs
        }
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

#[derive(Default)]
struct Opening {
    packs: Vec<Pack>,
    opened: HashSet<PathBuf>,
    rejected: u64,
}

impl Opening {
    fn open_all(&mut self, paths: Vec<PathBuf>) -> u64 {
        let mut vanished = 0;
        for path in paths {
            if self.opened.contains(&path) {
                continue;
            }
            match Pack::open(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    vanished += 1;
                    continue;
                }
                Ok(Some(pack)) => self.packs.push(pack),
                Ok(None) | Err(_) => self.rejected += 1,
            }
            self.opened.insert(path);
        }

        vanished
    }
}

fn pack_paths(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension() == Some(OsStr::new(pack::EXTENSION)) {
            paths.push(path);
        }
    }
    paths.sort_unstable();

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish(dir: &Path, content: &[u8]) -> PathBuf {
        let writer = PackWriter::new(dir);
        writer
            .insert(
                &Namespace::new("test", b"1"),
                &ContentHash::of(content),
                content,
            )
            .expect("insert");
        writer.publish().expect("publish").expect("a pack")
    }

    fn holds(store: &Store, content: &[u8]) -> bool {
        store.get(&Namespace::new("test", b"1"), &ContentHash::of(content)) == Some(content)
    }

    #[test]
    fn a_pack_a_merge_removed_after_the_listing_sends_the_store_back_to_the_listing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let kept = publish(dir.path(), b"kept");
        let merged = publish(dir.path(), b"merged");
        let removed = dir.path().join("removed by a merge.pack");
        let mut listings = 0;

        let store = Store::open_listed(dir.path().to_path_buf(), || {
            listings += 1;
            Ok(if listings == 1 {
                vec![kept.clone(), removed.clone()]
            } else {
                vec![kept.clone(), merged.clone()]
            })
        })
        .expect("open");

        assert_eq!(listings, 2);
        assert!(holds(&store, b"kept") && holds(&store, b"merged"));
        assert_eq!((store.stats().packs, store.stats().rejected_packs), (2, 0));
    }

    #[test]
    fn a_pack_that_keeps_vanishing_is_counted_after_the_last_listing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut listings = 0;

        let store = Store::open_listed(dir.path().to_path_buf(), || {
            listings += 1;
            Ok(vec![dir.path().join("never there.pack")])
        })
        .expect("open");

        assert_eq!(listings, LISTINGS);
        assert_eq!((store.stats().packs, store.stats().rejected_packs), (0, 1));
    }
}
