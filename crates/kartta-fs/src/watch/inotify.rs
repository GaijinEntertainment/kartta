use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{Found, WatchError, start_error};
use crate::index::tree_order;
use crate::list::is_below;
use crate::lock;

// constraint: no access events and no change of attributes, so the reads of a refresh raise none; a write is seen at
// every write and at the close
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
// constraint: an event's name is a component of a path, and PATH_MAX bounds a path and its NUL, so no event is longer
const LONGEST_EVENT: usize = HEADER + libc::PATH_MAX.unsigned_abs() as usize;
const MOVED_DIR: u32 = libc::IN_MOVED_FROM | libc::IN_ISDIR;

#[derive(Debug)]
pub(super) struct Backend {
    fd: OwnedFd,
    root: PathBuf,
    dirs: HashMap<i32, PathBuf>,
    walked: Mutex<Walked>,
    unwatched: AtomicUsize,
    refused: Mutex<Option<WatchError>>,
    buffer: Box<[u8]>,
    most_queued: usize,
}

impl Backend {
    pub(super) fn start(root: &Path, _prefixes: &[(PathBuf, String)]) -> Result<Self, WatchError> {
        let backend = Self::opened(root)?;
        backend.descend(root);
        Ok(backend)
    }

    fn opened(root: &Path) -> Result<Self, WatchError> {
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
            dirs: HashMap::new(),
            walked: Mutex::default(),
            unwatched: AtomicUsize::new(0),
            refused: Mutex::default(),
            buffer: vec![0; 64 * 1024].into_boxed_slice(),
            most_queued: most_queued(),
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
    // is made in the directory after its listing raises an event, and whatever was made before is in the listing;
    // until the watch limit runs out, every directory a walk keeps takes a watch call, one already watched included,
    // since the kernel answers the watch the directory has; a directory gone before its watch, or one the user cannot
    // read, takes none, as the walk cannot list it either and the build reports it, and any other refusal is held for
    // the walk's end
    fn descend(&self, dir: &Path) {
        if self.unwatched.load(Ordering::Relaxed) > 0 {
            self.unwatched.fetch_add(1, Ordering::Relaxed);
            return;
        }
        match self.watch(dir) {
            Ok(wd) if self.dirs.get(&wd).is_some_and(|held| held == dir) => {}
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
        let started = Instant::now();
        let mut left = timeout;
        let polled = loop {
            let millis = i32::try_from(left.as_millis()).unwrap_or(i32::MAX);
            // SAFETY: one pollfd, alive for the whole call.
            let polled = unsafe { libc::poll(&raw mut ready, 1, millis) };
            if polled >= 0 {
                break polled;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(WatchError::Stopped { source: error });
            }
            left = timeout.saturating_sub(started.elapsed());
        };
        // constraint: one buffer a call, so a writer faster than the reads cannot keep a wait from closing its batch
        let read = if polled == 0 {
            0
        } else {
            match self.read_once() {
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => 0,
                Err(error) => return Err(WatchError::Stopped { source: error }),
            }
        };
        // constraint: a read hands over every queued event that fits, so one that leaves room for the longest event
        // found the queue empty, with every change queued before it read
        if read + LONGEST_EVENT <= self.buffer.len() {
            found.unread = 0;
        }
        self.take(read, found);
        Ok(read > 0)
    }

    // constraint: FIONREAD counts the queue by walking it under the lock every writer into the tree takes to queue an
    // event, so a drain reads up to the most events the queue holds instead, and stops once a read finds it empty
    pub(super) const fn queued(&self) -> usize {
        self.most_queued
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

    // constraint: a watched directory's own notices name nothing but the root's own deletion or move, which tells the
    // root gone; `IN_IGNORED`, the notice of a released watch, removes that watch from the map alone
    fn take(&mut self, len: usize, found: &mut Found) {
        let mut at = 0;
        while at + HEADER <= len {
            found.unread = found.unread.saturating_sub(1);
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
                found.root_went |= event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0
                    && *dir == self.root;
                continue;
            }

            let mut path = PathBuf::with_capacity(dir.as_os_str().len() + 1 + name.len());
            path.push(dir);
            path.push(OsStr::from_bytes(name));
            if event.mask & MOVED_DIR == MOVED_DIR {
                self.unwatch_below(&path);
            }
            found.paths.push(path);
        }
    }

    // constraint: a watch follows its directory wherever it moves and keeps the path it had, so the watches below a
    // directory moved away are dropped, and the walk of a refresh that keeps it where it went watches it again
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

    pub(super) fn release(&mut self, dirs: &[String]) {
        if dirs.is_empty() {
            return;
        }
        std::mem::take(held(&mut self.walked)).into_dirs(&mut self.dirs);
        let fd = self.fd.as_raw_fd();
        let root = self.root.as_os_str().as_bytes();
        let watched = self.dirs.len();
        self.dirs.retain(|&wd, path| {
            let released = path
                .as_os_str()
                .as_bytes()
                .strip_prefix(root)
                .map(|rest| rest.strip_prefix(b"/").unwrap_or(rest))
                .and_then(|rest| std::str::from_utf8(rest).ok())
                .is_some_and(|rest| at_or_below_one(dirs, rest));
            if released {
                // SAFETY: the descriptor is this backend's own, and the kernel refuses a watch it no longer holds
                // without changing anything.
                unsafe { libc::inotify_rm_watch(fd, wd) };
            }
            !released
        });
        // constraint: a table keeps its buckets as its watches go, so a release shrinks it to the size the watches it
        // keeps need
        if self.dirs.len() < watched {
            self.dirs.shrink_to_fit();
        }
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
        // constraint: a watch removed can leave a tombstone that takes its room, and a `reserve` past that room doubles
        // a table over half full even where the watches fit without the tombstones, so a map short of room is made anew
        if dirs.capacity() - dirs.len() < self.wds.len() {
            let mut rebuilt = HashMap::with_capacity(dirs.len() + self.wds.len());
            rebuilt.extend(dirs.drain());
            *dirs = rebuilt;
        }
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

fn at_or_below_one(dirs: &[String], path: &str) -> bool {
    let after = dirs.partition_point(|dir| tree_order(dir, path).is_le());
    after > 0 && (dirs[after - 1] == path || is_below(path, &dirs[after - 1]))
}

// constraint: an inotify instance takes `max_queued_events` as it starts, and holds that many events and the overflow
// marker at most; where the limit cannot be read, a drain reads until it finds the queue empty
fn most_queued() -> usize {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
        .ok()
        .and_then(|limit| limit.trim().parse::<usize>().ok())
        .map_or(usize::MAX, |limit| limit.saturating_add(1))
}

fn gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write as _;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::watch::{LONGEST, Watcher};
    use crate::{BuildError, BuildOptions, Index};

    #[test]
    fn a_queue_overflow_loses_the_batch() {
        let tree = tempfile::tempdir().expect("a tree");
        let mut backend = Backend::start(tree.path(), &[]).expect("start watching");
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

        backend.take(HEADER, &mut found);

        assert!(found.lost);
    }

    fn watched(tree: &Path) -> Watcher {
        Watcher::start(tree)
            .and_then(Watcher::walked)
            .expect("start watching")
    }

    /// What a drain of a watcher of a new tree answers once `went` took the tree away.
    fn drained_once_the_root(went: impl FnOnce(&Path, &Path)) -> Result<Vec<String>, WatchError> {
        let parent = tempfile::tempdir().expect("a parent");
        let tree = parent.path().join("tree");
        std::fs::create_dir_all(tree.join("src")).expect("make the tree");
        let mut watcher = watched(&tree);

        went(&tree, parent.path());
        watcher.drain()
    }

    #[test]
    fn the_root_deleted_or_moved_away_is_told_as_the_root_gone() {
        let deleted = drained_once_the_root(|tree, _| {
            std::fs::remove_dir_all(tree).expect("delete the tree");
        });
        let moved = drained_once_the_root(|tree, parent| {
            std::fs::rename(tree, parent.join("moved")).expect("move the tree away");
        });

        assert!(matches!(deleted, Err(WatchError::RootGone)), "{deleted:?}");
        assert!(matches!(moved, Err(WatchError::RootGone)), "{moved:?}");
    }

    #[test]
    fn a_path_a_read_takes_holds_its_own_bytes_and_no_more() {
        let tree = tempfile::tempdir().expect("a tree");
        let mut watcher = watched(tree.path());
        std::fs::write(tree.path().join("a"), "a").expect("write a file");
        let mut found = Found::default();

        watcher
            .backend
            .poll(Duration::ZERO, &mut found)
            .expect("read");

        let held: Vec<(usize, usize)> = found
            .paths
            .iter()
            .map(|path| (path.as_os_str().len(), path.capacity()))
            .collect();
        let len = tree.path().join("a").as_os_str().len();
        assert_eq!(held, [(len, len); 3]);
    }

    /// The watches the kernel holds for `watcher`, one line each in its descriptor's fdinfo.
    fn held_watches(watcher: &Watcher) -> usize {
        let fd = watcher.backend.fd.as_raw_fd();
        std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
            .expect("read the descriptor's fdinfo")
            .lines()
            .filter(|line| line.starts_with("inotify wd:"))
            .count()
    }

    /// Builds and watches a tree that ignores `target/`, then makes a tree of six directories under `target/` and takes
    /// in what the watcher hands over for it, as a consumer that refreshes under its watcher does.
    fn ignored_build_output_made_after_the_start()
    -> (tempfile::TempDir, Index, Watcher, Vec<String>) {
        let tree = tempfile::tempdir().expect("a tree");
        std::fs::write(tree.path().join(".gitignore"), "target/\n").expect("write a file");
        let (outcome, watcher) =
            Index::build_watched(tree.path(), &BuildOptions::default(), None, None).expect("build");
        let (mut index, mut watcher) = (outcome.index, watcher.expect("start watching"));

        for dir in ["target/debug/deps", "target/debug/build/x/out"] {
            std::fs::create_dir_all(tree.path().join(dir)).expect("make a directory");
        }
        let made = watcher.drain().expect("drain");
        refresh_under(&mut index, &mut watcher, &made);

        (tree, index, watcher, made)
    }

    fn refresh_under(index: &mut Index, watcher: &mut Watcher, changes: &[String]) {
        let (_, followed) = index
            .refresh_paths_watched(changes.iter().map(String::as_str), watcher, None, None)
            .expect("refresh");
        followed.expect("watch what the refresh took in");
    }

    #[test]
    fn a_tree_made_under_an_ignored_path_after_the_start_takes_no_watch() {
        let (tree, _, mut watcher, made) = ignored_build_output_made_after_the_start();

        std::fs::write(tree.path().join("target/debug/deps/a.rlib"), "").expect("write a file");
        let inside = watcher.drain().expect("drain");

        assert_eq!(made, ["target"]);
        assert_eq!(inside, Vec::<String>::new());
        assert_eq!(held_watches(&watcher), 1);
    }

    #[test]
    fn a_tree_a_gitignore_edit_shows_is_watched_and_followed_once_a_refresh_takes_it_in() {
        let (tree, mut index, mut watcher, _) = ignored_build_output_made_after_the_start();

        std::fs::write(tree.path().join(".gitignore"), "").expect("write a file");
        let shown = watcher.drain().expect("drain");
        refresh_under(&mut index, &mut watcher, &shown);
        std::fs::write(tree.path().join("target/debug/build/x/out/a.o"), "").expect("write a file");
        let inside = watcher.drain().expect("drain");
        refresh_under(&mut index, &mut watcher, &inside);

        assert_eq!(shown, [".gitignore"]);
        assert_eq!(held_watches(&watcher), 7);
        assert_eq!(inside, ["target/debug/build/x/out/a.o"]);
        assert!(index.entry("target/debug/build/x/out/a.o").is_some());
    }

    /// Builds and watches a tree of ten directories, the root included, under `target/` and `sub/out/`, and writes the
    /// `.gitignore` files of the root and of `sub/` with these texts, refreshing under the watcher what it hands over
    /// for each, as a consumer that refreshes under its watcher does.
    fn watched_then_ignoring(texts: &[(&str, &str)]) -> (tempfile::TempDir, Index, Watcher, usize) {
        let tree = tempfile::tempdir().expect("a tree");
        for dir in [
            "target/debug/deps",
            "target/debug/build/x/out",
            "sub/out/deep",
        ] {
            std::fs::create_dir_all(tree.path().join(dir)).expect("make a directory");
        }
        let (outcome, watcher) =
            Index::build_watched(tree.path(), &BuildOptions::default(), None, None).expect("build");
        let (mut index, mut watcher) = (outcome.index, watcher.expect("start watching"));
        let watched_at_the_start = held_watches(&watcher);

        for (gitignore, text) in texts {
            std::fs::write(tree.path().join(gitignore), text).expect("write a file");
            let edited = watcher.drain().expect("drain");
            refresh_under(&mut index, &mut watcher, &edited);
        }

        (tree, index, watcher, watched_at_the_start)
    }

    #[test]
    fn a_tree_a_gitignore_edit_hides_loses_its_watches_once_a_refresh_drops_it() {
        let (tree, _, mut watcher, watched_at_the_start) =
            watched_then_ignoring(&[(".gitignore", "target/\n"), ("sub/.gitignore", "out/\n")]);

        std::fs::write(tree.path().join("target/debug/deps/a.rlib"), "").expect("write a file");
        std::fs::write(tree.path().join("sub/out/deep/a.o"), "").expect("write a file");
        let inside = watcher.drain().expect("drain");

        assert_eq!(watched_at_the_start, 10);
        assert_eq!(held_watches(&watcher), 2);
        assert_eq!(inside, Vec::<String>::new());
    }

    #[test]
    fn a_tree_a_gitignore_edit_hid_and_a_later_edit_shows_is_watched_again() {
        let (tree, mut index, mut watcher, _) = watched_then_ignoring(&[
            (".gitignore", "target/\n"),
            ("sub/.gitignore", "out/\n"),
            (".gitignore", ""),
            ("sub/.gitignore", ""),
        ]);

        std::fs::write(tree.path().join("target/debug/deps/a.rlib"), "").expect("write a file");
        std::fs::write(tree.path().join("sub/out/deep/a.o"), "").expect("write a file");
        let inside = watcher.drain().expect("drain");
        refresh_under(&mut index, &mut watcher, &inside);

        assert_eq!(held_watches(&watcher), 10);
        assert_eq!(inside, ["sub/out/deep/a.o", "target/debug/deps/a.rlib"]);
        assert!(index.entry("sub/out/deep/a.o").is_some());
    }

    #[test]
    fn a_directory_moved_unread_from_a_path_a_refresh_drops_to_one_it_keeps_stays_watched() {
        let tree = tempfile::tempdir().expect("a tree");
        for dir in ["a/deep", "keep"] {
            std::fs::create_dir_all(tree.path().join(dir)).expect("make a directory");
        }
        let (outcome, watcher) =
            Index::build_watched(tree.path(), &BuildOptions::default(), None, None).expect("build");
        let (mut index, mut watcher) = (outcome.index, watcher.expect("start watching"));

        std::fs::write(tree.path().join(".gitignore"), "").expect("write a file");
        let edited = watcher.drain().expect("drain");
        std::fs::rename(tree.path().join("a"), tree.path().join("keep/b")).expect("rename");
        refresh_under(&mut index, &mut watcher, &edited);
        std::fs::write(tree.path().join("keep/b/deep/c.rs"), "").expect("write a file");
        let seen = watcher.drain().expect("drain");

        assert_eq!(held_watches(&watcher), 4);
        assert_eq!(seen, ["a", "keep/b", "keep/b/deep/c.rs"]);
    }

    /// The bytes `watcher`'s queue holds, as FIONREAD counts them.
    fn queued_bytes(watcher: &Watcher) -> usize {
        let mut queued: libc::c_int = 0;
        // SAFETY: FIONREAD writes one int through the pointer, alive for the whole call, and the descriptor is the
        // watcher's own.
        let asked = unsafe {
            libc::ioctl(
                watcher.backend.fd.as_raw_fd(),
                libc::FIONREAD,
                &raw mut queued,
            )
        };
        assert!(
            asked >= 0,
            "count the queue: {}",
            io::Error::last_os_error()
        );
        usize::try_from(queued).expect("a count")
    }

    /// Writes to two files of `tree` in turn, so the kernel merges none of the events, until `watcher` holds more than
    /// `bytes` of them queued.
    fn queue_past(watcher: &Watcher, tree: &Path, bytes: usize) {
        let mut files = ["a", "b"].map(|name| File::create(tree.join(name)).expect("make a file"));
        let mut at = 0;
        while queued_bytes(watcher) <= bytes {
            files[at % 2].write_all(b"x").expect("write a file");
            at += 1;
        }
    }

    #[test]
    fn a_drain_reads_past_its_bound_every_change_queued_at_its_call() {
        let tree = tempfile::tempdir().expect("a tree");
        let mut watcher = watched(tree.path());
        queue_past(&watcher, tree.path(), watcher.backend.buffer.len());
        File::create(tree.path().join("c")).expect("make a file");

        let handed = watcher.drain_within(Duration::ZERO).expect("drain");

        assert_eq!(handed, ["a", "b", "c"]);
    }

    #[test]
    fn a_drain_is_bounded_by_the_most_events_the_queue_can_hold() {
        let tree = tempfile::tempdir().expect("a tree");
        let watcher = watched(tree.path());
        let limit = std::fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
            .expect("read the queue's limit");
        let limit: usize = limit.trim().parse().expect("a limit");

        assert_eq!(watcher.backend.queued(), limit + 1);
    }

    #[test]
    fn a_drain_counts_its_bound_down_by_the_events_it_reads() {
        let tree = tempfile::tempdir().expect("a tree");
        let mut watcher = watched(tree.path());
        queue_past(&watcher, tree.path(), watcher.backend.buffer.len());
        let bound = watcher.backend.buffer.len();
        let mut found = Found {
            unread: bound,
            ..Found::default()
        };

        watcher
            .backend
            .poll(Duration::ZERO, &mut found)
            .expect("read");

        assert_eq!(found.unread, bound - found.paths.len());
    }

    #[test]
    fn a_read_that_leaves_room_for_the_longest_event_has_read_the_queue_empty() {
        let tree = tempfile::tempdir().expect("a tree");
        let mut watcher = watched(tree.path());
        queue_past(&watcher, tree.path(), 0);
        let mut found = Found {
            unread: usize::MAX,
            ..Found::default()
        };

        watcher
            .backend
            .poll(Duration::ZERO, &mut found)
            .expect("read");

        assert_eq!(found.unread, 0);
    }

    #[test]
    fn a_batch_closed_before_its_reads_cover_what_was_queued_is_lost() {
        let tree = tempfile::tempdir().expect("a tree");
        let watcher = watched(tree.path());
        let unread = Found {
            paths: vec![tree.path().join("a")],
            unread: 1,
            ..Found::default()
        };

        assert_eq!(watcher.changes(unread).expect("the root is there"), [""]);
    }

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    extern "C" fn note_interrupted(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }

    /// The system calls `libc::poll` makes.
    #[cfg(target_arch = "x86_64")]
    const POLLS: [libc::c_long; 2] = [libc::SYS_poll, libc::SYS_ppoll];
    #[cfg(not(target_arch = "x86_64"))]
    const POLLS: [libc::c_long; 1] = [libc::SYS_ppoll];

    /// Whether the thread `tid` of this process waits in a poll, as its `syscall` file in `/proc` names the call it is
    /// blocked in.
    fn polling(tid: libc::pid_t) -> bool {
        std::fs::read_to_string(format!("/proc/self/task/{tid}/syscall"))
            .expect("read the thread's system call")
            .split_whitespace()
            .next()
            .and_then(|number| number.parse().ok())
            .is_some_and(|number| POLLS.contains(&number))
    }

    #[test]
    fn a_drain_whose_poll_a_signal_interrupts_waits_out_the_time_left() {
        let tree = tempfile::tempdir().expect("a tree");
        let mut watcher = watched(tree.path());
        // SAFETY: a zeroed action takes no flags and blocks no signal while its handler runs.
        let mut noting: libc::sigaction = unsafe { std::mem::zeroed() };
        noting.sa_sigaction = note_interrupted as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SAFETY: the action is alive for the whole call, and its handler only stores to an atomic.
        let installed =
            unsafe { libc::sigaction(libc::SIGUSR1, &raw const noting, std::ptr::null_mut()) };
        assert_eq!(
            installed,
            0,
            "handle the signal: {}",
            io::Error::last_os_error()
        );
        let process = libc::pid_t::try_from(std::process::id()).expect("a process id");
        // SAFETY: gettid takes nothing and cannot fail.
        let draining = unsafe { libc::gettid() };
        let made = tree.path().join("a");
        let interrupter = std::thread::spawn(move || {
            while !polling(draining) {
                std::hint::spin_loop();
            }
            // SAFETY: the thread is this process's own, and waits in its poll until this one makes the file.
            unsafe { libc::syscall(libc::SYS_tgkill, process, draining, libc::SIGUSR1) };
            while !INTERRUPTED.load(Ordering::SeqCst) {
                std::hint::spin_loop();
            }
            File::create(made).expect("make a file");
        });
        let unread = Found {
            unread: 1,
            ..Found::default()
        };

        let drained = watcher.gather(Duration::from_secs(10), LONGEST, unread);
        interrupter.join().expect("interrupt the drain");

        assert_eq!(drained.expect("drain"), ["a"]);
    }

    #[test]
    fn a_refused_watch_a_failed_refresh_met_is_reported_by_the_next_drain() {
        let tree = tempfile::tempdir().expect("a tree");
        let (outcome, watcher) =
            Index::build_watched(tree.path(), &BuildOptions::default(), None, None).expect("build");
        let (mut index, mut watcher) = (outcome.index, watcher.expect("start watching"));
        let refusal = io::Error::from_raw_os_error(libc::EINVAL);
        *held(&mut watcher.backend.refused) = Some(start_error(tree.path(), refusal));

        let refreshed = index.refresh_paths_watched(["../outside"], &mut watcher, None, None);
        let drained = watcher.drain();

        assert!(matches!(refreshed, Err(BuildError::Misspelled { .. })));
        assert!(matches!(drained, Err(WatchError::Start { .. })));
    }

    /// Makes `a` under `root` and as many directories inside it as `a` and the root leave room for in a map of `full`.
    fn make_a(root: &Path, full: usize) {
        for at in 0..full - 2 {
            std::fs::create_dir_all(root.join("a").join(at.to_string())).expect("make a directory");
        }
    }

    /// Builds and watches a tree of `a` and the directories inside it, whose watches and the root's fill the watcher's
    /// map, then takes directories of the tree away and brings them back as `away` and `back` do, refreshing under the
    /// watcher what it hands over after each, as a consumer that refreshes under its watcher does. Returns the map's
    /// capacity before and after.
    fn map_capacity_around(
        away: impl FnOnce(&Path),
        back: impl FnOnce(&Path, usize),
    ) -> (usize, usize) {
        let tree = tempfile::tempdir().expect("a tree");
        let full = HashMap::<i32, PathBuf>::with_capacity(500).capacity();
        make_a(tree.path(), full);
        let (outcome, watcher) =
            Index::build_watched(tree.path(), &BuildOptions::default(), None, None).expect("build");
        let (mut index, mut watcher) = (outcome.index, watcher.expect("start watching"));
        let before = watcher.backend.dirs.capacity();

        away(tree.path());
        let gone = watcher.drain().expect("drain");
        refresh_under(&mut index, &mut watcher, &gone);
        back(tree.path(), full);
        let made = watcher.drain().expect("drain");
        refresh_under(&mut index, &mut watcher, &made);

        assert_eq!((before, watcher.backend.dirs.len()), (full, full));
        (before, watcher.backend.dirs.capacity())
    }

    #[test]
    fn a_watched_directory_deleted_and_made_again_leaves_the_watch_map_its_size() {
        let (before, after) = map_capacity_around(
            |tree| std::fs::remove_dir_all(tree.join("a")).expect("delete a directory"),
            make_a,
        );

        assert_eq!(after, before);
    }

    #[test]
    fn a_watched_directory_moved_away_and_back_leaves_the_watch_map_its_size() {
        let outside = tempfile::tempdir().expect("a directory outside the tree");
        let (before, after) = map_capacity_around(
            |tree| {
                std::fs::rename(tree.join("a"), outside.path().join("a"))
                    .expect("move a directory away");
            },
            |tree, _| {
                std::fs::rename(outside.path().join("a"), tree.join("a"))
                    .expect("move a directory back");
            },
        );

        assert_eq!(after, before);
    }

    #[test]
    fn watched_directories_hidden_and_shown_again_leave_the_watch_map_its_size() {
        let fewer_than_half_of_a = "a/1*/\n";
        let (before, after) = map_capacity_around(
            |tree| {
                std::fs::write(tree.join(".gitignore"), fewer_than_half_of_a)
                    .expect("hide directories");
            },
            |tree, _| std::fs::write(tree.join(".gitignore"), "").expect("show them again"),
        );

        assert_eq!(after, before);
    }
}
