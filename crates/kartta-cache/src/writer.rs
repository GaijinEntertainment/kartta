use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::key::{self, ContentHash, Namespace};
use crate::pack::{self, Footer, IndexEntry, Published};

/// Gathers the entries one process computed and publishes them together as one immutable pack.
///
/// [`PackWriter::insert`] takes `&self`, so the threads of a build share one writer. Entries stream into a temporary
/// file in the store's directory as they arrive, so a writer holds its index in memory, not its payloads.
/// [`PackWriter::publish`] then finishes the file and links it into place under a name derived from its index: the
/// pack appears whole or not at all, and an existing file is never replaced, so a reader that mapped a pack is never
/// handed different bytes. A writer dropped without publishing removes its temporary file.
///
/// A key inserted twice keeps its first payload.
///
/// Every entry is stamped with the time the writer was made, which is when its age starts:
/// [`Store::merge`](crate::Store::merge) removes it once that age passes the time to live it is given.
#[derive(Debug)]
pub struct PackWriter {
    dir: PathBuf,
    published: Published,
    state: Mutex<Option<Writing>>,
}

#[derive(Debug)]
struct Writing {
    temp: PathBuf,
    file: BufWriter<File>,
    written: u64,
    entries: BTreeMap<Namespace, HashMap<ContentHash, Slot>>,
    failed: bool,
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    offset: u64,
    len: u32,
    check: u64,
    published: Published,
}

pub(crate) const TEMP_PREFIX: &str = ".tmp-";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const WRITE_BUFFER: usize = 1 << 20;

impl PackWriter {
    pub(crate) fn new(dir: &Path) -> Self {
        Self::published_at(dir, SystemTime::now())
    }

    pub(crate) fn published_at(dir: &Path, published: SystemTime) -> Self {
        Self {
            dir: dir.to_path_buf(),
            published: Published::at(published),
            state: Mutex::new(None),
        }
    }

    /// Adds `payload` under (`namespace`, `content`).
    ///
    /// # Errors
    ///
    /// Returns the error of creating or writing the temporary file, and [`io::ErrorKind::InvalidInput`] for a payload
    /// of 4 GiB or more, which a pack cannot hold. A failed write leaves part of a payload in the file, so the writer
    /// fails as a whole from then on: every later insert and [`PackWriter::publish`] return an error.
    pub fn insert(
        &self,
        namespace: &Namespace,
        content: &ContentHash,
        payload: &[u8],
    ) -> io::Result<()> {
        let len = u32::try_from(payload.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "a payload of 4 GiB or more")
        })?;
        let record = IndexEntry {
            content: *content,
            offset: 0,
            len,
            check: key::check(namespace, content, payload),
            published: self.published,
        };
        self.insert_record(namespace, record, payload)
    }

    pub(crate) fn insert_record(
        &self,
        namespace: &Namespace,
        record: IndexEntry,
        payload: &[u8],
    ) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        match &mut *state {
            Some(writing) => writing.append(namespace, &record, payload),
            None => state
                .insert(Writing::start(&self.dir)?)
                .append(namespace, &record, payload),
        }
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, |writing| {
                writing.entries.values().map(HashMap::len).sum()
            })
    }

    /// Finishes the pack and links it into the store's directory, returning its path, or `None` when nothing was
    /// inserted.
    ///
    /// A pack's name is hashed over its index without the publication times, so a pack with the same name already in
    /// place holds the same entries over the same payloads and finding one counts as published. The entries then age
    /// from that pack's publication times, not from this writer's.
    ///
    /// # Errors
    ///
    /// Returns the error of writing, syncing or linking the pack, or the failure of an earlier insert. The temporary
    /// file is removed either way.
    pub fn publish(mut self) -> io::Result<Option<PathBuf>> {
        let Some(writing) = self
            .state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return Ok(None);
        };
        if writing.failed {
            writing.discard();
            return Err(failed_earlier());
        }
        let temp = writing.temp.clone();
        let published = writing.finish(&self.dir);
        let _ = fs::remove_file(&temp);

        published.map(Some)
    }
}

impl Drop for PackWriter {
    fn drop(&mut self) {
        if let Some(writing) = self
            .state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            writing.discard();
        }
    }
}

impl Writing {
    fn start(dir: &Path) -> io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(
            "{TEMP_PREFIX}{}-{nanos}-{sequence}",
            std::process::id()
        ));
        let file = File::options().write(true).create_new(true).open(&temp)?;
        // constraint: the lock is what tells a merge in another process that the file is still being written; on a
        // filesystem without locks the file is unprotected once unwritten for an hour, which costs only this publish
        let _ = file.try_lock();

        Ok(Self {
            temp,
            file: BufWriter::with_capacity(WRITE_BUFFER, file),
            written: 0,
            entries: BTreeMap::new(),
            failed: false,
        })
    }

    fn append(
        &mut self,
        namespace: &Namespace,
        record: &IndexEntry,
        payload: &[u8],
    ) -> io::Result<()> {
        if self.failed {
            return Err(failed_earlier());
        }
        let Entry::Vacant(slot) = self
            .entries
            .entry(*namespace)
            .or_default()
            .entry(record.content)
        else {
            return Ok(());
        };
        if let Err(error) = self.file.write_all(payload) {
            self.failed = true;
            return Err(error);
        }
        slot.insert(Slot {
            offset: self.written,
            len: record.len,
            check: record.check,
            published: record.published,
        });
        self.written += u64::from(record.len);

        Ok(())
    }

    fn discard(self) {
        let (file, _unwritten) = self.file.into_parts();
        drop(file);
        let _ = fs::remove_file(&self.temp);
    }

    fn finish(mut self, dir: &Path) -> io::Result<PathBuf> {
        let mut name = blake3::Hasher::new();
        let index_offset = self.written;
        let mut entry_count = 0_u64;
        let mut namespaces: Vec<(Namespace, u64, u64)> = Vec::new();
        for (namespace, slots) in self.entries {
            let mut records: Vec<IndexEntry> = slots
                .into_iter()
                .map(|(content, slot)| IndexEntry {
                    content,
                    offset: slot.offset,
                    len: slot.len,
                    check: slot.check,
                    published: slot.published,
                })
                .collect();
            records.sort_unstable_by_key(|record| record.content);
            let start = entry_count;
            for record in &records {
                let record = record.encode();
                self.file.write_all(&record)?;
                name.update(&record[..pack::NAMED_RECORD_LEN]);
            }
            entry_count += u64::try_from(records.len()).unwrap_or(u64::MAX);
            namespaces.push((namespace, start, entry_count));
        }
        let namespaces_offset =
            index_offset + entry_count * u64::try_from(pack::ENTRY_RECORD_LEN).unwrap_or(u64::MAX);
        for (namespace, start, end) in &namespaces {
            let record = pack::namespace_record(namespace, *start..*end);
            self.file.write_all(&record)?;
            name.update(&record);
        }
        let footer = Footer {
            namespace_count: u32::try_from(namespaces.len()).unwrap_or(u32::MAX),
            entry_count,
            index_offset,
            namespaces_offset,
        };
        self.file.write_all(&footer.encode())?;
        self.file
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()?;

        link(
            &self.temp,
            &dir.join(format!("{}.{}", name.finalize().to_hex(), pack::EXTENSION)),
        )
    }
}

fn failed_earlier() -> io::Error {
    io::Error::other(
        "an earlier insert failed to write its payload, so the pack's offsets cannot be trusted",
    )
}

// constraint: a hard link creates the final name only when it is free, on every platform, where a rename replaces
// an existing file on unix and fails on Windows while another process maps it
fn link(temp: &Path, published: &Path) -> io::Result<PathBuf> {
    match fs::hard_link(temp, published) {
        Ok(()) => Ok(published.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(published.to_path_buf()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_file(dir: &Path) -> PathBuf {
        fs::read_dir(dir)
            .expect("list the store")
            .map(|entry| entry.expect("an entry").path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(TEMP_PREFIX))
            })
            .expect("the writer's temporary file")
    }

    #[test]
    fn a_writer_dropped_unpublished_writes_none_of_its_buffer_into_the_file_it_removes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let writer = PackWriter::new(dir.path());
        writer
            .insert(
                &Namespace::new("test", b"1"),
                &ContentHash::of(b"a"),
                b"a payload",
            )
            .expect("insert");
        let temp = temporary_file(dir.path());
        let kept = dir.path().join("the temporary file's bytes");
        fs::hard_link(&temp, &kept).expect("link the temporary file");

        drop(writer);

        assert!(!temp.exists(), "the temporary file is removed");
        assert_eq!(
            fs::metadata(&kept).expect("the kept bytes").len(),
            0,
            "the buffered payload is discarded, not written"
        );
    }

    #[test]
    fn entries_arriving_across_namespaces_in_any_order_are_each_served_their_own_payload() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (first, second) = (
            Namespace::new("first", b"1"),
            Namespace::new("second", b"1"),
        );
        let writer = PackWriter::new(dir.path());
        for (namespace, content, payload) in [
            (&second, "b", "second b"),
            (&first, "a", "first a"),
            (&second, "a", "second a"),
            (&first, "b", "first b"),
            (&first, "a", "a later first a"),
        ] {
            writer
                .insert(
                    namespace,
                    &ContentHash::of(content.as_bytes()),
                    payload.as_bytes(),
                )
                .expect("insert");
        }

        writer.publish().expect("publish");
        let store = crate::Store::open(dir.path()).expect("open");

        for (namespace, content, payload) in [
            (&first, "a", "first a"),
            (&first, "b", "first b"),
            (&second, "a", "second a"),
            (&second, "b", "second b"),
        ] {
            assert_eq!(
                store.get(namespace, &ContentHash::of(content.as_bytes())),
                Some(payload.as_bytes())
            );
        }
        assert_eq!(store.stats().entries, 4);
    }
}
