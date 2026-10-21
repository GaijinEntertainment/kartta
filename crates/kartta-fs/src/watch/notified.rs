use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use notify::{Config, EventKindMask, PathOp, RecommendedWatcher, UpdatePathsError, Watcher as _};

use super::{Found, WatchError, relative, start_error};
use crate::lock;

// constraint: notifications wait here while the caller walks the tree or refreshes it, so they are bounded; past this
// many the batch is lost, as an inotify queue that overflowed loses its own, and one poll takes no more than this many,
// so a stream that never stops still lets a wait close its batch
const QUEUED: usize = 65_536;

#[derive(Debug)]
pub(super) struct Backend {
    _watcher: RecommendedWatcher,
    queue: Arc<Queue>,
}

// constraint: a channel of `QUEUED` slots takes 2 MiB as the watcher starts, before any notification arrives, so they
// wait in a vector that grows as they come and that a poll takes whole under the lock that checks the bound, and no
// notification meets a full queue while fewer than `QUEUED` wait
#[derive(Debug, Default)]
struct Queue {
    held: Mutex<Held>,
    arrived: Condvar,
}

#[derive(Debug, Default)]
struct Held {
    signals: Vec<Signal>,
    overflowed: bool,
    ended: bool,
}

#[derive(Debug)]
enum Signal {
    Changed(Vec<PathBuf>),
    Lost,
}

impl Backend {
    pub(super) fn start(_root: &Path, prefixes: &[(PathBuf, String)]) -> Result<Self, WatchError> {
        let (handler, queue) = queue(prefixes);
        let config = Config::default().with_event_kinds(EventKindMask::CORE);
        let first = prefixes
            .first()
            .map_or_else(|| Path::new(""), |(path, _)| path.as_path());
        let mut notifier =
            RecommendedWatcher::new(handler, config).map_err(|error| start_error(first, error))?;
        // constraint: notify's FSEvents backend restarts its stream for each path a `watch` call adds, so every path
        // goes in at one call
        let paths = prefixes
            .iter()
            .map(|(path, _)| PathOp::watch_recursive(path.clone()));
        notifier.update_paths(paths.collect()).map_err(
            |UpdatePathsError { source, origin, .. }| {
                start_error(origin.as_ref().map_or(first, PathOp::as_path), source)
            },
        )?;

        Ok(Self {
            _watcher: notifier,
            queue,
        })
    }

    pub(super) fn poll(&self, timeout: Duration, found: &mut Found) -> Result<bool, WatchError> {
        let (mut held, _) = self
            .queue
            .arrived
            .wait_timeout_while(lock(&self.queue.held), timeout, |held| {
                held.signals.is_empty() && !held.ended
            })
            .unwrap_or_else(PoisonError::into_inner);
        if held.signals.is_empty() {
            return if held.ended {
                Err(WatchError::Stopped {
                    source: io::Error::other("the notification thread ended"),
                })
            } else {
                Ok(false)
            };
        }
        let signals = std::mem::take(&mut held.signals);
        found.lost |= std::mem::take(&mut held.overflowed);
        drop(held);
        for signal in signals {
            match signal {
                Signal::Changed(paths) => found.paths.extend(paths),
                Signal::Lost => found.lost = true,
            }
        }
        Ok(true)
    }

    #[expect(
        clippy::unused_self,
        reason = "one signature for every platform, and a poll takes as many notifications as the queue holds, so the \
                  first leaves none of those queued before it unread"
    )]
    pub(super) const fn queued(&self) -> usize {
        0
    }

    #[expect(
        clippy::unused_self,
        reason = "one signature for every platform, and the stream covers each directory from its start, so the walk \
                  has nothing to tell it"
    )]
    pub(super) const fn descending(&self) -> Option<fn(&Path)> {
        None
    }

    #[expect(
        clippy::unnecessary_wraps,
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "one signature for every platform, and the walk watched nothing, so no watch can have failed"
    )]
    pub(super) const fn descended(&mut self) -> Result<(), WatchError> {
        Ok(())
    }

    #[expect(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "one signature for every platform, and the stream holds no watch a directory to release"
    )]
    pub(super) const fn release(&mut self, _dirs: &[String]) {}
}

// constraint: notify names the directory to scan again, with everything below it, on a drop when the system names one,
// which `FSEvents` may name at the root or at a directory above it, and nothing when the whole watch lost track, as a
// Windows buffer that overflowed does; an error from the stream may have cost a change, so it counts as a loss too
fn signal(event: notify::Result<notify::Event>, prefixes: &[(PathBuf, String)]) -> Signal {
    match event {
        Ok(event) if rescans_whole_tree(&event, prefixes) => Signal::Lost,
        Ok(event) => Signal::Changed(event.paths),
        Err(_) => Signal::Lost,
    }
}

fn rescans_whole_tree(event: &notify::Event, prefixes: &[(PathBuf, String)]) -> bool {
    event.need_rescan()
        && (event.paths.is_empty()
            || event
                .paths
                .iter()
                .any(|path| relative(path, prefixes).is_none_or(|name| name.is_empty())))
}

struct Queuing {
    queue: Arc<Queue>,
    tree: Vec<(PathBuf, String)>,
}

impl notify::EventHandler for Queuing {
    fn handle_event(&mut self, event: notify::Result<notify::Event>) {
        let signal = signal(event, &self.tree);
        let mut held = lock(&self.queue.held);
        let was_empty = held.signals.is_empty();
        if held.signals.len() < QUEUED {
            held.signals.push(signal);
        } else {
            held.overflowed = true;
        }
        drop(held);
        if was_empty {
            self.queue.arrived.notify_one();
        }
    }
}

impl Drop for Queuing {
    fn drop(&mut self) {
        lock(&self.queue.held).ended = true;
        self.queue.arrived.notify_one();
    }
}

fn queue(prefixes: &[(PathBuf, String)]) -> (impl notify::EventHandler, Arc<Queue>) {
    let queue = Arc::new(Queue::default());
    let handler = Queuing {
        queue: Arc::clone(&queue),
        tree: prefixes.to_vec(),
    };

    (handler, queue)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Instant;

    use notify::event::{CreateKind, Flag};
    use notify::{Event, EventHandler, EventKind};

    use super::*;
    use crate::index::{Asked, Index};
    use crate::watch::{Watcher, event_prefixes, through_firmlinks, watch_walk};
    use crate::{BuildOptions, Content, ContentVisitor};

    const FIRMLINKS: &str = "/Users\tUsers\n";

    fn tree() -> Vec<(PathBuf, String)> {
        vec![(PathBuf::from("/parent/tree"), String::new())]
    }

    fn rescan() -> Event {
        Event::new(EventKind::Other).set_flag(Flag::Rescan)
    }

    fn rescan_of(path: &str, prefixes: &[(PathBuf, String)]) -> Signal {
        signal(Ok(rescan().add_path(PathBuf::from(path))), prefixes)
    }

    fn names(signal: &Signal, path: &str) -> bool {
        matches!(signal, Signal::Changed(paths) if *paths == [PathBuf::from(path)])
    }

    #[test]
    fn a_rescan_that_names_no_directory_loses_the_batch() {
        assert!(matches!(signal(Ok(rescan()), &tree()), Signal::Lost));
    }

    #[test]
    fn a_rescan_of_the_root_or_a_directory_above_it_loses_the_batch() {
        for path in ["/parent/tree", "/parent", "/"] {
            let signal = rescan_of(path, &tree());

            assert!(matches!(signal, Signal::Lost), "{path}: {signal:?}");
        }
    }

    #[test]
    fn a_rescan_that_names_a_directory_above_the_root_beside_one_below_it_loses_the_batch() {
        let event = rescan()
            .add_path(PathBuf::from("/parent/tree/src"))
            .add_path(PathBuf::from("/parent"));

        let signal = signal(Ok(event), &tree());

        assert!(matches!(signal, Signal::Lost), "{signal:?}");
    }

    #[test]
    fn a_rescan_of_a_directory_below_the_root_names_that_directory() {
        let signal = rescan_of("/parent/tree/src", &tree());

        assert!(names(&signal, "/parent/tree/src"), "{signal:?}");
    }

    #[test]
    fn a_rescan_of_a_firmlink_below_the_data_volume_names_that_firmlink() {
        let data_volume = through_firmlinks(Path::new("/System/Volumes/Data"), FIRMLINKS);

        let signal = rescan_of("/Users", &data_volume);

        assert!(names(&signal, "/Users"), "{signal:?}");
        for path in ["/System/Volumes/Data", "/System/Volumes", "/"] {
            let signal = rescan_of(path, &data_volume);

            assert!(matches!(signal, Signal::Lost), "{path}: {signal:?}");
        }
    }

    #[test]
    fn a_rescan_above_a_root_inside_a_firmlink_loses_the_batch() {
        let project = through_firmlinks(
            Path::new("/System/Volumes/Data/Users/me/project"),
            FIRMLINKS,
        );

        for path in ["/Users/me", "/Users", "/System/Volumes/Data/Users/me"] {
            let signal = rescan_of(path, &project);

            assert!(matches!(signal, Signal::Lost), "{path}: {signal:?}");
        }
    }

    #[test]
    fn a_change_names_its_path_inside_the_root_or_outside_it() {
        for path in ["/parent/tree/a.rs", "/parent/other.rs"] {
            let event =
                Event::new(EventKind::Create(CreateKind::File)).add_path(PathBuf::from(path));

            let signal = signal(Ok(event), &tree());

            assert!(names(&signal, path), "{signal:?}");
        }
    }

    #[test]
    fn a_change_under_each_path_a_root_is_watched_through_is_handed_over() {
        let trees: Vec<_> = (0..3)
            .map(|_| tempfile::tempdir().expect("a tree"))
            .collect();
        let prefixes: Vec<(PathBuf, String)> = trees
            .iter()
            .enumerate()
            .map(|(at, tree)| {
                let mut named = event_prefixes(tree.path()).expect("the paths events name");
                let base = if at == 0 {
                    String::new()
                } else {
                    format!("linked{at}")
                };
                (named.swap_remove(0).0, base)
            })
            .collect();
        let mut watcher = Watcher {
            backend: Backend::start(trees[0].path(), &prefixes).expect("start watching"),
            prefixes,
        };

        for tree in &trees {
            std::fs::write(tree.path().join("a.rs"), "a").expect("write a file");
        }

        let wanted = ["a.rs", "linked1/a.rs", "linked2/a.rs"];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = BTreeSet::new();
        while Instant::now() < deadline && !wanted.iter().all(|path| seen.contains(*path)) {
            seen.extend(watcher.wait(Duration::from_millis(500)).expect("wait"));
        }
        assert!(wanted.iter().all(|path| seen.contains(*path)), "{seen:?}");
    }

    #[test]
    fn an_error_from_the_stream_loses_the_batch() {
        assert!(matches!(
            signal(Err(notify::Error::generic("stream failed")), &tree()),
            Signal::Lost
        ));
    }

    struct OverflowsMidWalk<H> {
        root: PathBuf,
        handler: Mutex<H>,
    }

    impl<H: EventHandler> ContentVisitor for OverflowsMidWalk<H> {
        fn wants(&self, _path: &str) -> bool {
            let mut handler = self.handler.lock().expect("the handler");
            for at in 0..=QUEUED {
                let made = Event::new(EventKind::Create(CreateKind::File))
                    .add_path(self.root.join(format!("flood/{at}.rs")));
                handler.handle_event(Ok(made));
            }
            false
        }

        fn visit(&self, _content: Content<'_>) {}
    }

    #[test]
    fn more_notifications_than_the_queue_holds_while_the_build_walks_lose_the_batch() {
        let dir = tempfile::tempdir().expect("a tree");
        std::fs::write(dir.path().join("a.rs"), "a").expect("write a file");
        let prefixes = event_prefixes(dir.path()).expect("the paths events name");
        let (handler, queue) = queue(&prefixes);
        let hook = OverflowsMidWalk {
            root: prefixes[0].0.clone(),
            handler: Mutex::new(handler),
        };
        let unwatched = RecommendedWatcher::new(|_: notify::Result<Event>| {}, Config::default())
            .expect("a watcher");
        let watcher = Watcher {
            backend: Backend {
                _watcher: unwatched,
                queue,
            },
            prefixes,
        };

        let (_, watcher) = watch_walk(Ok(watcher), Asked::new(Some(&hook), None), |asked| {
            Index::build_walking(dir.path(), &BuildOptions::default(), asked)
        })
        .expect("build the index");
        let handed = watcher.expect("a watcher").drain().expect("drain");

        assert_eq!(handed, [""]);
    }

    #[test]
    fn a_queue_filled_to_its_bound_keeps_its_batch_and_room_for_the_next_once_read() {
        let (mut handler, queue) = queue(&tree());
        let unwatched = RecommendedWatcher::new(|_: notify::Result<Event>| {}, Config::default())
            .expect("a watcher");
        let backend = Backend {
            _watcher: unwatched,
            queue,
        };
        let made = |at: usize| {
            let path = PathBuf::from(format!("/parent/tree/{at}.rs"));
            Ok(Event::new(EventKind::Create(CreateKind::File)).add_path(path))
        };

        for at in 0..QUEUED {
            handler.handle_event(made(at));
        }
        let mut full = Found::default();
        backend.poll(Duration::ZERO, &mut full).expect("poll");
        handler.handle_event(made(QUEUED));
        let mut next = Found::default();
        backend.poll(Duration::ZERO, &mut next).expect("poll");

        assert_eq!((full.paths.len(), full.lost), (QUEUED, false));
        assert_eq!((next.paths.len(), next.lost), (1, false));
    }

    #[test]
    fn a_poll_once_the_notifications_stopped_coming_stops_the_watch() {
        let (handler, queue) = queue(&tree());
        let unwatched = RecommendedWatcher::new(|_: notify::Result<Event>| {}, Config::default())
            .expect("a watcher");
        let backend = Backend {
            _watcher: unwatched,
            queue,
        };

        drop(handler);
        let polled = backend.poll(Duration::from_secs(10), &mut Found::default());

        assert!(matches!(polled, Err(WatchError::Stopped { .. })));
    }

    /// Whether the thread `thread` waits, as the kernel reports its run state.
    #[cfg(target_os = "macos")]
    fn waiting(thread: libc::mach_port_t) -> bool {
        let mut info = std::mem::MaybeUninit::<libc::thread_basic_info>::zeroed();
        let mut count = libc::THREAD_BASIC_INFO_COUNT;
        // SAFETY: the buffer holds a whole thread_basic_info, as the count says, and both outlive the call.
        let asked = unsafe {
            libc::thread_info(
                thread,
                libc::THREAD_BASIC_INFO.unsigned_abs(),
                info.as_mut_ptr().cast(),
                &raw mut count,
            )
        };
        assert_eq!(asked, libc::KERN_SUCCESS, "ask for the thread's state");
        // SAFETY: the call succeeded, so it wrote the whole struct.
        unsafe { info.assume_init() }.run_state == libc::TH_STATE_WAITING
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_poll_that_waits_for_a_notification_wakes_when_one_arrives() {
        let (mut handler, queue) = queue(&tree());
        let unwatched = RecommendedWatcher::new(|_: notify::Result<Event>| {}, Config::default())
            .expect("a watcher");
        let backend = Backend {
            _watcher: unwatched,
            queue,
        };
        let (naming, names) = std::sync::mpsc::channel();
        let made = Event::new(EventKind::Create(CreateKind::File))
            .add_path(PathBuf::from("/parent/tree/a.rs"));

        let (polled, found, took) = std::thread::scope(|scope| {
            let polling = scope.spawn(|| {
                // SAFETY: pthread_self takes nothing and names the calling thread.
                let this = unsafe { libc::pthread_self() };
                // SAFETY: `this` is the calling thread, which runs for the whole call.
                let thread = unsafe { libc::pthread_mach_thread_np(this) };
                naming.send(thread).expect("name the polling thread");
                let started = Instant::now();
                let mut found = Found::default();
                let polled = backend.poll(Duration::from_secs(10), &mut found);
                (polled, found, started.elapsed())
            });
            let thread = names.recv().expect("the polling thread");
            while !waiting(thread) {
                std::hint::spin_loop();
            }
            handler.handle_event(Ok(made));
            polling.join().expect("poll")
        });

        assert!(matches!(polled, Ok(true)));
        assert_eq!(found.paths, [PathBuf::from("/parent/tree/a.rs")]);
        assert!(took < Duration::from_secs(10), "{took:?}");
    }
}
