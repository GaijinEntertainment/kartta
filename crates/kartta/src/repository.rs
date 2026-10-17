use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;
use std::{fs, io, iter};

use foldhash::fast::FixedState;
use kartta_fs::{EntryKind, Index, ListOptions};
use kartta_lang::{Repository, path};

pub(crate) struct IndexRepository<'i> {
    index: &'i Index,
    by_name: OnceLock<Box<[HashedAt]>>,
    folded_paths: OnceLock<FoldedPaths>,
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
        let mut by_hash: Vec<HashedAt> = index
            .entries()
            .enumerate()
            .filter(|(_, entry)| entry.kind() == EntryKind::File)
            .filter_map(|(position, entry)| {
                Some(HashedAt {
                    hash: folded_hash(entry.path()),
                    position: u32::try_from(position).ok()?,
                })
            })
            .collect();
        by_hash.sort_unstable();

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
        }
    }

    fn by_name(&self) -> &[HashedAt] {
        self.by_name.get_or_init(|| {
            let mut by_name: Vec<HashedAt> = self
                .index
                .entries()
                .enumerate()
                .filter(|(_, entry)| entry.kind() == EntryKind::File)
                .filter_map(|(position, entry)| {
                    Some(HashedAt {
                        hash: name_hash(entry.name()),
                        position: u32::try_from(position).ok()?,
                    })
                })
                .collect();
            by_name.sort_unstable();
            by_name.into_boxed_slice()
        })
    }
}

impl Repository for IndexRepository<'_> {
    fn contains(&self, path: &str) -> bool {
        self.index
            .entry(path)
            .is_some_and(|entry| entry.kind() == EntryKind::File)
    }

    fn files_in_dir<'a>(&'a self, dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let dir = if dir == "." { "" } else { dir };
        let Ok(listing) = self.index.list(dir, &ListOptions::default().depth(1)) else {
            return Box::new(iter::empty());
        };

        Box::new(
            listing
                .into_items()
                .into_iter()
                .map(|listed| listed.entry())
                .filter(|entry| entry.kind() == EntryKind::File)
                .map(|entry| entry.path().to_owned()),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        let mut paths = self.index.paths();

        Box::new(
            hashed(self.by_name(), name_hash(&name))
                .iter()
                .map(move |named| paths.get(named.position as usize).to_owned())
                .filter(move |found| path::base(found) == name),
        )
    }

    fn files_ignoring_case<'a>(&'a self, path: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let path = path.to_owned();
        let mut paths = self.index.paths();
        let folded_paths = self
            .folded_paths
            .get_or_init(|| FoldedPaths::new(self.index));

        Box::new(
            folded_paths
                .hashed(folded_hash(&path))
                .iter()
                .map(move |folded| paths.get(folded.position as usize).to_owned())
                .filter(move |found| found.eq_ignore_ascii_case(&path)),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        if !self.contains(path) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} is not a regular file the build indexed"),
            ));
        }
        fs::read(self.index.root().join(path))
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the shift leaves at most 16 bits"
)]
fn bucket(hash: u32, shift: u32) -> usize {
    (u64::from(hash) >> shift) as usize
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
            .into_index();
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
