use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Index, NO_POSITION, node_sized};
use crate::content::ContentId;
use crate::stamp::{self, Seen, Stamp};

pub(crate) struct Previous<'a> {
    index: &'a Index,
    stamps: &'a [Stamp],
    slots: Slots,
    carried: Box<[AtomicU64]>,
}

enum Slots {
    ByPosition(Box<[u32]>),
    Within(Vec<(u32, u32)>),
}

impl Slots {
    fn at(&self, at: usize) -> Option<u32> {
        match self {
            Self::ByPosition(slots) => slots.get(at).copied(),
            Self::Within(slots) => {
                let at = u32::try_from(at).ok()?;
                let found = slots
                    .binary_search_by_key(&at, |&(position, _)| position)
                    .ok()?;
                Some(slots[found].1)
            }
        }
    }
}

impl<'a> Previous<'a> {
    pub(super) fn of(index: &'a Index) -> Option<Self> {
        let stamps = held_stamps(index)?;
        let mut slots_at = vec![NO_POSITION; index.len()].into_boxed_slice();
        for (slot, &at) in (index.first_content_id..).zip(&index.content_positions) {
            if let Some(found) = slots_at.get_mut(at as usize) {
                *found = slot;
            }
        }

        Some(Self::with(index, stamps, Slots::ByPosition(slots_at)))
    }

    pub(super) fn within(index: &'a Index, ranges: &[Range<usize>]) -> Option<Self> {
        let stamps = held_stamps(index)?;
        let mut slots: Vec<(u32, u32)> = (index.first_content_id..)
            .zip(&index.content_positions)
            .filter(|&(_, &at)| at != NO_POSITION && covers(ranges, at as usize))
            .map(|(slot, &at)| (at, slot))
            .collect();
        slots.sort_unstable();

        Some(Self::with(index, stamps, Slots::Within(slots)))
    }

    fn with(index: &'a Index, stamps: &'a [Stamp], slots: Slots) -> Self {
        let carried = (0..index.content_positions.len().div_ceil(64))
            .map(|_| AtomicU64::new(0))
            .collect();

        Self {
            index,
            stamps,
            slots,
            carried,
        }
    }

    pub(crate) fn carry(
        &self,
        path: &str,
        visit: bool,
        seen: impl FnOnce() -> Option<Seen>,
    ) -> Option<ContentId> {
        let at = self.index.position(path)?;
        let slot = self.slots.at(at)?;
        let local = slot.checked_sub(self.index.first_content_id)? as usize;
        let stamp = *self.stamps.get(local)?;
        if !stamp::unchanged(stamp, self.index.inspection.known_size(at), seen()?, visit) {
            return None;
        }

        self.carried[local / 64].fetch_or(1 << (local % 64), Ordering::Relaxed);
        Some(ContentId::from_slot(slot))
    }

    pub(super) fn carried_any(&self) -> bool {
        self.carried
            .iter()
            .any(|word| word.load(Ordering::Relaxed) != 0)
    }

    pub(super) const fn first_id(&self) -> u32 {
        self.index.first_content_id
    }

    pub(super) const fn index(&self) -> &'a Index {
        self.index
    }

    pub(super) fn carried(&self, id: ContentId) -> Option<((u64, u8), Stamp)> {
        let local = id
            .slot()
            .checked_sub(self.index.first_content_id as usize)?;
        let at = *self.index.content_positions.get(local)? as usize;

        Some((self.index.inspection.raw(at), *self.stamps.get(local)?))
    }

    pub(super) fn retired(&self) -> Vec<Range<u32>> {
        let mut retired: Vec<Range<u32>> = Vec::new();
        for (local, (slot, &at)) in (self.index.first_content_id..)
            .zip(&self.index.content_positions)
            .enumerate()
        {
            let carried =
                self.carried[local / 64].load(Ordering::Relaxed) & (1 << (local % 64)) != 0;
            if at == NO_POSITION || carried {
                continue;
            }
            match retired.last_mut() {
                Some(run) if run.end == slot => run.end += 1,
                _ => retired.push(slot..slot + 1),
            }
        }
        let held = self.index.first_content_id + node_sized(self.index.content_positions.len());
        let handed_by_stopped_refreshes = held..self.index.next_content_id;
        if !handed_by_stopped_refreshes.is_empty() {
            retired.push(handed_by_stopped_refreshes);
        }
        retired
    }
}

fn held_stamps(index: &Index) -> Option<&[Stamp]> {
    index
        .content_stamps
        .as_deref()
        .filter(|stamps| !stamps.is_empty())
}

fn covers(ranges: &[Range<usize>], at: usize) -> bool {
    let passed = ranges.partition_point(|range| range.end <= at);
    ranges.get(passed).is_some_and(|range| range.start <= at)
}
