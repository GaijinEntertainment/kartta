//! The thread that follows a built index's tree and refreshes the index from each change the tree reports.

use std::sync::{Arc, Weak};
use std::time::Duration;

use kartta::{WatchError, Watcher};

use crate::state::Served;

/// How long the watch waits for a change before it looks again whether an `index_repo` replaced its index.
const WAKE: Duration = Duration::from_secs(1);

/// Refreshes `served` at the paths of `batch`, which `watcher` handed over for the changes made while the index was
/// built or refreshed since the watcher started, and hands the watcher back to follow the tree from there. When the
/// batch says the watcher dropped changes, stopped or met the inotify limit, or the refresh met the limit, the watcher
/// gives way to a new one over a refresh of the whole tree, which is caught up the same way, so `served` holds what
/// changed while the watch restarted too.
pub(crate) fn caught_up(
    served: &Served,
    mut watcher: Watcher,
    mut batch: Result<Vec<String>, WatchError>,
) -> Result<Watcher, String> {
    loop {
        match next(batch) {
            Next::Refresh(changes) if changes.is_empty() => return Ok(watcher),
            Next::Refresh(changes) => {
                match served.refresh_under(changes.iter().map(String::as_str), &mut watcher)? {
                    Ok(()) => return Ok(watcher),
                    Err(error) => batch = Err(error),
                }
            }
            Next::Restart(why) => {
                watcher = restarted(served, watcher, &why)?;
                batch = watcher.drain();
            }
            Next::Stop(reason) => return Err(reason),
        }
    }
}

/// What a batch from a watcher asks of the watch.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The paths to refresh, none when nothing changed.
    Refresh(Vec<String>),
    /// The watcher no longer covers the tree, for this reason, and a new one over a refresh of the whole tree does.
    Restart(String),
    /// The watch ends, for this reason.
    Stop(String),
}

// constraint: a batch that names the whole tree is one the watcher dropped changes from, and on Linux the notice of a
// directory moved meanwhile went with them, so the watcher would go on naming its changes by the path it had, and only
// a new watcher over a refresh of the whole tree follows the tree as it is
fn next(batch: Result<Vec<String>, WatchError>) -> Next {
    match batch {
        Ok(changes) if changes.iter().any(String::is_empty) => {
            Next::Restart("the notifications dropped changes".to_owned())
        }
        Ok(changes) => Next::Refresh(changes),
        Err(error @ (WatchError::Stopped { .. } | WatchError::Limit { .. })) => {
            Next::Restart(error.to_string())
        }
        Err(error) => Next::Stop(error.to_string()),
    }
}

/// Replaces `watcher`, which no longer covers the tree for the reason `why`, with a new one over a refresh of the
/// whole tree of `served`.
fn restarted(served: &Served, watcher: Watcher, why: &str) -> Result<Watcher, String> {
    // constraint: inotify's watch limit counts every watch the user holds, so a new watcher beside the old one would
    // need the limit twice over
    drop(watcher);
    served
        .refresh_watched()
        .map_err(|reason| format!("{why}, and a new watch failed: {reason}"))
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
    let mut batch = watcher.wait(WAKE);
    loop {
        let Some(served) = index.upgrade() else {
            return;
        };
        if served.is_replaced() {
            return;
        }
        match next(batch) {
            Next::Refresh(changes) if changes.is_empty() => {}
            Next::Refresh(changes) => {
                match served.refresh_under(changes.iter().map(String::as_str), &mut watcher) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        batch = Err(error);
                        continue;
                    }
                    Err(error) => return stop(&served, format!("a refresh failed: {error}")),
                }
            }
            Next::Restart(why) => match restarted(&served, watcher, &why) {
                Ok(restarted) => watcher = restarted,
                Err(reason) => return stop(&served, reason),
            },
            Next::Stop(reason) => return stop(&served, reason),
        }
        drop(served);
        batch = watcher.wait(WAKE);
    }
}

/// Records that `served` no longer follows its tree, unless an `index_repo` replaced it, which stopped the watch.
fn stop(served: &Served, reason: String) {
    if !served.is_replaced() {
        served.unwatch(reason);
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn a_batch_naming_paths_is_refreshed_and_an_empty_one_changes_nothing() {
        assert_eq!(
            next(Ok(vec!["a.rs".to_owned(), "src".to_owned()])),
            Next::Refresh(vec!["a.rs".to_owned(), "src".to_owned()])
        );
        assert_eq!(next(Ok(Vec::new())), Next::Refresh(Vec::new()));
    }

    #[test]
    fn a_watcher_that_dropped_changes_stopped_or_met_the_limit_is_restarted() {
        let stopped = WatchError::Stopped {
            source: io::Error::other("the notification thread ended"),
        };
        let limit = WatchError::Limit {
            watched: 8,
            wanted: 9,
        };

        assert_eq!(
            next(Ok(vec![String::new()])),
            Next::Restart("the notifications dropped changes".to_owned())
        );
        assert_eq!(
            next(Err(stopped)),
            Next::Restart("the watch stopped: the notification thread ended".to_owned())
        );
        assert!(
            matches!(next(Err(limit)), Next::Restart(why) if why.starts_with("cannot watch 9 directories")),
            "a limit met restarts the watch"
        );
    }
}
