use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TrySendError};
use std::time::Duration;
use std::{io, iter};

use notify::{Config, EventKindMask, RecommendedWatcher, RecursiveMode, Watcher as _};

use super::{Found, WatchError};
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
    pub(super) fn start(_index: &Index, watched: &[PathBuf]) -> Result<Self, WatchError> {
        let (sender, signals) = mpsc::sync_channel(QUEUED);
        let overflowed = Arc::new(AtomicBool::new(false));
        let overflowing = Arc::clone(&overflowed);
        // constraint: notify names the directory to scan again on a drop when the system names one, and nothing when the
        // whole watch lost track, as a Windows buffer that overflowed does; an error from the stream may have cost a
        // change, so it counts as a loss too
        let handler = move |event: notify::Result<notify::Event>| {
            let signal = match event {
                Ok(event) if event.need_rescan() && event.paths.is_empty() => Signal::Lost,
                Ok(event) => Signal::Changed(event.paths),
                Err(_) => Signal::Lost,
            };
            if let Err(TrySendError::Full(_)) = sender.try_send(signal) {
                overflowing.store(true, Ordering::Relaxed);
            }
        };
        let config = Config::default().with_event_kinds(EventKindMask::CORE);
        let first = watched
            .first()
            .map_or_else(|| Path::new(""), PathBuf::as_path);
        let mut notifier =
            RecommendedWatcher::new(handler, config).map_err(|error| start_error(first, error))?;
        for path in watched {
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
}

fn start_error(path: &Path, error: notify::Error) -> WatchError {
    WatchError::Start {
        native_path: path.to_path_buf(),
        source: Box::new(error),
    }
}
