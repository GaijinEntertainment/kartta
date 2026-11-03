use std::collections::BTreeSet;
use std::error::Error;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::CancelToken;
use crate::build::{BuildError, BuildOptions, PathFault};
use crate::content::ContentVisitor;
use crate::index::{Asked, BuildOutcome, Index, RefreshOutcome};
use crate::policy::Descending;

#[cfg(target_os = "linux")]
mod inotify;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod notified;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;

#[cfg(target_os = "linux")]
use inotify::Backend;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use notified::Backend;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use unsupported::Backend;

// constraint: one change arrives as a burst of events over a few milliseconds, so a batch closes once the tree has been
// quiet this long
const SETTLE: Duration = Duration::from_millis(20);
// constraint: a tree that never goes quiet still hands over a batch this often, so a steady writer cannot hold the
// refresh off
const LONGEST: Duration = Duration::from_millis(500);
// constraint: inotify queues a change as the call that made it returns, while FSEvents reported a write 12–13 ms
// after it, and notify hands ReadDirectoryChangesW's reports over from a thread of its own; FSEvents reports a change
// made in a stream's first 150 ms up to 323 ms after it, which a drain leaves to a later wait, as covering it would
// hold every macOS drain of a short walk for that long
#[cfg(target_os = "linux")]
const REPORTED: Duration = Duration::ZERO;
#[cfg(not(target_os = "linux"))]
const REPORTED: Duration = Duration::from_millis(50);

/// Follows a tree through the operating system's change notifications, and hands over what changed as paths for
/// [`Index::refresh_paths_watched`].
///
/// [`Index::build_watched`] starts one before its build walks the tree, and [`Index::refresh_watched`] before its
/// refresh walks it, so the watcher holds every change made while the walk ran, and [`Watcher::drain`] hands them over
/// once the index is there to take them in. The loop is the caller's: [`Watcher::wait`] blocks until the tree changes,
/// gathers the changes that follow until it has been quiet for a moment, and returns them, and the caller refreshes
/// the index with them under the watcher. The index holds no thread and no lock for the watcher, so an index nobody
/// watches pays nothing.
///
/// On Linux each directory a walk keeps is watched through inotify, one watch a directory, before the walk lists it,
/// so whatever is made in it after the listing raises a notification: the walk of a watched build or refresh, and that
/// of [`Index::refresh_paths_watched`], which takes in each directory made later. A directory named `.git` or in
/// [`BuildOptions::pruned_names`](crate::BuildOptions::pruned_names) is never watched, and one the ignore rules the
/// walk reads hide is not watched until a `.gitignore` edit shows it and the refresh that takes it in watches it; one
/// an edit hides loses its watches, and those of every directory below it, to the refresh that drops it from the
/// index. On macOS the root is watched through `FSEvents` and on Windows through `ReadDirectoryChangesW`, both of
/// which cover the whole tree, ignored and pruned parts included; a refresh of an ignored or pruned path changes
/// nothing and costs no pass over the index, except that a changed `.gitignore` whose own name is pruned re-walks the
/// directory whose rules it holds; a `.gitignore` inside a pruned directory changes nothing.
///
/// A change made before the watcher starts is not reported, but on macOS and Windows one made moments before can
/// still arrive; refreshing its path changes nothing when the index already holds the change, as for one the walk
/// read after it was made, and takes it in when it does not. When the operating system drops changes it could not
/// deliver, the batch names the whole tree, or on macOS the directory below the root that `FSEvents` names for them.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use kartta_fs::{BuildOptions, Index};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let (outcome, watcher) = Index::build_watched("project", &BuildOptions::default(), None, None)?;
/// let (mut index, mut watcher) = (outcome.index, watcher?);
/// let mut changes = watcher.drain()?;
/// loop {
///     if changes == [""] {
///         drop(watcher);
///         let (_, restarted) = index.refresh_watched(None, None)?;
///         watcher = restarted?;
///     } else if !changes.is_empty() {
///         let paths = changes.iter().map(String::as_str);
///         let (outcome, followed) = index.refresh_paths_watched(paths, &mut watcher, None, None)?;
///         followed?;
///         eprintln!("walked again: {:?}", outcome.walked);
///     }
///     changes = watcher.wait(Duration::from_secs(1))?;
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct Watcher {
    backend: Backend,
    prefixes: Vec<(PathBuf, String)>,
}

impl Watcher {
    fn start(root: &Path) -> Result<Self, WatchError> {
        let prefixes = event_prefixes(root)?;
        let backend = Backend::start(root, &prefixes)?;

        Ok(Self { backend, prefixes })
    }

    fn walked(mut self) -> Result<Self, WatchError> {
        self.backend.descended()?;
        Ok(self)
    }

    /// Waits up to `timeout` for the tree to change, then gathers the changes that follow until it has been quiet for
    /// 20 ms, or for 500 ms at most, and returns them. Empty when nothing changed within `timeout`, but on macOS and
    /// Windows a wait can also hand over a change made moments before the watcher started, as the [`Watcher`] docs say.
    ///
    /// The changes are root-relative forward-slash paths, each once, for [`Index::refresh_paths_watched`]. A path is
    /// named as the operating system reported it, so it may be ignored, pruned, inside `.git`, or gone again;
    /// [`Index::refresh_paths`] skips or widens each as it does any path. A name that is not valid UTF-8 is named by
    /// the directory that holds it, and one directly below the root names nothing, since the index holds no such name
    /// and nothing below it. A name a refresh refuses by its [`PathFault`], one that holds a `\` or a first name that
    /// starts with a drive such as `C:`, both legal outside Windows, is named by the directory that holds it too, and
    /// one directly below the root makes the batch the whole tree, since the index holds it. A notification for the root itself, such as `FSEvents` raises for a change of its times,
    /// names nothing either: a change of an entry inside it is reported for that entry. A path outside the tree names
    /// nothing. The batch is the empty path alone, the whole tree, when changes were dropped before they could be
    /// handed over: an inotify queue that overflowed, an error notify hands over in place of a notification, an
    /// `FSEvents` stream that dropped events with no path, at the root, at a directory above it or outside the tree, a
    /// `ReadDirectoryChangesW` buffer that overflowed, or 65,536 notifications that piled up while the caller was not
    /// waiting. Events `FSEvents` dropped below the root are named by the directory
    /// it names for them, which [`Index::refresh_paths`] walks again with everything below it. On Linux the notice of
    /// a directory moved while inotify dropped changes went with them, so the watcher would go on naming its changes by
    /// the path it had, and a caller takes the whole tree in through [`Index::refresh_watched`] and follows it with the
    /// watcher that returns.
    ///
    /// # Errors
    ///
    /// [`WatchError::RootGone`] when the root was deleted or moved, in place of the changes gathered with it,
    /// [`WatchError::Stopped`] when the notifications stopped arriving, and [`WatchError::Limit`] or
    /// [`WatchError::Start`] when a refresh under the watcher met them but returned an error of its own. Each way the
    /// watcher no longer covers the tree, and a new one has to be started.
    pub fn wait(&mut self, timeout: Duration) -> Result<Vec<String>, WatchError> {
        self.gather(timeout, LONGEST, Found::default())
    }

    /// Hands over every change made before the call, as [`Watcher::wait`] does, waiting for the operating system only
    /// as long as it takes to report a change: on Linux not at all, and on macOS and Windows up to 50 ms, so a change
    /// those systems report later comes with a later wait. Empty when nothing changed; once a change is there, it
    /// gathers those that follow as [`Watcher::wait`] does, until the tree has been quiet for 20 ms or for 500 ms at
    /// most, but not before it has read every change queued at the call, so a deep queue lengthens the drain by the
    /// time its reads take. A drain that cannot read them all hands over the empty path alone, the whole tree, as for
    /// changes the operating system dropped.
    ///
    /// # Errors
    ///
    /// As [`Watcher::wait`].
    pub fn drain(&mut self) -> Result<Vec<String>, WatchError> {
        self.drain_within(LONGEST)
    }

    fn drain_within(&mut self, longest: Duration) -> Result<Vec<String>, WatchError> {
        let found = Found {
            unread: self.backend.queued(),
            ..Found::default()
        };
        self.gather(REPORTED, longest, found)
    }

    fn gather(
        &mut self,
        timeout: Duration,
        longest: Duration,
        mut found: Found,
    ) -> Result<Vec<String>, WatchError> {
        self.backend.descended()?;
        if !self.backend.poll(timeout, &mut found)? {
            return self.changes(found);
        }
        let started = Instant::now();
        loop {
            let left = longest.saturating_sub(started.elapsed());
            let closed = left.is_zero() && found.unread == 0;
            if closed || !self.backend.poll(SETTLE.min(left), &mut found)? {
                break;
            }
        }

        self.changes(found)
    }

    fn changes(&self, found: Found) -> Result<Vec<String>, WatchError> {
        if found.root_went {
            return Err(WatchError::RootGone);
        }
        if found.lost || found.unread > 0 {
            return Ok(vec![String::new()]);
        }
        let mut paths = BTreeSet::new();
        for named in found
            .paths
            .into_iter()
            .filter_map(|path| relative(&path, &self.prefixes))
            // constraint: the root's own path, and a name directly below it that is not valid UTF-8, name no entry the
            // index holds, and the root's deletion or move has ended the batch already
            .filter(|named| !named.is_empty())
        {
            let Some(part) = refreshable(named) else {
                return Ok(vec![String::new()]);
            };
            paths.insert(part);
        }

        Ok(paths.into_iter().collect())
    }
}

/// `named` as [`Index::refresh_paths`] takes it: whole, or cut to the directory that holds its first name a refresh
/// refuses, one that holds a `\` or a first name that starts with a drive, which outside Windows are names like any
/// other. `None` when that directory is the root, which only a refresh of the whole tree walks again.
fn refreshable(mut named: String) -> Option<String> {
    let refused = match PathFault::of(&named) {
        None => return Some(named),
        Some(PathFault::Backslash) => named.find('\\').unwrap_or(0),
        Some(_) => 0,
    };
    let holder = named[..refused].rfind('/')?;
    named.truncate(holder);
    named.shrink_to_fit();
    Some(named)
}

impl Index {
    /// Starts a [`Watcher`] on `root`, then builds the index as [`Index::build_with`] does, so the watcher holds every
    /// change made while the build walked, which [`Watcher::drain`] then hands over for
    /// [`Index::refresh_paths_watched`].
    ///
    /// On Linux the walk watches each directory it keeps before it lists it: a change made in the directory after its
    /// listing raises a notification, and one made before is in the listing. That costs the walk a watch call a
    /// directory, which inotify serves one at a time whatever the walk's threads. A directory the walk cannot read is
    /// not watched, since the walk cannot list it either and the build reports it. A watcher that cannot start, or
    /// cannot watch a directory it should, does not stop the build: the index is built, and the second value says why
    /// there is no watcher.
    ///
    /// # Errors
    ///
    /// As [`Index::build_with`]. The second value is the watcher, or [`WatchError::Start`] when the operating system
    /// refused a watch, [`WatchError::Limit`] when Linux's inotify watch limit is too low for the directories the walk
    /// kept, and [`WatchError::Unsupported`] on a platform other than Linux, macOS and Windows.
    pub fn build_watched(
        root: impl AsRef<Path>,
        options: &BuildOptions,
        visitor: Option<&dyn ContentVisitor>,
        cancel: Option<&CancelToken>,
    ) -> Result<(BuildOutcome, Result<Watcher, WatchError>), BuildError> {
        let root = root.as_ref();
        let watcher = Watcher::start(root);
        watch_walk(watcher, Asked::new(visitor, cancel), |asked| {
            Self::build_walking(root, options, asked)
        })
    }

    /// Starts a [`Watcher`] on the index's root, then walks the tree again as [`Index::refresh`] does, so the watcher
    /// holds every change made while the refresh walked, as [`Index::build_watched`] holds those of a build.
    ///
    /// # Errors
    ///
    /// As [`Index::refresh`]. The second value is the watcher, or why there is none, as [`Index::build_watched`] says.
    pub fn refresh_watched(
        &mut self,
        visitor: Option<&dyn ContentVisitor>,
        cancel: Option<&CancelToken>,
    ) -> Result<(RefreshOutcome, Result<Watcher, WatchError>), BuildError> {
        let watcher = Watcher::start(self.root());
        watch_walk(watcher, Asked::new(visitor, cancel), |asked| {
            self.refresh_walking(asked)
        })
    }

    /// Walks again the parts of the tree at `paths` as [`Index::refresh_paths`] does, under `watcher`, so a change made
    /// in a directory the walk took in reaches `watcher` too.
    ///
    /// On Linux the walk watches each directory it keeps before it lists it, as [`Index::build_watched`] does, at a
    /// watch call a directory, one already watched included. The ignore rules the walk reads decide what it keeps, so
    /// a directory they hide is not watched, and one a `.gitignore` edit shows is watched by the refresh that takes it
    /// in. Each directory the refresh drops from the index, such as one an edit hides, loses its watches and those of
    /// every directory below it, at one pass over the watcher's watches for a refresh that drops any. inotify queues a
    /// notice of each watch released, so releasing more watches than `fs.inotify.max_queued_events` leaves room for
    /// overflows the queue, and the next [`Watcher::wait`] hands over the whole tree. On macOS and Windows the watcher
    /// already covers the whole tree, and this is [`Index::refresh_paths`].
    ///
    /// # Errors
    ///
    /// As [`Index::refresh_paths`]. The second value is [`WatchError::Limit`] when Linux's inotify watch limit ran out
    /// before every directory the walk kept was watched, and [`WatchError::Start`] when the operating system refused a
    /// watch: either way `watcher` no longer covers the tree, and a new one has to be started. A call that returns an
    /// error of its own leaves either for the next [`Watcher::wait`], [`Watcher::drain`] or call of this under
    /// `watcher` to report.
    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments of Index::refresh_paths and the watcher whose watches its walk adds"
    )]
    pub fn refresh_paths_watched<'p>(
        &mut self,
        paths: impl IntoIterator<Item = &'p str>,
        watcher: &mut Watcher,
        visitor: Option<&dyn ContentVisitor>,
        cancel: Option<&CancelToken>,
    ) -> Result<(RefreshOutcome, Result<(), WatchError>), BuildError> {
        let mut refreshed = {
            let hook = watcher.backend.descending();
            self.refresh_paths_walking(
                paths,
                Asked {
                    descending: hook.as_ref().map(|hook| hook as Descending<'_>),
                    ..Asked::new(visitor, cancel)
                },
            )
        }?;
        watcher
            .backend
            .release(&std::mem::take(&mut refreshed.removed_dirs));
        Ok((refreshed, watcher.backend.descended()))
    }
}

fn watch_walk<T>(
    watcher: Result<Watcher, WatchError>,
    asked: Asked<'_>,
    walk: impl FnOnce(Asked<'_>) -> Result<T, BuildError>,
) -> Result<(T, Result<Watcher, WatchError>), BuildError> {
    let walked = {
        let hook = watcher
            .as_ref()
            .ok()
            .and_then(|watcher| watcher.backend.descending());
        walk(Asked {
            descending: hook.as_ref().map(|hook| hook as Descending<'_>),
            ..asked
        })?
    };
    Ok((walked, watcher.and_then(Watcher::walked)))
}

/// Why a [`Watcher`] could not start, or stopped.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WatchError {
    /// This platform has none of the change notifications a watcher uses: inotify, `FSEvents` or
    /// `ReadDirectoryChangesW`.
    #[error("cannot watch {}: this platform has no change notifications to follow it with", native_path.display())]
    Unsupported {
        /// The root that was to be watched.
        native_path: PathBuf,
    },
    /// The operating system refused to watch a path.
    #[error("cannot watch {}: {source}", native_path.display())]
    Start {
        /// The path that could not be watched.
        native_path: PathBuf,
        /// The underlying failure, boxed so no dependency's error type reaches this crate's public API.
        source: Box<dyn Error + Send + Sync>,
    },
    /// Linux's inotify watch limit ran out before every directory a walk kept was watched.
    ///
    /// Each watched directory takes one watch from `fs.inotify.max_user_watches`, shared by every process of the user.
    #[error(
        "cannot watch {wanted} directories: the inotify watch limit stopped at {watched}; raise fs.inotify.max_user_watches"
    )]
    Limit {
        /// How many directories were watched when the limit ran out.
        watched: usize,
        /// How many directories were to be watched, the root included.
        wanted: usize,
    },
    /// The notifications stopped arriving.
    #[error("the watch stopped: {source}")]
    Stopped {
        /// The underlying failure.
        source: io::Error,
    },
    /// The root was deleted or moved away, so the watcher follows no tree at its path.
    ///
    /// It is told by the notice the operating system raises for the root itself: on macOS a notification for the
    /// root's path that carries notify's `root changed` info or is a removal, on Windows a removal of the root's path
    /// or a rename from it, which the watch of the root's parent reports for a move, and on Linux the root watch's own
    /// deletion or move notice. A full macOS or Windows queue keeps the notice beside its overflow, but an inotify
    /// queue that filled before the notice drops it with the rest, and the loss reads as changes dropped.
    #[error("the root was deleted or moved")]
    RootGone,
}

#[derive(Debug, Default)]
struct Found {
    paths: Vec<PathBuf>,
    lost: bool,
    root_went: bool,
    unread: usize,
}

#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    expect(
        dead_code,
        reason = "a platform with no notifications starts no watch to fail"
    )
)]
fn start_error(native_path: &Path, source: impl Error + Send + Sync + 'static) -> WatchError {
    WatchError::Start {
        native_path: native_path.to_path_buf(),
        source: Box::new(source),
    }
}

#[cfg(target_os = "macos")]
fn event_prefixes(root: &Path) -> Result<Vec<(PathBuf, String)>, WatchError> {
    // constraint: FSEvents names a change on the data volume by its firmlinked path, `/Users/...` rather than
    // `/System/Volumes/Data/Users/...`, so a stream on the data-volume spelling of a root hears nothing
    let canonical = std::fs::canonicalize(root).map_err(|source| start_error(root, source))?;
    let firmlinks = std::fs::read_to_string("/usr/share/firmlinks").unwrap_or_default();
    let mut prefixes = through_firmlinks(&canonical, &firmlinks);
    // constraint: the list names firmlinks a machine may not have, such as `/AppleInternal`, and FSEvents refuses a
    // path that does not exist
    prefixes.retain(|(path, base)| base.is_empty() || path.exists());

    Ok(prefixes)
}

#[cfg(not(target_os = "macos"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature for every platform, and macOS's has to canonicalize the root, which can fail"
)]
fn event_prefixes(root: &Path) -> Result<Vec<(PathBuf, String)>, WatchError> {
    Ok(vec![(root.to_path_buf(), String::new())])
}

#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(dead_code, reason = "only macOS names its paths through firmlinks")
)]
fn through_firmlinks(root: &Path, firmlinks: &str) -> Vec<(PathBuf, String)> {
    let Ok(on_data) = root.strip_prefix("/System/Volumes/Data") else {
        return vec![(root.to_path_buf(), String::new())];
    };

    let mut prefixes = vec![(root.to_path_buf(), String::new())];
    for (system, data) in firmlinks.lines().filter_map(|line| line.split_once('\t')) {
        let data = Path::new(data);
        if let Ok(inside) = on_data.strip_prefix(data) {
            return vec![(Path::new(system).join(inside), String::new())];
        }
        if let Ok(below) = data.strip_prefix(on_data)
            && let Some(below) = forward_slashed(below)
        {
            prefixes.push((PathBuf::from(system), below));
        }
    }
    prefixes
}

fn relative(path: &Path, prefixes: &[(PathBuf, String)]) -> Option<String> {
    let (rest, base) = prefixes
        .iter()
        .filter_map(|(prefix, base)| Some((path.strip_prefix(prefix).ok()?, base)))
        .min_by_key(|(rest, _)| rest.components().count())?;

    let rest_len = rest.as_os_str().len();
    let separator = usize::from(!base.is_empty() && rest_len > 0);
    let mut named = String::with_capacity(base.len() + separator + rest_len);
    named.push_str(base);
    for component in rest.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let Some(name) = name.to_str() else {
            named.shrink_to_fit();
            break;
        };
        if !named.is_empty() {
            named.push('/');
        }
        named.push_str(name);
    }
    Some(named)
}

fn forward_slashed(path: &Path) -> Option<String> {
    let names: Option<Vec<&str>> = path
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    Some(names?.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRMLINKS: &str =
        "/Applications\tApplications\n/Users\tUsers\n/private\tprivate\n/usr/local\tusr/local\n";

    #[test]
    fn a_root_inside_a_firmlink_is_watched_by_the_path_the_events_name() {
        let prefixes = through_firmlinks(
            Path::new("/System/Volumes/Data/Users/me/project"),
            FIRMLINKS,
        );

        assert_eq!(
            prefixes,
            [(PathBuf::from("/Users/me/project"), String::new())]
        );
    }

    #[test]
    fn a_root_above_firmlinks_is_watched_through_each_of_them() {
        let prefixes = through_firmlinks(Path::new("/System/Volumes/Data"), FIRMLINKS);

        assert_eq!(
            prefixes,
            [
                (PathBuf::from("/System/Volumes/Data"), String::new()),
                (PathBuf::from("/Applications"), "Applications".to_owned()),
                (PathBuf::from("/Users"), "Users".to_owned()),
                (PathBuf::from("/private"), "private".to_owned()),
                (PathBuf::from("/usr/local"), "usr/local".to_owned()),
            ]
        );
        assert_eq!(
            relative(Path::new("/usr/local/bin/tool"), &prefixes).as_deref(),
            Some("usr/local/bin/tool")
        );
        assert_eq!(
            relative(Path::new("/System/Volumes/Data/.hidden/file"), &prefixes).as_deref(),
            Some(".hidden/file")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_data_volume_is_watched_through_the_firmlinks_this_machine_has() {
        let prefixes = event_prefixes(Path::new("/System/Volumes/Data")).expect("prefixes");

        assert!(
            prefixes.iter().all(|(path, _)| path.exists()),
            "{prefixes:?}"
        );
        assert!(
            prefixes
                .iter()
                .any(|(path, base)| path == Path::new("/Users") && base == "Users")
        );
    }

    #[test]
    fn a_root_off_the_data_volume_is_watched_as_it_is() {
        let prefixes = through_firmlinks(Path::new("/opt/tree"), FIRMLINKS);

        assert_eq!(prefixes, [(PathBuf::from("/opt/tree"), String::new())]);
    }

    #[test]
    fn a_path_outside_every_prefix_names_nothing_and_the_root_names_the_whole_tree() {
        let prefixes = vec![(PathBuf::from("/tree"), String::new())];

        assert_eq!(relative(Path::new("/elsewhere/a.rs"), &prefixes), None);
        assert_eq!(relative(Path::new("/tree"), &prefixes).as_deref(), Some(""));
        assert_eq!(
            relative(Path::new("/tree/src/a.rs"), &prefixes).as_deref(),
            Some("src/a.rs")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn a_notification_for_the_root_itself_names_nothing_in_the_batch() {
        let tree = tempfile::tempdir().expect("a tree");
        let watcher = Watcher::start(tree.path()).expect("start watching");
        let root = watcher.prefixes[0].0.clone();
        let found = Found {
            paths: vec![root.clone(), root.join("a.rs")],
            ..Found::default()
        };

        assert_eq!(watcher.changes(found).expect("the root is there"), ["a.rs"]);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_name_directly_below_the_root_that_is_not_utf8_names_nothing_in_the_batch() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let tree = tempfile::tempdir().expect("a tree");
        let watcher = Watcher::start(tree.path()).expect("start watching");
        let root = watcher.prefixes[0].0.clone();
        let found = Found {
            paths: vec![root.join(OsStr::from_bytes(b"bad\xff")), root.join("a.rs")],
            ..Found::default()
        };

        assert_eq!(watcher.changes(found).expect("the root is there"), ["a.rs"]);
    }

    #[test]
    fn a_name_a_refresh_refuses_is_named_by_the_directory_that_holds_it() {
        for (named, part) in [
            ("src/a.rs", Some("src/a.rs")),
            ("src/C:x/a.rs", Some("src/C:x/a.rs")),
            ("src/lib/a\\b/c.rs", Some("src/lib")),
            ("src/a\\b.rs", Some("src")),
            ("a\\b.rs", None),
            ("C:x/a.rs", None),
        ] {
            let refreshed = refreshable(named.to_owned());

            assert_eq!(refreshed.as_deref(), part, "{named:?}");
            assert!(
                refreshed.is_none_or(|part| part.len() == part.capacity()),
                "{named:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_name_a_refresh_refuses_directly_below_the_root_makes_the_batch_the_whole_tree() {
        let tree = tempfile::tempdir().expect("a tree");
        let watcher = Watcher::start(tree.path()).expect("start watching");
        let root = watcher.prefixes[0].0.clone();
        let below = Found {
            paths: vec![root.join("src/a\\b.rs"), root.join("a.rs")],
            ..Found::default()
        };
        let at_the_root = Found {
            paths: vec![root.join("a.rs"), root.join("C:x")],
            ..Found::default()
        };

        assert_eq!(
            watcher.changes(below).expect("the root is there"),
            ["a.rs", "src"]
        );
        assert_eq!(
            watcher.changes(at_the_root).expect("the root is there"),
            [""]
        );
    }

    #[test]
    fn a_named_path_holds_its_own_bytes_and_no_more() {
        let prefixes = vec![
            (PathBuf::from("/tree"), String::new()),
            (PathBuf::from("/usr/local"), "usr/local".to_owned()),
        ];

        let named: Vec<(usize, usize)> = [
            "/tree/src/lib/a.rs",
            "/usr/local/bin/tools",
            "/tree",
            "/usr/local",
        ]
        .into_iter()
        .map(|path| relative(Path::new(path), &prefixes).expect("a path inside a prefix"))
        .map(|named| (named.len(), named.capacity()))
        .collect();

        assert_eq!(named, [(12, 12), (19, 19), (0, 0), (9, 9)]);
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_utf8_is_named_by_the_directory_that_holds_it() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let prefixes = vec![(PathBuf::from("/tree"), String::new())];
        let path = Path::new("/tree/src")
            .join(OsStr::from_bytes(b"bad\xff"))
            .join("a.rs");

        assert_eq!(relative(&path, &prefixes).as_deref(), Some("src"));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_named_by_the_directory_above_a_name_that_is_not_utf8_holds_its_own_bytes_and_no_more()
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let prefixes = vec![(PathBuf::from("/tree"), String::new())];
        let path = Path::new("/tree/src")
            .join(OsStr::from_bytes(b"bad\xff"))
            .join("a.rs");

        let named = relative(&path, &prefixes).expect("a path inside a prefix");

        assert_eq!((named.len(), named.capacity()), (3, 3));
    }
}
