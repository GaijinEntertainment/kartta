use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::postings::{self, Shape};
use crate::requirement::Requirement;
use crate::trigram::Trigram;

mod build;
mod compact;
mod storage;

pub(crate) use build::{Collector, Full, Pairs, Target, Unfinished};
use storage::Blob;

const PARTS: usize = 256;
const BUCKETS: usize = 1 << 16;
const BUCKET_BYTES: usize = 12;

/// Where a build keeps the content index it makes.
///
/// A content index records, for every trigram — every run of three bytes, ASCII letters folded to lowercase — which
/// files hold it. See [`BuildOptions::content_index`](crate::BuildOptions::content_index) for what a build pays for one.
///
/// The two file-backed kinds keep the index in a scratch file under `dir`, which must already exist. A scratch file is
/// private to the process and leaves nothing behind however the process ends. On Linux it is created with no name, where
/// the filesystem under `dir` allows that; elsewhere on unix it is removed from `dir` the moment it is created, which
/// leaves that one moment for a process of the same user to open it; on Windows it is opened with no sharing and deleted
/// on close. While it builds the index, a
/// build also spills what it has collected so far to a scratch file there rather than holding it in memory, so `dir`
/// needs room for about twice the index while the build runs and once the index is done.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ContentIndexStorage {
    /// In the process's own memory, beside the rest of the index. The build holds what it collects in memory too, so it
    /// peaks at about twice the index.
    Memory,
    /// In a scratch file under `dir`, read by position on each search: a search holds in memory only the lists it
    /// reads, for as long as it reads them.
    File {
        /// The directory to create the scratch files in.
        dir: PathBuf,
    },
    /// In a scratch file under `dir`, mapped into memory: the system keeps the pages searches read and gives them back
    /// under pressure, so the index costs memory only while it is used.
    Mapped {
        /// The directory to create the scratch files in.
        dir: PathBuf,
    },
}

impl ContentIndexStorage {
    pub(crate) fn dir(&self) -> Option<&Path> {
        match self {
            Self::Memory => None,
            Self::File { dir } | Self::Mapped { dir } => Some(dir),
        }
    }
}

/// What a content index holds, as [`Index::content_index_stats`](crate::Index::content_index_stats) reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentIndexStats {
    files: usize,
    trigrams: usize,
    bytes: u64,
}

impl ContentIndexStats {
    /// How many files the index holds the content of: every text file the build, or a refresh since, read to its end.
    ///
    /// A file that could not be read is not among them, and is reported in
    /// [`BuildOutcome::errors`](crate::BuildOutcome::errors) instead.
    #[must_use]
    pub const fn files(&self) -> usize {
        self.files
    }

    /// How many distinct trigrams those files hold, counted within each segment of the index.
    ///
    /// A build makes one segment. A refresh that carries text files over unread keeps their segments and indexes the
    /// files it reads in one more, so a trigram held both by files carried and by files read counts once in each, until
    /// a refresh merges the segments into one and each trigram counts once again.
    #[must_use]
    pub const fn trigrams(&self) -> usize {
        self.trigrams
    }

    /// How many bytes the index takes where it is kept.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl ContentIndexStats {
    fn sum(segments: &[Arc<ContentIndex>], unindexed: &[u32], files: usize) -> Self {
        Self {
            files,
            trigrams: segments.iter().map(|segment| segment.stats.trigrams).sum(),
            bytes: size_of_val(unindexed) as u64
                + segments
                    .iter()
                    .map(|segment| segment.stats.bytes)
                    .sum::<u64>(),
        }
    }
}

pub(crate) struct ContentIndex {
    stats: ContentIndexStats,
    first_id: u32,
    universe: u32,
    blob: Blob,
    directory_start: usize,
    buckets_start: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ContentIndexes {
    pub(crate) segments: Vec<Arc<ContentIndex>>,
    pub(crate) unindexed: Box<[u32]>,
    files: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct IdPositions<'a> {
    pub(crate) first: u32,
    pub(crate) positions: &'a [u32],
}

pub(crate) const NO_POSITION: u32 = u32::MAX;

impl IdPositions<'_> {
    fn is_live(self, id: u32) -> bool {
        id.checked_sub(self.first)
            .and_then(|slot| self.positions.get(slot as usize))
            .is_some_and(|&at| at != NO_POSITION)
    }
}

// constraint: each refresh that reads anything adds a segment, with its own bucket table and lists for the files it
// read again beside the lists their old ids keep, so past this many segments, or once more than a quarter of the files
// the segments hold lists for are gone, the segments are merged into one
const MAX_SEGMENTS: usize = 8;

pub(crate) struct Narrowed {
    positions: Bits,
}

impl Narrowed {
    pub(crate) fn admits(&self, at: usize) -> bool {
        self.positions.contains(at)
    }
}

impl ContentIndexes {
    pub(crate) fn new(
        segments: Vec<Arc<ContentIndex>>,
        mut unindexed: Vec<u32>,
        files: usize,
    ) -> Self {
        unindexed.sort_unstable();
        unindexed.dedup();

        Self {
            segments,
            unindexed: unindexed.into_boxed_slice(),
            files,
        }
    }

    pub(crate) fn stats(&self) -> ContentIndexStats {
        ContentIndexStats::sum(&self.segments, &self.unindexed, self.files)
    }

    pub(crate) const fn files(&self) -> usize {
        self.files
    }

    pub(crate) fn compacted_when_due(
        self,
        ids: IdPositions<'_>,
        target: Target<'_>,
    ) -> Result<Self, Unfinished> {
        let held: usize = self
            .segments
            .iter()
            .map(|segment| segment.stats.files)
            .sum();
        let dead = held.saturating_sub(self.files);
        if self.segments.len() <= MAX_SEGMENTS && dead * 4 <= held {
            return Ok(self);
        }

        let merged = compact::compact(&self.segments, ids, self.files, target)?;
        Ok(Self {
            segments: vec![Arc::new(merged)],
            ..self
        })
    }

    // constraint: a file the build could not index may hold anything, so it stays admitted whatever the requirement
    pub(crate) fn narrow(
        &self,
        requirement: &Requirement,
        ids: IdPositions<'_>,
        entries: usize,
    ) -> Option<Narrowed> {
        let mut positions = Bits::new(entries);
        for segment in &self.segments {
            segment.admit(requirement, ids, &mut positions)?;
        }
        for &at in &self.unindexed {
            positions.insert(at as usize);
        }

        Some(Narrowed { positions })
    }
}

struct Unreadable;

type Lookup<T> = Result<T, Unreadable>;

struct Bucket {
    postings: usize,
    directory: Range<usize>,
}

#[derive(Clone, Copy)]
struct Located {
    shape: Shape,
    start: usize,
}

impl ContentIndex {
    #[cfg(test)]
    const fn stats(&self) -> ContentIndexStats {
        self.stats
    }

    pub(crate) const fn files(&self) -> usize {
        self.stats.files
    }

    pub(crate) const fn with_first_id(mut self, first_id: u32) -> Self {
        self.first_id = first_id;
        self
    }

    fn admit(
        &self,
        requirement: &Requirement,
        ids: IdPositions<'_>,
        positions: &mut Bits,
    ) -> Option<()> {
        let found = self.ids(requirement).ok()??;
        let offset = self.first_id.checked_sub(ids.first)? as usize;
        found.for_each_one(|id| {
            if let Some(&at) = ids.positions.get(offset + id) {
                positions.insert(at as usize);
            }
        });

        Some(())
    }

    fn ids(&self, requirement: &Requirement) -> Lookup<Option<Bits>> {
        match requirement {
            Requirement::Anything => Ok(None),
            Requirement::Trigram(trigram) => Ok(Some(match self.locate(*trigram)? {
                Some(list) => self.decode(list.shape, &self.read(list)?),
                None => self.no_ids(),
            })),
            Requirement::All(parts) => self.all(parts),
            Requirement::Any(parts) => self.any(parts),
        }
    }

    // constraint: the rarest list goes first and each next one only filters what is left, so a requirement costs about
    // its rarest trigram's list however common the others are
    fn all(&self, parts: &[Requirement]) -> Lookup<Option<Bits>> {
        let mut lists = Vec::new();
        let mut others = Vec::new();
        for part in parts {
            match part {
                Requirement::Trigram(trigram) => match self.locate(*trigram)? {
                    Some(list) => lists.push(list),
                    None => return Ok(Some(self.no_ids())),
                },
                other => others.push(other),
            }
        }
        lists.sort_by_key(|list| list.shape.len());

        let mut found: Option<Bits> = None;
        for list in lists {
            let bytes = self.read(list)?;
            let kept = found.take().map_or_else(
                || self.decode(list.shape, &bytes),
                |found| found.keep_listed(list.shape, &bytes),
            );
            let none_left = kept.is_empty();
            found = Some(kept);
            if none_left {
                return Ok(found);
            }
        }
        for other in others {
            if let Some(ids) = self.ids(other)? {
                found = Some(match found {
                    None => ids,
                    Some(found) => found.and(&ids),
                });
            }
        }

        Ok(found)
    }

    fn any(&self, parts: &[Requirement]) -> Lookup<Option<Bits>> {
        let mut found = self.no_ids();
        for part in parts {
            if let Requirement::Trigram(trigram) = part {
                self.add_holding(&mut found, *trigram)?;
                continue;
            }
            match self.ids(part)? {
                None => return Ok(None),
                Some(ids) => found.or(&ids),
            }
        }

        Ok(Some(found))
    }

    fn add_holding(&self, found: &mut Bits, trigram: Trigram) -> Lookup<()> {
        if let Some(list) = self.locate(trigram)? {
            found.insert_listed(list.shape, &self.read(list)?);
        }

        Ok(())
    }

    fn no_ids(&self) -> Bits {
        Bits::new(self.universe as usize)
    }

    fn decode(&self, shape: Shape, bytes: &[u8]) -> Bits {
        let mut ids = self.no_ids();
        ids.insert_listed(shape, bytes);
        ids
    }

    // constraint: a bucket's entries end where the next bucket's begin, so one read of two adjacent table entries bounds
    // them
    fn bucket(&self, bucket: usize) -> Lookup<Bucket> {
        let at = self.buckets_start + bucket * BUCKET_BYTES;
        let entries = self
            .blob
            .bytes(at..at + 2 * BUCKET_BYTES)
            .ok_or(Unreadable)?;
        let (this, next) = entries.split_at(BUCKET_BYTES);
        let (postings, directory) = this.split_first_chunk::<8>().ok_or(Unreadable)?;
        let directory = directory.first_chunk::<4>().ok_or(Unreadable)?;
        let directory_end = next
            .get(8..)
            .and_then(|end| end.first_chunk::<4>())
            .ok_or(Unreadable)?;

        Ok(Bucket {
            postings: usize::try_from(u64::from_le_bytes(*postings)).map_err(|_| Unreadable)?,
            directory: u32::from_le_bytes(*directory) as usize
                ..u32::from_le_bytes(*directory_end) as usize,
        })
    }

    fn locate(&self, trigram: Trigram) -> Lookup<Option<Located>> {
        let bucket = self.bucket((trigram >> 8) as usize)?;
        let entries = self
            .blob
            .bytes(
                self.directory_start + bucket.directory.start
                    ..self.directory_start + bucket.directory.end,
            )
            .ok_or(Unreadable)?;
        let third = trigram.to_le_bytes()[0];

        let mut postings = bucket.postings;
        let mut at = 0;
        while let Some(&stored) = entries.get(at) {
            at += 1;
            let shape = Shape::new(
                read_varint(&entries, &mut at).ok_or(Unreadable)?,
                self.universe,
            );
            if stored == third {
                return Ok(Some(Located {
                    shape,
                    start: postings,
                }));
            }
            postings += shape.bytes();
        }

        Ok(None)
    }

    fn read(&self, list: Located) -> Lookup<Cow<'_, [u8]>> {
        self.blob
            .bytes(list.start..list.start + list.shape.bytes())
            .ok_or(Unreadable)
    }

    #[cfg(test)]
    fn files_holding(&self, trigram: Trigram) -> Vec<u32> {
        let mut ids = Vec::new();
        if let Ok(Some(list)) = self.locate(trigram)
            && let Ok(bytes) = self.read(list)
        {
            postings::for_each(list.shape, &bytes, |id| ids.push(id));
        }
        ids
    }

    #[cfg(test)]
    fn all_bytes(&self) -> Vec<u8> {
        self.blob
            .bytes(0..self.blob.len())
            .expect("the whole blob reads")
            .into_owned()
    }
}

#[derive(Debug, Clone)]
struct Bits {
    words: Vec<u64>,
}

impl Bits {
    fn new(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
        }
    }

    fn insert(&mut self, at: usize) {
        if let Some(word) = self.words.get_mut(at / 64) {
            *word |= 1 << (at % 64);
        }
    }

    fn contains(&self, at: usize) -> bool {
        self.words
            .get(at / 64)
            .is_some_and(|word| word & (1 << (at % 64)) != 0)
    }

    fn is_empty(&self) -> bool {
        self.words.iter().all(|&word| word == 0)
    }

    fn and(mut self, other: &Self) -> Self {
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word &= other;
        }
        self
    }

    fn or(&mut self, other: &Self) {
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }

    fn insert_listed(&mut self, shape: Shape, bytes: &[u8]) {
        postings::for_each(shape, bytes, |id| self.insert(id as usize));
    }

    fn keep_listed(mut self, shape: Shape, bytes: &[u8]) -> Self {
        if shape.is_bitmap() {
            for (word, listed) in self.words.iter_mut().zip(bytes.chunks(8)) {
                let mut padded = [0; 8];
                padded[..listed.len()].copy_from_slice(listed);
                *word &= u64::from_le_bytes(padded);
            }
            return self;
        }

        let mut kept = Self::new(self.words.len() * 64);
        postings::for_each(shape, bytes, |id| {
            if self.contains(id as usize) {
                kept.insert(id as usize);
            }
        });
        kept
    }

    fn for_each_one(&self, mut each: impl FnMut(usize)) {
        for (at, &word) in self.words.iter().enumerate() {
            let mut rest = word;
            while rest != 0 {
                each(at * 64 + rest.trailing_zeros() as usize);
                rest &= rest - 1;
            }
        }
    }
}

fn put_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push(value.to_le_bytes()[0] | 0x80);
        value >>= 7;
    }
    out.push(value.to_le_bytes()[0]);
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let mut value = 0;

    for shift in (0..u32::BITS).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u32::from(byte & 0x7F).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }

    None
}

// constraint: the derived form would print every byte of the index
impl fmt::Debug for ContentIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentIndex")
            .field("stats", &self.stats)
            .field("first_id", &self.first_id)
            .field("storage", &self.blob.kind())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(files: &[Vec<Trigram>], run_pairs: usize) -> ContentIndex {
        stored_in(&ContentIndexStorage::Memory, files, run_pairs)
    }

    fn stored_in(
        storage: &ContentIndexStorage,
        files: &[Vec<Trigram>],
        run_pairs: usize,
    ) -> ContentIndex {
        let collector = Collector::with_run_pairs(storage, run_pairs).expect("create the storage");
        let mut pairs = collector.pairs();
        for (id, trigrams) in (0..).zip(files) {
            if let Some(full) = pairs.push(id, trigrams, &collector) {
                collector.spill(full);
            }
        }
        let universe = u32::try_from(files.len()).expect("a small test");

        match collector.finish(pairs, universe, None) {
            Ok(index) => index,
            Err(Unfinished::Cancelled) => panic!("nothing cancels the build"),
            Err(Unfinished::Failed(error)) => panic!("the storage failed: {error}"),
        }
    }

    // Trigrams across many parts and buckets, each file holding its own subset.
    fn files(count: u32) -> Vec<Vec<Trigram>> {
        let trigrams: Vec<Trigram> = (0..300)
            .map(|at: Trigram| ((at % 7 * 37) << 16) | ((at % 11 * 23) << 8) | (at * 13 % 256))
            .collect();

        (0..count)
            .map(|id| {
                trigrams
                    .iter()
                    .copied()
                    .filter(|trigram| (trigram ^ id).is_multiple_of(3) || id % 5 == 0)
                    .collect()
            })
            .collect()
    }

    fn expected(files: &[Vec<Trigram>], trigram: Trigram) -> Vec<u32> {
        (0..)
            .zip(files)
            .filter(|(_, held)| held.contains(&trigram))
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn each_trigram_lists_the_files_holding_it() {
        let files = files(400);
        let index = index_of(&files, RUN_PAIRS_FOR_TESTS);

        let mut trigrams: Vec<Trigram> = files.iter().flatten().copied().collect();
        trigrams.sort_unstable();
        trigrams.dedup();
        for trigram in trigrams {
            assert_eq!(
                index.files_holding(trigram),
                expected(&files, trigram),
                "trigram {trigram:06x}"
            );
        }
        assert_eq!(
            index.files_holding(0xFF_FFFF),
            [] as [u32; 0],
            "a trigram no file holds"
        );
        assert_eq!(index.stats().files(), 400);
    }

    #[test]
    fn many_small_runs_merge_into_the_index_one_run_builds() {
        let files = files(400);

        let pairs: usize = files.iter().map(Vec::len).sum();
        let whole = index_of(&files, pairs + 1);
        let spilled = index_of(&files, 97);

        assert_eq!(whole.stats(), spilled.stats());
        assert!(
            whole.all_bytes() == spilled.all_bytes(),
            "the merged index is byte for byte the same"
        );
    }

    #[test]
    fn merging_segments_keeps_the_files_still_answered_for_and_only_those() {
        let only_gone_files_hold: Trigram = 0x00_0001;
        let mut earlier = files(40);
        earlier[3].push(only_gone_files_hold);
        let later = files(60).split_off(40);
        let segments = vec![
            Arc::new(index_of(&earlier, RUN_PAIRS_FOR_TESTS)),
            Arc::new(index_of(&later, RUN_PAIRS_FOR_TESTS).with_first_id(40)),
        ];
        let gone = [3, 7, 41, 59];
        let positions: Vec<u32> = (0..60)
            .map(|id| if gone.contains(&id) { NO_POSITION } else { id })
            .collect();
        let ids = IdPositions {
            first: 0,
            positions: &positions,
        };
        let target = Target {
            storage: &ContentIndexStorage::Memory,
            cancel: None,
        };

        let Ok(merged) = compact::compact(&segments, ids, 56, target) else {
            panic!("merge the segments");
        };

        let all: Vec<Vec<Trigram>> = earlier.iter().chain(&later).cloned().collect();
        let mut held: Vec<Trigram> = (0..)
            .zip(&all)
            .filter(|(id, _)| !gone.contains(id))
            .flat_map(|(_, trigrams)| trigrams.iter().copied())
            .collect();
        held.sort_unstable();
        held.dedup();
        for &trigram in &held {
            let answered: Vec<u32> = expected(&all, trigram)
                .into_iter()
                .filter(|id| !gone.contains(id))
                .collect();
            assert_eq!(
                merged.files_holding(trigram),
                answered,
                "trigram {trigram:06x}"
            );
        }
        assert_eq!(merged.files_holding(only_gone_files_hold), [] as [u32; 0]);
        assert_eq!(merged.stats().trigrams(), held.len());
        assert_eq!(merged.stats().files(), 56);
    }

    #[test]
    fn segments_merge_past_eight_or_once_more_than_a_quarter_of_their_files_are_gone() {
        let target = Target {
            storage: &ContentIndexStorage::Memory,
            cancel: None,
        };
        let segments_after = |count: u32, gone: u32| {
            let positions: Vec<u32> = (0..count * 4)
                .map(|id| if id < gone { NO_POSITION } else { id })
                .collect();
            let ids = IdPositions {
                first: 0,
                positions: &positions,
            };
            let segments = (0..count)
                .map(|at| Arc::new(index_of(&files(4), RUN_PAIRS_FOR_TESTS).with_first_id(at * 4)))
                .collect();
            let live = (count * 4 - gone) as usize;
            let Ok(indexes) =
                ContentIndexes::new(segments, Vec::new(), live).compacted_when_due(ids, target)
            else {
                panic!("merge the segments");
            };
            indexes.segments.len()
        };

        assert_eq!(segments_after(8, 0), 8, "eight segments stay apart");
        assert_eq!(segments_after(9, 0), 1, "a ninth merges them");
        assert_eq!(segments_after(2, 2), 2, "a quarter of their files gone");
        assert_eq!(segments_after(2, 3), 1, "more than a quarter gone");
    }

    #[test]
    fn a_build_that_indexed_nothing_answers_nothing() {
        let index = index_of(&[], RUN_PAIRS_FOR_TESTS);

        assert_eq!(index.stats().trigrams(), 0);
        assert_eq!(index.files_holding(0x61_6263), [] as [u32; 0]);
    }

    #[test]
    fn the_files_left_unindexed_count_toward_the_bytes_the_index_takes() {
        let index = index_of(&files(10), RUN_PAIRS_FOR_TESTS);
        let lists = index.stats().bytes();

        let indexes = ContentIndexes::new(vec![Arc::new(index)], vec![7, 3, 7], 10);

        assert_eq!(
            indexes.stats().bytes(),
            lists + 8,
            "two distinct positions of four bytes each"
        );
    }

    #[test]
    fn every_storage_holds_the_index_memory_holds() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let files = files(400);
        let in_memory = index_of(&files, 97);

        for storage in [
            ContentIndexStorage::File {
                dir: dir.path().to_path_buf(),
            },
            ContentIndexStorage::Mapped {
                dir: dir.path().to_path_buf(),
            },
        ] {
            let stored = stored_in(&storage, &files, 97);

            assert_eq!(stored.stats(), in_memory.stats(), "{storage:?}");
            assert!(
                stored.all_bytes() == in_memory.all_bytes(),
                "{storage:?} holds the bytes memory holds"
            );
            assert_eq!(
                stored.files_holding(files[0][0]),
                in_memory.files_holding(files[0][0])
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_scratch_file_leaves_no_name_behind() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let storage = ContentIndexStorage::Mapped {
            dir: dir.path().to_path_buf(),
        };

        let index = stored_in(&storage, &files(50), 97);

        assert_eq!(
            std::fs::read_dir(dir.path()).expect("list").count(),
            0,
            "the spill and the index are open, and neither has a name in the directory"
        );
        drop(index);
    }

    const RUN_PAIRS_FOR_TESTS: usize = 1 << 10;
}
