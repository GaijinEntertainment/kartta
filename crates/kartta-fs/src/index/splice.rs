use std::ops::Range;

use kartta_walk::EntryKind;

use super::{Encoder, Index, NO_POSITION, Overflow, RESTART, common_len, node_sized};
use crate::content::ContentId;
use crate::inspection::Inspection;
use crate::stamp::{Stamp, UNTRUSTED};

pub(crate) struct Replacement {
    pub(crate) range: Range<usize>,
    pub(crate) entries: Vec<Fresh>,
}

pub(crate) struct Fresh {
    pub(crate) path: String,
    pub(crate) kind: EntryKind,
    pub(crate) target: Option<Box<str>>,
    pub(crate) facts: (u64, u8),
    pub(crate) content: Option<(ContentId, Stamp)>,
    pub(crate) unindexed: bool,
}

impl Fresh {
    pub(crate) const fn new(path: String, kind: EntryKind) -> Self {
        Self {
            path,
            kind,
            target: None,
            facts: (0, 0),
            content: None,
            unindexed: false,
        }
    }
}

pub(crate) struct Spliced {
    pub(crate) retired: Vec<Range<u32>>,
    pub(crate) retired_text: usize,
}

struct Shifts<'a> {
    replacements: &'a [Replacement],
    added_before: Vec<isize>,
}

impl<'a> Shifts<'a> {
    fn new(replacements: &'a [Replacement]) -> Self {
        let added_before = std::iter::once(0)
            .chain(replacements.iter().scan(0, |added, replacement| {
                *added += added_by(replacement);
                Some(*added)
            }))
            .collect();

        Self {
            replacements,
            added_before,
        }
    }

    fn moved(&self, at: usize) -> Option<usize> {
        let passed = self
            .replacements
            .partition_point(|replacement| replacement.range.end <= at);
        if self
            .replacements
            .get(passed)
            .is_some_and(|replacement| replacement.range.start <= at)
        {
            return None;
        }

        at.checked_add_signed(self.added_before[passed])
    }

    fn len_after(&self, len: usize) -> usize {
        len.saturating_add_signed(*self.added_before.last().unwrap_or(&0))
    }

    fn placed(&self) -> impl Iterator<Item = (usize, &'a Fresh)> + '_ {
        self.replacements
            .iter()
            .zip(&self.added_before)
            .flat_map(|(replacement, &added)| {
                let start = replacement.range.start.saturating_add_signed(added);
                (start..).zip(&replacement.entries)
            })
    }
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "a replacement holds fewer entries than an isize counts, as every Vec does"
)]
fn added_by(replacement: &Replacement) -> isize {
    replacement.entries.len() as isize - replacement.range.len() as isize
}

pub(crate) struct Pending {
    encoder: Encoder,
    splicing: Splicing,
    placement: Placement,
    pub(crate) unindexed: Vec<u32>,
    spliced: Spliced,
}

impl Pending {
    pub(crate) fn positions(&self) -> &[u32] {
        &self.placement.positions
    }

    pub(crate) const fn retired_text(&self) -> usize {
        self.spliced.retired_text
    }
}

impl Index {
    #[cfg(test)]
    pub(crate) fn splice(&mut self, replacements: &[Replacement], handed: u32) -> Spliced {
        let mut pending = self.spliced(replacements, handed);
        let mut unindexed = std::mem::take(&mut pending.unindexed);
        unindexed.sort_unstable();
        self.content.unindexed = unindexed.into_boxed_slice();
        self.next_content_id += handed;
        self.install(pending)
    }

    pub(crate) fn spliced(&self, replacements: &[Replacement], handed: u32) -> Pending {
        debug_assert!(
            replacements
                .windows(2)
                .all(|pair| pair[0].range.end <= pair[1].range.start
                    && pair[0].range.start < pair[1].range.start),
            "replacements are sorted and do not overlap"
        );
        let shifts = Shifts::new(replacements);
        let len = shifts.len_after(self.len());
        let (placement, spliced) = self.placed_content(&shifts, handed);
        let mut splicing = self.spliced_entries(&shifts, replacements, len);

        Pending {
            encoder: self.spliced_paths(replacements, len),
            unindexed: std::mem::take(&mut splicing.unindexed),
            splicing,
            placement,
            spliced,
        }
    }

    pub(crate) fn install(&mut self, pending: Pending) -> Spliced {
        let Pending {
            encoder,
            splicing,
            placement,
            spliced,
            ..
        } = pending;
        self.suffixes = encoder.suffixes.into_boxed_str();
        self.blocks = encoder.blocks;
        self.offsets = encoder.offsets;
        self.shared = encoder.shared;
        self.overflow = encoder.overflow;
        self.kinds = splicing.kinds;
        self.inspection = splicing.inspection;
        self.symlink_targets = splicing.symlink_targets;
        self.content_positions = placement.positions.into_boxed_slice();
        self.content_stamps = placement.stamps.map(Vec::into_boxed_slice);

        spliced
    }

    fn spliced_entries(
        &self,
        shifts: &Shifts<'_>,
        replacements: &[Replacement],
        len: usize,
    ) -> Splicing {
        let mut splicing = Splicing {
            kinds: Vec::with_capacity(len),
            inspection: Inspection::new(len),
            symlink_targets: Vec::new(),
            unindexed: self
                .content
                .unindexed
                .iter()
                .filter_map(|&at| shifts.moved(at as usize).map(node_sized))
                .collect(),
        };

        let mut kept_from = 0;
        for replacement in replacements {
            splicing.keep(self, kept_from..replacement.range.start);
            for fresh in &replacement.entries {
                splicing.fresh(fresh);
            }
            kept_from = replacement.range.end;
        }
        splicing.keep(self, kept_from..self.len());
        splicing
    }

    fn placed_content(&self, shifts: &Shifts<'_>, handed: u32) -> (Placement, Spliced) {
        let ids = (self.next_content_id + handed - self.first_content_id) as usize;
        let mut placement = Placement {
            positions: covering(&self.content_positions, ids, NO_POSITION),
            stamps: self
                .content_stamps
                .as_deref()
                .map(|stamps| covering(stamps, ids, UNTRUSTED)),
        };
        let mut removed = Vec::new();
        for (local, position) in placement.positions.iter_mut().enumerate() {
            if *position == NO_POSITION {
                continue;
            }
            let old = *position as usize;
            *position = shifts.moved(old).map_or(NO_POSITION, node_sized);
            if *position == NO_POSITION {
                removed.push((local, old));
            }
        }

        for (at, fresh) in shifts.placed() {
            if let Some((id, stamp)) = fresh.content {
                placement.put(id.slot() - self.first_content_id as usize, at, stamp);
            }
        }

        let mut retired: Vec<Range<u32>> = Vec::new();
        let mut retired_text = 0;
        for (local, old) in removed {
            if placement.positions[local] != NO_POSITION {
                continue;
            }
            retired_text += usize::from(self.held_a_list(old));
            let slot = self.first_content_id + node_sized(local);
            match retired.last_mut() {
                Some(run) if run.end == slot => run.end += 1,
                _ => retired.push(slot..slot + 1),
            }
        }
        let held = self.first_content_id + node_sized(self.content_positions.len());
        let handed_by_stopped_refreshes = held..self.next_content_id;
        if !handed_by_stopped_refreshes.is_empty() {
            retired.push(handed_by_stopped_refreshes);
        }

        (
            placement,
            Spliced {
                retired,
                retired_text,
            },
        )
    }

    // constraint: a read that fails after the first chunk leaves its file classified as text with no list in any
    // segment, and only the unindexed positions record that
    fn held_a_list(&self, at: usize) -> bool {
        Inspection::raw_is_text(self.inspection.raw(at))
            && self
                .content
                .unindexed
                .binary_search(&node_sized(at))
                .is_err()
    }

    fn spliced_paths(&self, replacements: &[Replacement], len: usize) -> Encoder {
        let mut encoder = Encoder::with_capacity(len, self.suffixes.len());
        if self.block_count() == 0 {
            self.encode_run(&mut encoder, 0..0, replacements);
            return encoder;
        }

        let mut next = 0;
        let mut block = 0;
        while let Some(replacement) = replacements.get(next) {
            let touched = self.first_block_reached(replacement);
            encoder.copy_blocks(self, block..touched);
            let (last, through) = self.dirty_run(replacements, touched, next);
            let run = self.block_first(touched)..self.block_end(last);
            self.encode_run(&mut encoder, run, &replacements[next..through]);
            next = through;
            block = last + 1;
        }
        encoder.copy_blocks(self, block..self.block_count());
        encoder
    }

    fn first_block_reached(&self, replacement: &Replacement) -> usize {
        if replacement.range.start < self.len() {
            self.block_of(replacement.range.start)
        } else {
            self.block_count() - 1
        }
    }

    fn dirty_run(
        &self,
        replacements: &[Replacement],
        block: usize,
        mut next: usize,
    ) -> (usize, usize) {
        let mut last = block;
        while let Some(replacement) = replacements.get(next) {
            if !self.reaches(replacement, self.block_end(last)) {
                break;
            }
            while replacement.range.end > self.block_end(last) {
                last += 1;
            }
            next += 1;
        }
        (last, next)
    }

    fn reaches(&self, replacement: &Replacement, run_end: usize) -> bool {
        let range = &replacement.range;
        range.start < run_end
            || (range.is_empty() && range.start == run_end && run_end == self.len())
    }

    fn encode_run(&self, encoder: &mut Encoder, run: Range<usize>, replacements: &[Replacement]) {
        let mut reader = self.paths();
        let mut previous = String::new();
        let mut written = 0;
        let mut push = |path: &str| {
            let common = path.floor_char_boundary(common_len(previous.as_bytes(), path.as_bytes()));
            encoder.push(path, common, written % RESTART == 0);
            previous.truncate(common);
            previous.push_str(&path[common..]);
            written += 1;
        };

        let mut at = run.start;
        for replacement in replacements {
            for kept in at..replacement.range.start {
                push(reader.get(kept));
            }
            for fresh in &replacement.entries {
                push(&fresh.path);
            }
            at = replacement.range.end;
        }
        for kept in at..run.end {
            push(reader.get(kept));
        }
    }
}

struct Splicing {
    kinds: Vec<EntryKind>,
    inspection: Inspection,
    symlink_targets: Vec<(usize, Box<str>)>,
    unindexed: Vec<u32>,
}

impl Splicing {
    fn keep(&mut self, index: &Index, run: Range<usize>) {
        let at = self.kinds.len();
        self.kinds.extend_from_slice(&index.kinds[run.clone()]);
        index.inspection.copy_run(run.clone(), &self.inspection, at);
        let targets = &index.symlink_targets;
        let low = targets.partition_point(|&(position, _)| position < run.start);
        let high = targets.partition_point(|&(position, _)| position < run.end);
        self.symlink_targets.extend(
            targets[low..high]
                .iter()
                .map(|(position, target)| (position - run.start + at, target.clone())),
        );
    }

    fn fresh(&mut self, fresh: &Fresh) {
        let at = self.kinds.len();
        self.kinds.push(fresh.kind);
        self.inspection.set_raw(at, fresh.facts);
        if let Some(target) = &fresh.target {
            self.symlink_targets.push((at, target.clone()));
        }
        if fresh.unindexed {
            self.unindexed.push(node_sized(at));
        }
    }
}

struct Placement {
    positions: Vec<u32>,
    stamps: Option<Vec<Stamp>>,
}

impl Placement {
    fn put(&mut self, local: usize, at: usize, stamp: Stamp) {
        self.positions[local] = node_sized(at);
        if let Some(stamps) = &mut self.stamps {
            stamps[local] = stamp;
        }
    }
}

fn covering<T: Copy>(held: &[T], ids: usize, absent: T) -> Vec<T> {
    let mut covering = Vec::with_capacity(ids);
    covering.extend_from_slice(held);
    covering.resize(ids, absent);
    covering
}

impl Encoder {
    fn copy_blocks(&mut self, index: &Index, blocks: Range<usize>) {
        if blocks.is_empty() {
            return;
        }
        let first = index.block_first(blocks.start);
        let end = index.block_end(blocks.end - 1);
        let start = index.blocks[blocks.start].start as usize;
        let bytes_end = index
            .blocks
            .get(blocks.end)
            .map_or(index.suffixes.len(), |next| next.start as usize);
        let (at, bytes_at) = (self.offsets.len(), self.suffixes.len());

        self.blocks
            .extend(index.blocks[blocks].iter().map(|block| super::Block {
                first: node_sized(block.first as usize - first + at),
                start: super::narrow(block.start as usize - start + bytes_at),
            }));
        self.suffixes.push_str(&index.suffixes[start..bytes_end]);
        self.offsets.extend_from_slice(&index.offsets[first..end]);
        self.shared.extend_from_slice(&index.shared[first..end]);
        self.overflow.copy(&index.overflow, first..end, at);
    }
}

impl Overflow {
    fn copy(&mut self, from: &Self, positions: Range<usize>, to: usize) {
        let moved = |kept: &[(usize, usize)], into: &mut Vec<(usize, usize)>| {
            let low = kept.partition_point(|&(at, _)| at < positions.start);
            let high = kept.partition_point(|&(at, _)| at < positions.end);
            into.extend(
                kept[low..high]
                    .iter()
                    .map(|&(at, value)| (at - positions.start + to, value)),
            );
        };
        moved(&from.offsets, &mut self.offsets);
        moved(&from.shared, &mut self.shared);
    }
}

#[cfg(test)]
mod tests {
    use EntryKind::{Dir, File, Symlink};

    use super::*;
    use crate::index::tests::index_in_blocks;

    const TREE: [(&str, EntryKind); 8] = [
        ("a", Dir),
        ("a/b", Dir),
        ("a/b/c.rs", File),
        ("a/b/d.rs", File),
        ("a/e.rs", File),
        ("f", Dir),
        ("f/g.rs", File),
        ("h.rs", File),
    ];

    fn replacement(range: Range<usize>, entries: &[(&str, EntryKind)]) -> Replacement {
        Replacement {
            range,
            entries: entries
                .iter()
                .map(|&(path, kind)| Fresh::new(path.to_owned(), kind))
                .collect(),
        }
    }

    fn assert_holds(index: &Index, expected: &[(&str, EntryKind)]) {
        let mut reader = index.paths();
        let in_order: Vec<(String, EntryKind)> = (0..index.len())
            .map(|at| (reader.get(at).to_owned(), index.kind_at(at)))
            .collect();
        let expected_owned: Vec<(String, EntryKind)> = expected
            .iter()
            .map(|&(path, kind)| (path.to_owned(), kind))
            .collect();
        assert_eq!(in_order, expected_owned);

        let mut reader = index.paths();
        for at in (0..expected.len()).rev() {
            assert_eq!(
                reader.get(at),
                expected[at].0,
                "position {at} read backwards"
            );
        }
        for (at, &(path, _)) in expected.iter().enumerate() {
            assert_eq!(index.position(path), Some(at), "{path}");
        }
        assert!(
            (0..index.block_count())
                .all(|block| index.block_end(block) - index.block_first(block) <= RESTART),
            "no block outgrows the build's"
        );
    }

    #[test]
    fn a_range_replaced_across_blocks_reads_back_as_the_new_tree() {
        let mut index = index_in_blocks(&TREE, &[3, 1, 4]);

        index.splice(
            &[replacement(
                3..5,
                &[
                    ("a/b/d2.rs", File),
                    ("a/b/d3.rs", File),
                    ("a/e.rs", File),
                    ("a/z.rs", File),
                ],
            )],
            0,
        );

        assert_holds(
            &index,
            &[
                ("a", Dir),
                ("a/b", Dir),
                ("a/b/c.rs", File),
                ("a/b/d2.rs", File),
                ("a/b/d3.rs", File),
                ("a/e.rs", File),
                ("a/z.rs", File),
                ("f", Dir),
                ("f/g.rs", File),
                ("h.rs", File),
            ],
        );
    }

    #[test]
    fn entries_go_in_at_a_block_boundary_the_front_and_the_end_in_one_call() {
        let mut index = index_in_blocks(&TREE, &[3, 1, 4]);

        index.splice(
            &[
                replacement(0..0, &[("0.rs", File)]),
                replacement(3..3, &[("a/b/c2.rs", File)]),
                replacement(6..7, &[]),
                replacement(8..8, &[("z.rs", File)]),
            ],
            0,
        );

        assert_holds(
            &index,
            &[
                ("0.rs", File),
                ("a", Dir),
                ("a/b", Dir),
                ("a/b/c.rs", File),
                ("a/b/c2.rs", File),
                ("a/b/d.rs", File),
                ("a/e.rs", File),
                ("f", Dir),
                ("h.rs", File),
                ("z.rs", File),
            ],
        );
    }

    #[test]
    fn an_index_spliced_empty_takes_entries_again() {
        let mut index = index_in_blocks(&TREE, &[3, 1, 4]);

        index.splice(&[replacement(0..8, &[])], 0);
        assert_holds(&index, &[]);
        index.splice(&[replacement(0..0, &[("x", Dir), ("x/y.rs", File)])], 0);

        assert_holds(&index, &[("x", Dir), ("x/y.rs", File)]);
    }

    #[test]
    fn a_run_that_grows_past_a_block_is_laid_again_in_blocks_of_128_from_its_start() {
        let names: Vec<String> = (0..300).map(|at| format!("d/{at:04}.rs")).collect();
        let mut tree = vec![("d", Dir)];
        tree.extend(names.iter().map(|name| (name.as_str(), File)));
        let mut index = index_in_blocks(&tree, &[128, 128, 45]);
        let inserted: Vec<String> = (0..250).map(|at| format!("d/0150.{at:03}.rs")).collect();
        let fresh: Vec<(&str, EntryKind)> =
            inserted.iter().map(|name| (name.as_str(), File)).collect();

        index.splice(&[replacement(100..200, &fresh)], 0);

        let mut expected = tree[..100].to_vec();
        expected.extend(fresh);
        expected.extend_from_slice(&tree[200..]);
        assert_holds(&index, &expected);
        let firsts: Vec<u32> = index.blocks.iter().map(|block| block.first).collect();
        assert_eq!(
            firsts,
            [0, 128, 256, 384, 406],
            "the two blocks the change lands in are laid again, and the untouched one follows them whole"
        );
    }

    #[test]
    fn values_too_wide_for_their_arrays_move_with_their_entries() {
        let long = "x".repeat(300);
        let paths: Vec<String> = ["a.rs", "b", "b/c.rs", "q"]
            .iter()
            .map(|&path| path.to_owned())
            .chain([
                format!("q/{long}"),
                format!("q/{long}/a.rs"),
                format!("q/{long}/b.rs"),
            ])
            .collect();
        let kinds = [File, Dir, File, Dir, Dir, File, File];
        let tree: Vec<(&str, EntryKind)> = paths.iter().map(String::as_str).zip(kinds).collect();
        let mut index = index_in_blocks(&tree, &[3, 4]);

        index.splice(&[replacement(1..1, &[("a2.rs", File)])], 0);

        let mut expected = tree.clone();
        expected.insert(1, ("a2.rs", File));
        assert_holds(&index, &expected);
    }

    #[test]
    fn ids_targets_and_unindexed_files_follow_their_entries_and_the_removed_ids_retire() {
        let mut tree = TREE;
        tree[7] = ("h.rs", Symlink);
        let mut index = index_in_blocks(&tree, &[3, 1, 4]);
        index.symlink_targets = vec![(7, "target".into())];
        index.content_positions = vec![2, 4, 7].into_boxed_slice();
        index.content_stamps = Some(vec![11, 12, 13].into_boxed_slice());
        index.next_content_id = 3;
        index.content.unindexed = vec![6].into_boxed_slice();
        index.inspection.record_binary(4, false);
        let mut fresh = Fresh::new("a/b/a.rs".to_owned(), File);
        fresh.content = Some((ContentId::from_slot(3), 19));
        fresh.unindexed = true;

        let spliced = index.splice(
            &[
                Replacement {
                    range: 2..2,
                    entries: vec![fresh],
                },
                replacement(4..5, &[]),
            ],
            1,
        );

        assert_eq!(&*index.content_positions, [3, NO_POSITION, 7, 2]);
        assert_eq!(index.content_stamps.as_deref(), Some(&[11, 12, 13, 19][..]));
        let retired: Vec<u32> = spliced.retired.into_iter().flatten().collect();
        assert_eq!(retired, [1]);
        assert_eq!(spliced.retired_text, 1, "the removed file was text");
        assert_eq!(&*index.content.unindexed, [2, 6]);
        assert_eq!(index.symlink_targets, [(7, "target".into())]);
        assert_eq!(
            index.entry("h.rs").and_then(|entry| entry.target),
            Some("target")
        );
    }

    #[test]
    fn what_was_learned_of_a_kept_entry_moves_with_it() {
        let mut index = index_in_blocks(&TREE, &[3, 1, 4]);
        index.inspection.record_size(6, 42);
        index.inspection.record_binary(7, true);

        index.splice(&[replacement(1..1, &[("a/a.rs", File)])], 0);

        assert_eq!(index.position("f/g.rs"), Some(7));
        assert_eq!(index.inspection.known_size(7), Some(42));
        assert_eq!(index.inspection.known_binary(8), Some(true));
        assert_eq!(index.inspection.known_size(6), None, "f learned nothing");
    }

    #[test]
    fn the_ids_a_stopped_refresh_handed_out_retire_with_the_next_splice() {
        let mut index = index_in_blocks(&TREE, &[3, 1, 4]);
        index.content_positions = vec![2].into_boxed_slice();
        index.next_content_id = 3;
        let mut fresh = Fresh::new("a/b/c.rs".to_owned(), File);
        fresh.content = Some((ContentId::from_slot(3), UNTRUSTED));

        let spliced = index.splice(
            &[Replacement {
                range: 2..3,
                entries: vec![fresh],
            }],
            1,
        );

        assert_eq!(spliced.retired, [0..1, 1..3]);
        assert_eq!(
            &*index.content_positions,
            [NO_POSITION, NO_POSITION, NO_POSITION, 2]
        );
    }

    #[test]
    fn a_removed_file_left_unindexed_held_no_list() {
        let mut index = index_in_blocks(&TREE, &[3, 1, 4]);
        index.content_positions = vec![2, 3].into_boxed_slice();
        index.next_content_id = 2;
        index.inspection.record_binary(2, false);
        index.inspection.record_binary(3, false);
        index.content.unindexed = vec![3].into_boxed_slice();

        let spliced = index.splice(&[replacement(2..4, &[])], 0);

        let retired: Vec<u32> = spliced.retired.into_iter().flatten().collect();
        assert_eq!(retired, [0, 1]);
        assert_eq!(spliced.retired_text, 1, "only a/b/c.rs held a list");
        assert!(index.content.unindexed.is_empty());
    }
}
