//! The thread that follows a built index's tree and refreshes the index from each change the tree reports.

use std::sync::{Arc, Weak};
use std::time::Duration;

use kartta_fs::{WatchError, Watcher};

use crate::state::Served;

/// How long the watch waits for a change before it looks again whether an `index_repo` replaced its index.
const WAKE: Duration = Duration::from_secs(1);

/// Starts a watcher over the tree of `served` and refreshes the whole tree, since a change made before the watcher
/// started, while the index was built included, is not reported. The directories that refresh added are then watched
/// too, and refreshed for what changed inside each before its watch went on.
pub(crate) fn start(served: &Served) -> Result<Watcher, String> {
    let mut watcher = served.watcher()?;
    served.refresh(std::iter::empty())?;
    let covered = served.cover(&mut watcher)?;
    if !covered.is_empty() {
        served.refresh(covered.paths())?;
    }
    Ok(watcher)
}

/// Follows the tree of `served` through `watcher` on a thread of its own.
///
/// The thread holds the index only while it refreshes it, so a rebuild waiting for the index to be released waits at
/// most for one refresh, which the rebuild cancels; the thread ends at its next wake once the index is replaced.
pub(crate) fn follow(served: &Arc<Served>, watcher: Watcher) {
    let index = Arc::downgrade(served);
    let spawned = std::thread::Builder::new()
        .name("kartta-watch".to_owned())
        .spawn(move || run(&index, watcher));
    if let Err(error) = spawned {
        served.unwatch(format!("cannot start the watch thread: {error}"));
    }
}

fn run(index: &Weak<Served>, mut watcher: Watcher) {
    loop {
        let changes = watcher.wait(WAKE);
        let Some(served) = index.upgrade() else {
            return;
        };
        if served.is_replaced() {
            return;
        }
        match changes {
            Ok(changes) if changes.is_empty() => {}
            Ok(changes) => {
                if let Err(error) = served.refresh(changes.paths()) {
                    stop(&served, format!("a refresh failed: {error}"));
                    return;
                }
            }
            Err(error @ (WatchError::Stopped { .. } | WatchError::Limit { .. })) => {
                // constraint: inotify's watch limit counts every watch the user holds, so a new watcher beside the
                // stopped one would need the limit twice over
                drop(watcher);
                match start(&served) {
                    Ok(restarted) => watcher = restarted,
                    Err(reason) => {
                        return stop(
                            &served,
                            format!("{error}, and a new watch failed: {reason}"),
                        );
                    }
                }
            }
            Err(error) => {
                stop(&served, error.to_string());
                return;
            }
        }
    }
}

/// Records that `served` no longer follows its tree, unless an `index_repo` replaced it, which stopped the watch.
fn stop(served: &Served, reason: String) {
    if !served.is_replaced() {
        served.unwatch(reason);
    }
}
