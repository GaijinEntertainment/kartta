use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::mem;
use std::sync::{Mutex, MutexGuard, PoisonError};

use kartta_walk::CancelToken;
use rayon::prelude::*;

use super::{
    BUCKET_BYTES, BUCKETS, ContentIndex, ContentIndexStats, PARTS, put_varint, read_varint,
};
use crate::postings::{self, Shape, Writer};
use crate::trigram::{Trigram, Trigrams};

const RUN_PAIRS: usize = 1 << 20;
const BUCKETS_PER_PART: usize = BUCKETS / PARTS;
const PART_BUCKET_BYTES: usize = BUCKETS_PER_PART * BUCKET_BYTES;
const LOWS: usize = 1 << 16;

pub(crate) struct Collector {
    run_pairs: usize,
    runs: Mutex<Vec<Run>>,
    spare_pairs: Mutex<Vec<Vec<u64>>>,
    extractors: Mutex<Vec<Trigrams>>,
}

pub(crate) struct Pairs {
    buffer: Vec<u64>,
    base: u32,
    files: usize,
}

pub(crate) struct Full {
    pairs: Vec<u64>,
    base: u32,
    universe: u32,
}

struct Run {
    base: u32,
    universe: u32,
    parts: Vec<Box<[u8]>>,
}

struct Part {
    base: u32,
    universe: u32,
    bytes: Box<[u8]>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Collector {
    pub(crate) const fn new() -> Self {
        Self::with_run_pairs(RUN_PAIRS)
    }

    pub(super) const fn with_run_pairs(run_pairs: usize) -> Self {
        Self {
            run_pairs,
            runs: Mutex::new(Vec::new()),
            spare_pairs: Mutex::new(Vec::new()),
            extractors: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn pairs(&self) -> Pairs {
        Pairs {
            buffer: self.spare(),
            base: 0,
            files: 0,
        }
    }

    pub(crate) fn extractor(&self) -> Trigrams {
        lock(&self.extractors).pop().unwrap_or_else(Trigrams::new)
    }

    pub(crate) fn recycle(&self, mut extractor: Trigrams) {
        extractor.clear();
        lock(&self.extractors).push(extractor);
    }

    // constraint: a buffer the walk keeps filling stays allocated and faulted in, so a run is never filled into fresh
    // pages under the lock that orders the ids
    fn spare(&self) -> Vec<u64> {
        lock(&self.spare_pairs)
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(self.run_pairs))
    }

    pub(crate) fn spill(&self, full: Full) {
        let Full {
            mut pairs,
            base,
            universe,
        } = full;
        let mut scratch = self.spare();
        sort_by_trigram(&mut pairs, &mut scratch);
        let parts = encode_run(&pairs, base, universe);

        pairs.clear();
        scratch.clear();
        lock(&self.spare_pairs).extend([pairs, scratch]);
        lock(&self.runs).push(Run {
            base,
            universe,
            parts,
        });
    }

    pub(crate) fn finish(
        self,
        pairs: Pairs,
        universe: u32,
        cancel: Option<&CancelToken>,
    ) -> Option<ContentIndex> {
        let files = pairs.files;
        if !pairs.buffer.is_empty() {
            self.spill(Full {
                universe: universe - pairs.base,
                base: pairs.base,
                pairs: pairs.buffer,
            });
        }
        let parts = by_part(
            self.runs
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
        );

        let sizes: Vec<Sizes> = parts
            .par_iter()
            .map(|parts| Sizes::of(parts, universe))
            .collect();
        let layout = Layout::of(&sizes);
        let mut blob = vec![0; layout.bytes()].into_boxed_slice();
        let trigrams = sizes.iter().map(|sizes| sizes.lens.len()).sum();

        let tasks = layout.tasks(&mut blob, parts, sizes);
        let written = tasks.into_par_iter().all(|task| {
            let go_on = cancel.is_none_or(|cancel| !cancel.is_cancelled());
            if go_on {
                task.write(universe);
            }
            go_on
        });
        if !written {
            return None;
        }

        Some(ContentIndex {
            stats: ContentIndexStats {
                files,
                trigrams,
                bytes: blob.len() as u64,
            },
            universe,
            directory_start: layout.postings,
            buckets_start: layout.postings + layout.directory,
            blob,
            unindexed: Box::default(),
        })
    }
}

impl Pairs {
    pub(crate) fn push(
        &mut self,
        id: u32,
        trigrams: &[Trigram],
        collector: &Collector,
    ) -> Option<Full> {
        self.files += 1;
        self.buffer.extend(
            trigrams
                .iter()
                .map(|&trigram| (u64::from(trigram) << 32) | u64::from(id)),
        );
        if self.buffer.len() < collector.run_pairs {
            return None;
        }

        let full = Full {
            pairs: mem::replace(&mut self.buffer, collector.spare()),
            base: self.base,
            universe: id + 1 - self.base,
        };
        self.base = id + 1;
        Some(full)
    }
}

const fn trigram_of(pair: u64) -> Trigram {
    (pair >> 32) as Trigram
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a pair packs a u32 id below its trigram, so the low half is the id whole"
)]
const fn id_of(pair: u64) -> u32 {
    pair as u32
}

// constraint: the pairs arrive in ascending id order, so a stable sort on the trigram alone leaves each trigram's ids
// ascending, as a posting list needs them
fn sort_by_trigram(pairs: &mut Vec<u64>, scratch: &mut Vec<u64>) {
    const DIGIT: u32 = 12;

    scratch.resize(pairs.len(), 0);
    for shift in [32, 32 + DIGIT] {
        let digit = |pair: u64| ((pair >> shift) & ((1 << DIGIT) - 1)) as usize;
        let mut starts = vec![0; 1 << DIGIT];
        for &pair in pairs.iter() {
            starts[digit(pair)] += 1;
        }
        let mut sum = 0;
        for start in &mut starts {
            sum += mem::replace(start, sum);
        }
        for &pair in pairs.iter() {
            let start = &mut starts[digit(pair)];
            scratch[*start] = pair;
            *start += 1;
        }
        mem::swap(pairs, scratch);
    }
}

fn encode_run(pairs: &[u64], base: u32, universe: u32) -> Vec<Box<[u8]>> {
    let mut parts = vec![Box::default(); PARTS];
    let mut out = Vec::new();
    let mut part = None;
    let mut previous = 0;

    for group in pairs.chunk_by(|left, right| trigram_of(*left) == trigram_of(*right)) {
        let trigram = trigram_of(group[0]);
        let this_part = (trigram >> 16) as usize;
        if part != Some(this_part) {
            if let Some(done) = part {
                parts[done] = out.as_slice().into();
                out.clear();
            }
            part = Some(this_part);
            previous = 0;
        }

        let low = trigram & 0xFFFF;
        put_varint(&mut out, low - previous);
        previous = low;
        let shape = Shape::new(narrow(group.len()), universe);
        put_varint(&mut out, narrow(group.len()));
        let start = out.len();
        out.resize(start + shape.bytes(), 0);
        let mut writer = Writer::new(shape, &mut out[start..]);
        for &pair in group {
            writer.push(id_of(pair) - base);
        }
    }
    if let Some(done) = part {
        parts[done] = out.into_boxed_slice();
    }

    parts
}

#[expect(
    clippy::expect_used,
    reason = "one run holds fewer pairs than a u32 counts: it spills at a fixed size far below that"
)]
fn narrow(len: usize) -> u32 {
    u32::try_from(len).expect("a run holds fewer pairs than a u32 counts")
}

fn by_part(mut runs: Vec<Run>) -> Vec<Vec<Part>> {
    runs.sort_unstable_by_key(|run| run.base);
    let mut parts: Vec<Vec<Part>> = (0..PARTS).map(|_| Vec::with_capacity(runs.len())).collect();

    for run in runs {
        for (into, bytes) in parts.iter_mut().zip(run.parts) {
            if !bytes.is_empty() {
                into.push(Part {
                    base: run.base,
                    universe: run.universe,
                    bytes,
                });
            }
        }
    }

    parts
}

#[derive(Clone, Copy)]
struct Group<'a> {
    low: u16,
    len: u32,
    list: &'a [u8],
}

struct Groups<'a> {
    part: &'a Part,
    at: usize,
    low: u32,
}

impl<'a> Groups<'a> {
    const fn new(part: &'a Part) -> Self {
        Self {
            part,
            at: 0,
            low: 0,
        }
    }
}

impl<'a> Iterator for Groups<'a> {
    type Item = Group<'a>;

    fn next(&mut self) -> Option<Group<'a>> {
        let bytes: &'a [u8] = &self.part.bytes;
        self.low += read_varint(bytes, &mut self.at)?;
        let len = read_varint(bytes, &mut self.at)?;
        let start = self.at;
        self.at += Shape::new(len, self.part.universe).bytes();

        Some(Group {
            low: u16::try_from(self.low).ok()?,
            len,
            list: bytes.get(start..self.at)?,
        })
    }
}

struct Sizes {
    lens: Vec<(u16, u32)>,
    postings: usize,
    directory: usize,
}

impl Sizes {
    fn of(parts: &[Part], universe: u32) -> Self {
        if parts.is_empty() {
            return Self {
                lens: Vec::new(),
                postings: 0,
                directory: 0,
            };
        }

        let mut lens = vec![0u32; LOWS];
        for group in parts.iter().flat_map(Groups::new) {
            lens[usize::from(group.low)] += group.len;
        }
        let lens: Vec<(u16, u32)> = (0..=u16::MAX)
            .zip(lens)
            .filter(|&(_, len)| len > 0)
            .collect();

        Self {
            postings: lens
                .iter()
                .map(|&(_, len)| Shape::new(len, universe).bytes())
                .sum(),
            directory: lens.iter().map(|&(_, len)| 1 + varint_len(len)).sum(),
            lens,
        }
    }
}

const fn varint_len(value: u32) -> usize {
    (value | 1).bit_width().div_ceil(7) as usize
}

struct Layout {
    postings: usize,
    directory: usize,
}

impl Layout {
    fn of(sizes: &[Sizes]) -> Self {
        Self {
            postings: sizes.iter().map(|sizes| sizes.postings).sum(),
            directory: sizes.iter().map(|sizes| sizes.directory).sum(),
        }
    }

    const fn bytes(&self) -> usize {
        self.postings + self.directory + (BUCKETS + 1) * BUCKET_BYTES
    }

    fn tasks<'a>(
        &self,
        blob: &'a mut [u8],
        parts: Vec<Vec<Part>>,
        sizes: Vec<Sizes>,
    ) -> Vec<Task<'a>> {
        let (mut postings, rest) = blob.split_at_mut(self.postings);
        let (mut directory, buckets) = rest.split_at_mut(self.directory);
        let (buckets, end) = buckets.as_chunks_mut::<PART_BUCKET_BYTES>();
        write_bucket(end, self.postings, self.directory);
        let (mut postings_at, mut directory_at) = (0, 0);

        parts
            .into_iter()
            .zip(sizes)
            .zip(buckets)
            .map(|((parts, sizes), buckets)| {
                let task = Task {
                    postings: postings.split_off_mut(..sizes.postings).unwrap_or_default(),
                    directory: directory
                        .split_off_mut(..sizes.directory)
                        .unwrap_or_default(),
                    buckets,
                    postings_at,
                    directory_at,
                    parts,
                    lens: sizes.lens,
                };
                postings_at += sizes.postings;
                directory_at += sizes.directory;
                task
            })
            .collect()
    }
}

#[expect(
    clippy::expect_used,
    reason = "a directory entry takes at most six bytes and there are fewer than 2^24 trigrams, far below 4 GiB"
)]
fn write_bucket(bucket: &mut [u8], postings: usize, directory: usize) {
    let directory = u32::try_from(directory).expect("a directory stays below 4 GiB");
    bucket[..8].copy_from_slice(&(postings as u64).to_le_bytes());
    bucket[8..BUCKET_BYTES].copy_from_slice(&directory.to_le_bytes());
}

struct Task<'a> {
    parts: Vec<Part>,
    lens: Vec<(u16, u32)>,
    postings: &'a mut [u8],
    postings_at: usize,
    directory: &'a mut [u8],
    directory_at: usize,
    buckets: &'a mut [u8],
}

impl Task<'_> {
    fn write(self, universe: u32) {
        let mut merge = Merge::new(&self.parts);
        let mut directory = Vec::with_capacity(self.directory.len());
        let mut postings_at = 0;
        let mut starts = Starts {
            buckets: self.buckets,
            next: 0,
            postings_at: self.postings_at,
            directory_at: self.directory_at,
        };

        for &(low, len) in &self.lens {
            starts.through(usize::from(low >> 8), postings_at, directory.len());
            let shape = Shape::new(len, universe);
            let end = postings_at + shape.bytes();
            merge.write(
                low,
                &mut Writer::new(shape, &mut self.postings[postings_at..end]),
            );
            postings_at = end;
            directory.push(low.to_le_bytes()[0]);
            put_varint(&mut directory, len);
        }
        starts.through(BUCKETS_PER_PART - 1, postings_at, directory.len());
        self.directory.copy_from_slice(&directory);
    }
}

struct Starts<'a> {
    buckets: &'a mut [u8],
    next: usize,
    postings_at: usize,
    directory_at: usize,
}

impl Starts<'_> {
    fn through(&mut self, bucket: usize, postings: usize, directory: usize) {
        while self.next <= bucket {
            let entry = &mut self.buckets[self.next * BUCKET_BYTES..(self.next + 1) * BUCKET_BYTES];
            write_bucket(
                entry,
                self.postings_at + postings,
                self.directory_at + directory,
            );
            self.next += 1;
        }
    }
}

struct Merge<'a> {
    parts: &'a [Part],
    groups: Vec<Groups<'a>>,
    heads: Vec<Option<Group<'a>>>,
    queue: BinaryHeap<Reverse<(u16, usize)>>,
}

impl<'a> Merge<'a> {
    fn new(parts: &'a [Part]) -> Self {
        let mut groups: Vec<Groups<'a>> = parts.iter().map(Groups::new).collect();
        let heads: Vec<Option<Group<'a>>> = groups.iter_mut().map(Iterator::next).collect();
        let queue = heads
            .iter()
            .enumerate()
            .filter_map(|(at, head)| head.map(|group| Reverse((group.low, at))))
            .collect();

        Self {
            parts,
            groups,
            heads,
            queue,
        }
    }

    // constraint: the queue orders equal trigrams by part, and parts hold ascending id ranges, so each trigram's ids
    // reach the writer ascending
    fn write(&mut self, low: u16, writer: &mut Writer<'_>) {
        while let Some(&Reverse((head, at))) = self.queue.peek()
            && head == low
        {
            self.queue.pop();
            let Some(group) = self.heads[at] else {
                continue;
            };
            let part = &self.parts[at];
            postings::for_each(Shape::new(group.len, part.universe), group.list, |id| {
                writer.push(part.base + id);
            });

            self.heads[at] = self.groups[at].next();
            if let Some(next) = self.heads[at] {
                self.queue.push(Reverse((next.low, at)));
            }
        }
    }
}
