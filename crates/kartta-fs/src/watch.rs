use std::collections::BTreeSet;
use std::error::Error;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::index::Index;

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

/// Follows the tree an [`Index`] was built from through the operating system's change notifications, and hands over
/// what changed as paths for [`Index::refresh_paths`].
///
/// The loop is the caller's: [`Watcher::wait`] blocks until the tree changes, gathers the changes that follow until it
/// has been quiet for a moment, and returns them, and the caller refreshes the index with them. The index holds no
/// thread and no lock for the watcher, so an index nobody watches pays nothing.
///
/// On Linux each directory the index holds is watched through inotify, one watch a directory, and each directory made
/// later is watched as its creation is seen, before the refresh that names it walks it. A directory the ignore rules
/// hide is therefore never watched unless it is made after the start, and a directory named `.git` or in
/// [`BuildOptions::pruned_names`](crate::BuildOptions::pruned_names) is never watched. On macOS the root is watched
/// through `FSEvents` and on Windows through `ReadDirectoryChangesW`, both of which cover the whole tree, ignored and
/// pruned parts included; a refresh of an ignored or pruned path changes nothing and costs no pass over the index,
/// except that a changed `.gitignore` under a pruned name re-walks the directory whose rules it holds.
///
/// A change made before the watcher starts is not reported, but on macOS and Windows one made moments before can
/// still arrive; refreshing its path changes nothing when the index already holds the change, as for one made before
/// the build, and takes it in when it does not. A caller that builds and then watches refreshes once
/// more, for what changed in between, then calls [`Watcher::cover`] and refreshes the paths it names: on Linux a
/// directory that refresh added is watched only from that call. When the operating system drops changes it could not
/// deliver, the batch names the whole tree, or on macOS the directory below the root that `FSEvents` names for them.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use kartta_fs::{BuildOptions, Index, Watcher};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut index = Index::build("project", &BuildOptions::default())?.index;
/// let mut watcher = Watcher::new(&index)?;
/// index.refresh(None, None)?;
/// let covered = watcher.cover(&index)?;
/// index.refresh_paths(covered.iter().map(String::as_str), None, None)?;
/// loop {
///     let changes = watcher.wait(Duration::from_secs(1))?;
///     if !changes.is_empty() {
///         let outcome = index.refresh_paths(changes.iter().map(String::as_str), None, None)?;
///         eprintln!("walked again: {:?}", outcome.walked);
///     }
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct Watcher {
    backend: Backend,
    prefixes: Vec<(PathBuf, String)>,
}

impl Watcher {
    /// Starts following the tree `index` was built from, under the root it was built with.
    ///
    /// # Errors
    ///
    /// [`WatchError::Start`] when the operating system refuses the watch, [`WatchError::Limit`] when Linux's inotify
    /// watch limit is too low for the directories the index holds, and [`WatchError::Unsupported`] on a platform other
    /// than Linux, macOS and Windows.
    pub fn new(index: &Index) -> Result<Self, WatchError> {
        let prefixes = event_prefixes(index.root())?;
        let backend = Backend::start(index, &prefixes)?;

        Ok(Self { backend, prefixes })
    }

    /// Waits up to `timeout` for the tree to change, then gathers the changes that follow until it has been quiet for
    /// 20 ms, or for 500 ms at most, and returns them. Empty when nothing changed within `timeout`, but on macOS and
    /// Windows a wait can also hand over a change made moments before the watcher started, as the [`Watcher`] docs say.
    ///
    /// The changes are root-relative forward-slash paths, each once, for [`Index::refresh_paths`]. A path is named as
    /// the operating system reported it, so it may be ignored, pruned, inside `.git`, or gone again;
    /// [`Index::refresh_paths`] skips or widens each as it does any path. A name that is not valid UTF-8 is named by
    /// the directory that holds it. The batch is the empty path alone, the whole tree, when changes were dropped before
    /// they could be handed over: an inotify queue that overflowed, an `FSEvents` stream that dropped events at the
    /// root or at a directory above it, a `ReadDirectoryChangesW` buffer that overflowed, or 65,536 notifications that
    /// piled up while the caller was not waiting. Events `FSEvents` dropped below the root are named by the directory
    /// it names for them, which [`Index::refresh_paths`] walks again with everything below it.
    ///
    /// On Linux a wait also watches each directory made since the start as it reads of it, together with every
    /// directory already inside it, so a large tree moved in or unpacked lengthens the wait that meets it by a watch
    /// call a directory.
    ///
    /// # Errors
    ///
    /// [`WatchError::Stopped`] when the notifications stopped arriving, and [`WatchError::Limit`] when Linux's inotify
    /// watch limit ran out before a directory made since the start could be watched, so changes inside it would go
    /// unseen. Either way the watcher no longer covers the tree, and a new one has to be started.
    #[cfg_attr(
        not(target_os = "linux"),
        expect(
            clippy::needless_pass_by_ref_mut,
            reason = "Linux's backend adds watches as it reads, so a wait takes the watcher mutably everywhere"
        )
    )]
    pub fn wait(&mut self, timeout: Duration) -> Result<Vec<String>, WatchError> {
        let mut found = Found::default();
        if !self.backend.poll(timeout, &mut found)? {
            return Ok(Vec::new());
        }
        let started = Instant::now();
        loop {
            let left = LONGEST.saturating_sub(started.elapsed());
            if left.is_zero() || !self.backend.poll(SETTLE.min(left), &mut found)? {
                break;
            }
        }

        Ok(self.changes(found))
    }

    /// Watches each directory `index` holds that this watcher does not watch yet, together with every directory inside
    /// each, and returns those directories for [`Index::refresh_paths`], since a change made in one before its watch
    /// went on raised no event.
    ///
    /// On Linux [`Watcher::new`] watches the directories the index holds when it starts, so a directory that a later
    /// refresh adds, made before the watcher started, is watched only from this call; it makes one watch call for each
    /// directory the index holds, as [`Watcher::new`] does, and a listing for each directory it starts to watch. A
    /// directory found inside one that the operating system refuses to watch is skipped, as [`Watcher::wait`] skips
    /// it: refused for permission, it is one the walk cannot list either. On macOS and Windows the watcher covers the
    /// whole tree from its start, so this returns nothing.
    ///
    /// # Errors
    ///
    /// [`WatchError::Limit`] when Linux's inotify watch limit runs out before every such directory is watched, and
    /// [`WatchError::Start`] when the operating system refuses to watch one that is still there, so changes inside it
    /// would go unseen. The watcher then no longer covers the tree, and a new one has to be started.
    #[cfg_attr(
        not(target_os = "linux"),
        expect(
            clippy::needless_pass_by_ref_mut,
            reason = "Linux's backend adds watches, so covering takes the watcher mutably everywhere"
        )
    )]
    pub fn cover(&mut self, index: &Index) -> Result<Vec<String>, WatchError> {
        let mut found = Found::default();
        self.backend.cover(index, &mut found)?;
        Ok(self.changes(found))
    }

    fn changes(&self, found: Found) -> Vec<String> {
        if found.lost {
            return vec![String::new()];
        }
        let paths: BTreeSet<String> = found
            .paths
            .into_iter()
            .filter_map(|path| relative(&path, &self.prefixes))
            .collect();

        paths.into_iter().collect()
    }
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
    /// Linux's inotify watch limit ran out before every directory the index holds was watched.
    ///
    /// Each watched directory takes one watch from `fs.inotify.max_user_watches`, shared by every process of the user.
    #[error(
        "cannot watch {wanted} directories: the inotify watch limit stopped at {watched}; raise fs.inotify.max_user_watches"
    )]
    Limit {
        /// How many directories were watched when the limit ran out.
        watched: usize,
        /// How many directories the index holds, the root included.
        wanted: usize,
    },
    /// The notifications stopped arriving.
    #[error("the watch stopped: {source}")]
    Stopped {
        /// The underlying failure.
        source: io::Error,
    },
}

#[derive(Debug, Default)]
struct Found {
    paths: Vec<PathBuf>,
    lost: bool,
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

    let mut named = base.clone();
    for component in rest.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let Some(name) = name.to_str() else {
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
}
