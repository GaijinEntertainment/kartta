use std::fmt;

use crate::postings::{self, Shape};
use crate::requirement::Requirement;
use crate::trigram::Trigram;

mod build;

pub(crate) use build::{Collector, Full, Pairs};

const PARTS: usize = 256;
const BUCKETS: usize = 1 << 16;
const BUCKET_BYTES: usize = 12;

/// Where a build keeps the content index it makes.
///
/// A content index records, for every trigram — every run of three bytes, ASCII letters folded to lowercase — which
/// files hold it. See [`BuildOptions::content_index`](crate::BuildOptions::content_index) for what a build pays for one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ContentIndexStorage {
    /// In the process's own memory, beside the rest of the index.
    Memory,
}

/// What a content index holds, as [`Index::content_index_stats`](crate::Index::content_index_stats) reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentIndexStats {
    files: usize,
    trigrams: usize,
    bytes: u64,
}

impl ContentIndexStats {
    /// How many files the index holds the content of: every text file the build read to its end.
    ///
    /// A file the build could not read is not among them, and is reported in
    /// [`BuildOutcome::errors`](crate::BuildOutcome::errors) instead.
    #[must_use]
    pub const fn files(&self) -> usize {
        self.files
    }

    /// How many distinct trigrams those files hold between them.
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

pub(crate) struct ContentIndex {
    stats: ContentIndexStats,
    universe: u32,
    blob: Box<[u8]>,
    directory_start: usize,
    buckets_start: usize,
    unindexed: Box<[u32]>,
}

pub(crate) struct Narrowed {
    positions: Bits,
}

impl Narrowed {
    pub(crate) fn admits(&self, at: usize) -> bool {
        self.positions.contains(at)
    }
}

struct Unreadable;

type Lookup<T> = Result<T, Unreadable>;

impl ContentIndex {
    pub(crate) const fn stats(&self) -> ContentIndexStats {
        self.stats
    }

    pub(crate) fn with_unindexed(mut self, mut positions: Vec<u32>) -> Self {
        positions.sort_unstable();
        positions.dedup();
        self.unindexed = positions.into_boxed_slice();
        self.stats.bytes = (self.blob.len() + size_of_val(&*self.unindexed)) as u64;
        self
    }

    // constraint: a file the build could not index may hold anything, so it stays admitted whatever the requirement
    pub(crate) fn narrow(
        &self,
        requirement: &Requirement,
        content_positions: &[u32],
        entries: usize,
    ) -> Option<Narrowed> {
        let ids = self.ids(requirement).ok()??;
        let mut positions = Bits::new(entries);
        ids.for_each_one(|id| {
            if let Some(&at) = content_positions.get(id) {
                positions.insert(at as usize);
            }
        });
        for &at in &self.unindexed {
            positions.insert(at as usize);
        }

        Some(Narrowed { positions })
    }

    fn ids(&self, requirement: &Requirement) -> Lookup<Option<Bits>> {
        match requirement {
            Requirement::Anything => Ok(None),
            Requirement::Trigram(trigram) => Ok(Some(match self.list(*trigram)? {
                Some((shape, bytes)) => self.decode(shape, bytes),
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
                Requirement::Trigram(trigram) => match self.list(*trigram)? {
                    Some(list) => lists.push(list),
                    None => return Ok(Some(self.no_ids())),
                },
                other => others.push(other),
            }
        }
        lists.sort_by_key(|(shape, _)| shape.len());

        let mut found: Option<Bits> = None;
        for (shape, bytes) in lists {
            let kept = found.take().map_or_else(
                || self.decode(shape, bytes),
                |found| found.keep_listed(shape, bytes),
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
        if let Some((shape, bytes)) = self.list(trigram)? {
            found.insert_listed(shape, bytes);
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

    fn bucket(&self, bucket: usize) -> Lookup<(usize, usize)> {
        let at = self.buckets_start + bucket * BUCKET_BYTES;
        let entry = self.blob.get(at..at + BUCKET_BYTES).ok_or(Unreadable)?;
        let (postings, directory) = entry.split_first_chunk::<8>().ok_or(Unreadable)?;
        let directory = directory.first_chunk::<4>().ok_or(Unreadable)?;

        Ok((
            usize::try_from(u64::from_le_bytes(*postings)).map_err(|_| Unreadable)?,
            u32::from_le_bytes(*directory) as usize,
        ))
    }

    fn list(&self, trigram: Trigram) -> Lookup<Option<(Shape, &[u8])>> {
        let bucket = (trigram >> 8) as usize;
        let (mut postings, start) = self.bucket(bucket)?;
        let (_, end) = self.bucket(bucket + 1)?;
        let entries = self
            .blob
            .get(self.directory_start + start..self.directory_start + end)
            .ok_or(Unreadable)?;
        let third = trigram.to_le_bytes()[0];

        let mut at = 0;
        while let Some(&stored) = entries.get(at) {
            at += 1;
            let shape = Shape::new(
                read_varint(entries, &mut at).ok_or(Unreadable)?,
                self.universe,
            );
            if stored == third {
                let bytes = self
                    .blob
                    .get(postings..postings + shape.bytes())
                    .ok_or(Unreadable)?;
                return Ok(Some((shape, bytes)));
            }
            postings += shape.bytes();
        }

        Ok(None)
    }

    #[cfg(test)]
    fn files_holding(&self, trigram: Trigram) -> Vec<u32> {
        let mut ids = Vec::new();
        if let Ok(Some((shape, bytes))) = self.list(trigram) {
            postings::for_each(shape, bytes, |id| ids.push(id));
        }
        ids
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
            .field("unindexed", &self.unindexed.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(files: &[Vec<Trigram>], run_pairs: usize) -> ContentIndex {
        let collector = Collector::with_run_pairs(run_pairs);
        let mut pairs = collector.pairs();
        for (id, trigrams) in (0..).zip(files) {
            if let Some(full) = pairs.push(id, trigrams, &collector) {
                collector.spill(full);
            }
        }
        let universe = u32::try_from(files.len()).expect("a small test");

        collector
            .finish(pairs, universe, None)
            .expect("nothing cancels the build")
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
            whole.blob == spilled.blob,
            "the merged index is byte for byte the same"
        );
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

        let index = index.with_unindexed(vec![7, 3, 7]);

        assert_eq!(
            index.stats().bytes(),
            lists + 8,
            "two distinct positions of four bytes each"
        );
    }

    const RUN_PAIRS_FOR_TESTS: usize = 1 << 10;
}
