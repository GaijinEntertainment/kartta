use std::cmp::Ordering;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;
use std::{io, iter, mem};

use kartta_walk::{CancelToken, Children, EntryKind, Tree, WalkOptions, WalkOutcome, walk};
use rayon::{ThreadPool, ThreadPoolBuilder};

use crate::binary::BinaryExtensions;
use crate::build::{BuildError, BuildOptions, IgnoreSources};
use crate::cache::SharedCache;
use crate::content::{ContentId, ContentVisitor};
use crate::content_index::{
    Collector, ContentIndex, ContentIndexStats, ContentIndexStorage, ContentIndexes, IdPositions,
    NO_POSITION, Narrowed, Target, Unfinished,
};
use crate::entry::Entry;
use crate::filter::Filter;
use crate::inspection::Inspection;
use crate::list::excluded_ancestor;
use crate::policy::{
    Collected, Descending, DirState, Indexing, Learned, Numbering, Payload, Reading, Report,
};
use crate::pruned::PrunedNames;
use crate::requirement::Requirement;
use crate::stamp::{Stamp, Stamping, UNTRUSTED};

mod changed;
mod refresh;
mod scoped;
mod splice;

use changed::{ChangedFiles, Held, Replaced};

pub(crate) use refresh::Previous;

/// A snapshot of a directory tree: every entry the ignore rules kept, in the order a depth-first descent finds them.
///
/// Build one with [`Index::build`] and query it as often as you like. The structure is fixed at the moment of the
/// walk — which entries exist, what each one is, where a symlink points — and [`Index::refresh`] walks the tree again
/// to see a later change to it, or [`Index::refresh_paths`] only the parts of it a caller names.
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
/// assert!(outcome.errors.is_empty(), "a complete answer");
///
/// let index = outcome.index;
/// assert_eq!(index.size("main.rs"), Some(12));
/// assert!(index.entry("ignored.txt").is_none(), "the .gitignore applied");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Index {
    root: PathBuf,
    suffixes: Box<str>,
    blocks: Vec<Block>,
    offsets: Vec<u16>,
    shared: Vec<u8>,
    overflow: Overflow,
    kinds: Vec<EntryKind>,
    symlink_targets: Vec<(usize, Box<str>)>,
    inspection: Inspection,
    ignore_sources: IgnoreSources,
    pruned_names: PrunedNames,
    binary_extensions: BinaryExtensions,
    first_content_id: u32,
    next_content_id: u32,
    content_positions: Box<[u32]>,
    content_stamps: Option<Box<[Stamp]>>,
    content_storage: Option<ContentIndexStorage>,
    content_failure: Option<Box<str>>,
    content_cache: Option<PathBuf>,
    content_cache_ttl: Duration,
    content: ContentIndexes,
    pool: Option<Arc<ThreadPool>>,
    worktrees: Vec<Box<str>>,
}

impl Index {
    /// Walks `root` and builds the index.
    ///
    /// On Linux with glibc, a build that returns an index ends with `malloc_trim(0)`, because glibc keeps the pages
    /// the walk freed resident in the arenas of the threads that freed them. The trim returns the free pages of every
    /// arena in the process, the caller's own included, and holds each arena's lock while it trims that arena.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError::Walk`] if `root` itself cannot be listed, and [`BuildError::ThreadPool`] if the thread
    /// count [`BuildOptions::threads`] asks for cannot be started. Every deeper failure is collected into
    /// [`BuildOutcome::errors`] instead, leaving a partial index — check it before trusting the answer, because a
    /// partial index looks exactly like a complete one. A content index that cannot be kept where
    /// [`BuildOptions::content_index`] asks is such a failure: the index then answers as one built without it.
    pub fn build(
        root: impl AsRef<Path>,
        options: &BuildOptions,
    ) -> Result<BuildOutcome, BuildError> {
        Self::build_walking(root.as_ref(), options, Asked::new(None, None))
    }

    /// Walks `root` and builds the index, handing `visitor` the content of each text file it wants, read during the
    /// walk, and stopping when `cancel` is cancelled. With neither, this is [`Index::build`].
    ///
    /// A visitor is for a consumer that also needs file content — a parser, a content index — so the build reads each
    /// file it wants once, while the walk is in its directory, rather than the consumer opening every file again after
    /// the walk. [`ContentVisitor`] carries the contract. Each read also settles the file's size and binary verdict,
    /// which [`Index::size`] and [`Index::is_binary`] then answer without a read of their own. Without a visitor, the
    /// build reads only what [`BuildOptions::content_index`] asks it to, and with neither it reads no file. It ends
    /// with the trim [`Index::build`] ends with.
    ///
    /// # Errors
    ///
    /// As [`Index::build`], and [`BuildError::Stopped`] when `cancel` stopped the build before it saw every entry or
    /// finished the content index. A wanted file that cannot be read is collected into [`BuildOutcome::errors`] as
    /// [`BuildError::Read`].
    pub fn build_with(
        root: impl AsRef<Path>,
        options: &BuildOptions,
        visitor: Option<&dyn ContentVisitor>,
        cancel: Option<&CancelToken>,
    ) -> Result<BuildOutcome, BuildError> {
        Self::build_walking(root.as_ref(), options, Asked::new(visitor, cancel))
    }

    /// Walks the tree again and makes this index what a build of it would make now, handing `visitor` each wanted text
    /// file it reads, as a build does.
    ///
    /// The walk applies the rules the index was built with: the same root, ignore sources and binary extensions, a
    /// content index kept the same way, stamps or none, on the same threads. So a file added, removed or changed since,
    /// and a change to a `.gitignore`, all show once this returns.
    ///
    /// What it reads depends on [`BuildOptions::stamps`]. Without stamps, every file the visitor wants and every file
    /// the content index reads is read again, which costs what the build did. With them, a file whose size and change
    /// time match its stamp is carried over unread: it keeps its [`ContentId`], what the index knew of it
    /// and its place in the content index, and the visitor is not handed it again — unless the visitor wants it now
    /// and was never handed it before. A stamp is checked against the size and change time the directory listing gives
    /// where the platform lists them, as macOS does, at no call of its own; elsewhere, and in a directory the kernel
    /// cannot list with them, against one stat of the file. The files a refresh reads are indexed in a segment of the
    /// content index beside the segments that hold the files it carried. Once there are more than eight segments, the
    /// refresh merges the ones after the first into one, at a cost that follows what those segments hold; once more
    /// than a quarter of the files the segments hold lists for are gone, it merges all
    /// of them into one, at about what the build's own merge cost. Either way the index in use stays whole until
    /// the new one is done, so the two are held at once while it runs. A refresh that returns an error, a cancelled one
    /// included, leaves the index answering as it did; one that returns [`RefreshOutcome`] has replaced it, even when
    /// [`RefreshOutcome::errors`] lists what its walk could not read, and ends with the trim [`Index::build`] ends
    /// with, once the replaced index is dropped.
    ///
    /// Each file a refresh reads gets a new [`ContentId`], numbered past every id the index handed out before,
    /// including those of a refresh that returned an error or that a panicking visitor unwound, so one id never names
    /// two files. [`RefreshOutcome::retired`] lists the ids this index does not answer for now — every file read again
    /// or gone, and every id such a refresh handed out — which a consumer that keeps something per id drops.
    ///
    /// # Errors
    ///
    /// As [`Index::build`], [`BuildError::Cancelled`] when `cancel` stopped the refresh before it saw every entry or
    /// finished the content index, [`BuildError::ContentIdsExhausted`] when the index has no id left for a file the
    /// refresh reads, and [`BuildError::ContentIndex`] when the content index cannot be kept: unlike a build, a refresh
    /// has an index to go on answering from. One the build gave up stays absent, whatever a refresh finds, because the
    /// files a refresh carries over are not read for it; a new build asks for it again. A wanted file that cannot be
    /// read is collected into [`RefreshOutcome::errors`].
    pub fn refresh(
        &mut self,
        visitor: Option<&dyn ContentVisitor>,
        cancel: Option<&CancelToken>,
    ) -> Result<RefreshOutcome, BuildError> {
        self.refresh_walking(Asked::new(visitor, cancel))
    }

    pub(crate) fn refresh_walking(
        &mut self,
        asked: Asked<'_>,
    ) -> Result<RefreshOutcome, BuildError> {
        let (BuildOutcome { mut index, errors }, retired) =
            self.counting_handed(|index, handed| index.walk_again(asked, handed))?;
        let mut changed = ChangedFiles::since(self);
        changed.add_between(Held::all(self), Held::all(&index));
        index.content_failure = self.content_failure.take();
        *self = index;
        release_freed_memory();

        Ok(RefreshOutcome::new(
            errors,
            retired,
            vec![String::new()],
            changed,
        ))
    }

    fn counting_handed<R>(&mut self, operation: impl FnOnce(&Self, &AtomicU32) -> R) -> R {
        let handed = AtomicU32::new(0);
        // constraint: a visitor that panics unwinds through the walk after it was handed ids, and a caller that catches
        // the panic keeps this index, so the ids it handed out are counted before the panic goes on
        let ran = panic::catch_unwind(AssertUnwindSafe(|| operation(self, &handed)));
        self.next_content_id += handed.into_inner();

        ran.unwrap_or_else(|panicked| panic::resume_unwind(panicked))
    }

    fn stopped_refresh_ids(&self) -> Range<u32> {
        self.first_content_id + node_sized(self.content_positions.len())..self.next_content_id
    }

    fn retiring(&self, rising_slots: impl IntoIterator<Item = u32>) -> Vec<Range<u32>> {
        let mut runs: Vec<Range<u32>> = Vec::new();
        for slot in rising_slots {
            match runs.last_mut() {
                Some(run) if run.end == slot => run.end += 1,
                _ => runs.push(slot..slot + 1),
            }
        }
        let stopped = self.stopped_refresh_ids();
        if !stopped.is_empty() {
            runs.push(stopped);
        }
        runs
    }

    fn rebuild_options(&self) -> BuildOptions {
        BuildOptions {
            ignore_sources: self.ignore_sources,
            pruned_names: self.pruned_names.clone(),
            threads: None,
            binary_extensions: self.binary_extensions.clone(),
            content_index: self.content_storage.clone(),
            content_cache: self.content_cache.clone(),
            content_cache_ttl: self.content_cache_ttl,
            stamps: self.content_stamps.is_some(),
        }
    }

    fn walk_again(
        &self,
        asked: Asked<'_>,
        handed: &AtomicU32,
    ) -> Result<(BuildOutcome, Vec<Range<u32>>), BuildError> {
        let options = self.rebuild_options();
        let can_carry = asked.visitor.is_some() || self.content_storage.is_some();
        let previous = can_carry.then(|| Previous::of(self)).flatten();
        let walking = Walking {
            asked,
            first_content_id: self.next_content_id,
            handed: Some(handed),
            pool: self.pool.clone(),
            previous: previous.as_ref(),
        };
        let outcome = Self::walk_and_build(&self.root, &options, walking)?;
        let retired = previous.as_ref().map_or_else(
            || iter::once(self.first_content_id..self.next_content_id).collect(),
            Previous::retired,
        );

        Ok((outcome, retired))
    }

    pub(crate) fn build_walking(
        root: &Path,
        options: &BuildOptions,
        asked: Asked<'_>,
    ) -> Result<BuildOutcome, BuildError> {
        let walking = Walking {
            asked,
            first_content_id: 0,
            handed: None,
            pool: options.threads.map(thread_pool).transpose()?,
            previous: None,
        };
        let outcome = Self::walk_and_build(root, options, walking);
        release_freed_memory();

        outcome
    }

    fn walk_and_build(
        root: &Path,
        options: &BuildOptions,
        walking: Walking<'_>,
    ) -> Result<BuildOutcome, BuildError> {
        let Walking {
            asked:
                Asked {
                    visitor,
                    cancel,
                    descending,
                },
            first_content_id,
            handed,
            pool,
            previous,
        } = walking;
        let numbering = Numbering {
            first_id: first_content_id,
            handed,
            stamps: options.stamps.then(Stamping::starting_now),
            previous,
        };
        let policy = indexing(root, options, visitor, numbering)?.descending(descending);
        let mut walk_options = WalkOptions::default();
        walk_options.cancel_token = cancel.cloned();
        walk_options.listed_stats = policy.may_carry();

        let WalkOutcome {
            tree,
            errors: walk_errors,
            cancelled,
            unvisited,
            ..
        } = on_pool(pool.as_deref(), || {
            walk(root, &policy, &DirState::default(), &walk_options)
        })?;
        let refreshing = handed.is_some();
        if cancelled && refreshing {
            return Err(BuildError::Cancelled);
        }

        let Report {
            mut errors,
            learned,
            stamps,
            ran_out_of_ids,
            content,
            targets,
            worktrees,
        } = policy.into_report();
        if ran_out_of_ids {
            return Err(BuildError::ContentIdsExhausted);
        }
        errors.extend(walk_errors.into_iter().map(BuildError::Walk));

        let base = Base::of(root, options, pool);
        let placing = Placing {
            first_read: first_content_id,
            learned: &learned,
            stamps: options
                .stamps
                .then(|| stamps.as_deref().unwrap_or_default()),
            previous,
        };
        let (mut index, carried_text) = Self::from_tree(base, tree, targets, placing);
        index.note_worktrees(&[String::new()], worktrees);
        let stored = content.filter(|_| !cancelled).map(|content| {
            let reads = ReadIds {
                first: first_content_id,
                count: node_sized(learned.len()),
                carried_text,
                previous,
            };
            drop(learned);
            index.finish_content_index(content, reads, cancel)
        });
        let stopped =
            cancelled || (!refreshing && matches!(stored, Some(Err(BuildError::Cancelled))));
        index.settle_content_index(stored.filter(|_| !stopped), handed.is_none(), &mut errors)?;
        order_errors(&mut errors);
        if stopped {
            return Err(index.stopped(errors, unvisited));
        }

        Ok(BuildOutcome { index, errors })
    }

    fn settle_content_index(
        &mut self,
        stored: Option<Result<ContentIndexes, BuildError>>,
        building: bool,
        errors: &mut Vec<BuildError>,
    ) -> Result<(), BuildError> {
        match stored {
            Some(Ok(content)) => self.content = content,
            Some(Err(error @ BuildError::ContentIndex { .. })) if building => errors.push(error),
            Some(Err(error)) => return Err(error),
            None => {}
        }
        let unkept = |error: &&BuildError| matches!(error, BuildError::ContentIndex { .. });
        if let Some(failure) = errors.iter().find(unkept) {
            self.content_storage = None;
            self.content_failure = Some(failure.to_string().into());
        }

        Ok(())
    }

    fn stopped(mut self, errors: Vec<BuildError>, unvisited: usize) -> BuildError {
        // constraint: a search narrows through the content index of any index that names a storage, and this one holds
        // no content index, which would read as no file holding the pattern
        self.content_storage = None;
        BuildError::Stopped(Box::new(Stopped {
            index: self,
            errors,
            unvisited,
        }))
    }

    fn finish_content_index(
        &self,
        content: Collected,
        reads: ReadIds<'_>,
        cancel: Option<&CancelToken>,
    ) -> Result<ContentIndexes, BuildError> {
        let ReadIds {
            first,
            count,
            carried_text,
            previous,
        } = reads;
        let (Finished { segment, storage }, unindexed) =
            self.finished(content, first..first + count, cancel)?;
        let unindexed = unindexed
            .iter()
            .filter_map(|path| self.position(path))
            .map(node_sized)
            .collect();
        let held = match previous {
            Some(previous) if carried_text > 0 => previous.index().content.segments.clone(),
            _ => Vec::new(),
        };

        let ids = IdPositions {
            first: self.first_content_id,
            positions: &self.content_positions,
        };
        let target = Target {
            storage: &storage,
            cancel,
        };
        self.on_threads(|| {
            ContentIndexes::joined(held, carried_text, segment, unindexed).compacted_when_due(
                ids,
                &self.inspection,
                target,
            )
        })
        .map_err(|unfinished| unfinished_error(&storage, unfinished))
    }

    fn finished(
        &self,
        collected: Collected,
        reads: Range<u32>,
        cancel: Option<&CancelToken>,
    ) -> Result<(Finished, Vec<Box<str>>), BuildError> {
        let Collected {
            collector,
            pairs,
            unindexed,
        } = collected;
        let storage = collector.storage().clone();
        let segment = self
            .on_threads(|| collector.finish(pairs, reads.end - reads.start, cancel))
            .map_err(|unfinished| unfinished_error(&storage, unfinished))?;
        let finished = Finished {
            segment: segment.with_first_id(reads.start),
            storage,
        };

        Ok((finished, unindexed))
    }

    fn from_tree(
        base: Base,
        tree: Tree<Payload>,
        mut targets: Vec<Box<str>>,
        placing: Placing<'_>,
    ) -> (Self, usize) {
        let Base {
            root,
            ignore_sources,
            pruned_names,
            binary_extensions,
            content_storage,
            content_cache,
            content_cache_ttl,
            pool,
        } = base;
        let mut suffixes_len = 0;
        let mut at = 0;
        depth_first(&tree, |_, path, common| {
            suffixes_len += path.len() - stored_from(at, path.floor_char_boundary(common));
            at += 1;
        });

        let mut encoder = Encoder::with_capacity(tree.len(), suffixes_len);
        let mut kinds = Vec::with_capacity(tree.len());
        let mut symlink_targets = Vec::new();
        let inspection = Inspection::new(tree.len());
        let next_content_id = placing.first_read + node_sized(placing.learned.len());
        let mut placed = Placed::for_reads(placing);

        depth_first(&tree, |payload, path, common| {
            let at = kinds.len();
            match *payload {
                Payload::File(Some(id)) => placed.place(&inspection, id, at),
                Payload::Symlink(Some(target)) => {
                    symlink_targets.push((at, mem::take(&mut targets[target.at()])));
                }
                _ => {}
            }
            encoder.push(
                path,
                path.floor_char_boundary(common),
                at.is_multiple_of(RESTART),
            );
            kinds.push(payload.kind());
        });
        drop(tree);

        let Encoder {
            suffixes,
            blocks,
            offsets,
            shared,
            overflow,
        } = encoder;
        let index = Self {
            root,
            inspection,
            suffixes: suffixes.into_boxed_str(),
            blocks,
            offsets,
            shared,
            overflow,
            kinds,
            symlink_targets,
            ignore_sources,
            pruned_names,
            binary_extensions,
            first_content_id: placed.floor,
            next_content_id,
            content_positions: placed.positions,
            content_stamps: placed.stamps,
            content_storage,
            content_failure: None,
            content_cache,
            content_cache_ttl,
            content: ContentIndexes::default(),
            pool,
            worktrees: Vec::new(),
        };
        (index, placed.carried_text)
    }

    /// What the content index holds, or `None` when the build was not asked for one or could not keep it, which
    /// [`Index::content_index_failure`] then says.
    ///
    /// See [`BuildOptions::content_index`] for what a build with one reads.
    #[must_use]
    pub fn content_index_stats(&self) -> Option<ContentIndexStats> {
        self.content_storage.as_ref()?;
        Some(self.content.stats())
    }

    /// Why this index holds no content index though its build was asked for one: the text of the
    /// [`BuildError::ContentIndex`] the build collected, which outlives that error, since a refresh leaves the index
    /// without one and the build's failures do not stay with it. `None` when the build kept what it was asked for, or
    /// was asked for nothing.
    #[must_use]
    pub fn content_index_failure(&self) -> Option<&str> {
        self.content_failure.as_deref()
    }

    pub(crate) fn narrowed(&self, requirement: &Requirement) -> Option<Narrowed> {
        self.content_storage.as_ref()?;
        let ids = IdPositions {
            first: self.first_content_id,
            positions: &self.content_positions,
        };
        self.content.narrow(requirement, ids, self.len())
    }

    /// How many threads this index runs its parallel work on: the walk that built it, and the searches after.
    ///
    /// Exactly what [`BuildOptions::threads`] named, which every parallel query of this index then shares, so two
    /// searches at once divide those threads rather than each taking its own. When the build named none, the index
    /// runs on the ambient rayon pool, and this is that pool's size as seen from the caller — every core the machine
    /// reports, unless the process configured the pool otherwise.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.pool
            .as_deref()
            .map_or_else(rayon::current_num_threads, ThreadPool::current_num_threads)
    }

    /// Runs `operation` on the threads this index runs on, so that a consumer's parallel work after the build shares
    /// the [`BuildOptions::threads`] the index was built with rather than starting threads of its own.
    ///
    /// A rayon call inside `operation` — `par_iter`, `join`, `scope` — runs on the index's pool when the build named a
    /// count, and on the ambient pool when it named none, as the index's own searches do.
    pub fn on_threads<R: Send>(&self, operation: impl FnOnce() -> R + Send) -> R {
        on_pool(self.pool.as_deref(), operation)
    }

    /// Where the file the build read under `id` sits in [`Index::entries`].
    ///
    /// `None` for an id this index never handed out. An id is only meaningful to the index whose build handed it to a
    /// [`ContentVisitor`]: another build of the same tree may have read the same file under a different id, and this
    /// answers for whatever this build read under it.
    #[must_use]
    pub fn content_position(&self, id: ContentId) -> Option<usize> {
        let slot = id.slot().checked_sub(self.first_content_id as usize)?;
        self.content_positions
            .get(slot)
            .filter(|&&position| position != NO_POSITION)
            .map(|&position| position as usize)
    }

    pub(crate) const fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub(crate) fn block_of(&self, at: usize) -> usize {
        self.blocks
            .partition_point(|block| block.first as usize <= at)
            .saturating_sub(1)
    }

    pub(crate) fn block_first(&self, block: usize) -> usize {
        self.blocks[block].first as usize
    }

    pub(crate) fn block_end(&self, block: usize) -> usize {
        self.blocks
            .get(block + 1)
            .map_or(self.len(), |next| next.first as usize)
    }

    // constraint: an entry's offset counts the bytes its block stored before it, and every stored suffix is at least
    // one byte — the next path in tree order is never a prefix of the one before it — so only a block's first entry has
    // an offset of 0
    pub(crate) fn starts_block(&self, at: usize) -> bool {
        self.offsets[at] == 0
    }

    fn start_in(&self, block: usize, at: usize) -> usize {
        let offset = match self.offsets[at] {
            u16::MAX => self.overflow.offset(at),
            offset => usize::from(offset),
        };
        self.blocks[block].start as usize + offset
    }

    fn end_in(&self, block: usize, at: usize) -> usize {
        match self.offsets.get(at + 1) {
            None => self.suffixes.len(),
            Some(0) => self.blocks[block + 1].start as usize,
            Some(_) => self.start_in(block, at + 1),
        }
    }

    pub(crate) fn suffix_in(&self, block: usize, at: usize) -> &str {
        path_between(
            &self.suffixes,
            self.start_in(block, at),
            self.end_in(block, at),
        )
    }

    pub(crate) fn shared_at(&self, at: usize) -> usize {
        match self.shared[at] {
            u8::MAX => self.overflow.shared(at),
            shared => usize::from(shared),
        }
    }

    fn decode_into(&self, block: usize, at: usize, path: &mut String) {
        let shared = if self.starts_block(at) {
            0
        } else {
            self.shared_at(at)
        };
        path.clear();
        let len = shared + self.suffix_in(block, at).len();
        self.push_prefix(InBlock { block, at }, len, path);
    }

    fn push_prefix(&self, spot: InBlock, len: usize, path: &mut String) {
        if len == 0 {
            return;
        }
        let InBlock { block, mut at } = spot;
        loop {
            if self.starts_block(at) {
                path.push_str(&self.suffix_in(block, at)[..len]);
                return;
            }
            let shared = self.shared_at(at);
            if shared < len {
                self.push_prefix(InBlock { block, at: at - 1 }, shared, path);
                path.push_str(&self.suffix_in(block, at)[..len - shared]);
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
            path_at: None,
            block: 0,
            path: String::new(),
        }
    }

    pub(crate) fn candidates(
        &self,
        positions: Range<usize>,
        selection: Selection<'_>,
        mut visit: impl FnMut(Candidate<'_>) -> bool,
    ) {
        let Some(exclude) = selection.exclude else {
            return self.selected(positions, selection.filter, visit);
        };
        let mut paths = self.paths();
        let mut at = positions.start;

        if let Some(past) = self.past_excluded_ancestor(&positions, exclude, &mut paths) {
            if !visit(Candidate::ExcludedBeneath(at..past)) {
                return;
            }
            at = past;
        }

        while at < positions.end {
            let (candidate, next) = self.candidate_at(at..positions.end, selection, &mut paths);
            if candidate.is_some_and(|candidate| !visit(candidate)) {
                return;
            }
            at = next;
        }
    }

    fn selected(
        &self,
        positions: Range<usize>,
        filter: Option<&Filter>,
        mut visit: impl FnMut(Candidate<'_>) -> bool,
    ) {
        let mut paths = self.paths();
        for at in positions {
            if self.kind_at(at) == EntryKind::Dir {
                continue;
            }
            let path = paths.get(at);
            if filter.is_none_or(|filter| filter.matches(path))
                && !visit(Candidate::Selected(at, path))
            {
                return;
            }
        }
    }

    fn past_excluded_ancestor(
        &self,
        positions: &Range<usize>,
        exclude: &Filter,
        paths: &mut Paths<'_>,
    ) -> Option<usize> {
        if positions.is_empty() {
            return None;
        }
        let dir = excluded_ancestor(paths.get(positions.start), exclude)?;
        let dir_at = self.position(dir)?;
        Some(self.past_subtree(dir_at + 1, dir).min(positions.end))
    }

    fn candidate_at<'p>(
        &self,
        rest: Range<usize>,
        selection: Selection<'_>,
        paths: &'p mut Paths<'_>,
    ) -> (Option<Candidate<'p>>, usize) {
        let Selection { filter, exclude } = selection;
        let here = rest.start;

        if self.kind_at(here) == EntryKind::Dir {
            let dir = paths.get(here);
            if !exclude.is_some_and(|exclude| exclude.matches(dir)) {
                return (None, here + 1);
            }
            let past = self.past_subtree(here + 1, dir).min(rest.end);
            return (Some(Candidate::ExcludedBeneath(here + 1..past)), past);
        }

        let path = paths.get(here);
        let candidate = if filter.is_some_and(|filter| !filter.matches(path)) {
            None
        } else if exclude.is_some_and(|exclude| exclude.matches(path)) {
            Some(Candidate::Excluded)
        } else {
            Some(Candidate::Selected(here, path))
        };
        (candidate, here + 1)
    }

    /// What the entry at `position` is, a position as [`Paths`] reads one.
    ///
    /// # Panics
    ///
    /// Panics if `position` is not below [`Index::len`].
    #[must_use]
    pub fn kind_at(&self, position: usize) -> EntryKind {
        self.kinds[position]
    }

    /// The directories this index holds empty because each is a linked worktree of the repository its root belongs to,
    /// in [`Index::entries`] order.
    ///
    /// Such a directory is one whose `.git` is a gitfile naming a git directory that holds a `commondir`, which names
    /// the common git directory the root's own `.git` comes to: a second checkout of the root's repository, as
    /// `git worktree add` makes one inside the tree. The directory is an entry of the index, and nothing below it is.
    /// The ignore rules decide every other layout, so a submodule, a clone and a worktree of another repository are
    /// indexed as any other directory. While [`BuildOptions::ignore_sources`](crate::BuildOptions::ignore_sources) is
    /// [`IgnoreSources::None`], nothing is held empty.
    pub fn pruned_worktrees(&self) -> impl ExactSizeIterator<Item = &str> {
        self.worktrees.iter().map(AsRef::as_ref)
    }

    pub(crate) fn note_worktrees(&mut self, walked: &[String], found: Vec<Box<str>>) {
        self.worktrees
            .retain(|path| !walked.iter().any(|part| scoped::holds(part, path)));
        self.worktrees.extend(found);
        self.worktrees
            .sort_unstable_by(|left, right| tree_order(left, right));
        self.worktrees.dedup();
    }

    pub(crate) const fn inspection(&self) -> &Inspection {
        &self.inspection
    }

    pub(crate) const fn binary_extensions(&self) -> &BinaryExtensions {
        &self.binary_extensions
    }

    pub(crate) const fn pruned_names(&self) -> &PrunedNames {
        &self.pruned_names
    }

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

    pub(crate) fn entry_at(&self, at: usize) -> Entry<'_> {
        let mut path = String::new();
        self.decode_into(self.block_of(at), at, &mut path);
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
    /// `None` does not mean the path does not exist: an ignored file, a file under a pruned name, a file below an
    /// unreadable directory and a file created after the walk all answer the same way.
    #[must_use]
    pub fn entry(&self, path: &str) -> Option<Entry<'_>> {
        Some(self.entry_with(self.position(path)?, path.into()))
    }

    pub(crate) fn position(&self, path: &str) -> Option<usize> {
        self.search(path).ok()
    }

    pub(crate) fn search(&self, path: &str) -> Result<usize, usize> {
        let (mut low, mut high) = (0, self.block_count());

        while low < high {
            let middle = low + (high - low) / 2;
            let first = self.block_first(middle);
            match tree_order(self.suffix_in(middle, first), path) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(first),
            }
        }

        let Some(block) = low.checked_sub(1) else {
            return Err(0);
        };
        let first = self.block_first(block);
        let mut common = common_len(self.suffix_in(block, first).as_bytes(), path.as_bytes());
        for at in first + 1..self.block_end(block) {
            let shared = self.shared_at(at);
            let below_path_like_its_predecessor = shared > common;
            if below_path_like_its_predecessor {
                continue;
            }

            let stored = self.suffix_in(block, at);
            let Some(rest) = path.get(shared..) else {
                return Err(at);
            };
            match tree_order(stored, rest) {
                Ordering::Less => common = shared + common_len(stored.as_bytes(), rest.as_bytes()),
                Ordering::Equal => return Ok(at),
                Ordering::Greater => return Err(at),
            }
        }
        Err(self.block_end(block))
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
    /// looked at. A file already opened — by the build, for a [`ContentVisitor`] or for
    /// [`BuildOptions::content_index`], by [`Index::grep`], or to answer [`Index::is_binary`] — answers from that open,
    /// with no ask of its own.
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
    /// Two built-in names are not binary by construction. `svg` is XML, built in because in practice it holds
    /// machine-emitted path data whose lines are of no use to a reader, so an SVG reports binary here without ever
    /// being read. `dag` is Dagor's binary format and also names the text workflow files of the Condor job scheduler,
    /// so a tree that holds those states a set of its own.
    ///
    /// A name that settles nothing is answered by whether a NUL appears in the file's first 64 KiB, the span the
    /// searcher reads at a time, so a search's own reading of the file agrees with the verdict. That verdict is about
    /// a prefix rather than about a whole file: a file that is text for 64 KiB and binary after reports text here.
    ///
    /// **A name that settles nothing reads the filesystem**, on the first ask for a given entry, and the answer is
    /// kept — see [`Index::size`] for what taking an answer at ask time rather than at walk time costs in coherence.
    /// A name the set settles is never read at all, on the first ask or any later one, and a file the build read, for
    /// a [`ContentVisitor`] or for [`BuildOptions::content_index`], answers from that read.
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

    fn is_binary_at(&self, at: usize) -> Option<bool> {
        let path = || self.entry_at(at).path;
        self.inspection.is_binary(
            at,
            || self.binary_extensions.matches(&path()),
            || self.root.join(&*path()),
        )
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct Inspected<'a> {
    index: &'a Index,
    at: usize,
}

impl Index {
    pub(crate) const fn inspected_at(&self, at: usize) -> Inspected<'_> {
        Inspected { index: self, at }
    }
}

impl<'a> Inspected<'a> {
    pub(crate) fn entry(&self) -> Entry<'a> {
        self.index.entry_at(self.at)
    }

    pub(crate) fn size(&self) -> Option<u64> {
        if self.index.kind_at(self.at) != EntryKind::File {
            return None;
        }

        self.index.inspection.size(self.at, || {
            self.index.root.join(self.index.paths().get(self.at))
        })
    }

    pub(crate) fn is_binary(&self) -> Option<bool> {
        if self.index.kind_at(self.at) != EntryKind::File {
            return None;
        }

        self.index.is_binary_at(self.at)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Selection<'f> {
    pub(crate) filter: Option<&'f Filter>,
    pub(crate) exclude: Option<&'f Filter>,
}

#[derive(Debug)]
pub(crate) enum Candidate<'p> {
    Selected(usize, &'p str),
    Excluded,
    ExcludedBeneath(Range<usize>),
}

/// Reads the paths of an [`Index`]'s entries by position, rebuilding each into one buffer the reader reuses.
///
/// Positions are the ones [`Index::entries`] runs in, and the ones a [`Listed`](crate::Listed), a
/// [`FuzzyMatch`](crate::FuzzyMatch) and a [`Index::glob`] answer carry. Reading positions in rising order costs a few
/// bytes per entry; any other order rebuilds the path from the nearest entry before it that stores its whole path.
#[derive(Debug, Clone)]
pub struct Paths<'a> {
    index: &'a Index,
    path_at: Option<usize>,
    block: usize,
    path: String,
}

impl Paths<'_> {
    /// The path of the entry at `position`, valid until the next read.
    ///
    /// # Panics
    ///
    /// Panics if `position` is not below [`Index::len`].
    pub fn get(&mut self, position: usize) -> &str {
        match self.path_at.and_then(|at| position.checked_sub(at)) {
            Some(0) => {}
            Some(gap) if gap <= STEP_LIMIT => {
                (position - gap + 1..=position).for_each(|at| self.step_to(at));
            }
            _ => {
                self.block = self.index.block_of(position);
                self.index.decode_into(self.block, position, &mut self.path);
            }
        }
        self.path_at = Some(position);
        &self.path
    }

    fn step_to(&mut self, at: usize) {
        if self.index.starts_block(at) {
            self.block += 1;
            self.path.clear();
        } else {
            self.path.truncate(self.index.shared_at(at));
        }
        self.path.push_str(self.index.suffix_in(self.block, at));
    }
}

#[derive(Clone, Copy)]
struct InBlock {
    block: usize,
    at: usize,
}

#[derive(Debug, Clone, Copy)]
struct Block {
    first: u32,
    start: u32,
}

const RESTART: usize = 128;
const STEP_LIMIT: usize = 4;

const fn stored_from(at: usize, shared: usize) -> usize {
    if at.is_multiple_of(RESTART) {
        0
    } else {
        shared
    }
}

struct Base {
    root: PathBuf,
    ignore_sources: IgnoreSources,
    pruned_names: PrunedNames,
    binary_extensions: BinaryExtensions,
    content_storage: Option<ContentIndexStorage>,
    content_cache: Option<PathBuf>,
    content_cache_ttl: Duration,
    pool: Option<Arc<ThreadPool>>,
}

impl Base {
    fn of(root: &Path, options: &BuildOptions, pool: Option<Arc<ThreadPool>>) -> Self {
        Self {
            root: root.to_path_buf(),
            ignore_sources: options.ignore_sources,
            pruned_names: options.pruned_names.clone(),
            binary_extensions: options.binary_extensions.clone(),
            content_storage: options.content_index.clone(),
            content_cache: options.content_cache.clone(),
            content_cache_ttl: options.content_cache_ttl,
            pool,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Asked<'a> {
    pub(crate) visitor: Option<&'a dyn ContentVisitor>,
    pub(crate) cancel: Option<&'a CancelToken>,
    pub(crate) descending: Option<Descending<'a>>,
}

impl<'a> Asked<'a> {
    pub(crate) const fn new(
        visitor: Option<&'a dyn ContentVisitor>,
        cancel: Option<&'a CancelToken>,
    ) -> Self {
        Self {
            visitor,
            cancel,
            descending: None,
        }
    }
}

struct Walking<'a> {
    asked: Asked<'a>,
    first_content_id: u32,
    handed: Option<&'a AtomicU32>,
    pool: Option<Arc<ThreadPool>>,
    previous: Option<&'a Previous<'a>>,
}

struct Finished {
    segment: ContentIndex,
    storage: ContentIndexStorage,
}

#[derive(Clone, Copy)]
struct ReadIds<'a> {
    first: u32,
    count: u32,
    carried_text: usize,
    previous: Option<&'a Previous<'a>>,
}

fn on_pool<R: Send>(pool: Option<&ThreadPool>, operation: impl FnOnce() -> R + Send) -> R {
    match pool {
        Some(pool) => pool.install(operation),
        None => operation(),
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_freed_memory() {
    // SAFETY: `malloc_trim` takes no pointer: it returns the pages of glibc's own free chunks to the system, each arena
    // under its own lock, so it is sound from any thread at any time.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
const fn release_freed_memory() {}

fn thread_pool(threads: NonZeroUsize) -> Result<Arc<ThreadPool>, BuildError> {
    ThreadPoolBuilder::new()
        .num_threads(threads.get())
        .thread_name(|index| format!("kartta-fs-{index}"))
        .build()
        .map(Arc::new)
        .map_err(|source| BuildError::ThreadPool {
            threads,
            source: Box::new(source),
        })
}

struct Placing<'a> {
    first_read: u32,
    learned: &'a [Learned],
    stamps: Option<&'a [Stamp]>,
    previous: Option<&'a Previous<'a>>,
}

struct Placed<'a> {
    placing: Placing<'a>,
    floor: u32,
    positions: Box<[u32]>,
    stamps: Option<Box<[Stamp]>>,
    carried_text: usize,
}

impl<'a> Placed<'a> {
    fn for_reads(placing: Placing<'a>) -> Self {
        let floor = match placing.previous {
            Some(previous) if previous.carried_any() => previous.first_id(),
            _ => placing.first_read,
        };
        let ids = (placing.first_read - floor) as usize + placing.learned.len();

        Self {
            floor,
            positions: vec![NO_POSITION; ids].into_boxed_slice(),
            stamps: placing
                .stamps
                .map(|_| vec![UNTRUSTED; ids].into_boxed_slice()),
            carried_text: 0,
            placing,
        }
    }

    fn place(&mut self, inspection: &Inspection, id: ContentId, at: usize) {
        let slot = id.slot() - self.floor as usize;
        let read = id.slot().checked_sub(self.placing.first_read as usize);
        let stamp = match (read, self.placing.previous) {
            (Some(read), _) => {
                let learned = self.placing.learned[read];
                inspection.record_size(at, learned.size);
                if let Some(binary) = learned.binary {
                    inspection.record_binary(at, binary);
                }
                self.placing.stamps.map_or(UNTRUSTED, |stamps| stamps[read])
            }
            (None, Some(previous)) => {
                let Some((raw, stamp)) = previous.carried(id) else {
                    return;
                };
                inspection.set_raw(at, raw);
                if Inspection::raw_is_text(raw) {
                    self.carried_text += 1;
                }
                stamp
            }
            (None, None) => return,
        };
        self.positions[slot] = node_sized(at);
        if let Some(stamps) = &mut self.stamps {
            stamps[slot] = stamp;
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Overflow {
    offsets: Vec<(usize, usize)>,
    shared: Vec<(usize, usize)>,
}

impl Overflow {
    fn keep_offset(&mut self, at: usize, offset: usize) -> u16 {
        match u16::try_from(offset) {
            Ok(narrow) if narrow != u16::MAX => narrow,
            _ => {
                self.offsets.push((at, offset));
                u16::MAX
            }
        }
    }

    fn keep_shared(&mut self, at: usize, shared: usize) -> u8 {
        match u8::try_from(shared) {
            Ok(narrow) if narrow != u8::MAX => narrow,
            _ => {
                self.shared.push((at, shared));
                u8::MAX
            }
        }
    }

    fn offset(&self, at: usize) -> usize {
        kept(&self.offsets, at)
    }

    fn shared(&self, at: usize) -> usize {
        kept(&self.shared, at)
    }
}

#[derive(Debug, Default)]
struct Encoder {
    suffixes: String,
    blocks: Vec<Block>,
    offsets: Vec<u16>,
    shared: Vec<u8>,
    overflow: Overflow,
}

impl Encoder {
    fn with_capacity(entries: usize, suffix_bytes: usize) -> Self {
        Self {
            suffixes: String::with_capacity(suffix_bytes),
            blocks: Vec::with_capacity(entries.div_ceil(RESTART)),
            offsets: Vec::with_capacity(entries),
            shared: Vec::with_capacity(entries),
            overflow: Overflow::default(),
        }
    }

    fn push(&mut self, path: &str, common: usize, starts_block: bool) {
        let at = self.offsets.len();
        if starts_block {
            self.blocks.push(Block {
                first: node_sized(at),
                start: narrow(self.suffixes.len()),
            });
        }
        let start = self.blocks.last().map_or(0, |block| block.start as usize);
        self.offsets
            .push(self.overflow.keep_offset(at, self.suffixes.len() - start));
        self.shared.push(self.overflow.keep_shared(at, common));
        let stored = if starts_block { 0 } else { common };
        self.suffixes.push_str(&path[stored..]);
    }
}

fn kept(values: &[(usize, usize)], at: usize) -> usize {
    values[values.partition_point(|&(position, _)| position < at)].1
}

fn common_len(left: &[u8], right: &[u8]) -> usize {
    left.iter().zip(right).take_while(|(l, r)| l == r).count()
}

#[expect(
    clippy::expect_used,
    reason = "this needs over 4 GiB of stored path bytes, far past what the u32 offsets were chosen for"
)]
fn narrow(len: usize) -> u32 {
    u32::try_from(len).expect("an index stores fewer than 4 GiB of path bytes")
}

fn storage_error(storage: &ContentIndexStorage, source: io::Error) -> BuildError {
    BuildError::ContentIndex {
        native_path: storage.dir().map(Path::to_path_buf).unwrap_or_default(),
        source,
    }
}

fn unfinished_error(storage: &ContentIndexStorage, unfinished: Unfinished) -> BuildError {
    match unfinished {
        Unfinished::Cancelled => BuildError::Cancelled,
        Unfinished::Failed(source) => storage_error(storage, source),
    }
}

fn indexing<'a>(
    root: &Path,
    options: &'a BuildOptions,
    visitor: Option<&'a dyn ContentVisitor>,
    numbering: Numbering<'a>,
) -> Result<Indexing<'a>, BuildError> {
    let created = options
        .content_index
        .as_ref()
        .map(|storage| Collector::new(storage).map_err(|source| storage_error(storage, source)))
        .transpose();
    let (collector, uncreated) = match created {
        Ok(collector) => (collector, None),
        Err(error) if numbering.handed.is_none() => (None, Some(error)),
        Err(error) => return Err(error),
    };
    let reads = visitor.is_some() || collector.is_some();
    let opened = options
        .content_cache
        .as_deref()
        .filter(|_| reads)
        .map(|dir| SharedCache::open(dir, options.content_cache_ttl))
        .transpose();
    let (cache, unopened) = match opened {
        Ok(cache) => (cache, None),
        Err(error) => (None, Some(error)),
    };
    let reading = reads.then(|| {
        Reading::new(visitor, collector, &options.binary_extensions, numbering).with_cache(cache)
    });

    let indexing = Indexing::new(options.ignore_sources, &options.pruned_names, root, reading);
    for error in uncreated.into_iter().chain(unopened) {
        indexing.record(error);
    }
    Ok(indexing)
}

#[expect(
    clippy::expect_used,
    reason = "positions and read ids are node-sized, and the walk keeps its node ids in a u32"
)]
pub(crate) fn node_sized(count: usize) -> u32 {
    u32::try_from(count).expect("a walk records fewer nodes than a u32 counts")
}

fn path_between(suffixes: &str, start: usize, end: usize) -> &str {
    // SAFETY: every start `Index::start_in` rebuilds — an entry's, the next block's, or past the last entry
    // `suffixes.len()` itself — is `suffixes.len()` as the build read it before the first append or right after
    // appending a whole `&str`, so each one is a character boundary of `suffixes` and they rise in order; `start` and
    // `end` are an entry's start and its successor's, so `start..end` lies inside `suffixes`.
    unsafe { suffixes.get_unchecked(start..end) }
}

// constraint: `kartta-walk` sorts siblings bytewise by name, which makes depth-first order sorted by this comparison
// and lets `Index::position` binary-search it
pub(crate) fn tree_order(left: &str, right: &str) -> Ordering {
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

fn order_errors(errors: &mut [BuildError]) {
    errors.sort_by(|left, right| {
        left.native_path()
            .cmp(&right.native_path())
            .then_with(|| left.variant_rank().cmp(&right.variant_rank()))
    });
}

struct Frame<'a> {
    children: Children<'a, Payload>,
    prefix_len: usize,
}

fn depth_first(tree: &Tree<Payload>, visit: impl FnMut(&Payload, &str, usize)) {
    depth_first_below(tree, "", visit);
}

fn depth_first_below(
    tree: &Tree<Payload>,
    base: &str,
    mut visit: impl FnMut(&Payload, &str, usize),
) {
    let mut path = base.to_owned();
    let mut stack = vec![Frame {
        children: tree.children(),
        prefix_len: base.len(),
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
/// or logs and then drops.
#[derive(Debug)]
#[non_exhaustive]
pub struct BuildOutcome {
    /// The index the build produced. Check [`BuildOutcome::errors`] before keeping it alone: an index built from an
    /// incomplete walk looks exactly like one built from a complete walk.
    pub index: Index,
    /// What the build could not read, ordered by path so two builds of one tree report identically.
    ///
    /// A non-empty list means the index is wrong in a way it cannot detect, and each kind is wrong differently:
    ///
    /// - [`BuildError::Walk`] — the entry or the whole subtree it names is missing.
    /// - [`BuildError::Ignore`] — the rules went unread, so the index *holds* whatever that file would have hidden.
    ///   The only kind that makes an index wrong by addition, and the only one no later check can catch.
    /// - [`BuildError::Name`] — the entry is missing, and a directory takes its subtree with it.
    /// - [`BuildError::Read`] — the entry is indexed, and neither the [`ContentVisitor`] nor the content index holds
    ///   its content.
    /// - [`BuildError::ContentIndex`] — every entry is there, and the index holds no content index, so a search opens
    ///   every candidate. [`Index::content_index_failure`] keeps this reason.
    pub errors: Vec<BuildError>,
}

/// What a build holds when a cancel stops it, handed back in [`BuildError::Stopped`].
///
/// A cancel that lands in the walk leaves whichever entries the threads had reached, which depends on scheduling, so
/// two stopped builds of one tree hold different entries. The index answers every query over those entries and holds no
/// content index, so a [`Index::grep`] reads every file it selects. An [`Index::refresh`] of it walks the whole tree,
/// and keeps no content index.
#[derive(Debug)]
pub struct Stopped {
    /// The index of what the build reached. Check [`Stopped::unvisited`] before reading an absence from it.
    pub index: Index,
    /// What the build could not read before it stopped, as [`BuildOutcome::errors`] lists it.
    pub errors: Vec<BuildError>,
    /// The entries the walk listed and the cancel turned away, as [`kartta_walk::WalkOutcome::unvisited`] counts them:
    /// a floor on what the index lacks. Zero when the walk saw every entry and the cancel landed after it, which leaves
    /// an index of the whole tree.
    pub unvisited: usize,
}

/// What one [`Index::refresh`] or [`Index::refresh_paths`] reports: what it walked, what its walks could not read,
/// which regular files it added and removed, and which ids the index stopped answering for.
#[derive(Debug)]
pub struct RefreshOutcome {
    /// What the refresh could not read, as [`BuildOutcome::errors`] reports it for a build.
    pub errors: Vec<BuildError>,
    /// The root-relative paths the refresh walked again, in [`Index::entries`] order, none below another: the empty
    /// path when it walked the whole tree.
    ///
    /// [`Index::refresh_paths`] widens a path it cannot refresh alone to the nearest directory it can, so this is where
    /// to see that a few changed paths cost a walk of a large directory, or of the whole tree.
    pub walked: Vec<String>,
    /// The regular files the index holds now and did not hold before, in [`Index::entries`] order: files created,
    /// and paths that held another kind of entry or that the ignore rules hid. Only the walked parts are compared, so
    /// a [`Index::refresh`] pays one pass over the paths before and after it, and a [`Index::refresh_paths`] a pass
    /// over the parts it walked.
    pub added: Vec<String>,
    /// The regular files the index held before and does not hold now, in the order the index held them: files
    /// deleted, replaced by another kind of entry, or hidden by the ignore rules. A file whose content changed is in
    /// neither list; [`RefreshOutcome::retired`] names its old id.
    pub removed: Vec<String>,
    retired: Vec<Range<u32>>,
    /// The directories the index held before and does not hold now, in [`Index::entries`] order, none below another.
    #[cfg(feature = "watch")]
    pub(crate) removed_dirs: Vec<String>,
    len_before: usize,
    replaced: Vec<Replaced>,
    kept_positions: bool,
}

impl RefreshOutcome {
    fn new(
        errors: Vec<BuildError>,
        retired: Vec<Range<u32>>,
        walked: Vec<String>,
        changed: ChangedFiles,
    ) -> Self {
        Self {
            errors,
            walked,
            added: changed.added,
            removed: changed.removed,
            retired,
            #[cfg(feature = "watch")]
            removed_dirs: changed.removed_dirs,
            len_before: changed.len_before,
            replaced: changed.replaced,
            kept_positions: !changed.reshaped,
        }
    }

    /// Whether every position holds after this refresh the entry it held before, with the same path and kind: true of
    /// a refresh that only read files again, false of one that added, removed or retyped an entry. A position kept
    /// from before a refresh that answers `true` still names its entry; otherwise
    /// [`RefreshOutcome::position_after`] says where the entry went.
    #[must_use]
    pub const fn kept_positions(&self) -> bool {
        self.kept_positions
    }

    /// The position after this refresh of the entry at `position` before it, or `None` for an entry inside a part the
    /// refresh walked again, whose positions [`RefreshOutcome::walked_positions`] gives, and for a position past the
    /// entries the index held before it. A binary search of the parts.
    #[must_use]
    pub fn position_after(&self, position: usize) -> Option<usize> {
        if position >= self.len_before {
            return None;
        }
        let passed = self
            .replaced
            .partition_point(|replaced| replaced.before.end <= position);
        if self
            .replaced
            .get(passed)
            .is_some_and(|replaced| replaced.before.start <= position)
        {
            return None;
        }

        Some(passed.checked_sub(1).map_or(position, |last| {
            let last = &self.replaced[last];
            position - last.before.end + last.after.end
        }))
    }

    /// The positions the parts this refresh walked again hold after it, in rising order: one run for parts that lay
    /// side by side, empty for parts that hold nothing now. What a caller that keeps something per position reads
    /// again.
    pub fn walked_positions(&self) -> impl ExactSizeIterator<Item = Range<usize>> + '_ {
        self.replaced.iter().map(|replaced| replaced.after.clone())
    }

    /// The ids the index handed out before this refresh, including those of a refresh that returned an error or that a
    /// panicking visitor unwound, and does not answer for now, in rising order: [`Index::content_position`] answers
    /// `None` for each.
    pub fn retired(&self) -> impl Iterator<Item = ContentId> {
        self.retired
            .iter()
            .flat_map(Clone::clone)
            .map(ContentId::from_slot)
    }

    /// Whether the refresh walked again the part of the tree that holds `path`, a root-relative path spelled as
    /// [`Index::entries`] spells one: `path` or a directory above it is among [`RefreshOutcome::walked`]. A binary
    /// search of that list, in the order the refresh left it.
    #[must_use]
    pub fn walked_over(&self, path: &str) -> bool {
        let after = self
            .walked
            .partition_point(|part| tree_order(part, path).is_le());
        after
            .checked_sub(1)
            .is_some_and(|at| scoped::holds(&self.walked[at], path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_order_by_byte(left: &str, right: &str) -> Ordering {
        let rank = |byte: u8| if byte == b'/' { 0 } else { u16::from(byte) + 1 };
        left.bytes().map(rank).cmp(right.bytes().map(rank))
    }

    #[test]
    fn an_excluded_directory_reaches_a_path_query_as_one_range_wherever_a_run_starts() {
        // constraint: a fuzzy run starts inside a directory only past RANK_CHUNK_LEN entries, a tree too large to build
        // here, so the runs are walked directly
        let dir = tempfile::tempdir().expect("create temp dir");
        for path in ["vendor/a.rs", "vendor/deep/b.rs", "z.rs"] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("create directory");
            std::fs::write(full, "x").expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build")
            .index;
        let vendor = Filter::new(["vendor"]).expect("compile");
        let selection = Selection {
            filter: None,
            exclude: Some(&vendor),
        };
        let offered = |positions: Range<usize>| {
            let mut offered = Vec::new();
            index.candidates(positions, selection, |candidate| {
                offered.push(format!("{candidate:?}"));
                true
            });
            offered
        };

        assert_eq!(
            index
                .entries()
                .map(|entry| entry.path().to_owned())
                .collect::<Vec<_>>(),
            [
                "vendor",
                "vendor/a.rs",
                "vendor/deep",
                "vendor/deep/b.rs",
                "z.rs"
            ]
        );
        assert_eq!(
            offered(0..5),
            ["ExcludedBeneath(1..4)", r#"Selected(4, "z.rs")"#],
            "nothing beneath vendor is offered on its own"
        );
        assert_eq!(
            offered(2..5),
            ["ExcludedBeneath(2..4)", r#"Selected(4, "z.rs")"#],
            "a run starting inside vendor skips to its end"
        );
        assert_eq!(
            offered(1..3),
            ["ExcludedBeneath(1..3)"],
            "and stops at its own end"
        );
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

    // constraint: a build lays a block at every RESTART-th entry, so a block of any other length is stored here through
    // the build's own narrowing rather than walked
    pub(super) fn index_in_blocks(entries: &[(&str, EntryKind)], lengths: &[usize]) -> Index {
        let firsts: Vec<usize> = lengths
            .iter()
            .scan(0, |at, len| {
                let first = *at;
                *at += len;
                Some(first)
            })
            .collect();
        let mut encoder = Encoder::default();
        let mut previous = "";
        for (at, &(path, _)) in entries.iter().enumerate() {
            let common = common_len(previous.as_bytes(), path.as_bytes());
            encoder.push(path, common, firsts.contains(&at));
            previous = path;
        }
        let Encoder {
            suffixes,
            blocks,
            offsets,
            shared,
            overflow,
        } = encoder;
        Index {
            root: PathBuf::new(),
            suffixes: suffixes.into_boxed_str(),
            blocks,
            offsets,
            shared,
            overflow,
            kinds: entries.iter().map(|&(_, kind)| kind).collect(),
            symlink_targets: Vec::new(),
            inspection: Inspection::new(entries.len()),
            ignore_sources: IgnoreSources::default(),
            pruned_names: PrunedNames::default(),
            binary_extensions: BinaryExtensions::default(),
            first_content_id: 0,
            next_content_id: 0,
            content_positions: Box::default(),
            content_stamps: None,
            content_storage: None,
            content_failure: None,
            content_cache: None,
            content_cache_ttl: Duration::ZERO,
            content: ContentIndexes::default(),
            pool: None,
            worktrees: Vec::new(),
        }
    }

    #[test]
    fn a_block_past_what_a_u16_offset_reaches_reads_back_whole() {
        // constraint: no filesystem this suite runs on holds a path long enough to fill 64 KiB of one block
        let long = "d".repeat(70_000);
        let paths = [long.clone(), format!("{long}/a"), format!("{long}/b")];
        let entries = [
            (paths[0].as_str(), EntryKind::Dir),
            (paths[1].as_str(), EntryKind::File),
            (paths[2].as_str(), EntryKind::File),
        ];
        let index = index_in_blocks(&entries, &[3]);

        let mut reader = index.paths();
        for at in [2, 0, 1, 2] {
            assert_eq!(reader.get(at), paths[at], "position {at}");
        }
        assert_eq!(index.position(&paths[2]), Some(2));
    }

    #[test]
    fn blocks_of_any_length_read_back_and_answer_lookups_and_listings() {
        use EntryKind::{Dir, File};
        let entries = [
            ("a", Dir),
            ("a/b", Dir),
            ("a/b/c.rs", File),
            ("a/b/d.rs", File),
            ("a/e.rs", File),
            ("f", Dir),
            ("f/g.rs", File),
            ("h.rs", File),
        ];
        let index = index_in_blocks(&entries, &[3, 1, 4]);

        let mut reader = index.paths();
        for (at, &(path, _)) in entries.iter().enumerate() {
            assert_eq!(reader.get(at), path, "position {at} read in order");
        }
        let mut reader = index.paths();
        for at in [7, 0, 3, 6, 2, 4] {
            assert_eq!(
                reader.get(at),
                entries[at].0,
                "position {at} read out of order"
            );
        }
        for (at, &(path, _)) in entries.iter().enumerate() {
            assert_eq!(index.position(path), Some(at), "{path}");
        }
        assert_eq!(index.position("a/b/z.rs"), None);
        let listed = |dir| -> Vec<String> {
            let listing = index
                .list(dir, &crate::ListOptions::default())
                .expect("a directory");
            listing
                .items()
                .iter()
                .map(|item| item.entry().path().to_owned())
                .collect()
        };
        assert_eq!(listed("a"), ["a/b", "a/b/c.rs", "a/b/d.rs", "a/e.rs"]);
        assert_eq!(listed("a/b"), ["a/b/c.rs", "a/b/d.rs"]);
        assert_eq!(listed("f"), ["f/g.rs"]);
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

    // constraint: four billion reads are out of a test's reach, so the index is moved to `left` ids before its last
    fn two_files_with_ids_left(left: u32) -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().expect("create temp dir");
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.path().join(name), name).expect("write file");
        }
        let options = BuildOptions {
            content_index: Some(ContentIndexStorage::Memory),
            ..BuildOptions::default()
        };
        let mut index = Index::build(dir.path(), &options).expect("build").index;
        index.next_content_id = u32::MAX - left;
        (dir, index)
    }

    #[test]
    fn a_refresh_that_needs_an_id_past_the_last_fails_and_leaves_the_index_answering() {
        let (_dir, mut index) = two_files_with_ids_left(1);
        let built = ContentId::from_slot(0);
        let at = index.content_position(built).expect("a built id");

        let refreshed = index.refresh(None, None);

        assert!(matches!(refreshed, Err(BuildError::ContentIdsExhausted)));
        assert_eq!(
            index.content_position(built),
            Some(at),
            "the index answers as it did"
        );
    }

    #[test]
    fn a_refresh_numbers_a_read_with_the_last_id_an_index_has() {
        let (_dir, mut index) = two_files_with_ids_left(2);

        index
            .refresh(None, None)
            .expect("two ids are left for the two files");

        assert!(
            index
                .content_position(ContentId::from_slot(u32::MAX - 1))
                .is_some()
        );
    }
}
