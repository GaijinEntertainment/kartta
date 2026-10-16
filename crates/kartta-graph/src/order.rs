use crate::graph::narrow;

const BLOCK_LEN: usize = 256;
const LABEL_GAP: u64 = 1 << 16;
const LABELS: u64 = 1 << 32;

// constraint: a file's rank is its block's label above its place in the block, so ranks order the files by path and an
// insert or a removal renumbers only the files of one block, at most twice `BLOCK_LEN`; a block's label falls between
// its neighbours', each split halving the room, so only sixteen splits between two labels, at least sixteen times
// `BLOCK_LEN` inserts at one place, relabel every block
#[derive(Debug, Default)]
pub(crate) struct PathOrder {
    blocks: Vec<Block>,
    ranks: Vec<u64>,
}

#[derive(Debug)]
struct Block {
    label: u32,
    slots: Vec<u32>,
}

impl PathOrder {
    pub(crate) fn rank(&self, slot: u32) -> u64 {
        self.ranks[slot as usize]
    }

    // constraint: `sorts_before(held)` answers whether a held file's path sorts before the inserted file's
    pub(crate) fn insert(&mut self, slot: u32, sorts_before: impl Fn(u32) -> bool) {
        if self.ranks.len() <= slot as usize {
            self.ranks.resize(slot as usize + 1, 0);
        }
        let last = self.blocks.last().and_then(|block| block.slots.last());
        if last.is_none_or(|&last| sorts_before(last)) {
            self.append(slot);
            return;
        }
        let block = self
            .blocks
            .partition_point(|block| block.slots.last().is_some_and(|&last| sorts_before(last)));
        let position = self.blocks[block]
            .slots
            .partition_point(|&held| sorts_before(held));
        self.blocks[block].slots.insert(position, slot);
        if self.blocks[block].slots.len() > 2 * BLOCK_LEN {
            self.split(block);
        }
        self.renumber(block, position);
    }

    pub(crate) fn remove(&mut self, slot: u32) {
        let rank = self.ranks[slot as usize];
        let label = rank >> 32;
        let block = self
            .blocks
            .partition_point(|block| u64::from(block.label) < label);
        let position = narrow_label(rank & (LABELS - 1)) as usize;
        self.blocks[block].slots.remove(position);
        if self.blocks[block].slots.is_empty() {
            self.blocks.remove(block);
        } else {
            self.renumber(block, position);
        }
    }

    fn append(&mut self, slot: u32) {
        if self
            .blocks
            .last()
            .is_none_or(|block| block.slots.len() >= BLOCK_LEN)
        {
            let label = self.blocks.len().checked_sub(1).map_or_else(
                || narrow_label(LABEL_GAP),
                |last| self.free_label_after(last),
            );
            self.blocks.push(Block {
                label,
                slots: Vec::with_capacity(BLOCK_LEN),
            });
        }
        let last = self.blocks.len() - 1;
        self.blocks[last].slots.push(slot);
        self.renumber(last, self.blocks[last].slots.len() - 1);
    }

    fn split(&mut self, block: usize) {
        let label = self.free_label_after(block);
        let tail = self.blocks[block].slots.split_off(BLOCK_LEN);
        self.blocks.insert(block + 1, Block { label, slots: tail });
        self.renumber(block + 1, 0);
    }

    fn free_label_after(&mut self, block: usize) -> u32 {
        self.label_after(block).unwrap_or_else(|| {
            self.relabel();
            self.label_after(block).unwrap_or(u32::MAX)
        })
    }

    fn label_after(&self, block: usize) -> Option<u32> {
        let low = u64::from(self.blocks[block].label);
        let high = self
            .blocks
            .get(block + 1)
            .map_or(LABELS, |next| u64::from(next.label));
        let step = ((high - low) / 2).min(LABEL_GAP);
        (step > 0).then(|| narrow_label(low + step))
    }

    // constraint: labels spread evenly with room after the last, which holds while fewer than 2^31 blocks exist
    fn relabel(&mut self) {
        let gap = (LABELS / (self.blocks.len() as u64 + 2)).min(LABEL_GAP);
        for (at, block) in self.blocks.iter_mut().enumerate() {
            block.label = narrow_label(gap * (at as u64 + 1));
        }
        for block in 0..self.blocks.len() {
            self.renumber(block, 0);
        }
    }

    fn renumber(&mut self, block: usize, from: usize) {
        let block = &self.blocks[block];
        let label = u64::from(block.label) << 32;
        for (position, &slot) in block.slots.iter().enumerate().skip(from) {
            self.ranks[slot as usize] = label | u64::from(narrow(position));
        }
    }
}

fn narrow_label(label: u64) -> u32 {
    u32::try_from(label).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order_of(paths: &[u32]) -> PathOrder {
        let mut order = PathOrder::default();
        for &path in paths {
            order.insert(path, |held| held < path);
        }
        order
    }

    fn ranked(order: &PathOrder, slots: impl Iterator<Item = u32>) -> Vec<u32> {
        let mut slots: Vec<u32> = slots.collect();
        slots.sort_unstable_by_key(|&slot| order.rank(slot));
        slots
    }

    fn shuffled(count: u32) -> Vec<u32> {
        let mut slots: Vec<u32> = (0..count).collect();
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for at in (1..slots.len()).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            slots.swap(
                at,
                usize::try_from(state % (at as u64 + 1)).expect("below `at`"),
            );
        }
        slots
    }

    #[test]
    fn ranks_follow_the_paths_whatever_order_the_files_arrive_in() {
        let arrivals = shuffled(5_000);
        let order = order_of(&arrivals);

        assert_eq!(ranked(&order, 0..5_000), (0..5_000).collect::<Vec<_>>());
    }

    #[test]
    fn files_that_arrive_before_every_other_split_and_relabel_the_blocks() {
        let arrivals: Vec<u32> = (0..40_000).rev().collect();
        let order = order_of(&arrivals);

        assert_eq!(ranked(&order, 0..40_000), (0..40_000).collect::<Vec<_>>());
    }

    #[test]
    fn a_removal_keeps_the_order_of_the_rest() {
        let mut order = order_of(&shuffled(3_000));
        for slot in (0..3_000).filter(|slot| slot % 3 == 0) {
            order.remove(slot);
        }
        order.insert(0, |_| false);

        let kept: Vec<u32> = (0..3_000)
            .filter(|slot| slot % 3 != 0 || *slot == 0)
            .collect();
        assert_eq!(ranked(&order, kept.iter().copied()), kept);
    }
}
