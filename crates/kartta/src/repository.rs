use std::cell::RefCell;
use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;
use std::{fs, io, iter};

use foldhash::fast::{FixedState, FoldHasher};
use kartta_fs::{Entry, EntryKind, Index, ListOptions};
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
/// questions alone, and the files it read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Configuration {
    pub(crate) answers: u64,
    pub(crate) asked: Box<[u32]>,
    pub(crate) read: Box<[Box<str>]>,
}

enum Asking {
    File(Vec<u32>),
    Configuration {
        answers: FoldHasher<'static>,
        asked: Vec<u32>,
        read: Vec<Box<str>>,
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
        Some(Asking::Configuration {
            answers,
            asked,
            read,
        }) => {
            asked.push(self::asked(query, text));
            answers.write_u8(query as u8);
            answers.write(text.as_bytes());
            if matches!(query, Query::Read) {
                read.push(text.into());
            }
        }
        None => {}
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct HashedAt {
    hash: u32,
    position: u32,
}

struct FoldedPaths {
    by_hash: Box<[HashedAt]>,
    bucket_starts: Box<[u32]>,
    bucket_shift: u32,
}

const ENTRIES_PER_BUCKET_BITS: u32 = 6;
const MAX_BUCKET_BITS: u32 = 16;

impl FoldedPaths {
    fn new(index: &Index) -> Self {
        let by_hash = hashed_files(index, |entry| folded_hash(entry.path()));
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
    /// [`asked_while`] or [`configured_while`] runs on the asking thread.
    pub(crate) fn recording(index: &'i Index) -> Self {
        Self {
            records: true,
            ..Self::new(index)
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
            fs::read(self.index.root().join(path))
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
