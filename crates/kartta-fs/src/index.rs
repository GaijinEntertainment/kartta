//! [`Index`] — the snapshot one walk produces, and everything queries answer from.

use std::cmp::Ordering;
use std::ops::Range;
use std::path::{Path, PathBuf};

use kartta_walk::{Children, EntryKind, Tree, WalkOptions, walk};

use crate::binary::BinaryExtensions;
use crate::config::Config;
use crate::entry::Entry;
use crate::error::BuildError;
use crate::inspection::Inspection;
use crate::policy::{Indexing, Payload};
use crate::rules::Rules;

/// A snapshot of a directory tree: every entry the ignore rules kept, in the order a depth-first descent finds them.
///
/// Build one with [`Index::build`] and query it as often as you like. The structure is fixed at the moment of the
/// walk — which entries exist, what each one is, where a symlink points — and rebuilding is the only way to see a
/// later change to it.
///
/// A file's own contents are not part of that snapshot. [`Index::size`] reads the filesystem on the first ask for an
/// entry and keeps the answer, because stat'ing a whole tree during the walk costs about half a build and most of
/// those sizes are never looked at. So structure and contents belong to different moments, and only the structure is
/// coherent across entries.
///
/// # Order
///
/// [`Index::entries`] is in depth-first order, which means a directory is immediately followed by everything beneath
/// it, and entries within one directory are name-sorted. That is the canonical order: a subtree is a contiguous run of
/// the list, which a bytewise path sort would not give (`a.txt` sorts between `a` and `a/b`).
///
/// # Examples
///
/// ```
/// use kartta_fs::{Config, Index};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// std::fs::write(dir.path().join("main.rs"), "fn main() {}")?;
/// std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
/// std::fs::write(dir.path().join("ignored.txt"), "x")?;
///
/// let outcome = Index::build(dir.path(), &Config::default())?;
/// assert!(outcome.errors().is_empty(), "a complete answer");
///
/// let index = outcome.into_index();
/// assert_eq!(index.size("main.rs"), Some(12));
/// assert!(index.entry("ignored.txt").is_none(), "the .gitignore applied");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Index {
    root: PathBuf,
    paths: Box<str>,
    path_starts: Vec<usize>,
    kinds: Vec<EntryKind>,
    symlink_targets: Vec<(usize, Box<str>)>,
    /// What the walk did not read, filled by the queries that need it.
    inspection: Inspection,
    binary_extensions: BinaryExtensions,
}

impl Index {
    /// Walks `root` and builds the index.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError::Walk`] if `root` itself cannot be listed, or if the thread count
    /// [`Config::threads`] asks for cannot be started. Every deeper failure is collected into
    /// [`BuildOutcome::errors`] instead, leaving a partial index — check it before trusting the answer, because a
    /// partial index looks exactly like a complete one.
    pub fn build(root: impl AsRef<Path>, config: &Config) -> Result<BuildOutcome, BuildError> {
        let root = root.as_ref();
        let policy = Indexing::new(config.ignore, root);
        let options = WalkOptions {
            threads: config.threads,
            ..WalkOptions::default()
        };

        let (tree, walk_errors, _cancelled) =
            walk(root, &policy, &Rules::default(), &options)?.into_parts();

        let mut errors = policy.into_errors();
        errors.extend(walk_errors.into_iter().map(BuildError::Walk));
        order_report(&mut errors);

        Ok(BuildOutcome {
            index: Self::from_tree(root.to_path_buf(), &tree, config.binary_extensions.clone()),
            errors,
        })
    }

    fn from_tree(root: PathBuf, tree: &Tree<Payload>, binary_extensions: BinaryExtensions) -> Self {
        let mut bytes = 0;
        depth_first(tree, |_, path| bytes += path.len());

        let mut paths = String::with_capacity(bytes);
        let mut path_starts = Vec::with_capacity(tree.len() + 1);
        path_starts.push(0);
        let mut kinds = Vec::with_capacity(tree.len());
        let mut symlink_targets = Vec::new();

        depth_first(tree, |payload, path| {
            if let Some(target) = &payload.target {
                symlink_targets.push((kinds.len(), target.clone()));
            }
            paths.push_str(path);
            path_starts.push(paths.len());
            kinds.push(payload.kind);
        });

        Self {
            root,
            inspection: Inspection::new(kinds.len()),
            paths: paths.into_boxed_str(),
            path_starts,
            kinds,
            symlink_targets,
            binary_extensions,
        }
    }

    /// The path of the entry at position `at`.
    pub(crate) fn path_at(&self, at: usize) -> &str {
        path_in(&self.paths, &self.path_starts, at)
    }

    /// The kind and path of every entry in `positions`, in order, for a sweep that reads each one once.
    pub(crate) fn kinded_paths(
        &self,
        positions: Range<usize>,
    ) -> impl DoubleEndedIterator<Item = (EntryKind, &str)> + ExactSizeIterator {
        let starts = &self.path_starts[positions.start..=positions.end];
        let bounds = starts.iter().zip(&starts[1..]);

        self.kinds[positions]
            .iter()
            .zip(bounds)
            .map(|(&kind, (&start, &end))| (kind, path_between(&self.paths, start, end)))
    }

    /// The kind of the entry at position `at`.
    pub(crate) fn kind_at(&self, at: usize) -> EntryKind {
        self.kinds[at]
    }

    /// The entry at position `at`.
    pub(crate) fn entry_at(&self, at: usize) -> Entry<'_> {
        let kind = self.kind_at(at);
        let target = (kind == EntryKind::Symlink)
            .then(|| {
                self.symlink_targets
                    .binary_search_by_key(&at, |&(position, _)| position)
                    .ok()
            })
            .flatten()
            .map(|found| &*self.symlink_targets[found].1);

        Entry {
            path: self.path_at(at),
            kind,
            target,
        }
    }

    /// The directory the index was built from, as it was given to [`Index::build`].
    ///
    /// Join an entry's path onto this to address it on disk — see [`Entry::path`] for when that is also safe to open.
    /// This is the one path on the type that is the platform's own spelling rather than forward-slash form.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every indexed entry, in depth-first order.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = Entry<'_>> + DoubleEndedIterator {
        (0..self.len()).map(|at| self.entry_at(at))
    }

    /// The entry at this root-relative forward-slash path, or `None` if nothing was indexed there.
    ///
    /// `None` does not mean the path does not exist: an ignored file, a file below an unreadable directory and a file
    /// created after the walk all answer the same way.
    #[must_use]
    pub fn entry(&self, path: &str) -> Option<Entry<'_>> {
        Some(self.entry_at(self.position(path)?))
    }

    /// Where the entry at this root-relative forward-slash path sits in [`Index::entries`], or `None` if nothing was
    /// indexed there.
    ///
    /// The position is what a query needs to reach an entry's neighbours: [`Index::entries`] is depth-first, so a
    /// directory's subtree is the run that starts immediately after it.
    ///
    /// Depth-first order with name-sorted siblings is itself sorted, by [`tree_order`], so the lookup is a binary
    /// search over the entries as they lie.
    pub(crate) fn position(&self, path: &str) -> Option<usize> {
        let (mut low, mut high) = (0, self.len());

        while low < high {
            let middle = low + (high - low) / 2;
            match tree_order(self.path_at(middle), path) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    /// The size in bytes of the regular file at this root-relative forward-slash path.
    ///
    /// `None` for a path nothing is indexed at, for every kind but a regular file, where a size would not mean
    /// anything, and for a file that would not stat. `Some(0)` is a genuinely empty file and is
    /// never a stand-in for an unknown size.
    ///
    /// **This reads the filesystem**, on the first ask for a given entry, and keeps the answer — so a second ask
    /// costs nothing and a listing that is drawn twice is stat'd once. The walk does not collect sizes, because
    /// stat'ing every file of a large tree is roughly half of what a build costs and most of those sizes are never
    /// looked at.
    ///
    /// What that buys is paid for in coherence. A size belongs to the moment it was first asked for rather than to
    /// the walk, so two sizes from one index may describe two different moments, and neither is refreshed once taken.
    /// A file that a symlink or a directory replaced since the walk answers `None`: neither is measured, and a link is
    /// never followed.
    #[must_use]
    pub fn size(&self, path: &str) -> Option<u64> {
        let at = self.position(path)?;

        if self.kind_at(at) != EntryKind::File {
            return None;
        }

        self.inspection.size(at, || self.root.join(path))
    }

    /// Whether the regular file at this root-relative forward-slash path holds binary content.
    ///
    /// `None` for a path nothing is indexed at, for every kind but a regular file, and for a file whose name settles
    /// nothing and which then would not open or would not read.
    ///
    /// Two rules settle it, in order. An extension in [`Config::binary_extensions`] answers `Some(true)` with no file
    /// opened at all — which is most of the weight on a real tree, because those are the archives, media,
    /// disk images and model weights. That answer is about the *name*, so it stands even for a file since deleted or
    /// replaced: opening one to confirm would spend on every such entry the very syscall the rule exists to save.
    ///
    /// `svg` is the one built-in name that is not binary by construction. It is XML, and it is built in because in
    /// practice it holds machine-emitted path data whose lines are of no use to a reader — so an SVG reports binary
    /// here without ever being read.
    ///
    /// A name that settles nothing is answered by whether a NUL appears in the file's first 64 KiB, the rule git and
    /// ripgrep both apply. That verdict is about a prefix rather than about a whole file: a file that is text for
    /// 64 KiB and binary after reports text here.
    ///
    /// **A name that settles nothing reads the filesystem**, on the first ask for a given entry, and the answer is
    /// kept — see [`Index::size`] for what taking an answer at ask time rather than at walk time costs in coherence.
    /// A name the set settles is never read at all, on the first ask or any later one.
    #[must_use]
    pub fn is_binary(&self, path: &str) -> Option<bool> {
        let at = self.position(path)?;

        if self.kind_at(at) != EntryKind::File {
            return None;
        }

        self.is_binary_at(at, path)
    }

    fn is_binary_at(&self, at: usize, path: &str) -> Option<bool> {
        self.inspection.is_binary(
            at,
            || self.binary_extensions.matches(path),
            || self.root.join(path),
        )
    }

    /// How many indexed regular files carry each extension of [`Config::binary_extensions`], in the order
    /// [`BinaryExtensions::extensions`] yields them.
    ///
    /// Only names are read, never a file. A count of zero is an extension no file in this tree carries — true of most
    /// of the built-in set on any one tree, and of a misspelled extension in a caller's own set, which is the case this
    /// exists to show.
    #[must_use]
    pub fn binary_extension_counts(&self) -> Vec<(&str, usize)> {
        let mut counts = vec![0; self.binary_extensions.extensions().len()];

        let files = (0..self.len()).filter(|&at| self.kind_at(at) == EntryKind::File);
        for found in files.filter_map(|at| self.binary_extensions.position(self.path_at(at))) {
            counts[found] += 1;
        }

        self.binary_extensions.extensions().zip(counts).collect()
    }

    /// Every indexed entry paired with the facts read on demand, in depth-first order.
    ///
    /// This is the surface for a pass over the whole index — totalling sizes, tallying what is binary. Reaching the
    /// same answers through [`Index::size`] and [`Index::is_binary`] costs a path lookup per entry, because those take
    /// a path and must find the entry again; here the position is already known. On a 1.1M-file index that lookup is
    /// about 300ms of the sweep with every answer already cached.
    ///
    /// It takes no limit, and deliberately. A bounded answer has to say whether the bound turned anything away — see
    /// [`Capped`](crate::Capped) — and an iterator has nowhere to put that, because the caller ends it rather than the query. Bound
    /// a query that returns a [`Capped`](crate::Capped); sweep with this.
    ///
    /// Reading is lazy per entry, so an entry the caller skips costs nothing, and one it stops before is never
    /// touched. Each fact is read on its first ask and kept, exactly as the keyed methods do.
    pub fn inspected(&self) -> impl Iterator<Item = Inspected<'_>> {
        (0..self.len()).map(|at| self.inspected_at(at))
    }

    /// How many entries the index holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Whether the index holds no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

/// One entry of a sweep, with the late reads reachable without looking the entry up again.
///
/// Holding one borrows the index, so the facts stay readable for as long as the entry does. Nothing is read until
/// asked for.
#[derive(Debug, Clone, Copy)]
pub struct Inspected<'a> {
    index: &'a Index,
    at: usize,
}

impl Index {
    /// One entry of this index by position, for a query that already knows where the entry sits.
    pub(crate) const fn inspected_at(&self, at: usize) -> Inspected<'_> {
        Inspected { index: self, at }
    }
}

impl<'a> Inspected<'a> {
    /// The entry itself.
    #[must_use]
    pub fn entry(&self) -> Entry<'a> {
        self.index.entry_at(self.at)
    }

    /// The entry's size, on the terms [`Index::size`] states.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        if self.index.kind_at(self.at) != EntryKind::File {
            return None;
        }

        self.index.inspection.size(self.at, || {
            self.index.root.join(self.index.path_at(self.at))
        })
    }

    /// Whether the entry holds binary content, on the terms [`Index::is_binary`] states.
    #[must_use]
    pub fn is_binary(&self) -> Option<bool> {
        if self.index.kind_at(self.at) != EntryKind::File {
            return None;
        }

        self.index
            .is_binary_at(self.at, self.index.path_at(self.at))
    }
}

/// The path at position `at` of `paths`, the index's paths laid end to end, which begins at `starts[at]` and stops at
/// `starts[at + 1]`.
fn path_in<'a>(paths: &'a str, starts: &[usize], at: usize) -> &'a str {
    path_between(paths, starts[at], starts[at + 1])
}

/// The path of `paths` between `start` and `end`, which must be two consecutive offsets of the index's path starts.
///
/// Slicing a `str` by bytes checks that both ends sit on a character boundary, which reads the path's bytes. A sweep
/// that only needs where each path lies would read every byte of the buffer for nothing, and a listing pays a third
/// more for it.
fn path_between(paths: &str, start: usize, end: usize) -> &str {
    // SAFETY: every path start is `paths.len()` read right after a whole `&str` was appended to `paths`, so each one
    // is a character boundary of `paths` and they rise in order; `start` and `end` are two consecutive ones, read
    // through a bounds-checked index or a zip of the starts with themselves, so `start..end` lies inside `paths`.
    unsafe { paths.get_unchecked(start..end) }
}

/// The order [`Index::entries`] lies in: paths compared component by component, so a directory's subtree follows it
/// immediately — which is bytewise order with `/` ranked below every other byte.
///
/// The first differing byte decides, and it is found eight bytes at a time, because the paths being compared share long
/// prefixes and comparing those a byte at a time made a lookup four times slower.
fn tree_order(left: &str, right: &str) -> Ordering {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    let mut at = 0;

    while let (Some(left_word), Some(right_word)) = (
        left[at..].first_chunk::<8>(),
        right[at..].first_chunk::<8>(),
    ) {
        let differ = u64::from_le_bytes(*left_word) ^ u64::from_le_bytes(*right_word);
        if differ != 0 {
            at += (differ.trailing_zeros() / 8) as usize;
            break;
        }
        at += 8;
    }

    left[at..]
        .iter()
        .zip(&right[at..])
        .find(|(l, r)| l != r)
        .map_or_else(
            || left.len().cmp(&right.len()),
            |(&l, &r)| match (l, r) {
                (b'/', _) => Ordering::Less,
                (_, b'/') => Ordering::Greater,
                _ => l.cmp(&r),
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order spelled out a byte at a time, which the word-wise comparison has to agree with.
    fn tree_order_by_byte(left: &str, right: &str) -> Ordering {
        let rank = |byte: u8| if byte == b'/' { 0 } else { u16::from(byte) + 1 };
        left.bytes().map(rank).cmp(right.bytes().map(rank))
    }

    #[test]
    fn tree_order_agrees_with_the_byte_by_byte_order_on_either_side_of_a_word() {
        let paths = [
            "",
            "a",
            "a/b",
            "a b",
            "a.txt",
            "abcdefg",
            "abcdefgh",
            "abcdefgh/",
            "abcdefgh.",
            "abcdefgh/i",
            "abcdefg/hi",
            "abcdefg.hi",
            "abcdefghijklmnop/q",
            "abcdefghijklmnop.q",
            "abcdefghijklmnopq",
            "a\0b",
            "abcdefghijklmnop\0q",
        ];

        for left in paths {
            for right in paths {
                assert_eq!(
                    tree_order(left, right),
                    tree_order_by_byte(left, right),
                    "{left:?} against {right:?}"
                );
            }
        }
    }
}

/// Puts the report in an order two builds of one tree agree on.
///
/// The walk collects failures as its threads reach them, so the order they arrive in is the scheduler's. An index that
/// is identical across builds is worth little beside a report that is not.
fn order_report(errors: &mut [BuildError]) {
    errors.sort_by(|left, right| {
        left.path()
            .cmp(&right.path())
            .then_with(|| left.kind_order().cmp(&right.kind_order()))
    });
}

/// One directory being expanded during the depth-first pass: the children left to visit, and how much of the shared
/// path buffer this directory's own path occupies.
struct Frame<'a> {
    children: Children<'a, Payload>,
    prefix: usize,
}

/// Visits the walk's tree in depth-first order, handing `visit` each node's payload and full path.
///
/// One buffer carries the path of the entry being visited, truncated back to the parent's length on the way down a
/// sibling: every node under one directory shares that directory's prefix, so the bytes below the truncation point are
/// always already correct. The stack holds one frame per level, not one per pending node.
fn depth_first(tree: &Tree<Payload>, mut visit: impl FnMut(&Payload, &str)) {
    let mut path = String::new();
    let mut stack = vec![Frame {
        children: tree.roots(),
        prefix: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        let Some(node) = frame.children.next() else {
            stack.pop();
            continue;
        };
        let prefix = frame.prefix;

        path.truncate(prefix);
        if prefix != 0 {
            path.push('/');
        }
        path.push_str(node.name());

        visit(node.value(), &path);

        stack.push(Frame {
            children: node.children(),
            prefix: path.len(),
        });
    }
}

/// What one build produced: the index, and what the build could not read.
///
/// The index is the part worth keeping; the errors are a report about this particular build, which a consumer surfaces
/// or logs and then drops. [`BuildOutcome::into_index`] takes the snapshot and leaves the report behind.
#[derive(Debug)]
pub struct BuildOutcome {
    index: Index,
    errors: Vec<BuildError>,
}

impl BuildOutcome {
    /// The index the build produced.
    #[must_use]
    pub const fn index(&self) -> &Index {
        &self.index
    }

    /// Takes the index, dropping the report.
    ///
    /// Check [`BuildOutcome::errors`] first: an index built from an incomplete walk looks exactly like one built from
    /// a complete walk.
    #[must_use]
    pub fn into_index(self) -> Index {
        self.index
    }

    /// What the build could not read, ordered by path so two builds of one tree report identically.
    ///
    /// A non-empty list means the index is wrong in a way it cannot detect, and each kind is wrong differently:
    ///
    /// - [`BuildError::Walk`] — the entry or the whole subtree it names is missing.
    /// - [`BuildError::Ignore`] — the rules went unread, so the index *holds* whatever that file would have hidden.
    ///   The only kind that makes an index wrong by addition, and the only one no later check can catch.
    /// - [`BuildError::Name`] — the entry is missing, and a directory takes its subtree with it.
    #[must_use]
    pub fn errors(&self) -> &[BuildError] {
        &self.errors
    }
}
