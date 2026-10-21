use std::cmp::Ordering;
use std::ops::Range;

use kartta_walk::EntryKind;

use super::splice::Fresh;
use super::{Index, Paths, tree_order};
#[cfg(feature = "watch")]
use crate::list::is_below;

#[derive(Debug, Default)]
pub(crate) struct ChangedFiles {
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
    #[cfg(feature = "watch")]
    pub(crate) removed_dirs: Vec<String>,
}

pub(crate) trait Side {
    fn count(&self) -> usize;
    fn at(&mut self, at: usize) -> (&str, EntryKind);
}

pub(crate) struct Held<'i> {
    index: &'i Index,
    paths: Paths<'i>,
    range: Range<usize>,
}

impl<'i> Held<'i> {
    pub(crate) const fn within(index: &'i Index, range: Range<usize>) -> Self {
        Self {
            index,
            paths: index.paths(),
            range,
        }
    }

    pub(crate) const fn all(index: &'i Index) -> Self {
        Self::within(index, 0..index.len())
    }
}

impl Side for Held<'_> {
    fn count(&self) -> usize {
        self.range.len()
    }

    fn at(&mut self, at: usize) -> (&str, EntryKind) {
        let at = self.range.start + at;
        (self.paths.get(at), self.index.kind_at(at))
    }
}

impl Side for &[Fresh] {
    fn count(&self) -> usize {
        self.len()
    }

    fn at(&mut self, at: usize) -> (&str, EntryKind) {
        (&self[at].path, self[at].kind)
    }
}

impl ChangedFiles {
    pub(crate) fn add_between(&mut self, mut before: impl Side, mut after: impl Side) {
        let (mut early, mut late) = (0, 0);
        while early < before.count() || late < after.count() {
            let order = if early == before.count() {
                Ordering::Greater
            } else if late == after.count() {
                Ordering::Less
            } else {
                tree_order(before.at(early).0, after.at(late).0)
            };
            let was = (order != Ordering::Greater).then(|| before.at(early).1);
            let now = (order != Ordering::Less).then(|| after.at(late).1);
            if was == Some(EntryKind::File) && now != was {
                self.removed.push(before.at(early).0.to_owned());
            }
            #[cfg(feature = "watch")]
            if was == Some(EntryKind::Dir) && now != was {
                self.remove_dir(before.at(early).0);
            }
            if now == Some(EntryKind::File) && was != now {
                self.added.push(after.at(late).0.to_owned());
            }
            early += usize::from(was.is_some());
            late += usize::from(now.is_some());
        }
    }

    #[cfg(feature = "watch")]
    fn remove_dir(&mut self, dir: &str) {
        if !self
            .removed_dirs
            .last()
            .is_some_and(|above| is_below(dir, above))
        {
            self.removed_dirs.push(dir.to_owned());
        }
    }
}
