use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use memmap2::Mmap;

use crate::key::{self, ContentHash, Namespace};

pub(crate) const MAGIC: [u8; 8] = *b"KARTPACK";
pub(crate) const VERSION: u32 = 2;
pub(crate) const EXTENSION: &str = "pack";

pub(crate) const FOOTER_LEN: usize = 40;
pub(crate) const NAMESPACE_RECORD_LEN: usize = 48;
pub(crate) const ENTRY_RECORD_LEN: usize = 56;
pub(crate) const NAMED_RECORD_LEN: usize = 52;

pub(crate) struct Footer {
    pub(crate) namespace_count: u32,
    pub(crate) entry_count: u64,
    pub(crate) index_offset: u64,
    pub(crate) namespaces_offset: u64,
}

impl Footer {
    pub(crate) fn encode(&self) -> [u8; FOOTER_LEN] {
        let mut bytes = [0; FOOTER_LEN];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.namespace_count.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.entry_count.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.index_offset.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.namespaces_offset.to_le_bytes());

        bytes
    }

    fn decode(bytes: &[u8; FOOTER_LEN]) -> Option<Self> {
        let (magic, rest) = bytes.split_first_chunk::<8>()?;
        let (version, rest) = rest.split_first_chunk::<4>()?;
        let (namespace_count, rest) = rest.split_first_chunk::<4>()?;
        let (entry_count, rest) = rest.split_first_chunk::<8>()?;
        let (index_offset, rest) = rest.split_first_chunk::<8>()?;
        let (namespaces_offset, _) = rest.split_first_chunk::<8>()?;
        if *magic != MAGIC || u32::from_le_bytes(*version) != VERSION {
            return None;
        }

        Some(Self {
            namespace_count: u32::from_le_bytes(*namespace_count),
            entry_count: u64::from_le_bytes(*entry_count),
            index_offset: u64::from_le_bytes(*index_offset),
            namespaces_offset: u64::from_le_bytes(*namespaces_offset),
        })
    }
}

pub(crate) fn namespace_record(
    namespace: &Namespace,
    entries: Range<u64>,
) -> [u8; NAMESPACE_RECORD_LEN] {
    let mut record = [0; NAMESPACE_RECORD_LEN];
    record[..32].copy_from_slice(namespace.as_bytes());
    record[32..40].copy_from_slice(&entries.start.to_le_bytes());
    record[40..48].copy_from_slice(&entries.end.to_le_bytes());

    record
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IndexEntry {
    pub(crate) content: ContentHash,
    pub(crate) offset: u64,
    pub(crate) len: u32,
    pub(crate) check: u64,
    pub(crate) published: Published,
}

impl IndexEntry {
    pub(crate) fn encode(&self) -> [u8; ENTRY_RECORD_LEN] {
        let mut record = [0; ENTRY_RECORD_LEN];
        record[..32].copy_from_slice(self.content.as_bytes());
        record[32..40].copy_from_slice(&self.offset.to_le_bytes());
        record[40..44].copy_from_slice(&self.len.to_le_bytes());
        record[44..52].copy_from_slice(&self.check.to_le_bytes());
        record[52..56].copy_from_slice(&self.published.0.to_le_bytes());

        record
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Published(u32);

impl Published {
    pub(crate) fn at(time: SystemTime) -> Self {
        let seconds = time
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());

        Self(u32::try_from(seconds).unwrap_or(u32::MAX))
    }

    pub(crate) fn rounded_up(time: SystemTime) -> Self {
        let fraction = time
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());

        Self::at(time + Duration::from_secs(u64::from(fraction > 0)))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Aging {
    pub(crate) entries: u64,
    pub(crate) bytes: u64,
}

#[derive(Debug)]
pub(crate) struct Pack {
    path: PathBuf,
    map: Mmap,
    entries: Range<usize>,
    namespaces: Range<usize>,
}

impl Pack {
    pub(crate) fn open(path: &Path) -> io::Result<Option<Self>> {
        let file = File::open(path)?;
        if usize::try_from(file.metadata()?.len()).map_or(true, |len| len < FOOTER_LEN) {
            return Ok(None);
        }
        // SAFETY: a published pack is never written again — the writer links a finished file into place and nothing
        // in this crate opens one for writing — so the mapped bytes stay what they were when the pack was published.
        // A program outside this crate that truncates a pack breaks that protocol and is not defended against.
        let map = unsafe { Mmap::map(&file)? };

        Ok(Self::laid_out(&map)
            .map(|(entries, namespaces)| Self {
                path: path.to_path_buf(),
                map,
                entries,
                namespaces,
            })
            .filter(Self::ranges_tile_the_index))
    }

    // constraint: a pack whose namespace ranges do not cover its index in order is refused whole, because a merge that
    // read such a range as empty would publish without its entries and then remove the only copy of them
    fn ranges_tile_the_index(&self) -> bool {
        let mut next = 0;
        let mut previous: Option<&[u8]> = None;
        for record in self.namespace_records() {
            let Some(range) = entry_range(record) else {
                return false;
            };
            let ascending = previous.is_none_or(|previous| previous < &record[..32]);
            if !ascending || range.start != next || range.end <= range.start {
                return false;
            }
            next = range.end;
            previous = Some(&record[..32]);
        }

        next == self.entry_count()
    }

    fn laid_out(map: &[u8]) -> Option<(Range<usize>, Range<usize>)> {
        let footer_at = map.len().checked_sub(FOOTER_LEN)?;
        let footer = Footer::decode(map[footer_at..].first_chunk()?)?;
        let index_offset = usize::try_from(footer.index_offset).ok()?;
        let namespaces_offset = usize::try_from(footer.namespaces_offset).ok()?;
        let entries_len = usize::try_from(footer.entry_count)
            .ok()?
            .checked_mul(ENTRY_RECORD_LEN)?;
        let namespaces_len = usize::try_from(footer.namespace_count)
            .ok()?
            .checked_mul(NAMESPACE_RECORD_LEN)?;
        let laid_out = index_offset.checked_add(entries_len)? == namespaces_offset
            && namespaces_offset.checked_add(namespaces_len)? == footer_at;

        laid_out.then_some((
            index_offset..namespaces_offset,
            namespaces_offset..footer_at,
        ))
    }

    pub(crate) fn get(&self, namespace: &Namespace, content: &ContentHash) -> Option<&[u8]> {
        let entries = self.entry_records().get(self.namespace_range(namespace)?)?;
        let found = entries
            .binary_search_by(|record| record[..32].cmp(content.as_bytes()))
            .ok()?;

        self.payload(&entries[found])
            .filter(|payload| key::check(namespace, content, payload) == check_of(&entries[found]))
    }

    fn namespace_range(&self, namespace: &Namespace) -> Option<Range<usize>> {
        let records = self.namespace_records();
        let found = records
            .binary_search_by(|record| record[..32].cmp(namespace.as_bytes()))
            .ok()?;

        entry_range(&records[found])
    }

    pub(crate) fn entries_since(
        &self,
        since: Published,
    ) -> impl Iterator<Item = (Namespace, IndexEntry, &[u8])> {
        self.namespace_records().iter().flat_map(move |record| {
            let namespace = record.first_chunk().copied().map(Namespace::from_bytes);
            let entries = entry_range(record)
                .and_then(|range| self.entry_records().get(range))
                .unwrap_or_default();
            entries
                .iter()
                .filter(move |entry| published_of(entry) >= since)
                .filter_map(move |entry| self.checked_entry(namespace?, entry))
        })
    }

    fn checked_entry(
        &self,
        namespace: Namespace,
        entry: &[u8; ENTRY_RECORD_LEN],
    ) -> Option<(Namespace, IndexEntry, &[u8])> {
        let content = ContentHash::from_bytes(*entry.first_chunk()?);
        let payload = self.payload(entry)?;
        let decoded = IndexEntry {
            content,
            offset: 0,
            len: len_of(entry),
            check: check_of(entry),
            published: published_of(entry),
        };

        (key::check(&namespace, &content, payload) == decoded.check)
            .then_some((namespace, decoded, payload))
    }

    pub(crate) fn aged_before(&self, cutoff: Published) -> Aging {
        self.entry_records()
            .iter()
            .filter(|record| published_of(record) < cutoff)
            .fold(Aging::default(), |aging, record| Aging {
                entries: aging.entries + 1,
                bytes: aging.bytes + u64::from(len_of(record)),
            })
    }

    pub(crate) fn payload_bytes(&self) -> u64 {
        u64::try_from(self.entries.start).unwrap_or(u64::MAX)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn payload(&self, record: &[u8; ENTRY_RECORD_LEN]) -> Option<&[u8]> {
        let offset = usize::try_from(u64::from_le_bytes(*record[32..].first_chunk()?)).ok()?;
        let len = usize::try_from(u32::from_le_bytes(*record[40..].first_chunk()?)).ok()?;
        let end = offset.checked_add(len)?;

        (end <= self.entries.start).then(|| &self.map[offset..end])
    }

    fn entry_records(&self) -> &[[u8; ENTRY_RECORD_LEN]] {
        self.map[self.entries.clone()].as_chunks().0
    }

    fn namespace_records(&self) -> &[[u8; NAMESPACE_RECORD_LEN]] {
        self.map[self.namespaces.clone()].as_chunks().0
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len() / ENTRY_RECORD_LEN
    }
}

fn entry_range(record: &[u8; NAMESPACE_RECORD_LEN]) -> Option<Range<usize>> {
    let start = u64::from_le_bytes(*record[32..].first_chunk()?);
    let end = u64::from_le_bytes(*record[40..].first_chunk()?);

    Some(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
}

fn len_of(record: &[u8; ENTRY_RECORD_LEN]) -> u32 {
    record[40..]
        .first_chunk()
        .map_or(0, |len| u32::from_le_bytes(*len))
}

fn check_of(record: &[u8; ENTRY_RECORD_LEN]) -> u64 {
    record[44..]
        .first_chunk()
        .map_or(0, |check| u64::from_le_bytes(*check))
}

const fn published_of(record: &[u8; ENTRY_RECORD_LEN]) -> Published {
    match record.last_chunk() {
        Some(published) => Published(u32::from_le_bytes(*published)),
        None => Published(0),
    }
}
