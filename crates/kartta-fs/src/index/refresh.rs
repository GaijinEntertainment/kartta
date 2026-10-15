use std::fs::Metadata;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Index, NO_POSITION, node_sized};
use crate::content::ContentId;
use crate::stamp::{self, Stamp};

pub(crate) struct Previous<'a> {
    index: &'a Index,
    stamps: &'a [Stamp],
    slots_at: Box<[u32]>,
    carried: Box<[AtomicU64]>,
}

impl<'a> Previous<'a> {
    pub(super) fn of(index: &'a Index) -> Option<Self> {
        let stamps = index
            .content_stamps
            .as_deref()
            .filter(|stamps| !stamps.is_empty())?;
        let mut slots_at = vec![NO_POSITION; index.len()].into_boxed_slice();
        for (slot, &at) in (index.first_content_id..).zip(&index.content_positions) {
            if let Some(found) = slots_at.get_mut(at as usize) {
                *found = slot;
            }
        }
        let carried = (0..index.content_positions.len().div_ceil(64))
            .map(|_| AtomicU64::new(0))
            .collect();

        Some(Self {
            index,
            stamps,
            slots_at,
            carried,
        })
    }

    pub(crate) fn carry(
        &self,
        path: &str,
        visit: bool,
        metadata: impl FnOnce() -> Option<Metadata>,
    ) -> Option<ContentId> {
        let at = self.index.position(path)?;
        let slot = *self.slots_at.get(at)?;
        let local = slot.checked_sub(self.index.first_content_id)? as usize;
        let stamp = *self.stamps.get(local)?;
        let metadata = metadata()?;
        if !stamp::unchanged(
            stamp,
            self.index.inspection.known_size(at),
            &metadata,
            visit,
        ) {
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
