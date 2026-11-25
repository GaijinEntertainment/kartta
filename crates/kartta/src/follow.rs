use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use kartta_fs::{BuildError, CancelToken, WatchError, Watcher};

use crate::engine::{Engine, EngineRefresh};

/// How long a follow waits for a change before it looks again whether its engine was let go.
const WAKE: Duration = Duration::from_secs(1);

/// An engine its owner shares under a lock of its own, which a [`Follower`] refreshes from what its watcher reports.
///
/// The owner keeps beside the engine what it answers with it, such as the failures of the build and of the refreshes
/// since, and takes each refresh of a follow in under the same lock, so no query reads the engine and that state apart.
pub trait Followed: Send + Sync + 'static {
    /// The token that ends the follow: once it is cancelled, a refresh under way stops before its walk ends, and the
    /// follow ends at its next look. The owner cancels it as it lets the engine go.
    fn token(&self) -> &CancelToken;

    /// Runs `refresh` on the engine under the owner's write lock, takes in what it refreshed, such as the failures
    /// [`EngineRefresh::errors`] lists, and answers `refresh`'s second value.
    ///
    /// # Errors
    ///
    /// Why the engine was not refreshed, worded for the owner's reader: the failure `refresh` returned, which is
    /// [`BuildError::Cancelled`] once [`Followed::token`] is cancelled, or a lock a panic poisoned.
    fn refreshed<T>(
        &self,
        refresh: impl FnOnce(&mut Engine) -> Result<(EngineRefresh, T), BuildError>,
    ) -> Result<T, String>;

    /// Sets `watch` beside the engine: what the follow reports of its watcher now.
    fn watched(&self, watch: Watch);
}

/// What a [`Follower`] reports of its watch: whether a watcher follows the tree, and how many times one was started
/// over. The default is a watch that follows its tree and never started over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Watch {
    /// Why no watcher follows the tree any more, so that the engine takes a change in only through a refresh; `None`
    /// while one does. A restart that failed names what asked for it, such as a watcher that stopped.
    pub off: Option<String>,
    /// How many times the follow let its watcher go and started a new one over a refresh of the whole tree, because the
    /// watcher dropped changes, stopped, met the inotify limit, or saw its root deleted or moved.
    pub restarts: usize,
}

/// A watcher an engine follows its tree through, and the count of the watchers its follow started over.
///
/// [`Engine::build_watched`] starts the watcher before its build walks the tree, so it holds every change made while
/// the build walked. [`Follower::caught_up`] takes those changes in before the owner publishes the engine, so no query
/// sees it miss one, and [`Follower::follow`] then refreshes the engine from each change the watcher reports, on a
/// thread of its own.
///
/// A batch of changed paths is refreshed under the watcher, as [`Engine::refresh_paths_watched`] does, so the watcher
/// watches a directory the refresh takes in. A batch that names the whole tree, because the operating system dropped
/// changes, a root deleted or moved, a watcher that stopped, and a refresh under it that met the inotify limit each let
/// the watcher go and start a new one over a refresh of the whole tree, as [`Engine::refresh_watched`] does: on Linux
/// the move of a directory went with the dropped changes, so the old watcher would go on naming that directory's
/// changes by the path it had, and inotify counts every watch the user holds, so the old watcher goes first. Any other
/// failure ends the follow, a restart that fails because nothing is at the root's path any more included, and
/// [`Watch::off`] says why.
///
/// An engine built without [`BuildOptions::stamps`](kartta_fs::BuildOptions::stamps) refuses every refresh, so
/// [`Engine::build_watched`] refuses to build an engine without them.
#[derive(Debug)]
pub struct Follower {
    watcher: Watcher,
    restarts: usize,
}

impl Follower {
    /// Takes in on `followed` every change `watcher` holds, as [`Watcher::drain`] hands them over, and answers the
    /// follower that follows the tree from there. A batch that asks for a new watcher is taken in the same way, through
    /// the new watcher's own drain, so the engine holds what changed while it was built and while the watch restarted.
    ///
    /// `None` once [`Followed::watched`] was told why no watcher follows the tree: `watcher` is the error
    /// [`Engine::build_watched`] answered, a refresh failed, or no new watcher could start. An error `watcher` holds
    /// starts no new watcher and drains nothing, so the engine stays as built.
    #[must_use]
    pub fn caught_up(
        followed: &impl Followed,
        watcher: Result<Watcher, WatchError>,
    ) -> Option<Self> {
        let mut restarts = 0;
        let caught_up = watcher
            .map_err(|error| error.to_string())
            .and_then(|mut watcher| {
                let batch = watcher.drain();
                caught_up(followed, watcher, batch, &mut restarts)
            });
        match caught_up {
            Ok(watcher) => Some(Self { watcher, restarts }),
            Err(reason) => {
                followed.watched(Watch {
                    off: Some(reason),
                    restarts,
                });
                None
            }
        }
    }

    /// Follows the tree on a thread of its own, `kartta-watch`, which refreshes `followed` from each batch of changes
    /// the watcher reports, and answers the thread's handle, or `None` once [`Followed::watched`] was told the thread
    /// could not start.
    ///
    /// The thread holds `followed` only while it refreshes it, and waits for a change a second at a time. After a
    /// restart of its own it drains nothing: its next wait hands the new watcher's changes over. It ends at its next
    /// look once nothing else holds `followed` or [`Followed::token`] is cancelled, and lets the watcher go as it ends.
    /// A follow that ends on a failure tells [`Followed::watched`] why, unless the token was cancelled, so a refresh
    /// the cancel made fail tells it nothing.
    pub fn follow<F: Followed>(self, followed: &Arc<F>) -> Option<JoinHandle<()>> {
        let restarts = self.restarts;
        let held = Arc::downgrade(followed);
        let spawned = thread::Builder::new()
            .name("kartta-watch".to_owned())
            .spawn(move || self.run(&held));
        spawned
            .inspect_err(|error| {
                followed.watched(Watch {
                    off: Some(format!("cannot start the watch thread: {error}")),
                    restarts,
                });
            })
            .ok()
    }

    fn run<F: Followed>(self, held: &Weak<F>) {
        let Self {
            mut watcher,
            mut restarts,
        } = self;
        let mut batch = watcher.wait(WAKE);
        loop {
            let Some(followed) = held.upgrade() else {
                return;
            };
            if followed.token().is_cancelled() {
                return;
            }
            match next(batch) {
                Next::Refresh(changes) if changes.is_empty() => {}
                Next::Refresh(changes) => match refreshed_under(&*followed, &changes, &mut watcher)
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        batch = Err(error);
                        continue;
                    }
                    Err(error) => {
                        return stop(&*followed, format!("a refresh failed: {error}"), restarts);
                    }
                },
                Next::Restart(why) => match restarted(&*followed, watcher, &why, &mut restarts) {
                    Ok(restarted) => watcher = restarted,
                    Err(reason) => return stop(&*followed, reason, restarts),
                },
                Next::Stop(reason) => return stop(&*followed, reason, restarts),
            }
            drop(followed);
            batch = watcher.wait(WAKE);
        }
    }
}

/// Refreshes `followed` at the paths of `batch` under `watcher`, and once the batch asks for a new watcher, starts one
/// and takes its own drain in the same way, until a batch leaves the watcher covering the tree.
fn caught_up(
    followed: &impl Followed,
    mut watcher: Watcher,
    mut batch: Result<Vec<String>, WatchError>,
    restarts: &mut usize,
) -> Result<Watcher, String> {
    loop {
        match next(batch) {
            Next::Refresh(changes) if changes.is_empty() => return Ok(watcher),
            Next::Refresh(changes) => match refreshed_under(followed, &changes, &mut watcher)? {
                Ok(()) => return Ok(watcher),
                Err(error) => batch = Err(error),
            },
            Next::Restart(why) => {
                watcher = restarted(followed, watcher, &why, restarts)?;
                batch = watcher.drain();
            }
            Next::Stop(reason) => return Err(reason),
        }
    }
}

/// What a batch from a watcher asks of the follow.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The paths to refresh, none when nothing changed.
    Refresh(Vec<String>),
    /// The watcher no longer covers the tree, for this reason, and a new one over a refresh of the whole tree does.
    Restart(String),
    /// The follow ends, for this reason.
    Stop(String),
}

// constraint: a batch that names the whole tree is one the watcher dropped changes from, and on Linux the notice of a
// directory moved meanwhile went with them, so the watcher would go on naming its changes by the path it had, and only
// a new watcher over a refresh of the whole tree follows the tree as it is; a root deleted or moved may have a tree
// moved into its place, which only such a restart finds
fn next(batch: Result<Vec<String>, WatchError>) -> Next {
    match batch {
        Ok(changes) if changes.iter().any(String::is_empty) => {
            Next::Restart("the notifications dropped changes".to_owned())
        }
        Ok(changes) => Next::Refresh(changes),
        Err(
            error @ (WatchError::RootGone | WatchError::Stopped { .. } | WatchError::Limit { .. }),
        ) => Next::Restart(error.to_string()),
        Err(error) => Next::Stop(error.to_string()),
    }
}

fn refreshed_under(
    followed: &impl Followed,
    changes: &[String],
    watcher: &mut Watcher,
) -> Result<Result<(), WatchError>, String> {
    let token = followed.token();
    followed.refreshed(|engine| {
        engine.refresh_paths_watched(changes.iter().map(String::as_str), watcher, Some(token))
    })
}

/// Replaces `watcher`, which no longer covers the tree for the reason `why`, with a new one over a refresh of the
/// whole tree of `followed`, and counts it.
fn restarted(
    followed: &impl Followed,
    watcher: Watcher,
    why: &str,
    restarts: &mut usize,
) -> Result<Watcher, String> {
    // constraint: inotify's watch limit counts every watch the user holds, so a new watcher beside the old one would
    // need the limit twice over
    drop(watcher);
    let token = followed.token();
    let watcher = followed
        .refreshed(|engine| engine.refresh_watched(Some(token)))
        .and_then(|watcher| watcher.map_err(|error| error.to_string()))
        .map_err(|reason| format!("{why}, and a new watch failed: {reason}"))?;
    *restarts += 1;
    followed.watched(Watch {
        off: None,
        restarts: *restarts,
    });
    Ok(watcher)
}

/// Tells `followed` why it no longer follows its tree, unless its token was cancelled, which ended the follow.
fn stop(followed: &impl Followed, reason: String, restarts: usize) {
    if !followed.token().is_cancelled() {
        followed.watched(Watch {
            off: Some(reason),
            restarts,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::RwLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use kartta_fs::BuildOptions;
    use kartta_lang::{Language, RawExtraction, Repository, Resolver};

    use super::*;

    /// An engine under a lock of its own, as an owner shares it, with what its follow reported last.
    #[derive(Debug)]
    struct Owner {
        engine: RwLock<Engine>,
        token: CancelToken,
        watch: RwLock<Watch>,
        walked: RwLock<Vec<Vec<String>>>,
    }

    impl Followed for Owner {
        fn token(&self) -> &CancelToken {
            &self.token
        }

        fn refreshed<T>(
            &self,
            refresh: impl FnOnce(&mut Engine) -> Result<(EngineRefresh, T), BuildError>,
        ) -> Result<T, String> {
            let mut engine = self.engine.write().unwrap();
            let (refreshed, walked) = refresh(&mut engine).map_err(|error| error.to_string())?;
            self.walked.write().unwrap().push(refreshed.index.walked);
            Ok(walked)
        }

        fn watched(&self, watch: Watch) {
            *self.watch.write().unwrap() = watch;
        }
    }

    impl Owner {
        fn holds(&self, path: &str) -> bool {
            self.engine.read().unwrap().index().entry(path).is_some()
        }

        /// Whether the engine holds `path` before 10 s pass.
        fn holds_soon(&self, path: &str) -> bool {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !self.holds(path) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            self.holds(path)
        }
    }

    /// Claims nothing, and the first time a walk asks it about `asked` makes each file of `makes`, with the directories
    /// that hold it, while the walk is in the directory of `asked`.
    #[derive(Debug)]
    struct WritesMidWalk {
        root: PathBuf,
        asked: &'static str,
        makes: Vec<String>,
        written: AtomicBool,
    }

    impl WritesMidWalk {
        fn languages(
            root: &Path,
            asked: &'static str,
            makes: Vec<String>,
        ) -> Vec<Box<dyn Language>> {
            vec![Box::new(Self {
                root: root.to_owned(),
                asked,
                makes,
                written: AtomicBool::new(false),
            })]
        }
    }

    impl Language for WritesMidWalk {
        fn name(&self) -> &'static str {
            "writes-mid-walk"
        }

        fn claims(&self, path: &str) -> bool {
            if path != self.asked || self.written.swap(true, Ordering::Relaxed) {
                return false;
            }
            for made in &self.makes {
                let made = self.root.join(made);
                std::fs::create_dir_all(made.parent().unwrap()).unwrap();
                std::fs::write(made, "").unwrap();
            }
            false
        }

        fn fingerprint(&self) -> &[u8] {
            b"writes-mid-walk"
        }

        fn extract(&self, _source: &[u8]) -> RawExtraction {
            RawExtraction::default()
        }

        fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
            unreachable!("a language that claims nothing resolves nothing")
        }
    }

    /// Builds `tree` watched with `languages`, with stamps, and catches the engine up with the changes its watcher
    /// holds, as an owner does before it publishes the engine.
    fn caught_up_on(tree: &Path, languages: Vec<Box<dyn Language>>) -> (Owner, Option<Follower>) {
        let mut options = BuildOptions::default();
        options.stamps = true;
        let (outcome, watcher) = Engine::build_watched(tree, &options, languages, None).unwrap();
        let owner = Owner {
            engine: RwLock::new(outcome.engine),
            token: CancelToken::new(),
            watch: RwLock::default(),
            walked: RwLock::default(),
        };
        let follower = Follower::caught_up(&owner, watcher);
        (owner, follower)
    }

    /// A watcher that dropped changes, with the batch it hands over for them: the whole tree. It watches `elsewhere`,
    /// so an owner that kept it would follow nothing of the tree it stands in for, as on Linux it would follow no
    /// directory made while it dropped them.
    fn dropped_changes(elsewhere: &Path) -> (Watcher, Result<Vec<String>, WatchError>) {
        let (_, follower) = caught_up_on(elsewhere, Vec::new());
        (follower.unwrap().watcher, Ok(vec![String::new()]))
    }

    /// Every path `watcher` hands over until it names `path`, or 10 s pass.
    fn seen_soon(watcher: &mut Watcher, path: &str) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while Instant::now() < deadline && !seen.iter().any(|seen| seen == path) {
            seen.extend(watcher.wait(Duration::from_millis(500)).unwrap());
        }
        seen
    }

    // constraint: a change the system reports after the drain comes with a later wait, as FSEvents reports one made in
    // a stream's first moments, so a change made during the walk may reach the engine only after it was caught up
    fn held_once_told(owner: &Owner, mut watcher: Watcher, path: &str) -> Watcher {
        if !owner.holds(path) {
            let late = seen_soon(&mut watcher, path);
            watcher = caught_up(owner, watcher, Ok(late), &mut 0).unwrap();
        }
        watcher
    }

    #[test]
    fn a_caught_up_engine_holds_a_directory_made_while_it_was_built_and_follows_it() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("a.txt"), "a").unwrap();
        let languages =
            WritesMidWalk::languages(tree.path(), "a.txt", vec!["late/b.txt".to_owned()]);

        let (owner, follower) = caught_up_on(tree.path(), languages);

        assert_eq!(owner.watch.read().unwrap().off, None);
        let mut watcher = held_once_told(&owner, follower.unwrap().watcher, "late/b.txt");
        assert!(owner.holds("late/b.txt"));
        std::fs::write(tree.path().join("late/c.txt"), "c").unwrap();
        let seen = seen_soon(&mut watcher, "late/c.txt");
        assert!(seen.iter().any(|path| path == "late/c.txt"), "{seen:?}");
    }

    #[test]
    fn a_directory_made_while_the_watch_dropped_changes_or_restarted_is_taken_in_and_followed() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("a.txt"), "a").unwrap();
        let languages = WritesMidWalk::languages(
            tree.path(),
            "late/b.txt",
            vec!["late/deeper/c.txt".to_owned()],
        );
        let (owner, _) = caught_up_on(tree.path(), languages);
        std::fs::create_dir(tree.path().join("late")).unwrap();
        std::fs::write(tree.path().join("late/b.txt"), "b").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let (dropped, batch) = dropped_changes(elsewhere.path());
        let mut restarts = 0;

        let watcher = caught_up(&owner, dropped, batch, &mut restarts).unwrap();

        let mut watcher = held_once_told(&owner, watcher, "late/deeper/c.txt");
        assert!(owner.holds("late/b.txt") && owner.holds("late/deeper/c.txt"));
        assert!(restarts >= 1, "the dropped changes restarted the watch");
        assert_eq!(owner.watch.read().unwrap().restarts, restarts);
        for path in ["late/c.txt", "late/deeper/d.txt"] {
            std::fs::write(tree.path().join(path), "c").unwrap();
            let seen = seen_soon(&mut watcher, path);
            assert!(seen.iter().any(|seen| seen == path), "{path}: {seen:?}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_directory_a_gitignore_edit_shows_is_followed_once_the_watch_takes_it_in() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join(".gitignore"), "out/\n").unwrap();
        std::fs::create_dir_all(tree.path().join("out/deep")).unwrap();
        std::fs::write(tree.path().join("out/deep/a.txt"), "a").unwrap();
        let (owner, follower) = caught_up_on(tree.path(), Vec::new());
        let mut watcher = follower.unwrap().watcher;
        let mut restarts = 0;

        std::fs::write(tree.path().join(".gitignore"), "").unwrap();
        let shown = watcher.drain();
        let mut watcher = caught_up(&owner, watcher, shown, &mut restarts).unwrap();
        std::fs::write(tree.path().join("out/deep/b.txt"), "b").unwrap();
        let changed = watcher.drain();
        caught_up(&owner, watcher, changed, &mut restarts).unwrap();

        assert_eq!(
            ["out/deep/a.txt", "out/deep/b.txt"].map(|path| owner.holds(path)),
            [true, true]
        );
    }

    #[test]
    fn a_followed_engine_takes_an_edit_in_by_itself_until_its_token_is_cancelled() {
        let tree = tempfile::tempdir().unwrap();
        let (owner, follower) = caught_up_on(tree.path(), Vec::new());
        let owner = Arc::new(owner);
        let following = follower.unwrap().follow(&owner).unwrap();

        std::fs::write(tree.path().join("a.txt"), "a").unwrap();
        assert!(owner.holds_soon("a.txt"), "the follow took the edit in");
        owner.token.cancel();

        following.join().unwrap();
        assert_eq!(owner.watch.read().unwrap().off, None);
    }

    /// Follows a tree of one file, makes the change `change` makes at its root, then a file after it, and answers what
    /// the follow reported of its watch and the parts each refresh walked once it took that file in.
    #[cfg(unix)]
    fn followed_past(change: impl FnOnce(&Path)) -> (Watch, Vec<Vec<String>>) {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("a.txt"), "a").unwrap();
        let (owner, follower) = caught_up_on(tree.path(), Vec::new());
        let owner = Arc::new(owner);
        let following = follower.unwrap().follow(&owner).unwrap();

        change(tree.path());
        std::fs::write(tree.path().join("b.txt"), "b").unwrap();
        assert!(
            owner.holds_soon("b.txt"),
            "the follow took in the file made after the change"
        );
        owner.token.cancel();
        following.join().unwrap();

        let watch = owner.watch.read().unwrap().clone();
        let walked = owner.walked.read().unwrap().clone();
        (watch, walked)
    }

    #[cfg(unix)]
    #[test]
    fn a_touch_of_the_root_keeps_the_watch_and_walks_no_whole_tree() {
        let (watch, walked) = followed_past(|root| {
            let root = std::fs::File::open(root).unwrap();
            root.set_modified(std::time::SystemTime::now()).unwrap();
        });

        assert_eq!(watch, Watch::default());
        assert!(walked.iter().all(|walked| walked != &[""]), "{walked:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_name_that_is_not_utf8_made_at_the_root_keeps_the_watch_and_walks_no_whole_tree() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let (watch, walked) = followed_past(|root| {
            std::fs::write(root.join(OsStr::from_bytes(b"bad\xff")), "x").unwrap();
        });

        assert_eq!(watch, Watch::default());
        assert!(walked.iter().all(|walked| walked != &[""]), "{walked:?}");
    }

    /// Follows `root`, makes the change `went` makes to it, and answers what the follow reported of its watch once it
    /// stopped, or after 10 s.
    fn stopped_once(root: &Path, went: impl FnOnce(&Path)) -> Watch {
        let (owner, follower) = caught_up_on(root, Vec::new());
        let owner = Arc::new(owner);
        let following = follower.unwrap().follow(&owner).unwrap();

        went(root);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !following.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        owner.token.cancel();
        following.join().unwrap();
        owner.watch.read().unwrap().clone()
    }

    /// A tree of one file at `tree` below `parent`.
    fn tree_below(parent: &Path) -> PathBuf {
        let root = parent.join("tree");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        root
    }

    #[test]
    fn a_follow_whose_root_was_deleted_or_moved_away_names_it_and_stops_once_no_restart_finds_the_root()
     {
        let parent = tempfile::tempdir().unwrap();
        let deleted = tree_below(&parent.path().join("deleted"));
        let moved = tree_below(&parent.path().join("moved"));

        let watches = [
            stopped_once(&deleted, |root| std::fs::remove_dir_all(root).unwrap()),
            stopped_once(&moved, |root| {
                std::fs::rename(root, root.with_file_name("away")).unwrap();
            }),
        ];

        for watch in watches {
            let off = watch.off.as_deref().unwrap_or_default();
            assert!(
                off.starts_with("the root was deleted or moved, and a new watch failed: "),
                "{watch:?}"
            );
            assert_eq!(watch.restarts, 0, "{watch:?}");
        }
    }

    #[test]
    fn a_follow_whose_root_was_replaced_restarts_over_the_tree_now_at_its_path() {
        let parent = tempfile::tempdir().unwrap();
        let root = tree_below(parent.path());
        let next = parent.path().join("next");
        std::fs::create_dir(&next).unwrap();
        std::fs::write(next.join("b.txt"), "b").unwrap();
        let (owner, follower) = caught_up_on(&root, Vec::new());
        let owner = Arc::new(owner);
        let following = follower.unwrap().follow(&owner).unwrap();

        std::fs::rename(&root, parent.path().join("old")).unwrap();
        std::fs::rename(&next, &root).unwrap();
        assert!(
            owner.holds_soon("b.txt"),
            "the restart took the new tree in"
        );
        std::fs::write(root.join("c.txt"), "c").unwrap();
        assert!(
            owner.holds_soon("c.txt"),
            "the new watcher follows the new tree"
        );
        owner.token.cancel();
        following.join().unwrap();

        let watch = owner.watch.read().unwrap().clone();
        assert_eq!(watch.off, None);
        assert!(watch.restarts >= 1, "{watch:?}");
    }

    #[test]
    fn a_follow_ends_once_nothing_else_holds_its_engine() {
        let tree = tempfile::tempdir().unwrap();
        let (owner, follower) = caught_up_on(tree.path(), Vec::new());
        let owner = Arc::new(owner);
        let following = follower.unwrap().follow(&owner).unwrap();

        drop(owner);

        following.join().unwrap();
    }

    #[test]
    fn a_watcher_that_could_not_start_turns_the_watch_off_with_its_reason() {
        let tree = tempfile::tempdir().unwrap();
        let (owner, _) = caught_up_on(tree.path(), Vec::new());
        let unsupported = WatchError::Unsupported {
            native_path: tree.path().to_owned(),
        };
        let reason = unsupported.to_string();

        assert!(Follower::caught_up(&owner, Err(unsupported)).is_none());
        assert_eq!(
            *owner.watch.read().unwrap(),
            Watch {
                off: Some(reason),
                restarts: 0
            }
        );
    }

    #[test]
    fn a_follow_that_stops_says_why_and_how_often_it_restarted_unless_its_token_was_cancelled() {
        let tree = tempfile::tempdir().unwrap();
        let (owner, _) = caught_up_on(tree.path(), Vec::new());

        stop(&owner, "a refresh failed: gone".to_owned(), 2);
        let stopped = owner.watch.read().unwrap().clone();
        owner.token.cancel();
        stop(&owner, "the watch stopped".to_owned(), 3);

        let off = Watch {
            off: Some("a refresh failed: gone".to_owned()),
            restarts: 2,
        };
        assert_eq!(stopped, off);
        assert_eq!(*owner.watch.read().unwrap(), off);
    }

    #[test]
    fn a_batch_naming_paths_is_refreshed_and_an_empty_one_changes_nothing() {
        assert_eq!(
            next(Ok(vec!["a.rs".to_owned(), "src".to_owned()])),
            Next::Refresh(vec!["a.rs".to_owned(), "src".to_owned()])
        );
        assert_eq!(next(Ok(Vec::new())), Next::Refresh(Vec::new()));
    }

    #[test]
    fn a_watcher_that_dropped_changes_lost_its_root_stopped_or_met_the_limit_is_restarted_with_that_reason()
     {
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
            next(Err(WatchError::RootGone)),
            Next::Restart("the root was deleted or moved".to_owned())
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
