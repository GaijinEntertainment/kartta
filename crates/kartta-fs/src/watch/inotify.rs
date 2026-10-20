use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use kartta_walk::EntryKind;

use super::{Found, WatchError, start_error};
use crate::index::Index;
use crate::lock;
use crate::policy::pruned;
use crate::pruned::PrunedNames;

// constraint: no access events, so the reads of a refresh raise none; a write is seen at every write and at the close
const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MODIFY
    | libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_ONLYDIR
    | libc::IN_DONT_FOLLOW
    | libc::IN_EXCL_UNLINK;

const HEADER: usize = size_of::<libc::inotify_event>();

#[derive(Debug)]
pub(super) struct Backend {
    fd: OwnedFd,
    root: PathBuf,
    pruned_names: PrunedNames,
    dirs: HashMap<i32, PathBuf>,
    walked: Mutex<Walked>,
    unwatched: AtomicUsize,
    refused: Mutex<Option<WatchError>>,
    moved_from: Option<(u32, PathBuf)>,
    buffer: Box<[u8]>,
}

impl Backend {
    pub(super) fn start(
        root: &Path,
        _prefixes: &[(PathBuf, String)],
        pruned_names: &PrunedNames,
    ) -> Result<Self, WatchError> {
        let backend = Self::opened(root, pruned_names)?;
        backend.descend(root);
        Ok(backend)
    }

    pub(super) fn from_index(index: &Index) -> Result<Self, WatchError> {
        let root = index.root().to_path_buf();
        let mut backend = Self::opened(&root, index.pruned_names())?;
        backend.add_at_start(&root, || 1 + dirs_from(index, 0))?;
        let mut paths = index.paths();
        for at in (0..index.len()).filter(|&at| index.kind_at(at) == EntryKind::Dir) {
            backend.add_at_start(&root.join(paths.get(at)), || dirs_from(index, at))?;
        }
        Ok(backend)
    }

    fn opened(root: &Path, pruned_names: &PrunedNames) -> Result<Self, WatchError> {
        // SAFETY: inotify_init1 takes no pointers; a negative return is a failure and anything else is a descriptor
        // nothing else holds.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(start_error(root, io::Error::last_os_error()));
        }
        // SAFETY: `raw` was just opened by inotify_init1 and is owned by nothing else.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        Ok(Self {
            fd,
            root: root.to_path_buf(),
            pruned_names: pruned_names.clone(),
            dirs: HashMap::new(),
            walked: Mutex::default(),
            unwatched: AtomicUsize::new(0),
            refused: Mutex::default(),
            moved_from: None,
            buffer: vec![0; 64 * 1024].into_boxed_slice(),
        })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "one signature for every platform, and a stream on macOS or Windows needs no word from the walk"
    )]
    pub(super) fn descending(&self) -> Option<impl Fn(&Path) + Sync + '_> {
        Some(|dir: &Path| self.descend(dir))
    }

    // constraint: the walk lists a directory only once its parent's visit kept it, which is when this runs, so whatever
    // is made in the directory after its listing raises an event, and whatever was made before is in the listing
    fn descend(&self, dir: &Path) {
        if self.unwatched.load(Ordering::Relaxed) > 0 {
            self.unwatched.fetch_add(1, Ordering::Relaxed);
            return;
        }
        match self.watch(dir) {
            Ok(wd) => lock(&self.walked).push(wd, dir),
            Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                self.unwatched.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) if gone(&error) || error.raw_os_error() == Some(libc::EACCES) => {}
            Err(error) => {
                lock(&self.refused).get_or_insert_with(|| start_error(dir, error));
            }
        }
    }

    pub(super) fn descended(&mut self) -> Result<(), WatchError> {
        std::mem::take(held(&mut self.walked)).into_dirs(&mut self.dirs);
        let watched = self.dirs.len();
        let unwatched = *self.unwatched.get_mut();
        if unwatched > 0 {
            return Err(WatchError::Limit {
                watched,
                wanted: watched + unwatched,
            });
        }
        held(&mut self.refused).take().map_or(Ok(()), Err)
    }

    fn add_at_start(
        &mut self,
        dir: &Path,
        unwatched: impl FnOnce() -> usize,
    ) -> Result<(), WatchError> {
        match self.add(dir) {
            Ok(_) => Ok(()),
            Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => Err(WatchError::Limit {
                watched: self.dirs.len(),
                wanted: self.dirs.len() + unwatched(),
            }),
            Err(error) if gone(&error) => Ok(()),
            Err(error) => Err(start_error(dir, error)),
        }
    }

    pub(super) fn poll(
        &mut self,
        timeout: Duration,
        found: &mut Found,
    ) -> Result<bool, WatchError> {
        let mut ready = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: one pollfd, alive for the whole call.
        let polled = unsafe { libc::poll(&raw mut ready, 1, millis) };
        if polled < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(false)
            } else {
                Err(WatchError::Stopped { source: error })
            };
        }
        if polled == 0 {
            return Ok(false);
        }

        // constraint: one buffer a call, so a writer faster than the reads cannot keep a wait from closing its batch
        let read = match self.read_once() {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(WatchError::Stopped { source: error }),
        };
        let mut made = Vec::new();
        self.take(read, found, &mut made);
        self.follow(made)?;
        Ok(read > 0)
    }

    pub(super) fn cover(&mut self, index: &Index, found: &mut Found) -> Result<(), WatchError> {
        let mut paths = index.paths();
        for at in (0..index.len()).filter(|&at| index.kind_at(at) == EntryKind::Dir) {
            let dir = self.root.join(paths.get(at));
            let within_limit = match self.add(&dir) {
                Ok(true) => self
                    .follow(subdirs(&dir, &self.pruned_names).collect())
                    .is_ok(),
                Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => false,
                Err(error) if !gone(&error) => return Err(start_error(&dir, error)),
                Ok(false) | Err(_) => continue,
            };
            if !within_limit {
                return Err(WatchError::Limit {
                    watched: self.dirs.len(),
                    wanted: 1 + dirs_from(index, 0),
                });
            }
            found.paths.push(dir);
        }
        Ok(())
    }

    fn read_once(&mut self) -> io::Result<usize> {
        loop {
            // SAFETY: the buffer is writable for its whole length, and the descriptor is this backend's own.
            let read = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    self.buffer.as_mut_ptr().cast(),
                    self.buffer.len(),
                )
            };
            if let Ok(read) = usize::try_from(read) {
                return Ok(read);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn take(&mut self, len: usize, found: &mut Found, made: &mut Vec<PathBuf>) {
        let mut at = 0;
        while at + HEADER <= len {
            // SAFETY: the kernel hands over whole events, so a header lies at `at` within the bytes it wrote, and an
            // unaligned read takes it wherever it starts.
            let event: libc::inotify_event =
                unsafe { std::ptr::read_unaligned(self.buffer[at..].as_ptr().cast()) };
            let name_end = (at + HEADER + event.len as usize).min(len);
            let name = &self.buffer[at + HEADER..name_end];
            let name = &name[..name
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(name.len())];
            at = name_end;

            if event.mask & libc::IN_Q_OVERFLOW != 0 {
                found.lost = true;
                continue;
            }
            if event.mask & libc::IN_IGNORED != 0 {
                self.dirs.remove(&event.wd);
                continue;
            }
            let Some(dir) = self.dirs.get(&event.wd) else {
                continue;
            };
            if name.is_empty() {
                let root_went = event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0
                    && *dir == self.root;
                found.lost |= root_went;
                continue;
            }

            let name = OsStr::from_bytes(name);
            let path = dir.join(name);
            let pruned = prunes(name, &self.pruned_names);
            if event.mask & libc::IN_ISDIR != 0 && self.follows(&event, &path, pruned) {
                made.push(path.clone());
            }
            found.paths.push(path);
        }
    }

    // constraint: a rename raises IN_MOVED_FROM and then IN_MOVED_TO under one cookie, and the watches inside the moved
    // directory keep the path it had until a follow of the new name re-points them; a pruned name is never followed, so
    // its watches are dropped instead of reporting changes under the old name
    fn follows(&mut self, event: &libc::inotify_event, path: &Path, pruned: bool) -> bool {
        if event.mask & libc::IN_MOVED_FROM != 0 {
            self.moved_from = Some((event.cookie, path.to_path_buf()));
        }
        if event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) == 0 {
            return false;
        }
        if !pruned {
            return true;
        }
        if event.mask & libc::IN_MOVED_TO != 0
            && let Some((_, from)) = self
                .moved_from
                .take_if(|(cookie, _)| *cookie == event.cookie)
        {
            self.unwatch_below(&from);
        }
        false
    }

    fn unwatch_below(&mut self, dir: &Path) {
        let fd = self.fd.as_raw_fd();
        self.dirs.retain(|&wd, path| {
            let below = path.starts_with(dir);
            if below {
                // SAFETY: the descriptor is this backend's own, and the kernel refuses a watch it no longer holds
                // without changing anything.
                unsafe { libc::inotify_rm_watch(fd, wd) };
            }
            !below
        });
    }

    // constraint: whatever is made in a new directory before its watch is added raises no event, so the watch goes on
    // before the refresh that names the directory walks it, and every directory already inside it is watched alike
    fn follow(&mut self, mut pending: Vec<PathBuf>) -> Result<(), WatchError> {
        while let Some(dir) = pending.pop() {
            match self.add(&dir) {
                Ok(_) => pending.extend(subdirs(&dir, &self.pruned_names)),
                Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                    return Err(WatchError::Limit {
                        watched: self.dirs.len(),
                        wanted: self.dirs.len() + pending.len() + 1,
                    });
                }
                Err(_) => {}
            }
        }
        Ok(())
    }

    // constraint: inotify_add_watch hands back the descriptor a directory already has, so a descriptor this backend
    // holds is a directory it watches, re-pointed here to the path it has now
    fn add(&mut self, dir: &Path) -> io::Result<bool> {
        let wd = self.watch(dir)?;
        Ok(self.dirs.insert(wd, dir.to_path_buf()).is_none())
    }

    fn watch(&self, dir: &Path) -> io::Result<i32> {
        let native = CString::new(dir.as_os_str().as_bytes())?;
        // SAFETY: `native` is NUL-terminated and outlives the call, and the descriptor is this backend's own.
        let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), native.as_ptr(), MASK) };
        if wd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(wd)
    }
}

// constraint: a path allocated on a walk thread stays in that thread's glibc arena among the pages the walk freed,
// which the trim after the build then cannot return, so the walk appends to these buffers and the caller's thread
// makes the map from them
#[derive(Debug, Default)]
struct Walked {
    wds: Vec<i32>,
    ends: Vec<usize>,
    bytes: Vec<u8>,
}

impl Walked {
    fn push(&mut self, wd: i32, dir: &Path) {
        self.wds.push(wd);
        self.bytes.extend_from_slice(dir.as_os_str().as_bytes());
        self.ends.push(self.bytes.len());
    }

    fn into_dirs(self, dirs: &mut HashMap<i32, PathBuf>) {
        dirs.reserve(self.wds.len());
        let mut start = 0;
        for (wd, end) in self.wds.into_iter().zip(self.ends) {
            dirs.insert(
                wd,
                PathBuf::from(OsStr::from_bytes(&self.bytes[start..end])),
            );
            start = end;
        }
    }
}

fn held<T>(mutex: &mut Mutex<T>) -> &mut T {
    mutex.get_mut().unwrap_or_else(PoisonError::into_inner)
}

fn dirs_from(index: &Index, from: usize) -> usize {
    (from..index.len())
        .filter(|&at| index.kind_at(at) == EntryKind::Dir)
        .count()
}

fn subdirs(dir: &Path, pruned_names: &PrunedNames) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !prunes(&entry.file_name(), pruned_names))
        .map(|entry| entry.path())
}

fn prunes(name: &OsStr, pruned_names: &PrunedNames) -> bool {
    name.to_str().is_some_and(|name| pruned(name, pruned_names))
}

fn gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_overflow_loses_the_batch() {
        let tree = tempfile::tempdir().expect("a tree");
        let mut backend =
            Backend::start(tree.path(), &[], &PrunedNames::builtin()).expect("start watching");
        let overflow = libc::inotify_event {
            wd: -1,
            mask: libc::IN_Q_OVERFLOW,
            cookie: 0,
            len: 0,
        };
        // SAFETY: the buffer holds 64 KiB, more than one event header, and an unaligned write puts the header at its
        // start whatever its alignment.
        unsafe { std::ptr::write_unaligned(backend.buffer.as_mut_ptr().cast(), overflow) };
        let mut found = Found::default();

        backend.take(HEADER, &mut found, &mut Vec::new());

        assert!(found.lost);
    }
}
