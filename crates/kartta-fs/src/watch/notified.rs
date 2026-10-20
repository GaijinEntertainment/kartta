use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TrySendError};
use std::time::Duration;
use std::{io, iter};

use notify::{Config, EventKindMask, RecommendedWatcher, RecursiveMode, Watcher as _};

use super::{Found, WatchError, relative, start_error};
use crate::index::Index;

// constraint: notifications wait here while the caller refreshes, so they are bounded; past this many the batch is lost,
// as an inotify queue that overflowed loses its own, and one poll takes no more than this many, so a stream that never
// stops still lets a wait close its batch
const QUEUED: usize = 65_536;

#[derive(Debug)]
pub(super) struct Backend {
    _watcher: RecommendedWatcher,
    signals: Receiver<Signal>,
    overflowed: Arc<AtomicBool>,
}

#[derive(Debug)]
enum Signal {
    Changed(Vec<PathBuf>),
    Lost,
}

impl Backend {
    pub(super) fn start(
        _index: &Index,
        prefixes: &[(PathBuf, String)],
    ) -> Result<Self, WatchError> {
        let (sender, signals) = mpsc::sync_channel(QUEUED);
        let overflowed = Arc::new(AtomicBool::new(false));
        let overflowing = Arc::clone(&overflowed);
        let tree = prefixes.to_vec();
        let handler = move |event: notify::Result<notify::Event>| {
            if let Err(TrySendError::Full(_)) = sender.try_send(signal(event, &tree)) {
                overflowing.store(true, Ordering::Relaxed);
            }
        };
        let config = Config::default().with_event_kinds(EventKindMask::CORE);
        let first = prefixes
            .first()
            .map_or_else(|| Path::new(""), |(path, _)| path.as_path());
        let mut notifier =
            RecommendedWatcher::new(handler, config).map_err(|error| start_error(first, error))?;
        for (path, _) in prefixes {
            notifier
                .watch(path, RecursiveMode::Recursive)
                .map_err(|error| start_error(path, error))?;
        }

        Ok(Self {
            _watcher: notifier,
            signals,
            overflowed,
        })
    }

    pub(super) fn poll(&self, timeout: Duration, found: &mut Found) -> Result<bool, WatchError> {
        let first = match self.signals.recv_timeout(timeout) {
            Ok(signal) => signal,
            Err(RecvTimeoutError::Timeout) => return Ok(false),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(WatchError::Stopped {
                    source: io::Error::other("the notification thread ended"),
                });
            }
        };
        for signal in iter::once(first).chain(self.signals.try_iter().take(QUEUED)) {
            match signal {
                Signal::Changed(paths) => found.paths.extend(paths),
                Signal::Lost => found.lost = true,
            }
        }
        found.lost |= self.overflowed.swap(false, Ordering::Relaxed);
        Ok(true)
    }

    #[expect(
        clippy::unnecessary_wraps,
        clippy::unused_self,
        reason = "one signature for every platform, and the stream covers the whole tree from its start, so nothing \
                  is left to watch"
    )]
    pub(super) const fn cover(&self, _index: &Index, _found: &mut Found) -> Result<(), WatchError> {
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use notify::event::{CreateKind, Flag};
    use notify::{Event, EventKind};

    use super::*;
    use crate::watch::through_firmlinks;

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
    fn an_error_from_the_stream_loses_the_batch() {
        assert!(matches!(
            signal(Err(notify::Error::generic("stream failed")), &tree()),
            Signal::Lost
        ));
    }
}
