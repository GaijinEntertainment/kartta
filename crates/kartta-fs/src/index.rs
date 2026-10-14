//! [`Index`] — the snapshot one walk produces, and everything queries answer from.

use std::cmp::Ordering;
use std::ops::Range;
use std::path::{Path, PathBuf};

use kartta_walk::{Children, EntryKind, Tree, WalkOptions, walk};

use crate::binary::BinaryExtensions;
use crate::build::{BuildError, BuildOptions};
use crate::entry::Entry;
use crate::filter::Filter;
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
/// use kartta_fs::{BuildOptions, Index};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// std::fs::write(dir.path().join("main.rs"), "fn main() {}")?;
/// std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
/// std::fs::write(dir.path().join("ignored.txt"), "x")?;
///
/// let outcome = Index::build(dir.path(), &BuildOptions::default())?;
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
    /// Each entry's path past the bytes it shares with the previous entry's path, laid end to end; a restart's whole
    /// path.
    suffixes: Box<str>,
    /// Where each block of [`RESTART`] entries starts in `suffixes`.
    block_starts: Vec<u32>,
    /// Where each entry's stored bytes start, counted from its block's start. `u16::MAX` stands for an offset kept in
    /// `overflow`.
    offsets: Vec<u16>,
    /// How many leading bytes each entry's path shares with the previous entry's path, floored to a character boundary
    /// so the stored bytes always start one. Kept for a restart too, which stores its whole path regardless. `u8::MAX`
    /// stands for a length kept in `overflow`.
    shared: Vec<u8>,
    /// The offsets and shared lengths too large for their arrays, which almost no real path needs.
    overflow: Overflow,
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
    /// [`BuildOptions::threads`] asks for cannot be started. Every deeper failure is collected into
    /// [`BuildOutcome::errors`] instead, leaving a partial index — check it before trusting the answer, because a
    /// partial index looks exactly like a complete one.
    pub fn build(
        root: impl AsRef<Path>,
        options: &BuildOptions,
    ) -> Result<BuildOutcome, BuildError> {
        let root = root.as_ref();
        let policy = Indexing::new(options.ignore_sources, root);
        let walk_options = WalkOptions {
            threads: options.threads,
            ..WalkOptions::default()
        };

        let (tree, walk_errors, _cancelled) =
            walk(root, &policy, &Rules::default(), &walk_options)?.into_parts();

        let mut errors = policy.into_errors();
        errors.extend(walk_errors.into_iter().map(BuildError::Walk));
        order_errors(&mut errors);

        Ok(BuildOutcome {
            index: Self::from_tree(root.to_path_buf(), tree, options.binary_extensions.clone()),
            errors,
        })
    }

    fn from_tree(root: PathBuf, tree: Tree<Payload>, binary_extensions: BinaryExtensions) -> Self {
        let mut suffixes_len = 0;
        let mut at = 0;
        depth_first(&tree, |_, path, common| {
            suffixes_len += path.len() - stored_from(at, path.floor_char_boundary(common));
            at += 1;
        });

        let mut suffixes = String::with_capacity(suffixes_len);
        let mut block_starts = Vec::with_capacity(tree.len().div_ceil(RESTART));
        let mut offsets = Vec::with_capacity(tree.len());
        let mut shared = Vec::with_capacity(tree.len());
        let mut overflow = Overflow::default();
        let mut kinds = Vec::with_capacity(tree.len());
        let mut symlink_targets = Vec::new();

        depth_first(&tree, |payload, path, common| {
            let at = kinds.len();
            if let Some(target) = &payload.target {
                symlink_targets.push((at, target.clone()));
            }
            if at.is_multiple_of(RESTART) {
                block_starts.push(narrow(suffixes.len()));
            }
            let common = path.floor_char_boundary(common);
            let offset = suffixes.len() - block_starts[at / RESTART] as usize;
            offsets.push(overflow.keep_offset(at, offset));
            shared.push(overflow.keep_shared(at, common));
            suffixes.push_str(&path[stored_from(at, common)..]);
            kinds.push(payload.kind);
        });
        drop(tree);

        Self {
            root,
            inspection: Inspection::new(kinds.len()),
            suffixes: suffixes.into_boxed_str(),
            block_starts,
            offsets,
            shared,
            overflow,
            kinds,
            symlink_targets,
            binary_extensions,
        }
    }

    /// Where the bytes stored for the entry at `at` start in `suffixes`; past the last entry, the end of them.
    fn start_of(&self, at: usize) -> usize {
        if at == self.len() {
            return self.suffixes.len();
        }
        let offset = match self.offsets[at] {
            u16::MAX => self.overflow.offset(at),
            offset => usize::from(offset),
        };
        self.block_starts[at / RESTART] as usize + offset
    }

    /// The bytes stored for the entry at `at`: its whole path at a restart, the part past its shared prefix elsewhere.
    pub(crate) fn suffix(&self, at: usize) -> &str {
        path_between(&self.suffixes, self.start_of(at), self.start_of(at + 1))
    }

    /// How many leading bytes the path at `at` shares with the path before it.
    pub(crate) fn shared_at(&self, at: usize) -> usize {
        match self.shared[at] {
            u8::MAX => self.overflow.shared(at),
            shared => usize::from(shared),
        }
    }

    /// Turns `path`, holding the path at `at - 1`, into the path at `at`.
    fn step_into(&self, at: usize, path: &mut String) {
        if at.is_multiple_of(RESTART) {
            path.clear();
        } else {
            path.truncate(self.shared_at(at));
        }
        path.push_str(self.suffix(at));
    }

    /// Rebuilds the path at `at` into `path` from the bytes stored at and before it.
    fn decode_into(&self, at: usize, path: &mut String) {
        let shared = if at.is_multiple_of(RESTART) {
            0
        } else {
            self.shared_at(at)
        };
        path.clear();
        self.push_prefix(at, shared + self.suffix(at).len(), path);
    }

    /// Appends the first `len` bytes of the path at `at` to `path`.
    ///
    /// While an entry shares at least `len` bytes with the path before it, those bytes are the earlier path's, so the
    /// walk goes back to the last entry that stores some of them: its shared prefix comes from before it, and its own
    /// bytes follow. Every byte is copied once, and an entry with nothing to give costs one load, so a decode costs the
    /// path's length plus the entries back to the restart rather than a copy per entry.
    fn push_prefix(&self, mut at: usize, len: usize, path: &mut String) {
        if len == 0 {
            return;
        }
        loop {
            if at.is_multiple_of(RESTART) {
                path.push_str(&self.suffix(at)[..len]);
                return;
            }
            let shared = self.shared_at(at);
            if shared < len {
                self.push_prefix(at - 1, shared, path);
                path.push_str(&self.suffix(at)[..len - shared]);
                return;
            }
            at -= 1;
        }
    }

    /// A reader for the paths of this index's entries. See [`Paths`].
    #[must_use]
    pub const fn paths(&self) -> Paths<'_> {
        Paths {
            index: self,
            at: None,
            path: String::new(),
        }
    }

    /// Visits every entry in `positions` that is not a directory and that `filter` selects, with its path, until
    /// `visit` answers `false`.
    pub(crate) fn candidates(
        &self,
        positions: Range<usize>,
        filter: Option<&Filter>,
        mut visit: impl FnMut(usize, &str) -> bool,
    ) {
        let mut paths = self.paths();
        for at in positions {
            if self.kind_at(at) == EntryKind::Dir {
                continue;
            }
            let path = paths.get(at);
            if filter.is_none_or(|filter| filter.matches(path)) && !visit(at, path) {
                return;
            }
        }
    }

    /// The kind of the entry at position `at`.
    pub(crate) fn kind_at(&self, at: usize) -> EntryKind {
        self.kinds[at]
    }

    /// The entry at position `at`, with `path` as its path.
    fn entry_with(&self, at: usize, path: Box<str>) -> Entry<'_> {
        let kind = self.kind_at(at);
        let target = (kind == EntryKind::Symlink)
            .then(|| {
                self.symlink_targets
                    .binary_search_by_key(&at, |&(position, _)| position)
                    .ok()
            })
            .flatten()
            .map(|found| &*self.symlink_targets[found].1);

        Entry { path, kind, target }
    }

    /// The entry at position `at`, its path decoded.
    pub(crate) fn entry_at(&self, at: usize) -> Entry<'_> {
        let mut path = String::new();
        self.decode_into(at, &mut path);
        self.entry_with(at, path.into_boxed_str())
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
    ///
    /// Each entry owns its path, so this allocates one per entry; a pass that only reads paths reads them through
    /// [`Index::paths`] instead.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = Entry<'_>> + DoubleEndedIterator {
        let mut paths = self.paths();
        (0..self.len()).map(move |at| self.entry_with(at, paths.get(at).into()))
    }

    /// The entry at this root-relative forward-slash path, or `None` if nothing was indexed there.
    ///
    /// `None` does not mean the path does not exist: an ignored file, a file below an unreadable directory and a file
    /// created after the walk all answer the same way.
    #[must_use]
    pub fn entry(&self, path: &str) -> Option<Entry<'_>> {
        Some(self.entry_with(self.position(path)?, path.into()))
    }

    /// Where the entry at this root-relative forward-slash path sits in [`Index::entries`], or `None` if nothing was
    /// indexed there.
    ///
    /// The position is what a query needs to reach an entry's neighbours: [`Index::entries`] is depth-first, so a
    /// directory's subtree is the run that starts immediately after it.
    ///
    /// Depth-first order with name-sorted siblings is itself sorted, by [`tree_order`], so the lookup is a binary
    /// search over the restarts, whose whole paths are stored, and then a walk forward through one block that compares
    /// stored bytes against `path` without rebuilding any entry's path.
    pub(crate) fn position(&self, path: &str) -> Option<usize> {
        let (mut low, mut high) = (0, self.len().div_ceil(RESTART));

        while low < high {
            let middle = low + (high - low) / 2;
            match tree_order(self.suffix(middle * RESTART), path) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Some(middle * RESTART),
            }
        }

        // Every entry the walk passes sorts below `path`, and `common` is how many leading bytes the last one shares
        // with it. An entry sharing more than that with its predecessor differs from `path` at the same byte and in
        // the same direction, so it is passed without a look at its bytes.
        let restart = low.checked_sub(1)? * RESTART;
        let mut common = common_len(self.suffix(restart).as_bytes(), path.as_bytes());
        for at in restart + 1..self.len().min(restart + RESTART) {
            let shared = self.shared_at(at);
            if shared > common {
                continue;
            }

            let stored = self.suffix(at);
            let rest = path.get(shared..)?;
            match tree_order(stored, rest) {
                Ordering::Less => common = shared + common_len(stored.as_bytes(), rest.as_bytes()),
                Ordering::Equal => return Some(at),
                Ordering::Greater => return None,
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
    /// Two rules settle it, in order. An extension in [`BuildOptions::binary_extensions`] answers `Some(true)` with no
    /// file opened at all — which is most of the weight on a real tree, because those are the archives, media, disk
    /// images and model weights. That answer is about the *name*, so it stands even for a file since deleted or
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

        self.inspection.is_binary(
            at,
            || self.binary_extensions.matches(path),
            || self.root.join(path),
        )
    }

    /// [`Index::is_binary`] for an entry known by position, decoding its path only if the slot is still unread.
    fn is_binary_at(&self, at: usize) -> Option<bool> {
        let path = || self.entry_at(at).path;
        self.inspection.is_binary(
            at,
            || self.binary_extensions.matches(&path()),
            || self.root.join(&*path()),
        )
    }

    /// How many indexed regular files carry each extension of [`BuildOptions::binary_extensions`], in the order
    /// [`BinaryExtensions::extensions`] yields them.
    ///
    /// Only names are read, never a file. A count of zero is an extension no file in this tree carries — true of most
    /// of the built-in set on any one tree, and of a misspelled extension in a caller's own set, which is the case this
    /// exists to show.
    #[must_use]
    pub fn binary_extension_counts(&self) -> Vec<(&str, usize)> {
        let mut counts = vec![0; self.binary_extensions.extensions().len()];

        self.candidates(0..self.len(), None, |at, path| {
            if self.kind_at(at) == EntryKind::File
                && let Some(found) = self.binary_extensions.position(path)
            {
                counts[found] += 1;
            }
            true
        });

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
    /// [`Capped`](crate::Capped) and [`Listing`](crate::Listing) — and an iterator has nowhere to put that, because the
    /// caller ends it rather than the query. Bound a query that returns one of those; sweep with this.
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
    /// The entry itself, its path decoded.
    #[must_use]
    pub fn entry(&self) -> Entry<'a> {
        self.index.entry_at(self.at)
    }

    /// Where the entry sits in [`Index::entries`], for reading its path through [`Index::paths`].
    #[must_use]
    pub const fn position(&self) -> usize {
        self.at
    }

    /// The entry's size, on the terms [`Index::size`] states.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        if self.index.kind_at(self.at) != EntryKind::File {
            return None;
        }

        self.index.inspection.size(self.at, || {
            self.index.root.join(self.index.paths().get(self.at))
        })
    }

    /// Whether the entry holds binary content, on the terms [`Index::is_binary`] states.
    #[must_use]
    pub fn is_binary(&self) -> Option<bool> {
        if self.index.kind_at(self.at) != EntryKind::File {
            return None;
        }

        self.index.is_binary_at(self.at)
    }
}

/// Reads the paths of an [`Index`]'s entries by position, rebuilding each into one buffer the reader reuses.
///
/// Positions are the ones [`Index::entries`] runs in, and the ones a [`Listed`](crate::Listed), a
/// [`FuzzyMatch`](crate::FuzzyMatch) and a [`Index::glob`] answer carry. Reading positions in rising order costs a few
/// bytes per entry; any other order rebuilds the path from the nearest entry before it that stores its whole path.
#[derive(Debug, Clone)]
pub struct Paths<'a> {
    index: &'a Index,
    /// The position whose path `path` holds, `None` before the first read.
    at: Option<usize>,
    path: String,
}

impl Paths<'_> {
    /// The path of the entry at `position`, valid until the next read.
    ///
    /// # Panics
    ///
    /// Panics if `position` is not below [`Index::len`].
    pub fn get(&mut self, position: usize) -> &str {
        match self.at.and_then(|at| position.checked_sub(at)) {
            Some(0) => {}
            Some(gap) if gap <= STEP_LIMIT => {
                (position - gap + 1..=position)
                    .for_each(|at| self.index.step_into(at, &mut self.path));
            }
            _ => self.index.decode_into(position, &mut self.path),
        }
        self.at = Some(position);
        &self.path
    }
}

/// How many entries every restart covers: every `RESTART`-th entry stores its whole path, so rebuilding any path reads
/// the stored bytes of at most this many entries.
pub(crate) const RESTART: usize = 128;

/// How far ahead [`Paths::get`] steps entry by entry rather than rebuilding the requested path outright: stepping
/// copies each entry's stored bytes, rebuilding copies the requested path once.
const STEP_LIMIT: usize = 4;

/// Where an entry's stored bytes start in its path: nothing is skipped at a restart, the shared prefix elsewhere.
const fn stored_from(at: usize, shared: usize) -> usize {
    if at.is_multiple_of(RESTART) {
        0
    } else {
        shared
    }
}

/// The per-entry values too large for the narrow arrays that hold the rest, by position.
///
/// A narrow array holds its largest value in place of a value kept here, so only an entry that needs one pays a search.
/// The build keeps them in rising position order.
#[derive(Debug, Clone, Default)]
struct Overflow {
    offsets: Vec<(usize, usize)>,
    shared: Vec<(usize, usize)>,
}

impl Overflow {
    /// `offset` as [`Index::offsets`] holds it, kept here when it does not fit below `u16::MAX`.
    fn keep_offset(&mut self, at: usize, offset: usize) -> u16 {
        match u16::try_from(offset) {
            Ok(narrow) if narrow != u16::MAX => narrow,
            _ => {
                self.offsets.push((at, offset));
                u16::MAX
            }
        }
    }

    /// `shared` as [`Index::shared`] holds it, kept here when it does not fit below `u8::MAX`.
    fn keep_shared(&mut self, at: usize, shared: usize) -> u8 {
        match u8::try_from(shared) {
            Ok(narrow) if narrow != u8::MAX => narrow,
            _ => {
                self.shared.push((at, shared));
                u8::MAX
            }
        }
    }

    /// The offset kept for the entry at `at`.
    fn offset(&self, at: usize) -> usize {
        kept(&self.offsets, at)
    }

    /// The shared length kept for the entry at `at`.
    fn shared(&self, at: usize) -> usize {
        kept(&self.shared, at)
    }
}

/// The value `values` keeps for the entry at `at`, which must have one.
fn kept(values: &[(usize, usize)], at: usize) -> usize {
    values[values.partition_point(|&(position, _)| position < at)].1
}

/// How many leading bytes `left` and `right` share.
fn common_len(left: &[u8], right: &[u8]) -> usize {
    left.iter().zip(right).take_while(|(l, r)| l == r).count()
}

/// Narrows an offset into the stored bytes to the width the index keeps.
#[expect(
    clippy::expect_used,
    reason = "this needs over 4 GiB of stored path bytes, far past what the u32 offsets were chosen for"
)]
fn narrow(len: usize) -> u32 {
    u32::try_from(len).expect("an index stores fewer than 4 GiB of path bytes")
}

/// The stored bytes of `suffixes` between `start` and `end`, which must be two consecutive entry starts.
///
/// Slicing a `str` by bytes checks that both ends sit on a character boundary, which reads the bytes. A sweep that only
/// needs where each entry's bytes lie would read every byte of the buffer for nothing.
fn path_between(suffixes: &str, start: usize, end: usize) -> &str {
    // SAFETY: every start `Index::start_of` rebuilds is `suffixes.len()` as the build read it before the first append or
    // right after appending a whole `&str`, or past the last entry `suffixes.len()` itself, so each one is a character
    // boundary of `suffixes` and they rise in order; `start` and `end` are two consecutive ones, so `start..end` lies
    // inside `suffixes`.
    unsafe { suffixes.get_unchecked(start..end) }
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

/// Puts the report in an order two builds of one tree agree on.
///
/// The walk collects failures as its threads reach them, so the order they arrive in is the scheduler's. An index that
/// is identical across builds is worth little beside a report that is not.
fn order_errors(errors: &mut [BuildError]) {
    errors.sort_by(|left, right| {
        left.native_path()
            .cmp(&right.native_path())
            .then_with(|| left.variant_rank().cmp(&right.variant_rank()))
    });
}

/// One directory being expanded during the depth-first pass: the children left to visit, and how much of the shared
/// path buffer this directory's own path occupies.
struct Frame<'a> {
    children: Children<'a, Payload>,
    prefix_len: usize,
}

/// Visits the walk's tree in depth-first order, handing `visit` each node's payload, full path, and how many leading
/// bytes that path shares with the one visited before it.
///
/// One buffer carries the path of the entry being visited, truncated back to the parent's length on the way down a
/// sibling: every node under one directory shares that directory's prefix, so the bytes below the truncation point are
/// always already correct. The node visited before is the parent or something beneath the parent, so its path also
/// starts with the parent's, and the shared length is the parent's length plus however far the old tail and the new one
/// agree. The stack holds one frame per level, not one per pending node.
fn depth_first(tree: &Tree<Payload>, mut visit: impl FnMut(&Payload, &str, usize)) {
    let mut path = String::new();
    let mut stack = vec![Frame {
        children: tree.children(),
        prefix_len: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        let Some(node) = frame.children.next() else {
            stack.pop();
            continue;
        };
        let prefix_len = frame.prefix_len;

        let separator: &[u8] = if prefix_len == 0 { b"" } else { b"/" };
        let tail = separator.iter().chain(node.name().as_bytes());
        let common = prefix_len
            + path.as_bytes()[prefix_len..]
                .iter()
                .zip(tail)
                .take_while(|(old, new)| old == new)
                .count();

        path.truncate(prefix_len);
        if prefix_len != 0 {
            path.push('/');
        }
        path.push_str(node.name());

        visit(node.value(), &path, common);

        stack.push(Frame {
            children: node.children(),
            prefix_len: path.len(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The order spelled out a byte at a time, which the word-wise comparison has to agree with.
    fn tree_order_by_byte(left: &str, right: &str) -> Ordering {
        let rank = |byte: u8| if byte == b'/' { 0 } else { u16::from(byte) + 1 };
        left.bytes().map(rank).cmp(right.bytes().map(rank))
    }

    #[test]
    fn a_value_too_wide_for_its_array_is_kept_aside_and_read_back() {
        let mut overflow = Overflow::default();
        let offsets = [
            0,
            1,
            usize::from(u16::MAX) - 1,
            usize::from(u16::MAX),
            1 << 20,
        ];
        let shared = [0, 254, 255, 256, 70_000];

        let narrow_offsets: Vec<u16> = (0..)
            .zip(offsets)
            .map(|(at, offset)| overflow.keep_offset(at, offset))
            .collect();
        let narrow_shared: Vec<u8> = (0..)
            .zip(shared)
            .map(|(at, shared)| overflow.keep_shared(at, shared))
            .collect();

        assert_eq!(narrow_offsets, [0, 1, u16::MAX - 1, u16::MAX, u16::MAX]);
        assert_eq!(narrow_shared, [0, 254, u8::MAX, u8::MAX, u8::MAX]);
        assert_eq!(overflow.offset(3), usize::from(u16::MAX));
        assert_eq!(overflow.offset(4), 1 << 20);
        assert_eq!(overflow.shared(2), 255);
        assert_eq!(overflow.shared(3), 256);
        assert_eq!(overflow.shared(4), 70_000);
    }

    #[test]
    fn a_block_past_what_a_u16_offset_reaches_reads_back_whole() {
        // No filesystem this suite runs on holds a path long enough to fill 64 KiB of one block, so the block is
        // stored here through the build's own narrowing and read back through the index's reader.
        let long = "d".repeat(70_000);
        let paths = [long.clone(), format!("{long}/a"), format!("{long}/b")];
        let mut overflow = Overflow::default();
        let (mut suffixes, mut offsets, mut shared) = (String::new(), Vec::new(), Vec::new());
        let mut previous = "";
        for (at, path) in paths.iter().enumerate() {
            let common = common_len(previous.as_bytes(), path.as_bytes());
            offsets.push(overflow.keep_offset(at, suffixes.len()));
            shared.push(overflow.keep_shared(at, common));
            suffixes.push_str(&path[stored_from(at, common)..]);
            previous = path;
        }
        let index = Index {
            root: PathBuf::new(),
            suffixes: suffixes.into_boxed_str(),
            block_starts: vec![0],
            offsets,
            shared,
            overflow,
            kinds: vec![EntryKind::Dir, EntryKind::File, EntryKind::File],
            symlink_targets: Vec::new(),
            inspection: Inspection::new(paths.len()),
            binary_extensions: BinaryExtensions::default(),
        };

        let mut reader = index.paths();
        for at in [2, 0, 1, 2] {
            assert_eq!(reader.get(at), paths[at], "position {at}");
        }
        assert_eq!(index.position(&paths[2]), Some(2));
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
