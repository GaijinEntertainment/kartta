use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use memmap2::Mmap;

use super::ContentIndexStorage;

pub(super) enum Blob {
    Memory(Box<[u8]>),
    File { file: File, len: usize },
    Mapped(Mmap),
}

impl Blob {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Memory(bytes) => bytes.len(),
            Self::File { len, .. } => *len,
            Self::Mapped(map) => map.len(),
        }
    }

    pub(super) fn bytes(&self, range: Range<usize>) -> Option<Cow<'_, [u8]>> {
        match self {
            Self::Memory(bytes) => bytes.get(range).map(Cow::Borrowed),
            Self::Mapped(map) => map.get(range).map(Cow::Borrowed),
            Self::File { file, len } => {
                if range.start > range.end || range.end > *len {
                    return None;
                }
                let mut bytes = vec![0; range.len()];
                read_exact_at(file, &mut bytes, range.start as u64).ok()?;
                Some(Cow::Owned(bytes))
            }
        }
    }

    pub(super) const fn kind(&self) -> &'static str {
        match self {
            Self::Memory(_) => "memory",
            Self::File { .. } => "file",
            Self::Mapped(_) => "mapped",
        }
    }
}

// constraint: the index is written by position, never through a writable mapping: a mapping keeps every page it wrote in
// the process's resident set, and a full filesystem kills the process with SIGBUS where a write returns an error
pub(super) enum Writing {
    Memory(Box<[u8]>),
    Scratch {
        file: File,
        len: usize,
        mapped: bool,
    },
}

impl Writing {
    pub(super) fn new(storage: &ContentIndexStorage, len: usize) -> io::Result<Self> {
        let (dir, mapped) = match storage {
            ContentIndexStorage::Memory => {
                return Ok(Self::Memory(vec![0; len].into_boxed_slice()));
            }
            ContentIndexStorage::File { dir } => (dir, false),
            ContentIndexStorage::Mapped { dir } => (dir, true),
        };

        Ok(Self::Scratch {
            file: scratch_file(dir)?,
            len,
            mapped,
        })
    }

    pub(super) fn region(&mut self) -> Region<'_> {
        match self {
            Self::Memory(bytes) => Region::Memory(bytes),
            Self::Scratch { file, .. } => Region::File { file, at: 0 },
        }
    }

    pub(super) fn finish(self) -> io::Result<Blob> {
        match self {
            Self::Memory(bytes) => Ok(Blob::Memory(bytes)),
            Self::Scratch {
                file, mapped: true, ..
            } => {
                // SAFETY: the file is this process's own scratch file, which no other process can find by name once
                // `scratch_file` returns it (see its constraint for the one moment a process of the same user could),
                // and this process writes it no more once it is mapped.
                Ok(Blob::Mapped(unsafe { Mmap::map(&file)? }))
            }
            Self::Scratch { file, len, .. } => Ok(Blob::File { file, len }),
        }
    }
}

pub(super) enum Region<'a> {
    Memory(&'a mut [u8]),
    File { file: &'a File, at: u64 },
}

impl Region<'_> {
    pub(super) fn split_to(&mut self, len: usize) -> Self {
        match self {
            Self::Memory(bytes) => {
                let (front, rest) = mem::take(bytes).split_at_mut(len);
                *bytes = rest;
                Self::Memory(front)
            }
            Self::File { file, at } => {
                let front = Self::File { file, at: *at };
                *at += len as u64;
                front
            }
        }
    }

    pub(super) fn write_at(&mut self, offset: usize, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Memory(into) => {
                into[offset..offset + bytes.len()].copy_from_slice(bytes);
                Ok(())
            }
            Self::File { file, at } => write_all_at(file, bytes, *at + offset as u64),
        }
    }
}

pub(super) struct Spill {
    file: File,
    written: usize,
}

impl Spill {
    pub(super) fn create(dir: &Path) -> io::Result<Self> {
        Ok(Self {
            file: scratch_file(dir)?,
            written: 0,
        })
    }

    pub(super) fn append(&mut self, bytes: &[u8]) -> io::Result<Range<usize>> {
        self.file.write_all(bytes)?;
        let start = self.written;
        self.written += bytes.len();

        Ok(start..self.written)
    }

    pub(super) fn into_map(self) -> io::Result<Option<Mmap>> {
        if self.written == 0 {
            return Ok(None);
        }

        // SAFETY: the file is this process's own scratch file, which no other process can find by name once
        // `scratch_file` returns it (see its constraint for the one moment a process of the same user could), and this
        // process writes it no more once it is mapped.
        Ok(Some(unsafe { Mmap::map(&self.file)? }))
    }
}

// constraint: a scratch file is private to the process and leaves nothing behind however the process ends. Linux
// creates it with no name where the filesystem allows; elsewhere on unix it is unlinked the moment it is created, the
// one moment a process of the same user could open it; Windows opens it delete-on-close with no sharing.
fn scratch_file(dir: &Path) -> io::Result<File> {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[cfg(target_os = "linux")]
    if let Some(file) = open_unnamed(dir)? {
        return Ok(file);
    }

    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    loop {
        let name = format!(
            "kartta-content-index-{}-{started}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        match open_private(&dir.join(name)) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            opened => return opened,
        }
    }
}

// constraint: `O_TMPFILE` creates a file no name ever reaches; a filesystem without it refuses with one of these, and
// the named file then serves
#[cfg(target_os = "linux")]
fn open_unnamed(dir: &Path) -> io::Result<Option<File>> {
    use std::os::unix::fs::OpenOptionsExt;

    let opened = OpenOptions::new()
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_TMPFILE)
        .open(dir);
    match opened {
        Ok(file) => Ok(Some(file)),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::EOPNOTSUPP | libc::EISDIR | libc::EINVAL)
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn open_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    std::fs::remove_file(path)?;

    Ok(file)
}

#[cfg(windows)]
fn open_private(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
}

#[cfg(unix)]
fn read_exact_at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, bytes, offset)
}

#[cfg(unix)]
fn write_all_at(file: &File, bytes: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, bytes, offset)
}

#[cfg(windows)]
fn write_all_at(file: &File, mut bytes: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;

    while !bytes.is_empty() {
        match file.seek_write(bytes, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => {
                bytes = &bytes[written..];
                offset += written as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;

    while !bytes.is_empty() {
        match file.seek_read(bytes, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => {
                bytes = &mut bytes[read..];
                offset += read as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }

    Ok(())
}
