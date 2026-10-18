use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::key::{ContentHash, Namespace};
use crate::pack::{self, Aging, Pack, Published};
use crate::writer::{PackWriter, TEMP_PREFIX};

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
}

const MERGE_PAST: usize = 8;
const LISTINGS: u64 = 4;
const SWEEP_AGE_FLOOR: Duration = Duration::from_hours(1);

/// The time to live of an entry when its caller has no reason to pick another: one week.
pub const DEFAULT_TTL: Duration = Duration::from_hours(7 * 24);

/// What a [`Store::merge`] did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Merged {
    /// Packs merged, `0` when the store held too few to merge and none held enough expired entries to rewrite.
    pub packs: u64,
    /// Entries the merged pack holds.
    pub entries: u64,
    /// The merged pack, `None` when nothing was merged, or when every merged entry had expired or failed its check.
    pub published: Option<PathBuf>,
    /// Merged packs that could not be removed, and so still sit beside the merged pack, holding the same entries.
    pub left_in_place: u64,
    /// Entries past their time to live that the merge removed.
    pub expired: u64,
    /// Entries past their time to live still in the store: in a pack where they hold less than half of the payload
    /// bytes, which the merge leaves as it is unless it merges the pack anyway, and in merged packs that could not be
    /// removed. Lookups still find them.
    pub expired_kept: u64,
    /// Temporary files of writers that never published, an hour old or more, that the merge removed.
    pub orphans: u64,
    /// The same, that could not be removed.
    pub orphans_left: u64,
    /// Files with the pack extension that the store did not map — of another format version, damaged, foreign, or
    /// published after it opened — and last written longer ago than both the time to live and an hour, so every entry
    /// they hold has expired and no writer is still publishing them, that the merge removed.
    pub unreadable: u64,
    /// The same, that could not be removed.
    pub unreadable_left: u64,
}

/// What a [`Store`] holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Stats {
    /// Packs mapped when the store opened.
    pub packs: u64,
    /// Entries across those packs.
    pub entries: u64,
}

impl Store {
    /// Opens the cache directory `dir`, creating it when absent, and maps every pack in it.
    ///
    /// A pack that vanishes between the listing and its opening was removed by another process's merge, which
    /// publishes the merged pack before it removes anything, so the store lists the directory again and maps what it
    /// finds there, up to four listings. Only a merge finishing between every two of them leaves a pack the store does
    /// not hold.
    ///
    /// # Errors
    ///
    /// Returns the error of creating or listing the directory. A file with the pack extension that cannot be opened or
    /// read as a pack — truncated, foreign, or of another format version — is not an error: it is left in place and
    /// answers nothing.
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
        for _ in 0..LISTINGS {
            if !opening.open_all(list()?) {
                break;
            }
        }
        let mut packs = opening.packs;
        // constraint: the largest pack answers first, so a hit in a store whose packs a merge keeps few and uneven
        // costs one search rather than one per pack
        packs.sort_by_key(|pack| std::cmp::Reverse(pack.entry_count()));

        Ok(Self { dir, packs })
    }

    /// The payload cached under (`namespace`, `content`), borrowed from the mapped pack that holds it.
    #[must_use]
    pub fn get(&self, namespace: &Namespace, content: &ContentHash) -> Option<&[u8]> {
        self.packs
            .iter()
            .find_map(|pack| pack.get(namespace, content))
    }

    /// Removes the entries published longer ago than `ttl`, merges this store's packs into fewer when it holds more
    /// than eight, and removes the packs it merged and the files a writer or an older format left behind.
    ///
    /// Each publish of a new pack adds one and every lookup searches the packs until one answers, so a store left alone
    /// answers more slowly with every build. While the largest pack holds more entries than all the others together,
    /// the others merge into one and the largest stays as it is, so a merge copies what builds added since; otherwise
    /// every pack merges into one. An entry that fails its check is dropped, and a key held by several packs is written
    /// once.
    ///
    /// An entry's age is counted from when the writer that published it was made, and a merge keeps it: a lookup that
    /// finds an entry does not renew it, so an entry that every build finds expires once per `ttl` and the next build
    /// computes and publishes it again. A merge drops every expired entry of the packs it merges, and merges a pack
    /// whose expired entries hold half of its payload bytes or more whatever the count of packs, so a store never holds
    /// much more than its unexpired entries and an expired entry costs nothing until it would. An entry records the
    /// second it was published, and its age counts from the start of that second: it expires once more than `ttl` has
    /// passed since then, so no entry outlives its `ttl`, and a `ttl` of zero expires every entry whose second began
    /// before the merge. [`DEFAULT_TTL`] is a week.
    ///
    /// The merge also removes the temporary files of writers that never published, once an hour has passed since they
    /// were last written and no writer holds their lock, and the files with the pack extension the store did not map
    /// once both `ttl` and an hour have passed since they were last written: every entry of such a file was published
    /// before then, and a pack another process published after this store opened stays for that hour whatever `ttl`
    /// is.
    ///
    /// The merge reads the packs this store mapped, so a pack published after it opened is left for a later merge, and
    /// a store that mapped a merged pack keeps answering from it after it is removed. Two processes that merge the same
    /// packs at once each publish a merged pack, which is wasted work but no lost entry: each removes only packs it
    /// merged, and the next merge folds the duplicates.
    ///
    /// # Errors
    ///
    /// Returns the error of listing the directory, or of writing or publishing the merged pack; the packs it was
    /// merging then stay in place. A file that cannot be removed is not an error: it stays in place and is counted in
    /// [`Merged::left_in_place`], [`Merged::orphans_left`] or [`Merged::unreadable_left`].
    pub fn merge(&self, ttl: Duration) -> io::Result<Merged> {
        self.merge_at(ttl, SystemTime::now())
    }

    fn merge_at(&self, ttl: Duration, now: SystemTime) -> io::Result<Merged> {
        let mut merged = self.sweep(ttl, now)?;
        let cutoff = Published::rounded_up(now.checked_sub(ttl).unwrap_or(UNIX_EPOCH));
        let aged: Vec<Aging> = self
            .packs
            .iter()
            .map(|pack| pack.aged_before(cutoff))
            .collect();
        merged.expired_kept = aged.iter().map(|aged| aged.entries).sum();
        let inputs = self.merge_inputs(&aged);
        if inputs.is_empty() {
            return Ok(merged);
        }
        let writer = self.writer();
        for (pack, _) in &inputs {
            for (namespace, record, payload) in pack.entries_since(cutoff) {
                writer.insert_record(&namespace, record, payload)?;
            }
        }
        merged.packs = u64::try_from(inputs.len()).unwrap_or(u64::MAX);
        merged.entries = u64::try_from(writer.entry_count()).unwrap_or(u64::MAX);
        merged.published = writer.publish()?;

        for (pack, aged) in inputs {
            if merged.published.as_deref() == Some(pack.path()) {
                continue;
            }
            match fs::remove_file(pack.path()) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => merged.left_in_place += 1,
                _ => {
                    merged.expired += aged.entries;
                    merged.expired_kept -= aged.entries;
                }
            }
        }

        Ok(merged)
    }

    fn merge_inputs<'s>(&'s self, aged: &'s [Aging]) -> Vec<(&'s Pack, &'s Aging)> {
        let largest = self.packs.first().map_or(0, Pack::entry_count);
        let rest: usize = self.packs.iter().skip(1).map(Pack::entry_count).sum();
        let first_merged = if self.packs.len() <= MERGE_PAST {
            self.packs.len()
        } else {
            usize::from(largest > rest)
        };

        self.packs
            .iter()
            .zip(aged)
            .enumerate()
            .filter(|(at, (pack, aged))| {
                *at >= first_merged || (aged.entries > 0 && aged.bytes * 2 >= pack.payload_bytes())
            })
            .map(|(_, inputs)| inputs)
            .collect()
    }

    fn sweep(&self, ttl: Duration, now: SystemTime) -> io::Result<Merged> {
        let mut swept = Merged::default();
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            let is_orphan = path
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(TEMP_PREFIX.as_bytes()));
            let is_unreadable = path.extension() == Some(OsStr::new(pack::EXTENSION))
                && !self.packs.iter().any(|pack| pack.path() == path);
            let (removed, left, age) = if is_orphan {
                (&mut swept.orphans, &mut swept.orphans_left, SWEEP_AGE_FLOOR)
            } else if is_unreadable {
                (
                    &mut swept.unreadable,
                    &mut swept.unreadable_left,
                    ttl.max(SWEEP_AGE_FLOOR),
                )
            } else {
                continue;
            };
            if !written_before(&path, now, age) || (is_orphan && is_being_written(&path)) {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => *removed += 1,
                Err(error) if error.kind() != io::ErrorKind::NotFound => *left += 1,
                Err(_) => {}
            }
        }

        Ok(swept)
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

    /// What the store holds.
    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            packs: u64::try_from(self.packs.len()).unwrap_or(u64::MAX),
            entries: self
                .packs
                .iter()
                .map(|pack| u64::try_from(pack.entry_count()).unwrap_or(u64::MAX))
                .sum(),
        }
    }
}

#[derive(Default)]
struct Opening {
    packs: Vec<Pack>,
    opened: HashSet<PathBuf>,
}

impl Opening {
    fn open_all(&mut self, paths: Vec<PathBuf>) -> bool {
        let mut vanished = false;
        for path in paths {
            if self.opened.contains(&path) {
                continue;
            }
            match Pack::open(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    vanished = true;
                    continue;
                }
                Ok(Some(pack)) => self.packs.push(pack),
                Ok(None) | Err(_) => {}
            }
            self.opened.insert(path);
        }

        vanished
    }
}

fn is_being_written(path: &Path) -> bool {
    fs::File::open(path).is_ok_and(|file| matches!(file.try_lock(), Err(TryLockError::WouldBlock)))
}

fn written_before(path: &Path, now: SystemTime, age: Duration) -> bool {
    fs::symlink_metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|since| since >= age))
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
        assert_eq!(store.stats().packs, 2);
    }

    #[test]
    fn a_pack_that_keeps_vanishing_is_given_up_after_the_last_listing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut listings = 0;

        let store = Store::open_listed(dir.path().to_path_buf(), || {
            listings += 1;
            Ok(vec![dir.path().join("never there.pack")])
        })
        .expect("open");

        assert_eq!(listings, LISTINGS);
        assert_eq!(store.stats().packs, 0);
    }

    const DAY: Duration = Duration::from_hours(24);

    fn publish_at(dir: &Path, published: SystemTime, contents: &[String]) -> Option<PathBuf> {
        let writer = PackWriter::published_at(dir, published);
        for content in contents {
            writer
                .insert(
                    &Namespace::new("test", b"1"),
                    &ContentHash::of(content.as_bytes()),
                    content.as_bytes(),
                )
                .expect("insert");
        }
        writer.publish().expect("publish")
    }

    fn named(prefix: &str, count: usize) -> Vec<String> {
        (0..count).map(|at| format!("{prefix} {at}")).collect()
    }

    fn merged_at(dir: &Path, ttl: Duration, now: SystemTime) -> Merged {
        Store::open(dir)
            .expect("open")
            .merge_at(ttl, now)
            .expect("merge")
    }

    fn one_pack_of_two_ages(dir: &Path, now: SystemTime, old: usize, fresh: usize) {
        for content in named("old", old) {
            publish_at(dir, now - 6 * DAY, &[content]);
        }
        for content in named("new", fresh) {
            publish_at(dir, now - DAY, &[content]);
        }
        let merged = merged_at(dir, DEFAULT_TTL, now);
        assert_eq!((merged.packs, merged.expired), ((old + fresh) as u64, 0));
    }

    #[test]
    fn a_pack_whose_every_entry_expired_is_removed_and_its_entries_counted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();
        let pack = publish_at(dir.path(), now - 8 * DAY, &named("file", 3)).expect("a pack");

        let merged = merged_at(dir.path(), DEFAULT_TTL, now);

        assert_eq!(
            (
                merged.packs,
                merged.entries,
                merged.published,
                merged.expired,
                merged.expired_kept
            ),
            (1, 0, None, 3, 0)
        );
        assert!(!pack.exists());
    }

    #[test]
    fn an_entry_within_its_time_to_live_stays() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();
        let pack = publish_at(dir.path(), now - 6 * DAY, &named("file", 3)).expect("a pack");

        let merged = merged_at(dir.path(), DEFAULT_TTL, now);

        assert_eq!(merged, Merged::default());
        assert!(pack.exists());
    }

    #[test]
    fn a_pack_whose_expired_entries_hold_half_its_bytes_is_rewritten_without_them() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();
        one_pack_of_two_ages(dir.path(), now, 5, 4);

        let merged = merged_at(dir.path(), DEFAULT_TTL, now + 2 * DAY);

        assert_eq!(
            (
                merged.packs,
                merged.entries,
                merged.expired,
                merged.expired_kept
            ),
            (1, 4, 5, 0)
        );
        let store = Store::open(dir.path()).expect("reopen");
        assert!(
            named("new", 4)
                .iter()
                .all(|new| holds(&store, new.as_bytes()))
        );
        assert!(
            !named("old", 5)
                .iter()
                .any(|old| holds(&store, old.as_bytes()))
        );
    }

    #[test]
    fn a_rewritten_pack_keeps_each_entry_s_publication_time() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();
        one_pack_of_two_ages(dir.path(), now, 5, 4);
        merged_at(dir.path(), DEFAULT_TTL, now + 2 * DAY);

        let merged = merged_at(dir.path(), DEFAULT_TTL, now + 7 * DAY);

        assert_eq!((merged.packs, merged.expired), (1, 4));
        assert_eq!(Store::open(dir.path()).expect("reopen").stats().packs, 0);
    }

    #[test]
    fn expired_entries_under_half_a_pack_s_bytes_are_counted_and_still_found() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();
        one_pack_of_two_ages(dir.path(), now, 4, 5);

        let merged = merged_at(dir.path(), DEFAULT_TTL, now + 2 * DAY);

        assert_eq!(
            (merged.packs, merged.expired, merged.expired_kept),
            (0, 0, 4)
        );
        let store = Store::open(dir.path()).expect("reopen");
        assert!(
            named("old", 4)
                .iter()
                .all(|old| holds(&store, old.as_bytes()))
        );
    }

    #[test]
    fn a_ttl_of_zero_expires_every_entry_published_before_the_merge() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();
        publish_at(dir.path(), now, &named("file", 3));

        let merged = merged_at(dir.path(), Duration::ZERO, now + Duration::from_secs(1));

        assert_eq!((merged.packs, merged.expired), (1, 3));
    }

    #[test]
    fn an_entry_expires_once_more_than_a_ttl_with_a_fraction_of_a_second_has_passed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let second = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        publish_at(dir.path(), second, &named("file", 1));

        let exactly = merged_at(
            dir.path(),
            Duration::from_millis(1500),
            second + Duration::from_millis(1500),
        );
        let past = merged_at(
            dir.path(),
            Duration::from_millis(500),
            second + Duration::from_millis(1100),
        );

        assert_eq!((exactly.expired, exactly.expired_kept), (0, 0));
        assert_eq!(past.expired, 1);
    }

    #[test]
    fn the_same_entries_published_again_are_one_pack_aging_from_the_first_publish() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();

        let first = publish_at(dir.path(), now - DAY, &named("file", 3));
        let second = publish_at(dir.path(), now, &named("file", 3));
        let merged = merged_at(dir.path(), DEFAULT_TTL, now + DEFAULT_TTL - DAY / 2);

        assert_eq!(first, second);
        assert_eq!((merged.packs, merged.expired), (1, 3));
    }

    #[test]
    fn a_pack_published_after_the_store_opened_stays_for_an_hour_whatever_the_ttl() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open(dir.path()).expect("open");
        let pack = publish_at(dir.path(), SystemTime::now(), &named("file", 1)).expect("a pack");
        let now = SystemTime::now();

        let early = store
            .merge_at(Duration::ZERO, now + Duration::from_secs(1))
            .expect("merge");
        assert!(pack.exists());
        let late = store
            .merge_at(Duration::ZERO, now + SWEEP_AGE_FLOOR * 2)
            .expect("merge");

        assert_eq!((early.unreadable, late.unreadable), (0, 1));
        assert!(!pack.exists());
    }

    #[test]
    fn a_temporary_file_is_removed_once_unwritten_for_an_hour() {
        let dir = tempfile::tempdir().expect("temp dir");
        let orphan = dir.path().join(format!("{TEMP_PREFIX}1-2-3"));
        fs::write(&orphan, b"part of a pack").expect("write an orphan");
        let now = SystemTime::now();

        let early = merged_at(dir.path(), DEFAULT_TTL, now + SWEEP_AGE_FLOOR / 2);
        let late = merged_at(dir.path(), DEFAULT_TTL, now + SWEEP_AGE_FLOOR * 2);

        assert_eq!((early.orphans, late.orphans, late.orphans_left), (0, 1, 0));
        assert!(!orphan.exists());
    }

    #[test]
    fn the_temporary_file_of_a_writer_still_writing_is_left_however_old() {
        let dir = tempfile::tempdir().expect("temp dir");
        let writer = PackWriter::new(dir.path());
        writer
            .insert(&Namespace::new("test", b"1"), &ContentHash::of(b"a"), b"a")
            .expect("insert");

        let merged = merged_at(
            dir.path(),
            DEFAULT_TTL,
            SystemTime::now() + SWEEP_AGE_FLOOR * 2,
        );

        assert_eq!((merged.orphans, merged.orphans_left), (0, 0));
        assert!(writer.publish().expect("publish").is_some());
    }

    #[test]
    fn a_pack_the_store_cannot_read_is_removed_once_older_than_the_ttl() {
        let dir = tempfile::tempdir().expect("temp dir");
        let now = SystemTime::now();
        publish_at(dir.path(), now, &named("file", 1));
        let unreadable = dir.path().join("of another format version.pack");
        fs::write(&unreadable, b"no footer this version reads").expect("write the pack");

        let early = merged_at(dir.path(), DEFAULT_TTL, now + DEFAULT_TTL / 2);
        let late = merged_at(dir.path(), DEFAULT_TTL, now + DEFAULT_TTL + DAY);

        assert_eq!(
            (early.unreadable, late.unreadable, late.unreadable_left),
            (0, 1, 0)
        );
        assert_eq!(late.expired, 1, "a pack the store maps ages by its entries");
        assert!(!unreadable.exists());
    }
}
