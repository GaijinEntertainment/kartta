use std::fmt;

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
    /// How many files the index holds the content of: every text file the build read whole.
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

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "only content search reads the lists back, and it does not search through the index yet"
    )
)]
pub(crate) struct ContentIndex {
    stats: ContentIndexStats,
    universe: u32,
    blob: Box<[u8]>,
    directory_start: usize,
    buckets_start: usize,
    unindexed: Box<[u32]>,
}

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

    #[cfg(test)]
    fn bucket(&self, bucket: usize) -> (usize, usize) {
        let at = self.buckets_start + bucket * BUCKET_BYTES;
        let entry = &self.blob[at..at + BUCKET_BYTES];
        let postings = u64::from_le_bytes(entry[..8].try_into().expect("eight bytes"));
        let directory = u32::from_le_bytes(entry[8..].try_into().expect("four bytes"));

        (
            usize::try_from(postings).expect("an in-memory offset"),
            directory as usize,
        )
    }

    #[cfg(test)]
    fn list(&self, trigram: crate::trigram::Trigram) -> Option<(crate::postings::Shape, &[u8])> {
        let bucket = (trigram >> 8) as usize;
        let (mut postings, start) = self.bucket(bucket);
        let (_, end) = self.bucket(bucket + 1);
        let entries = &self.blob[self.directory_start + start..self.directory_start + end];
        let third = trigram.to_le_bytes()[0];

        let mut at = 0;
        while at < entries.len() {
            let stored = entries[at];
            at += 1;
            let shape = crate::postings::Shape::new(read_varint(entries, &mut at)?, self.universe);
            if stored == third {
                return Some((shape, &self.blob[postings..postings + shape.bytes()]));
            }
            postings += shape.bytes();
        }

        None
    }

    #[cfg(test)]
    fn files_holding(&self, trigram: crate::trigram::Trigram) -> Vec<u32> {
        let mut ids = Vec::new();
        if let Some((shape, bytes)) = self.list(trigram) {
            crate::postings::for_each(shape, bytes, |id| ids.push(id));
        }
        ids
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
    use crate::trigram::Trigram;

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
