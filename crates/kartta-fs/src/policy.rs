use std::io::{Read, Seek};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{fs, io};

use kartta_cache::{ContentHash, ContentHasher};
use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit};

use crate::binary::BinaryExtensions;
use crate::build::{BuildError, IgnoreSources};
use crate::cache::SharedCache;
use crate::content::{Content, ContentId, ContentVisitor};
use crate::content_index::{Collector, Full, Pairs};
use crate::index::Previous;
use crate::inspection::{PROBE, read_prefix};
use crate::open::{Symlinks, open_regular};
use crate::pruned::PrunedNames;
use crate::rules::{DOT_GIT, GITIGNORE, Rules};
use crate::stamp::{Seen, Stamp, Stamping, UNTRUSTED};
use crate::trigram::{self, Trigram, Trigrams};
use crate::worktree::{common_dir, linked_common_dir};
use crate::{into_inner, lock};

#[derive(Debug)]
pub(crate) enum Payload {
    Dir,
    File(Option<ContentId>),
    Symlink(Option<TargetId>),
    Other,
}

impl Payload {
    pub(crate) const fn kind(&self) -> EntryKind {
        match self {
            Self::Dir => EntryKind::Dir,
            Self::File(_) => EntryKind::File,
            Self::Symlink(_) => EntryKind::Symlink,
            Self::Other => EntryKind::Other,
        }
    }
}

/// A symlink's target by its place among the targets its walk read, which [`Report::targets`] holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetId(NonZeroU32);

impl TargetId {
    pub(crate) const fn at(self) -> usize {
        self.0.get() as usize - 1
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Learned {
    pub(crate) size: u64,
    pub(crate) binary: Option<bool>,
}

pub(crate) type Descending<'a> = &'a (dyn Fn(&Path) + Sync);

pub(crate) struct Indexing<'a> {
    ignore_sources: IgnoreSources,
    pruned_names: &'a PrunedNames,
    canonical_root: PathBuf,
    repository: Option<PathBuf>,
    worktrees: Mutex<Vec<Box<str>>>,
    // constraint: `Policy::enter` and `Policy::visit` return no `Result`, so a failure that costs the build information
    // collects here
    errors: Mutex<Vec<BuildError>>,
    reading: Option<Reading<'a>>,
    descending: Option<Descending<'a>>,
    targets: Mutex<Vec<Box<str>>>,
}

pub(crate) struct Reading<'a> {
    visitor: Option<&'a dyn ContentVisitor>,
    collector: Option<Collector>,
    cache: Option<SharedCache>,
    binary_extensions: &'a BinaryExtensions,
    numbering: Numbering<'a>,
    // constraint: an id is handed out and its trigrams join the run under one lock, so a run's pairs arrive in the
    // ascending id order its posting lists are written in
    kept: Mutex<Kept>,
    unindexed: Mutex<Vec<Box<str>>>,
    chunks: Mutex<Vec<Box<[u8]>>>,
}

#[derive(Clone, Copy)]
pub(crate) struct Numbering<'a> {
    pub(crate) first_id: u32,
    pub(crate) handed: Option<&'a AtomicU32>,
    pub(crate) stamps: Option<Stamping>,
    pub(crate) previous: Option<&'a Previous<'a>>,
}

struct Kept {
    learned: Vec<Learned>,
    stamps: Option<Vec<Stamp>>,
    pairs: Option<Pairs>,
    room: usize,
    ran_out: bool,
}

impl Kept {
    #[expect(
        clippy::expect_used,
        reason = "a slot stays below the room, which is at most u32::MAX"
    )]
    fn keep(
        &mut self,
        observed: Observed,
        indexed: Option<(&[Trigram], &Collector)>,
    ) -> Option<(u32, Option<Full>)> {
        // shortcut: ids never return to use, so an index runs out of them after about four billion reads and every
        // refresh that reads a file fails from then on — renumber every live id in one refresh to lift that
        if self.learned.len() == self.room {
            self.ran_out = true;
            return None;
        }
        self.learned.push(observed.learned);
        if let Some(stamps) = &mut self.stamps {
            stamps.push(observed.stamp);
        }
        let slot = u32::try_from(self.learned.len() - 1).expect("a slot below the room fits a u32");
        let full = indexed
            .zip(self.pairs.as_mut())
            .and_then(|((trigrams, collector), pairs)| pairs.push(slot, trigrams, collector));

        Some((slot, full))
    }
}

#[derive(Clone, Copy)]
struct Observed {
    learned: Learned,
    stamp: Stamp,
}

impl Observed {
    const fn untrusted(learned: Learned) -> Self {
        Self {
            learned,
            stamp: UNTRUSTED,
        }
    }
}

pub(crate) struct Report {
    pub(crate) errors: Vec<BuildError>,
    pub(crate) learned: Vec<Learned>,
    pub(crate) stamps: Option<Vec<Stamp>>,
    pub(crate) ran_out_of_ids: bool,
    pub(crate) content: Option<Collected>,
    pub(crate) targets: Vec<Box<str>>,
    pub(crate) worktrees: Vec<Box<str>>,
}

pub(crate) struct Collected {
    pub(crate) collector: Collector,
    pub(crate) pairs: Pairs,
    pub(crate) unindexed: Vec<Box<str>>,
}

impl<'a> Reading<'a> {
    pub(crate) fn new(
        visitor: Option<&'a dyn ContentVisitor>,
        collector: Option<Collector>,
        binary_extensions: &'a BinaryExtensions,
        numbering: Numbering<'a>,
    ) -> Self {
        let pairs = collector.as_ref().map(Collector::pairs);

        Self {
            visitor,
            collector,
            cache: None,
            binary_extensions,
            numbering,
            kept: Mutex::new(Kept {
                learned: Vec::new(),
                stamps: numbering.stamps.map(|_| Vec::new()),
                pairs,
                room: (u32::MAX - numbering.first_id) as usize,
                ran_out: false,
            }),
            unindexed: Mutex::new(Vec::new()),
            chunks: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn with_cache(self, cache: Option<SharedCache>) -> Self {
        Self { cache, ..self }
    }
}

impl<'a> Indexing<'a> {
    pub(crate) fn new(
        ignore_sources: IgnoreSources,
        pruned_names: &'a PrunedNames,
        root: &Path,
        reading: Option<Reading<'a>>,
    ) -> Self {
        let repository = (ignore_sources != IgnoreSources::None)
            .then(|| common_dir(&root.join(DOT_GIT)))
            .flatten();

        Self {
            ignore_sources,
            pruned_names,
            canonical_root: fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            repository,
            worktrees: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            reading,
            descending: None,
            targets: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn descending(self, descending: Option<Descending<'a>>) -> Self {
        Self { descending, ..self }
    }

    pub(crate) fn descend(&self, native_dir: &Path) {
        if let Some(descending) = self.descending {
            descending(native_dir);
        }
    }

    pub(crate) fn into_report(self) -> Report {
        let mut errors = into_inner(self.errors);
        let targets = into_inner(self.targets);
        let worktrees = into_inner(self.worktrees);
        let Some(reading) = self.reading else {
            return Report {
                errors,
                learned: Vec::new(),
                stamps: None,
                ran_out_of_ids: false,
                content: None,
                targets,
                worktrees,
            };
        };

        let Kept {
            learned,
            stamps,
            pairs,
            ran_out,
            ..
        } = into_inner(reading.kept);
        errors.extend(reading.cache.and_then(SharedCache::publish));
        let content = reading
            .collector
            .zip(pairs)
            .map(|(collector, pairs)| Collected {
                collector,
                pairs,
                unindexed: into_inner(reading.unindexed),
            });

        Report {
            errors,
            learned,
            stamps,
            ran_out_of_ids: ran_out,
            content,
            targets,
            worktrees,
        }
    }

    pub(crate) fn record(&self, error: BuildError) {
        lock(&self.errors).push(error);
    }

    fn prunes_worktree(&self, dir_path: &str, dot_git: &Path) -> bool {
        let pruned = !dir_path.is_empty()
            && self
                .repository
                .as_ref()
                .is_some_and(|repository| linked_common_dir(dot_git).as_ref() == Some(repository));
        if pruned {
            lock(&self.worktrees).push(dir_path.into());
        }
        pruned
    }

    fn read_ignore_file(&self, native_path: &Path) -> io::Result<IgnoreFile> {
        let via_symlink = fs::symlink_metadata(native_path)?.is_symlink();
        if via_symlink && !self.leads_into_tree(native_path)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "symlink to an ignore file outside the tree",
            ));
        }

        // constraint: a name the check found regular is opened refusing a link, so a link renamed onto it after the
        // check fails the read rather than escaping the tree
        let symlinks = if via_symlink {
            Symlinks::Follow
        } else {
            Symlinks::Refuse
        };
        let (file, metadata) = open_regular(native_path, symlinks)?;

        Ok(IgnoreFile {
            bytes: read_all(&file)?,
            metadata,
            via_symlink,
        })
    }

    fn apply_ignore_file(&self, dir: &Dir<'_>, name: &str, rules: &mut Rules) -> IgnoreFileRead {
        let Some(read) = self.push_ignore_file(dir.native_path().join(name), dir.path(), rules)
        else {
            return IgnoreFileRead::Unread;
        };

        if self.reading.is_none() {
            return IgnoreFileRead::Read(None);
        }
        let path = child_path(dir.path(), name);
        if read.via_symlink || self.pruned_names.matches(name) || rules.ignores(&path, false) {
            return IgnoreFileRead::Read(None);
        }

        IgnoreFileRead::Read(self.hand_over_ignore_file(&path, &read))
    }

    fn push_ignore_file(
        &self,
        native_path: PathBuf,
        dir_path: &str,
        rules: &mut Rules,
    ) -> Option<IgnoreFile> {
        let read = match self.read_ignore_file(&native_path) {
            Ok(read) => read,
            Err(source) => {
                self.record(BuildError::Ignore {
                    native_path,
                    source,
                });
                return None;
            }
        };

        if let Err(source) = rules.push(&read.bytes, dir_path) {
            self.record(BuildError::Ignore {
                native_path,
                source,
            });
        }
        Some(read)
    }

    // constraint: a walk learns which control files a directory holds from the listing it already made; a directory
    // above a part walked again was not listed, so it is asked for each by name, two stats a directory and a third
    // where the ignore file is read
    pub(crate) fn rules_in(&self, native_dir: &Path, dir_path: &str) -> DirRules {
        let dot_git = native_dir.join(DOT_GIT);
        let starts_repository = fs::symlink_metadata(&dot_git).is_ok();
        let worktree = starts_repository && self.prunes_worktree(dir_path, &dot_git);
        let mut own = Rules::default();
        let native_path = native_dir.join(GITIGNORE);
        if !worktree && fs::symlink_metadata(&native_path).is_ok() {
            _ = self.push_ignore_file(native_path, dir_path, &mut own);
        }

        DirRules {
            starts_repository,
            worktree,
            own,
        }
    }

    pub(crate) fn single(
        &self,
        path: &str,
        native_path: &Path,
        metadata: &fs::Metadata,
    ) -> Payload {
        self.payload(
            EntryKind::from(metadata.file_type()),
            || native_path,
            || self.read_file(path, || native_path.to_path_buf(), || Seen::of(metadata)),
        )
    }

    pub(crate) fn file(
        &self,
        path: &str,
        native_path: impl FnOnce() -> PathBuf,
        seen: Seen,
    ) -> Payload {
        Payload::File(self.read_file(path, native_path, || Some(seen)))
    }

    fn payload<P: AsRef<Path>>(
        &self,
        kind: EntryKind,
        native_path: impl FnOnce() -> P,
        read: impl FnOnce() -> Option<ContentId>,
    ) -> Payload {
        match kind {
            EntryKind::Dir => Payload::Dir,
            EntryKind::File => Payload::File(read()),
            EntryKind::Symlink => Payload::Symlink(self.target(native_path().as_ref())),
            EntryKind::Other => Payload::Other,
        }
    }

    fn target(&self, native_path: &Path) -> Option<TargetId> {
        let target = read_target(native_path)?;
        let mut targets = lock(&self.targets);
        let id = u32::try_from(targets.len() + 1)
            .ok()
            .and_then(NonZeroU32::new)?;
        targets.push(target);
        drop(targets);
        Some(TargetId(id))
    }

    fn hand_over_ignore_file(&self, path: &str, read: &IgnoreFile) -> Option<ContentId> {
        let (reading, visit) = self.wanted(path)?;
        if let Some(carried) = reading.carry(path, visit, || Seen::of(&read.metadata)) {
            return Some(carried);
        }
        let bytes = &read.bytes;
        let binary = bytes[..bytes.len().min(PROBE)].contains(&0);
        let learned = Learned {
            size: read.metadata.len(),
            binary: Some(binary),
        };
        if binary {
            return reading.keep(reading.observed(learned, &read.metadata, false), None);
        }
        let observed = reading.observed(learned, &read.metadata, visit);

        let hash = reading.hash_of(bytes);
        let id = reading.keep_text(observed, bytes, hash.as_ref())?;
        reading.hand_over(
            visit,
            Content {
                id,
                path,
                bytes,
                cache: reading.lent(hash),
            },
        );
        Some(id)
    }

    fn wanted(&self, path: &str) -> Option<(&Reading<'a>, bool)> {
        let reading = self.reading.as_ref()?;
        let visit = reading.visitor.is_some_and(|visitor| visitor.wants(path));
        let indexed = reading.collector.is_some();

        ((visit || indexed) && !reading.binary_extensions.matches(path)).then_some((reading, visit))
    }

    fn leads_into_tree(&self, native_path: &Path) -> io::Result<bool> {
        Ok(fs::canonicalize(native_path)?.starts_with(&self.canonical_root))
    }

    fn admits_name(&self, entry: &Entry<'_>) -> bool {
        if !entry.name().contains(char::REPLACEMENT_CHARACTER) {
            return true;
        }

        let native_path = entry.native_path();
        if native_path
            .file_name()
            .is_some_and(|name| name.to_str().is_some())
        {
            return true;
        }

        self.record(BuildError::Name { native_path });
        false
    }

    pub(crate) const fn reads(&self) -> bool {
        self.reading.is_some()
    }

    pub(crate) fn may_carry(&self) -> bool {
        self.reading
            .as_ref()
            .is_some_and(|reading| reading.numbering.previous.is_some())
    }

    fn read_content(&self, entry: &Entry<'_>) -> Option<ContentId> {
        self.read_file(
            entry.path(),
            || entry.native_path(),
            || {
                entry.listed_stat().map(Seen::listed).or_else(|| {
                    let metadata = entry.metadata().ok()?;
                    Seen::of(&metadata)
                })
            },
        )
    }

    pub(crate) fn read_file(
        &self,
        path: &str,
        native_path: impl FnOnce() -> PathBuf,
        seen: impl FnOnce() -> Option<Seen>,
    ) -> Option<ContentId> {
        let (reading, visit) = self.wanted(path)?;
        if let Some(carried) = reading.carry(path, visit, seen) {
            return Some(carried);
        }

        let native_path = native_path();
        let outcome = match (visit, &reading.collector) {
            (false, Some(collector)) => reading.read_for_index(collector, &native_path),
            _ => reading.read_whole(&native_path, path, visit),
        };

        match outcome {
            ReadOutcome::Kept(id) => id,
            ReadOutcome::Failed { id, source } => {
                self.record(BuildError::Read {
                    native_path,
                    source,
                });
                reading.leave_unindexed(path);
                id
            }
        }
    }
}

enum ReadOutcome {
    Kept(Option<ContentId>),
    Failed {
        id: Option<ContentId>,
        source: io::Error,
    },
}

struct IgnoreFile {
    bytes: Vec<u8>,
    metadata: fs::Metadata,
    via_symlink: bool,
}

enum Readout {
    Text(Vec<u8>),
    Binary,
    Failed(io::Error),
}

enum Streamed {
    Text,
    Binary,
    Failed(io::Error),
}

impl Reading<'_> {
    fn read_whole(&self, native_path: &Path, path: &str, visit: bool) -> ReadOutcome {
        let (learned, metadata, readout) = match read_text(native_path) {
            Ok(read) => read,
            Err(source) => return ReadOutcome::Failed { id: None, source },
        };

        match readout {
            Readout::Text(bytes) => {
                let hash = self.hash_of(&bytes);
                let observed = self.observed(learned, &metadata, visit);
                let id = self.keep_text(observed, &bytes, hash.as_ref());
                if let Some(id) = id {
                    self.hand_over(
                        visit,
                        Content {
                            id,
                            path,
                            bytes: &bytes,
                            cache: self.lent(hash),
                        },
                    );
                }
                ReadOutcome::Kept(id)
            }
            Readout::Binary => {
                ReadOutcome::Kept(self.keep(self.observed(learned, &metadata, false), None))
            }
            Readout::Failed(source) => ReadOutcome::Failed {
                id: self.keep(Observed::untrusted(learned), None),
                source,
            },
        }
    }

    fn carry(
        &self,
        path: &str,
        visit: bool,
        seen: impl FnOnce() -> Option<Seen>,
    ) -> Option<ContentId> {
        self.numbering.previous?.carry(path, visit, seen)
    }

    fn observed(&self, learned: Learned, metadata: &fs::Metadata, visited: bool) -> Observed {
        Observed {
            learned,
            stamp: self
                .numbering
                .stamps
                .map_or(UNTRUSTED, |stamping| stamping.stamp(metadata, visited)),
        }
    }

    // constraint: only the index wants this file, and the index needs its trigrams rather than its bytes, so the read
    // streams through one chunk however large the file is
    fn read_for_index(&self, collector: &Collector, native_path: &Path) -> ReadOutcome {
        let mut chunk = lock(&self.chunks)
            .pop()
            .unwrap_or_else(|| vec![0; PROBE].into_boxed_slice());
        let mut extractor = collector.extractor();
        let cache = self.cache.as_ref();

        let outcome = match stream_text(native_path, &mut chunk, &mut extractor, cache) {
            Err(source) => ReadOutcome::Failed { id: None, source },
            Ok((learned, metadata, Streamed::Text)) => ReadOutcome::Kept(self.keep(
                self.observed(learned, &metadata, false),
                Some(extractor.found()),
            )),
            Ok((learned, metadata, Streamed::Binary)) => {
                ReadOutcome::Kept(self.keep(self.observed(learned, &metadata, false), None))
            }
            Ok((learned, _, Streamed::Failed(source))) => ReadOutcome::Failed {
                id: self.keep(Observed::untrusted(learned), None),
                source,
            },
        };

        collector.recycle(extractor);
        lock(&self.chunks).push(chunk);
        outcome
    }

    fn keep(&self, observed: Observed, trigrams: Option<&[Trigram]>) -> Option<ContentId> {
        let collector = self.collector.as_ref();
        let (slot, full) = lock(&self.kept).keep(observed, trigrams.zip(collector))?;
        if let Some(handed) = self.numbering.handed {
            handed.fetch_max(slot + 1, Ordering::Relaxed);
        }
        if let (Some(full), Some(collector)) = (full, collector) {
            collector.spill(full);
        }

        Some(ContentId::from_slot(self.numbering.first_id + slot))
    }

    fn hash_of(&self, bytes: &[u8]) -> Option<ContentHash> {
        self.cache.as_ref().map(|_| ContentHash::of(bytes))
    }

    fn lent(&self, hash: Option<ContentHash>) -> Option<(&SharedCache, ContentHash)> {
        self.cache.as_ref().zip(hash)
    }

    fn keep_text(
        &self,
        observed: Observed,
        bytes: &[u8],
        hash: Option<&ContentHash>,
    ) -> Option<ContentId> {
        let Some(collector) = &self.collector else {
            return self.keep(observed, None);
        };

        let mut extractor = collector.extractor();
        match self.cache.as_ref().zip(hash) {
            Some((cache, hash)) => cache.fill_trigrams(hash, bytes, &mut extractor),
            None => extractor.feed(bytes),
        }
        let id = self.keep(observed, Some(extractor.found()));
        collector.recycle(extractor);
        id
    }

    fn hand_over(&self, visit: bool, content: Content<'_>) {
        if visit && let Some(visitor) = self.visitor {
            visitor.visit(content);
        }
    }

    fn leave_unindexed(&self, path: &str) {
        if self.collector.is_some() {
            lock(&self.unindexed).push(path.into());
        }
    }
}

#[expect(
    clippy::verbose_file_reads,
    reason = "the point is to read the handle whose identity was checked, which fs::read cannot do -- it reopens the \
              name and reopening is what the check exists to avoid"
)]
fn read_text(native_path: &Path) -> io::Result<(Learned, fs::Metadata, Readout)> {
    let (mut file, metadata) = open_regular(native_path, Symlinks::Refuse)?;
    let size = metadata.len();
    let whole = usize::try_from(size).unwrap_or(usize::MAX);
    let mut learned = Learned { size, binary: None };

    let mut bytes = Vec::with_capacity(whole.min(PROBE));
    if let Err(error) = Read::by_ref(&mut file)
        .take(PROBE as u64)
        .read_to_end(&mut bytes)
    {
        return Ok((learned, metadata, Readout::Failed(error)));
    }

    let binary = bytes.contains(&0);
    learned.binary = Some(binary);
    if binary {
        return Ok((learned, metadata, Readout::Binary));
    }
    let ended_within_probe = bytes.len() < PROBE;
    if ended_within_probe {
        return Ok((learned, metadata, Readout::Text(bytes)));
    }

    let rest = bytes
        .try_reserve_exact(whole.saturating_sub(bytes.len()))
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))
        .and_then(|()| file.read_to_end(&mut bytes));

    match rest {
        Ok(_) => Ok((learned, metadata, Readout::Text(bytes))),
        Err(error) => Ok((learned, metadata, Readout::Failed(error))),
    }
}

fn stream_text(
    native_path: &Path,
    chunk: &mut [u8],
    extractor: &mut Trigrams,
    cache: Option<&SharedCache>,
) -> io::Result<(Learned, fs::Metadata, Streamed)> {
    let (mut file, metadata) = open_regular(native_path, Symlinks::Refuse)?;
    let mut learned = Learned {
        size: metadata.len(),
        binary: None,
    };

    let filled = match read_prefix(&mut file, chunk) {
        Ok(filled) => filled,
        Err(error) => return Ok((learned, metadata, Streamed::Failed(error))),
    };
    let binary = chunk[..filled].contains(&0);
    learned.binary = Some(binary);
    if binary {
        return Ok((learned, metadata, Streamed::Binary));
    }

    let streamed = match cache {
        Some(cache) if filled < chunk.len() => {
            let whole = &chunk[..filled];
            cache.fill_trigrams(&ContentHash::of(whole), whole, extractor);
            Ok(())
        }
        Some(cache) => stream_cached(&mut file, chunk, extractor, cache),
        None => feed_rest(&mut file, chunk, filled, &mut |bytes| extractor.feed(bytes)),
    };
    match streamed {
        Ok(()) => Ok((learned, metadata, Streamed::Text)),
        Err(error) => Ok((learned, metadata, Streamed::Failed(error))),
    }
}

// constraint: a lookup needs the hash of the whole file before the scan it would skip, so a file past one chunk is read
// to hash it and, when the store lacks it, read again to scan it; the scan's own hash keys what it shares, so a write
// between the two reads cannot file one content's trigrams under another's hash
fn stream_cached(
    file: &mut fs::File,
    chunk: &mut [u8],
    extractor: &mut Trigrams,
    cache: &SharedCache,
) -> io::Result<()> {
    let mut hasher = ContentHasher::new();
    let mut filled = chunk.len();
    while filled > 0 {
        hasher.update(&chunk[..filled]);
        filled = read_prefix(file, chunk)?;
    }
    if cache.take_trigrams(&hasher.finish(), extractor) {
        return Ok(());
    }

    file.rewind()?;
    let filled = read_prefix(file, chunk)?;
    let mut scanned = ContentHasher::new();
    feed_rest(file, chunk, filled, &mut |bytes| {
        extractor.feed(bytes);
        scanned.update(bytes);
    })?;
    cache.share_trigrams(&scanned.finish(), extractor);

    Ok(())
}

fn feed_rest(
    file: &mut fs::File,
    chunk: &mut [u8],
    mut filled: usize,
    feed: &mut impl FnMut(&[u8]),
) -> io::Result<()> {
    while filled == chunk.len() {
        let held = trigram::unfinished_fold(chunk);
        feed(&chunk[..filled - held]);
        chunk.copy_within(filled - held..filled, 0);
        filled = held + read_prefix(file, &mut chunk[held..])?;
    }
    feed(&chunk[..filled]);

    Ok(())
}

fn read_target(native_path: &Path) -> Option<Box<str>> {
    fs::read_link(native_path)
        .ok()?
        .into_os_string()
        .into_string()
        .ok()
        .map(String::into_boxed_str)
}

#[expect(
    clippy::verbose_file_reads,
    reason = "the point is to read the handle whose identity was checked, which fs::read cannot do -- it reopens the \
              name and reopening is what the check exists to avoid"
)]
fn read_all(mut file: &fs::File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    Ok(bytes)
}

// constraint: git folds the case of a control name only on Windows, whose filesystem resolves `.GIT` and `.GITIGNORE`
// to the lowercase names; on unix `.GIT` is an ordinary directory git tracks
pub(crate) fn names_match(name: &str, control: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(control)
    } else {
        name == control
    }
}

pub(crate) fn pruned(name: &str, pruned_names: &PrunedNames) -> bool {
    names_match(name, DOT_GIT) || pruned_names.matches(name)
}

fn listed_control_name<'a>(dir: &Dir<'a>, control: &'a str) -> Option<&'a str> {
    if !cfg!(windows) {
        return dir.contains(control).then_some(control);
    }

    dir.names().find(|name| names_match(name, control))
}

fn inherited_across_repository_boundary(starts_repository: bool, parent_state: &Rules) -> Rules {
    if starts_repository {
        Rules::default()
    } else {
        parent_state.clone()
    }
}

#[derive(Debug)]
pub(crate) struct DirRules {
    starts_repository: bool,
    worktree: bool,
    own: Rules,
}

impl DirRules {
    /// The rules below this directory, or `None` where it is a worktree the walk holds empty.
    pub(crate) fn below(self, inherited: &Rules) -> Option<Rules> {
        if self.worktree {
            return None;
        }
        let mut rules = if self.starts_repository {
            Rules::default()
        } else {
            inherited.clone()
        };
        rules.extend(self.own);
        Some(rules)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirState {
    rules: Rules,
    ignore_file: IgnoreFileRead,
    worktree: bool,
}

impl DirState {
    pub(crate) const fn below(rules: Rules) -> Self {
        Self {
            rules,
            ignore_file: IgnoreFileRead::Unread,
            worktree: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
enum IgnoreFileRead {
    #[default]
    Unread,
    Read(Option<ContentId>),
}

impl Policy for Indexing<'_> {
    type State = DirState;
    type Value = Payload;

    fn enter(&self, dir: &Dir<'_>, parent_state: &DirState) -> DirState {
        if self.ignore_sources == IgnoreSources::None {
            return DirState::default();
        }

        let dot_git = listed_control_name(dir, DOT_GIT);
        if let Some(name) = dot_git
            && self.prunes_worktree(dir.path(), &dir.native_path().join(name))
        {
            return DirState {
                worktree: true,
                ..DirState::default()
            };
        }

        let mut state = DirState {
            rules: inherited_across_repository_boundary(dot_git.is_some(), &parent_state.rules),
            ignore_file: IgnoreFileRead::Unread,
            worktree: false,
        };

        if let Some(name) = listed_control_name(dir, GITIGNORE) {
            state.ignore_file = self.apply_ignore_file(dir, name, &mut state.rules);
        }

        state
    }

    fn visit(&self, entry: &Entry<'_>, state: &DirState) -> Visit<Payload> {
        if state.worktree || pruned(entry.name(), self.pruned_names) {
            return Visit::Skip;
        }

        let kind = entry.kind();

        if !self.admits_name(entry) {
            return Visit::Skip;
        }

        if state.rules.ignores(entry.path(), kind == EntryKind::Dir) {
            return Visit::Skip;
        }

        if kind == EntryKind::Dir
            && let Some(descending) = self.descending
        {
            descending(&entry.native_path());
        }

        Visit::Keep(self.payload(
            kind,
            || entry.native_path(),
            || match state.ignore_file {
                IgnoreFileRead::Read(content) if names_match(entry.name(), GITIGNORE) => content,
                _ => self.read_content(entry),
            },
        ))
    }
}

pub(crate) fn child_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_holds_its_kind_and_one_id_in_eight_bytes() {
        assert_eq!(
            size_of::<Payload>(),
            8,
            "every walk node holds one of these until the index is built, a file's content id or a symlink's target \
             id beside its kind"
        );
    }
}
