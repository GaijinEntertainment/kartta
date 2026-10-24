use std::cell::RefCell;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::sync::OnceLock;
use std::{fs, io, iter};

use foldhash::fast::{FixedState, FoldHasher};
use kartta_fs::{Entry, EntryKind, FileStamp, Index, ListOptions, RefreshOutcome};
use kartta_lang::{Repository, path};

pub(crate) struct IndexRepository<'i> {
    index: &'i Index,
    by_name: OnceLock<Box<[HashedAt]>>,
    folded_paths: OnceLock<FoldedPaths>,
    records: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Query {
    Contains,
    InDir,
    Named,
    IgnoringCase,
    Read,
}

/// What a resolver asked the repository while it was built: a hash of each question and of each answer it read, the
/// questions alone, and the files the index held that it read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Configuration {
    pub(crate) answers: u64,
    pub(crate) asked: Box<[u32]>,
    pub(crate) read: Box<[ReadFile]>,
}

/// A file a resolver read while it was built, and its stamp from just before the read, `None` when it had none to
/// trust.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReadFile {
    pub(crate) path: Box<str>,
    pub(crate) stamp: Option<FileStamp>,
}

impl ReadFile {
    /// Whether the file under `root` holds now what the resolver read: its stamp then and now are trusted and equal.
    pub(crate) fn unchanged(&self, root: &Path) -> bool {
        self.stamp
            .is_some_and(|read| FileStamp::of(&root.join(&*self.path)) == Some(read))
    }
}

enum Asking {
    File(Vec<u32>),
    Configuration {
        answers: FoldHasher<'static>,
        asked: Vec<u32>,
        read: Vec<ReadFile>,
    },
}

thread_local! {
    static ASKING: RefCell<Option<Asking>> = const { RefCell::new(None) };
}

/// Runs `resolve`, and answers beside its result the questions it put to a recording repository on this thread, as
/// [`asked`] keys them, sorted and once each.
pub(crate) fn asked_while<R>(resolve: impl FnOnce() -> R) -> (R, Box<[u32]>) {
    ASKING.set(Some(Asking::File(Vec::new())));
    let resolved = resolve();
    let mut asked = match ASKING.take() {
        Some(Asking::File(asked)) => asked,
        _ => Vec::new(),
    };
    asked.sort_unstable();
    asked.dedup();
    (resolved, asked.into_boxed_slice())
}

/// Runs `construct`, and answers beside the resolver it made what it asked a recording repository on this thread.
pub(crate) fn configured_while<R>(construct: impl FnOnce() -> R) -> (R, Configuration) {
    ASKING.set(Some(Asking::Configuration {
        answers: FixedState::default().build_hasher(),
        asked: Vec::new(),
        read: Vec::new(),
    }));
    let made = construct();
    let configuration = match ASKING.take() {
        Some(Asking::Configuration {
            answers,
            mut asked,
            read,
        }) => {
            asked.sort_unstable();
            asked.dedup();
            Configuration {
                answers: answers.finish(),
                asked: asked.into_boxed_slice(),
                read: read.into_boxed_slice(),
            }
        }
        _ => Configuration {
            answers: 0,
            asked: Box::default(),
            read: Box::default(),
        },
    };
    (made, configuration)
}

/// The key a recording repository files a question under: the query and the path, directory or name it names.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a key that collides only makes a refresh resolve one more file again"
)]
pub(crate) fn asked(query: Query, text: &str) -> u32 {
    FixedState::default().hash_one((query as u8, text)) as u32
}

/// The keys of every question whose answer a file's appearing or vanishing at `path` can change.
pub(crate) fn reached_by_presence(path: &str) -> [u32; 5] {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    [
        asked(Query::Contains, path),
        asked(Query::InDir, dir),
        asked(Query::Named, name),
        asked(Query::IgnoringCase, &path.to_ascii_lowercase()),
        asked(Query::Read, path),
    ]
}

fn record(query: Query, text: &str) {
    ASKING.with_borrow_mut(|asking| match asking {
        Some(Asking::File(asked)) => asked.push(self::asked(query, text)),
        Some(Asking::Configuration { answers, asked, .. }) => {
            asked.push(self::asked(query, text));
            answers.write_u8(query as u8);
            answers.write(text.as_bytes());
        }
        None => {}
    });
}

/// Files `path`, stamped by `stamp`, among the files the resolver being built on this thread read; stamps nothing
/// for a file's resolution.
fn record_read(path: &str, stamp: impl FnOnce() -> Option<FileStamp>) {
    ASKING.with_borrow_mut(|asking| {
        if let Some(Asking::Configuration { read, .. }) = asking {
            read.push(ReadFile {
                path: path.into(),
                stamp: stamp(),
            });
        }
    });
}

fn record_answer(answer: Option<&[u8]>) {
    ASKING.with_borrow_mut(|asking| {
        if let Some(Asking::Configuration { answers, .. }) = asking {
            answers.write_u8(u8::from(answer.is_some()));
            let answer = answer.unwrap_or_default();
            answers.write_usize(answer.len());
            answers.write(answer);
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct HashedAt {
    hash: u32,
    position: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct FoldedPaths {
    by_hash: Box<[HashedAt]>,
    bucket_starts: Box<[u32]>,
    bucket_shift: u32,
}

/// The tables a repository finds an index's files in, by name and by path in any ASCII case, each built on the first
/// question that needs it, in hash order: what an engine that follows its tree keeps from one refresh to the next.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FileTables {
    by_name: Option<Box<[HashedAt]>>,
    folded_paths: Option<FoldedPaths>,
}

const ENTRIES_PER_BUCKET_BITS: u32 = 6;
const MAX_BUCKET_BITS: u32 = 16;

impl FileTables {
    /// These tables as they stand for `index` after the refresh `outcome` reports: kept as they are when it kept every
    /// position, and otherwise each file outside the parts it walked moved to its position now and the files the
    /// parts hold now hashed in, so a refresh costs a pass over each table, not a walk of every entry.
    pub(crate) fn refreshed(self, index: &Index, outcome: &RefreshOutcome) -> Self {
        if outcome.kept_positions() || (self.by_name.is_none() && self.folded_paths.is_none()) {
            return self;
        }
        let (mut named, mut folded) = (Vec::new(), Vec::new());
        let mut paths = index.paths();
        for at in outcome.walked_positions().flatten() {
            let Ok(position) = u32::try_from(at) else {
                break;
            };
            if index.kind_at(at) != EntryKind::File {
                continue;
            }
            let path = paths.get(at);
            if self.by_name.is_some() {
                named.push(HashedAt {
                    hash: name_hash(path::base(path)),
                    position,
                });
            }
            if self.folded_paths.is_some() {
                folded.push(HashedAt {
                    hash: folded_hash(path),
                    position,
                });
            }
        }

        Self {
            by_name: self
                .by_name
                .map(|table| moved(&table, outcome, named).into()),
            folded_paths: self
                .folded_paths
                .map(|paths| FoldedPaths::bucketed(moved(&paths.by_hash, outcome, folded))),
        }
    }
}

/// `table`'s files outside the walked parts at their positions after `outcome`, and `fresh` merged in, in hash order.
fn moved(table: &[HashedAt], outcome: &RefreshOutcome, mut fresh: Vec<HashedAt>) -> Vec<HashedAt> {
    fresh.sort_unstable();
    let mut moved = Vec::with_capacity(table.len() + fresh.len());
    let mut fresh = fresh.into_iter().peekable();
    let kept = table.iter().filter_map(|at| {
        let position = outcome.position_after(at.position as usize)?;
        Some(HashedAt {
            position: u32::try_from(position).ok()?,
            ..*at
        })
    });
    for at in kept {
        while let Some(first) = fresh.next_if(|first| *first < at) {
            moved.push(first);
        }
        moved.push(at);
    }
    moved.extend(fresh);
    moved
}

impl FoldedPaths {
    fn new(index: &Index) -> Self {
        Self::bucketed(hashed_files(index, |entry| folded_hash(entry.path())))
    }

    fn bucketed(by_hash: Vec<HashedAt>) -> Self {
        let bits = by_hash
            .len()
            .bit_width()
            .saturating_sub(ENTRIES_PER_BUCKET_BITS)
            .min(MAX_BUCKET_BITS);
        let bucket_shift = u32::BITS - bits;
        let mut bucket_starts = vec![0; (1 << bits) + 1];
        for at in &by_hash {
            bucket_starts[bucket(at.hash, bucket_shift) + 1] += 1;
        }
        let mut total = 0;
        for start in &mut bucket_starts {
            total += *start;
            *start = total;
        }

        Self {
            by_hash: by_hash.into_boxed_slice(),
            bucket_starts: bucket_starts.into_boxed_slice(),
            bucket_shift,
        }
    }

    fn hashed(&self, hash: u32) -> &[HashedAt] {
        let bucket = bucket(hash, self.bucket_shift);
        let run = self.bucket_starts[bucket] as usize..self.bucket_starts[bucket + 1] as usize;
        hashed(&self.by_hash[run], hash)
    }
}

impl<'i> IndexRepository<'i> {
    pub(crate) const fn new(index: &'i Index) -> Self {
        Self {
            index,
            by_name: OnceLock::new(),
            folded_paths: OnceLock::new(),
            records: false,
        }
    }

    /// A repository that files each question it is asked, and each answer while a resolver is built, with whatever
    /// [`asked_while`] or [`configured_while`] runs on the asking thread, and finds files in `tables` where they hold
    /// a table: tables made for `index` as it stands.
    pub(crate) fn recording(index: &'i Index, tables: FileTables) -> Self {
        Self {
            index,
            by_name: tables.by_name.map_or_else(OnceLock::new, OnceLock::from),
            folded_paths: tables
                .folded_paths
                .map_or_else(OnceLock::new, OnceLock::from),
            records: true,
        }
    }

    /// The tables this repository built or was given, for the index as it stands.
    pub(crate) fn into_tables(self) -> FileTables {
        FileTables {
            by_name: self.by_name.into_inner(),
            folded_paths: self.folded_paths.into_inner(),
        }
    }

    fn ask(&self, query: Query, text: &str) {
        if self.records {
            record(query, text);
        }
    }

    fn answering<'a>(
        &self,
        paths: impl Iterator<Item = String> + 'a,
    ) -> Box<dyn Iterator<Item = String> + 'a> {
        if self.records {
            Box::new(paths.inspect(|path| record_answer(Some(path.as_bytes()))))
        } else {
            Box::new(paths)
        }
    }

    fn by_name(&self) -> &[HashedAt] {
        self.by_name
            .get_or_init(|| hashed_files(self.index, |entry| name_hash(entry.name())).into())
    }
}

impl Repository for IndexRepository<'_> {
    fn contains(&self, path: &str) -> bool {
        self.ask(Query::Contains, path);
        let contains = self.holds_file(path);
        if self.records {
            record_answer(Some(&[u8::from(contains)]));
        }
        contains
    }

    fn files_in_dir<'a>(&'a self, dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let dir = if dir == "." { "" } else { dir };
        self.ask(Query::InDir, dir);
        let Ok(listing) = self.index.list(dir, &ListOptions::default().depth(1)) else {
            return Box::new(iter::empty());
        };

        self.answering(
            listing
                .into_items()
                .into_iter()
                .map(|listed| listed.entry())
                .filter(|entry| entry.kind() == EntryKind::File)
                .map(|entry| entry.path().to_owned()),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        self.ask(Query::Named, name);
        let name = name.to_owned();
        let mut paths = self.index.paths();

        self.answering(
            hashed(self.by_name(), name_hash(&name))
                .iter()
                .map(move |named| paths.get(named.position as usize).to_owned())
                .filter(move |found| path::base(found) == name),
        )
    }

    fn files_ignoring_case<'a>(&'a self, path: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let path = path.to_ascii_lowercase();
        self.ask(Query::IgnoringCase, &path);
        let mut paths = self.index.paths();
        let folded_paths = self
            .folded_paths
            .get_or_init(|| FoldedPaths::new(self.index));

        self.answering(
            folded_paths
                .hashed(folded_hash(&path))
                .iter()
                .map(move |folded| paths.get(folded.position as usize).to_owned())
                .filter(move |found| found.eq_ignore_ascii_case(&path)),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.ask(Query::Read, path);
        let read = if self.holds_file(path) {
            let full = self.index.root().join(path);
            if self.records {
                record_read(path, || FileStamp::of(&full));
            }
            fs::read(full)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} is not a regular file the build indexed"),
            ))
        };
        if self.records {
            record_answer(read.as_deref().ok());
        }
        read
    }
}

impl IndexRepository<'_> {
    fn holds_file(&self, path: &str) -> bool {
        self.index
            .entry(path)
            .is_some_and(|entry| entry.kind() == EntryKind::File)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the shift leaves at most 16 bits"
)]
fn bucket(hash: u32, shift: u32) -> usize {
    (u64::from(hash) >> shift) as usize
}

/// Each regular file `index` holds, at its position under the hash `hash` gives it, in hash order.
fn hashed_files(index: &Index, hash: impl Fn(&Entry<'_>) -> u32) -> Vec<HashedAt> {
    let mut hashed: Vec<HashedAt> = index
        .entries()
        .enumerate()
        .filter(|(_, entry)| entry.kind() == EntryKind::File)
        .filter_map(|(position, entry)| {
            Some(HashedAt {
                hash: hash(&entry),
                position: u32::try_from(position).ok()?,
            })
        })
        .collect();
    hashed.sort_unstable();
    hashed
}

fn hashed(table: &[HashedAt], hash: u32) -> &[HashedAt] {
    let first = table.partition_point(|at| at.hash < hash);
    let len = table[first..].partition_point(|at| at.hash == hash);
    &table[first..first + len]
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the table keeps the low half of the hash, and a lookup confirms each name it selects"
)]
fn name_hash(name: &str) -> u32 {
    FixedState::default().hash_one(name) as u32
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the table keeps the low half of the hash, and a lookup confirms each path it selects"
)]
fn folded_hash(path: &str) -> u32 {
    let mut hasher = FixedState::default().build_hasher();
    let mut folded = [0; 64];
    for chunk in path.as_bytes().chunks(folded.len()) {
        let folded = &mut folded[..chunk.len()];
        folded.copy_from_slice(chunk);
        folded.make_ascii_lowercase();
        hasher.write(folded);
    }
    hasher.finish() as u32
}

#[cfg(test)]
mod tests {
    use kartta_fs::BuildOptions;

    use super::*;

    /// The tables of `index` a repository of it builds when it is asked by name, in another case, or both.
    fn tables_of(index: &Index, (by_name, in_another_case): (bool, bool)) -> FileTables {
        let repository = IndexRepository::recording(index, FileTables::default());
        if by_name {
            assert_eq!(repository.files_named("absent.rs").count(), 0);
        }
        if in_another_case {
            assert_eq!(repository.files_ignoring_case("ABSENT.RS").count(), 0);
        }
        repository.into_tables()
    }

    fn write(root: &Path, path: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("create the directories");
        fs::write(path, "text").expect("write a file");
    }

    #[test]
    fn tables_brought_through_each_refresh_are_those_of_the_index_it_left() {
        for asked in [(true, false), (false, true), (true, true)] {
            let dir = tempfile::tempdir().expect("create temp dir");
            let root = dir.path();
            for path in [
                "a/b.rs", "a/c.rs", "d/e.rs", "f.rs", "g/h.rs", "g/i/j.rs", "z.rs",
            ] {
                write(root, path);
            }
            let mut index = Index::build(root, &BuildOptions::default())
                .expect("build the index")
                .index;
            let mut tables = tables_of(&index, asked);
            let mut refresh = |index: &mut Index, paths: &[&str]| {
                let outcome = index
                    .refresh_paths(paths.iter().copied(), None, None)
                    .expect("refresh the paths");
                tables = std::mem::take(&mut tables).refreshed(index, &outcome);
                assert_eq!(tables, tables_of(index, asked), "{asked:?} after {paths:?}");
                outcome.kept_positions()
            };

            fs::write(root.join("a/b.rs"), "edited").expect("edit a file");
            assert!(refresh(&mut index, &["a/b.rs", "g"]));
            fs::remove_file(root.join("a/c.rs")).expect("remove a file");
            write(root, "d/new.rs");
            write(root, "n/deep/F.rs");
            fs::remove_dir_all(root.join("g")).expect("remove a directory");
            assert!(!refresh(
                &mut index,
                &["a/c.rs", "d/new.rs", "n/deep/F.rs", "g"]
            ));
            write(root, "0.rs");
            assert!(!refresh(&mut index, &["0.rs", "z.rs", "zz.rs"]));
            write(root, "d/e2.rs");
            assert!(!refresh(&mut index, &[""]));
        }
    }

    #[test]
    fn every_file_is_found_in_another_case_across_the_table_s_buckets() {
        let dir = tempfile::tempdir().expect("create temp dir");
        for at in 0..300 {
            fs::write(dir.path().join(format!("File{at}.blk")), "").expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build the index")
            .index;
        let repository = IndexRepository::new(&index);

        for at in 0..300 {
            let found: Vec<String> = repository
                .files_ignoring_case(&format!("FILE{at}.BLK"))
                .collect();
            assert_eq!(found, [format!("File{at}.blk")]);
        }
        assert_eq!(repository.files_ignoring_case("File300.blk").count(), 0);
    }

    #[test]
    fn a_path_hashes_alike_in_any_ascii_case_past_one_buffer() {
        let path = "Game/Data/Units/".repeat(6);

        assert_eq!(folded_hash(&path), folded_hash(&path.to_ascii_uppercase()));
        assert_eq!(folded_hash(&path), folded_hash(&path.to_ascii_lowercase()));
        assert_ne!(folded_hash("a/b.blk"), folded_hash("a/c.blk"));
    }
}
