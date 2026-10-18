use std::io;
use std::ops::ControlFlow;
use std::sync::Arc;

use rayon::prelude::*;

use super::build::{LOWS, Lists, Sizes, Target, Task, Unfinished, Universe, Written};
use super::{BUCKETS_PER_PART, ContentIndex, IdPositions, Located, Lookup, PARTS, Unreadable};
use crate::postings::{self, Writer};

pub(super) fn compact(
    segments: &[Arc<ContentIndex>],
    ids: IdPositions<'_>,
    files: usize,
    target: Target<'_>,
) -> Result<ContentIndex, Unfinished> {
    let universe = Universe {
        first_id: ids.first,
        ids: u32::try_from(ids.positions.len()).unwrap_or(u32::MAX),
    };
    let parts: Vec<(Sizes, usize)> = (0..PARTS)
        .into_par_iter()
        .map(|part| Ok((Merging::of(segments, part, ids)?.sizes(universe.ids)?, part)))
        .collect::<Lookup<_>>()
        .map_err(|Unreadable| Unfinished::Failed(unreadable_segment()))?;
    let merge = |part, task: Task<'_>| {
        let merging =
            Merging::of(segments, part, ids).map_err(|Unreadable| unreadable_segment())?;
        task.write(universe.ids, merging)
    };

    Written::lists(target, parts, merge, universe).map(|written| written.into_index(files))
}

fn unreadable_segment() -> io::Error {
    io::Error::other("a segment of the content index could not be read back to merge it")
}

struct Listed {
    low: u16,
    located: Located,
}

struct Source<'a> {
    segment: &'a ContentIndex,
    lists: Vec<Listed>,
    next: usize,
}

impl Source<'_> {
    fn live_ids(
        &self,
        listed: &Listed,
        ids: IdPositions<'_>,
        mut each: impl FnMut(u32),
    ) -> Lookup<()> {
        let first = self.segment.first_id;
        let bytes = self.segment.read(listed.located)?;
        postings::for_each(listed.located.shape, &bytes, |id| {
            if ids.is_live(first + id) {
                each(first + id - ids.first);
            }
        });

        Ok(())
    }

    fn write(&mut self, low: u16, ids: IdPositions<'_>, writer: &mut Writer<'_>) -> Lookup<()> {
        while let Some(listed) = self.lists.get(self.next)
            && listed.low <= low
        {
            if listed.low == low {
                self.live_ids(listed, ids, |id| writer.push(id))?;
            }
            self.next += 1;
        }

        Ok(())
    }
}

struct Merging<'a> {
    sources: Vec<Source<'a>>,
    ids: IdPositions<'a>,
}

impl<'a> Merging<'a> {
    fn of(segments: &'a [Arc<ContentIndex>], part: usize, ids: IdPositions<'a>) -> Lookup<Self> {
        let sources = segments
            .iter()
            .map(|segment| {
                Ok(Source {
                    segment,
                    lists: segment.lists_in(part)?,
                    next: 0,
                })
            })
            .collect::<Lookup<_>>()?;

        Ok(Self { sources, ids })
    }

    fn sizes(&self, universe: u32) -> Lookup<Sizes> {
        let mut lens = vec![0u32; LOWS];
        for source in &self.sources {
            for listed in &source.lists {
                source.live_ids(listed, self.ids, |_| lens[usize::from(listed.low)] += 1)?;
            }
        }

        Ok(Sizes::counted(&lens, universe))
    }
}

impl Lists for Merging<'_> {
    // constraint: segments hold ascending id ranges in the order the index keeps them, so a trigram's ids reach the
    // writer ascending; a trigram no live file holds is never asked for, so its lists are stepped over
    fn write(&mut self, low: u16, writer: &mut Writer<'_>) -> io::Result<()> {
        for source in &mut self.sources {
            source
                .write(low, self.ids, writer)
                .map_err(|Unreadable| unreadable_segment())?;
        }

        Ok(())
    }
}

impl ContentIndex {
    fn lists_in(&self, part: usize) -> Lookup<Vec<Listed>> {
        let mut lists = Vec::new();
        for bucket in part * BUCKETS_PER_PART..(part + 1) * BUCKETS_PER_PART {
            let second = bucket.to_le_bytes()[0];
            self.bucket_lists(bucket, |third, located| {
                lists.push(Listed {
                    low: u16::from_le_bytes([third, second]),
                    located,
                });
                ControlFlow::<()>::Continue(())
            })?;
        }

        Ok(lists)
    }
}
