use std::hash::BuildHasher;
use std::sync::OnceLock;
use std::{fs, io, iter};

use foldhash::fast::FixedState;
use kartta_fs::{EntryKind, Index, ListOptions};
use kartta_lang::{Repository, path};

pub(crate) struct IndexRepository<'i> {
    index: &'i Index,
    by_name: OnceLock<Box<[NamedAt]>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NamedAt {
    name_hash: u32,
    position: u32,
}

impl<'i> IndexRepository<'i> {
    pub(crate) const fn new(index: &'i Index) -> Self {
        Self {
            index,
            by_name: OnceLock::new(),
        }
    }

    fn by_name(&self) -> &[NamedAt] {
        self.by_name.get_or_init(|| {
            let mut by_name: Vec<NamedAt> = self
                .index
                .entries()
                .enumerate()
                .filter(|(_, entry)| entry.kind() == EntryKind::File)
                .filter_map(|(position, entry)| {
                    Some(NamedAt {
                        name_hash: name_hash(entry.name()),
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
        let by_name = self.by_name();
        let hash = name_hash(name);
        let first = by_name.partition_point(|named| named.name_hash < hash);
        let name = name.to_owned();
        let mut paths = self.index.paths();

        Box::new(
            by_name[first..]
                .iter()
                .take_while(move |named| named.name_hash == hash)
                .map(move |named| paths.get(named.position as usize).to_owned())
                .filter(move |found| path::base(found) == name),
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
    reason = "the table keeps the low half of the hash, and a lookup confirms each name it selects"
)]
fn name_hash(name: &str) -> u32 {
    FixedState::default().hash_one(name) as u32
}
