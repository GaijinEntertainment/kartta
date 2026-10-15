use std::borrow::Cow;
use std::cell::RefCell;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::policy::{EntryKind, ListedEntry, ListedStat, Origin};

// constraint: libc does not define it; the value is the one in the SDK's `sys/attr.h`
const ATTR_CMN_ERROR: u32 = 0x2000_0000;

const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

const BUFFER: usize = 16 * 1024;
const RETURNED_COMMON: usize = 4;
const RETURNED_FILE: usize = 16;
const FIRST_FIELD: usize = 24;

// constraint: `None` sends the directory back to `read_dir` whenever the kernel cannot complete the listing with
// attributes — a call that fails, or an entry reported with an error and so without its type — so a walk asked for
// stats records the tree a walk without them records
pub(crate) fn list(native_path: &Path) -> io::Result<Option<Vec<ListedEntry>>> {
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(native_path)?;
    let mut asked = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS
            | ATTR_CMN_ERROR
            | libc::ATTR_CMN_NAME
            | libc::ATTR_CMN_OBJTYPE
            | libc::ATTR_CMN_CHGTIME,
        volattr: 0,
        dirattr: 0,
        fileattr: libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };
    let mut entries = Vec::new();

    let listed = LISTED.with_borrow_mut(|buffer| {
        loop {
            // SAFETY: `asked` and `buffer` outlive the call, which writes at most `buffer.len()` bytes into `buffer`,
            // and the descriptor stays open while `dir` lives.
            let count = unsafe {
                libc::getattrlistbulk(
                    dir.as_raw_fd(),
                    (&raw mut asked).cast(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    0,
                )
            };
            match usize::try_from(count) {
                Ok(0) => return Some(()),
                Ok(count) => read_groups(buffer, count, &mut entries)?,
                Err(_) => return None,
            }
        }
    });

    Ok(listed.map(|()| entries))
}

// constraint: the buffer outlives each listing, because one allocated for each directory slowed every listing call by
// the pages the kernel had to fault in to fill it
thread_local! {
    static LISTED: RefCell<Box<[u8]>> = RefCell::new(vec![0; BUFFER].into_boxed_slice());
}

fn read_groups(buffer: &[u8], count: usize, entries: &mut Vec<ListedEntry>) -> Option<()> {
    let mut at = 0;
    for _ in 0..count {
        let len = usize::try_from(Fields::at(buffer, at).word()?).ok()?;
        let group = buffer.get(at..at.checked_add(len)?)?;
        entries.push(entry(group)?);
        at += len;
    }

    Some(())
}

fn entry(group: &[u8]) -> Option<ListedEntry> {
    let common = Fields::at(group, RETURNED_COMMON).word()?;
    let file = Fields::at(group, RETURNED_FILE).word()?;
    let mut fields = Fields::at(group, FIRST_FIELD);

    if common & ATTR_CMN_ERROR != 0 && fields.word()? != 0 {
        return None;
    }
    if common & libc::ATTR_CMN_NAME == 0 || common & libc::ATTR_CMN_OBJTYPE == 0 {
        return None;
    }
    let raw = fields.name()?;
    let kind = kind(fields.word()?);
    let changed = if common & libc::ATTR_CMN_CHGTIME == 0 {
        None
    } else {
        Some((fields.long()?, fields.long()?))
    };
    let len = if file & libc::ATTR_FILE_DATALENGTH == 0 {
        None
    } else {
        Some(fields.long()?)
    };

    Some(listed(raw, kind, stat(kind, changed, len)))
}

fn stat(kind: EntryKind, changed: Option<(i64, i64)>, len: Option<i64>) -> Option<ListedStat> {
    let ((ctime, ctime_nsec), len) = changed.zip(len)?;

    (kind == EntryKind::File).then_some(ListedStat {
        size: u64::try_from(len).ok()?,
        ctime,
        ctime_nsec,
    })
}

fn listed(raw: &[u8], kind: EntryKind, stat: Option<ListedStat>) -> ListedEntry {
    let name = String::from_utf8_lossy(raw);
    let raw_name = matches!(name, Cow::Owned(_)).then(|| OsStr::from_bytes(raw).into());

    ListedEntry {
        name: name.into(),
        origin: Origin::Bulk {
            kind,
            stat,
            raw_name,
        },
    }
}

const fn kind(object_type: u32) -> EntryKind {
    match object_type {
        VREG => EntryKind::File,
        VDIR => EntryKind::Dir,
        VLNK => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

struct Fields<'a> {
    group: &'a [u8],
    at: usize,
}

impl<'a> Fields<'a> {
    const fn at(group: &'a [u8], at: usize) -> Self {
        Self { group, at }
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let bytes = self.group.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(bytes)
    }

    fn word(&mut self) -> Option<u32> {
        self.take().map(u32::from_ne_bytes)
    }

    fn long(&mut self) -> Option<i64> {
        self.take().map(i64::from_ne_bytes)
    }

    // constraint: a name is an `attrreference_t` — an offset from the reference itself, then a length that counts the
    // terminating NUL
    fn name(&mut self) -> Option<&'a [u8]> {
        let reference = self.at;
        let offset = usize::try_from(self.word()?).ok()?;
        let len = usize::try_from(self.word()?).ok()?;
        let start = reference.checked_add(offset)?;

        self.group.get(start..start + len.checked_sub(1)?)
    }
}
