use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use kartta::{
    BuildError, BuildOptions, CancelToken, Engine, EngineOutcome, EngineRefresh, WatchError,
    Watcher,
};
use kartta_fs::EntryKind;
use tokio::sync::watch;

/// How long a rebuild waits between looks at whether a query still holds the engine it replaces.
const RELEASE_POLL: Duration = Duration::from_millis(10);

/// What one build is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) root: PathBuf,
    pub(crate) languages: Vec<&'static str>,
    pub(crate) index_ignored: bool,
}

/// The engine of one repository, with the facts about the build that every answer about the index repeats.
#[derive(Debug)]
pub(crate) struct Indexed {
    pub(crate) root: String,
    pub(crate) gitignore: bool,
    pub(crate) engine: Engine,
    pub(crate) errors: Vec<Failure>,
    pub(crate) watch: Watch,
}

/// Whether an index follows its tree by itself, which decides whether an answer can be older than the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Watch {
    Following,
    /// The index takes a change in only through the `refresh` tool, for the reason given.
    Off(String),
}

/// One failure the tree gave the index: its text, and the root-relative path it names when that path is in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    path: Option<String>,
    pub(crate) text: String,
}

impl Failure {
    fn new(error: &BuildError, root: &Path) -> Self {
        let path = error
            .native_path()
            .and_then(|path| path.strip_prefix(root).ok())
            .map(|relative| {
                relative
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            });
        Self {
            path,
            text: error.to_string(),
        }
    }
}

/// What one refresh did, and how long it took once it held the index.
#[derive(Debug)]
pub(crate) struct Refreshed {
    pub(crate) refresh: EngineRefresh,
    pub(crate) elapsed: Duration,
}

impl Indexed {
    /// How many entries of the index are not directories.
    pub(crate) fn files(&self) -> usize {
        self.engine
            .index()
            .entries()
            .filter(|entry| entry.kind() != EntryKind::Dir)
            .count()
    }

    /// A row for each family whose graph holds a file, or whose languages claimed one.
    pub(crate) fn graphs(&self) -> Vec<GraphRow> {
        let stats = self.engine.stats();
        stats
            .families
            .iter()
            .map(|(family, graph)| {
                let (claimed, held) = stats
                    .languages
                    .iter()
                    .filter(|language| language.family == *family)
                    .fold((0, 0), |(claimed, held), language| {
                        (claimed + language.claimed, held + language.held)
                    });
                GraphRow {
                    family: (*family).to_owned(),
                    files: graph.files,
                    files_without_symbols: graph.files_without_symbols,
                    symbols: graph.symbols,
                    calls: graph.calls + graph.local_calls,
                    uses: graph.uses,
                    includes: graph.includes,
                    unheld: claimed.saturating_sub(held),
                }
            })
            .filter(|row| row.files > 0 || row.unheld > 0)
            .collect()
    }
}

/// A built index as the server shares it: every query reads it under one lock, and a refresh writes it under that
/// lock, so a query waits for a refresh under way.
#[derive(Debug)]
pub(crate) struct Served {
    indexed: RwLock<Indexed>,
    cancel: CancelToken,
}

impl Served {
    /// The index to answer from, or the error that names the fix when a refresh panicked partway through it.
    pub(crate) fn read(&self) -> Result<RwLockReadGuard<'_, Indexed>, String> {
        self.indexed
            .read()
            .map_err(|poisoned| broken(&poisoned.into_inner()))
    }

    /// Brings the index up to the tree at `paths`, or across the whole tree when there are none, until an
    /// `index_repo` that replaces the index cancels it.
    pub(crate) fn refresh<'p>(
        &self,
        paths: impl ExactSizeIterator<Item = &'p str>,
    ) -> Result<Refreshed, String> {
        let (refreshed, ()) = self.refreshed(|engine, cancel| {
            let refresh = if paths.len() == 0 {
                engine.refresh(cancel)
            } else {
                engine.refresh_paths(paths, cancel)
            };
            Ok((refresh?, ()))
        })?;
        Ok(refreshed)
    }

    /// Brings the index up to the tree at `paths`, as [`Served::refresh`] does, under `watcher`, which watches each
    /// directory the refresh takes in, and answers whether `watcher` still covers the tree.
    pub(crate) fn refresh_under<'p>(
        &self,
        paths: impl IntoIterator<Item = &'p str>,
        watcher: &mut Watcher,
    ) -> Result<Result<(), WatchError>, String> {
        let (_, followed) =
            self.refreshed(|engine, cancel| engine.refresh_paths_watched(paths, watcher, cancel))?;
        Ok(followed)
    }

    /// Starts a new watcher, then brings the index up to the whole tree, so the watcher holds every change made while
    /// the refresh walked for its next drain or wait to hand over, as the watcher of a build holds those made while it
    /// walked.
    pub(crate) fn refresh_watched(&self) -> Result<Watcher, String> {
        let (_, watcher) = self.refreshed(Engine::refresh_watched)?;
        watcher.map_err(|error| error.to_string())
    }

    fn refreshed<T>(
        &self,
        refresh: impl FnOnce(
            &mut Engine,
            Option<&CancelToken>,
        ) -> Result<(EngineRefresh, T), BuildError>,
    ) -> Result<(Refreshed, T), String> {
        let mut indexed = self
            .indexed
            .write()
            .map_err(|poisoned| broken(&poisoned.into_inner()))?;
        let started = Instant::now();
        let refreshed = refresh(&mut indexed.engine, Some(&self.cancel));
        let (refresh, walked) = refreshed.map_err(|error| match error {
            BuildError::Cancelled => {
                "a later index_repo replaced this index before the refresh finished".to_owned()
            }
            error => format!("cannot refresh {}: {error}", indexed.root),
        })?;
        let root = indexed.engine.index().root();
        let failed = refresh
            .errors()
            .map(|error| Failure::new(error, root))
            .collect();
        replace_failures(&mut indexed.errors, &refresh.index.walked, failed);
        drop(indexed);
        let refreshed = Refreshed {
            refresh,
            elapsed: started.elapsed(),
        };
        Ok((refreshed, walked))
    }

    /// Records that the index no longer follows its tree, and why.
    pub(crate) fn unwatch(&self, reason: String) {
        self.indexed
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .watch = Watch::Off(reason);
    }

    /// Whether an `index_repo` replaced this index, so nothing refreshes it again.
    pub(crate) fn is_replaced(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// The error a query answers once a refresh panicked while it held the index.
fn broken(indexed: &Indexed) -> String {
    format!(
        "a refresh of {} panicked partway, so the index may hold half of it - call index_repo to build it again",
        indexed.root
    )
}

/// Takes the failures of a refresh that `walked` the parts at those paths into `failures`, so they list what the tree
/// fails now, each once: a failure under a part walked again, or one the refresh met again, gives way to the
/// refresh's. A failure that names no path in the tree, such as an unwritable cache, stays until a refresh of the
/// whole tree.
fn replace_failures(failures: &mut Vec<Failure>, walked: &[String], failed: Vec<Failure>) {
    let walked: HashSet<&str> = walked.iter().map(String::as_str).collect();
    let failed_paths: HashSet<&str> = failed
        .iter()
        .filter_map(|failure| failure.path.as_deref())
        .collect();
    let failed_pathless: HashSet<&str> = failed
        .iter()
        .filter(|failure| failure.path.is_none())
        .map(|failure| failure.text.as_str())
        .collect();
    failures.retain(|failure| {
        failure.path.as_deref().map_or_else(
            || !walked.contains("") && !failed_pathless.contains(failure.text.as_str()),
            |path| {
                !failed_paths.contains(path) && !ancestors(path).any(|part| walked.contains(part))
            },
        )
    });
    failures.extend(failed);
}

/// The root-relative path `path`, and every directory above it up to the root, the empty path.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    std::iter::once("")
        .chain(path.match_indices('/').filter_map(|(at, _)| path.get(..at)))
        .chain(std::iter::once(path))
}

/// What one family's graph holds, and how many of the files its languages claimed it does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphRow {
    pub(crate) family: String,
    pub(crate) files: usize,
    pub(crate) files_without_symbols: usize,
    pub(crate) symbols: usize,
    pub(crate) calls: usize,
    pub(crate) uses: usize,
    pub(crate) includes: usize,
    pub(crate) unheld: usize,
}

pub(crate) type Outcome = Result<Arc<Served>, String>;

/// Where the server keeps its one repository: nothing yet, a build under way, the engine it built, or why it failed.
#[derive(Debug)]
pub(crate) struct Slot {
    state: Mutex<State>,
    builds: AtomicU64,
    watch: bool,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Empty,
    Building(Build),
    Ready(Arc<Served>),
    Failed {
        root: String,
        error: String,
    },
}

#[derive(Debug)]
struct Build {
    id: u64,
    request: Request,
    started: Instant,
    cancel: CancelToken,
    outcome: watch::Receiver<Option<Outcome>>,
}

impl Slot {
    /// A slot with nothing indexed, whose every built index follows its tree when `watch` is set.
    pub(crate) fn new(watch: bool) -> Self {
        Self {
            state: Mutex::default(),
            builds: AtomicU64::default(),
            watch,
        }
    }

    /// Starts building `request` on a blocking thread and returns where its outcome arrives.
    ///
    /// A build of the same request already under way is joined rather than started again. A build of another one is
    /// cancelled, and so is a refresh of the engine this one replaces. The new build starts once the engine it replaces
    /// is gone, the cancelled build's included, and once every query that was answering from it has answered, so a
    /// rebuild never holds two engines.
    pub(crate) fn start(
        self: &Arc<Self>,
        request: Request,
        options: BuildOptions,
    ) -> watch::Receiver<Option<Outcome>> {
        let mut state = self.lock();
        if let State::Building(build) = &*state {
            if build.request == request {
                return build.outcome.clone();
            }
            build.cancel.cancel();
        }
        let id = self.builds.fetch_add(1, Ordering::Relaxed);
        let (sender, outcome) = watch::channel(None);
        let cancel = CancelToken::new();
        let replaced = std::mem::replace(
            &mut *state,
            State::Building(Build {
                id,
                request: request.clone(),
                started: Instant::now(),
                cancel: cancel.clone(),
                outcome: outcome.clone(),
            }),
        );
        drop(state);
        if let State::Ready(served) = &replaced {
            served.cancel.cancel();
        }

        let slot = Arc::clone(self);
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let replaced = match replaced {
                State::Building(mut build) => runtime
                    .block_on(build.outcome.wait_for(Option::is_some))
                    .ok()
                    .and_then(|outcome| outcome.clone())
                    .and_then(Result::ok),
                State::Ready(served) => Some(served),
                State::Empty | State::Failed { .. } => None,
            };
            if let Some(replaced) = replaced {
                release(replaced);
            }
            let (built, watcher) = match build(&request, &options, &cancel, slot.watch) {
                Ok((served, watcher)) => (Ok(Arc::new(served)), watcher),
                Err(error) => (Err(error), None),
            };
            let ready = slot.finish(id, &request, &built);
            match (&built, watcher) {
                (Ok(served), _) if !ready => {
                    served.unwatch("a later index_repo replaced this index".to_owned());
                }
                (Ok(served), Some(watcher)) => crate::watch::follow(served, watcher),
                _ => {}
            }
            sender.send_replace(Some(built));
        });
        outcome
    }

    /// The engine to answer a query from, or the error that says why there is none.
    pub(crate) fn current(&self) -> Result<Arc<Served>, String> {
        match &*self.lock() {
            State::Ready(served) => Ok(Arc::clone(served)),
            State::Empty => Err("no repository indexed: call index_repo first".to_owned()),
            State::Building(build) => Err(format!(
                "the index of {} is still building ({}s so far) - index_stats answers once it is ready, and \
                 index_repo with the same root waits for it",
                build.request.root.display(),
                build.started.elapsed().as_secs()
            )),
            State::Failed { root, error } => Err(format!(
                "indexing {root} failed: {error} - call index_repo again"
            )),
        }
    }

    /// The root a build under way indexes and how long it has run, or `None` when no build runs.
    pub(crate) fn building(&self) -> Option<(String, Duration)> {
        let State::Building(build) = &*self.lock() else {
            return None;
        };
        Some((
            build.request.root.display().to_string(),
            build.started.elapsed(),
        ))
    }

    /// Settles the slot on the outcome of build `id`, unless a later build replaced it, and answers whether it did.
    fn finish(&self, id: u64, request: &Request, outcome: &Outcome) -> bool {
        let mut state = self.lock();
        if !matches!(&*state, State::Building(running) if running.id == id) {
            return false;
        }
        *state = match outcome {
            Ok(served) => State::Ready(Arc::clone(served)),
            Err(error) => State::Failed {
                root: request.root.display().to_string(),
                error: error.clone(),
            },
        };
        true
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Waits until nothing else holds `served`, then drops it: a query answering from it keeps it until it answers.
fn release(mut served: Arc<Served>) {
    loop {
        match Arc::try_unwrap(served) {
            Ok(unshared) => return drop(unshared),
            Err(shared) => served = shared,
        }
        std::thread::sleep(RELEASE_POLL);
    }
}

/// Builds the engine `request` asks for, and with `watch` a watcher started before the build walked, which follows the
/// tree once the index holds what changed while it was built.
fn build(
    request: &Request,
    options: &BuildOptions,
    cancel: &CancelToken,
    watch: bool,
) -> Result<(Served, Option<Watcher>), String> {
    let languages =
        kartta_languages::select(&request.languages).map_err(|error| error.to_string())?;
    let failed = |error| match error {
        BuildError::Cancelled => {
            "a later index_repo replaced this build before it finished".to_owned()
        }
        error => format!("cannot index {}: {error}", request.root.display()),
    };
    if !watch {
        let outcome =
            Engine::build(&request.root, options, languages, Some(cancel)).map_err(failed)?;
        let off = Watch::Off("turned off in the server configuration".to_owned());
        return Ok((Served::new(request, outcome, cancel, off), None));
    }
    let (outcome, watcher) =
        Engine::build_watched(&request.root, options, languages, Some(cancel)).map_err(failed)?;
    Ok(served_watched(request, outcome, cancel, watcher))
}

/// The engine `outcome` holds as served, with the changes `watcher` holds taken in, so the index answers for what
/// changed while it was built; the watcher is handed back to follow the tree from there, or the watch is off with the
/// reason it could not start or take those changes in.
fn served_watched(
    request: &Request,
    outcome: EngineOutcome,
    cancel: &CancelToken,
    watcher: Result<Watcher, WatchError>,
) -> (Served, Option<Watcher>) {
    let served = Served::new(request, outcome, cancel, Watch::Following);
    match watcher
        .map_err(|error| error.to_string())
        .and_then(|mut watcher| {
            let batch = watcher.drain();
            crate::watch::caught_up(&served, watcher, batch)
        }) {
        Ok(watcher) => (served, Some(watcher)),
        Err(reason) => {
            served.unwatch(reason);
            (served, None)
        }
    }
}

impl Served {
    fn new(request: &Request, outcome: EngineOutcome, cancel: &CancelToken, watch: Watch) -> Self {
        let root = outcome.engine.index().root();
        let errors = outcome
            .errors
            .iter()
            .map(|error| Failure::new(error, root))
            .collect();
        Self {
            indexed: RwLock::new(Indexed {
                root: request.root.display().to_string(),
                gitignore: !request.index_ignored,
                engine: outcome.engine,
                errors,
                watch,
            }),
            cancel: cancel.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use kartta::{Language, RawExtraction, Repository, Resolver};

    use super::*;

    fn failure(path: Option<&str>, text: &str) -> Failure {
        Failure {
            path: path.map(str::to_owned),
            text: text.to_owned(),
        }
    }

    fn texts(failures: &[Failure]) -> Vec<&str> {
        failures
            .iter()
            .map(|failure| failure.text.as_str())
            .collect()
    }

    #[test]
    fn a_refresh_replaces_the_failures_under_what_it_walked_and_those_it_met_again() {
        let mut failures = vec![
            failure(Some("src/a.rs"), "a unread"),
            failure(Some("src/deep/b.rs"), "b unread"),
            failure(Some("srcs/c.rs"), "c unread"),
            failure(Some("lib/d.rs"), "d unread"),
            failure(None, "cache unwritable"),
            failure(None, "cache unmergeable"),
        ];

        replace_failures(
            &mut failures,
            &["src".to_owned()],
            vec![
                failure(Some("lib/d.rs"), "d unread again"),
                failure(None, "cache unmergeable"),
            ],
        );

        assert_eq!(
            texts(&failures),
            [
                "c unread",
                "cache unwritable",
                "d unread again",
                "cache unmergeable"
            ]
        );
    }

    #[test]
    fn a_refresh_of_the_tree_replaces_every_failure() {
        let mut failures = vec![
            failure(Some("a.rs"), "a unread"),
            failure(None, "cache unwritable"),
        ];

        replace_failures(
            &mut failures,
            &[String::new()],
            vec![failure(Some("b.rs"), "b unread")],
        );

        assert_eq!(texts(&failures), ["b unread"]);
    }

    #[test]
    fn a_rebuild_releases_the_watched_index_before_it_builds_its_own() {
        let first = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("a.go"), "package a\n\nfunc A() {}\n").unwrap();
        let second = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let slot = Arc::new(Slot::new(true));
        let options = crate::Config::default().build_options(false);
        let request = |root: &Path| Request {
            root: root.to_owned(),
            languages: vec!["go"],
            index_ignored: false,
        };

        let mut built = slot.start(request(first.path()), options.clone());
        let watched = Arc::downgrade(
            &runtime
                .block_on(built.wait_for(Option::is_some))
                .unwrap()
                .clone()
                .unwrap()
                .unwrap(),
        );
        drop(built);
        std::fs::write(first.path().join("b.go"), "package a\n\nfunc B() {}\n").unwrap();
        let mut rebuilt = slot.start(request(second.path()), options);
        drop(runtime.block_on(rebuilt.wait_for(Option::is_some)).unwrap());

        assert!(
            watched.upgrade().is_none(),
            "the first index outlived the rebuild"
        );
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

        fn resolver<'r>(&'r self, _repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
            unreachable!("a language that claims nothing resolves nothing")
        }
    }

    /// Builds `tree` watched with `languages` and serves the index as `index_repo` does, with the changes its watcher
    /// holds taken in.
    fn served_from(tree: &Path, languages: Vec<Box<dyn Language>>) -> (Served, Option<Watcher>) {
        let request = Request {
            root: tree.to_owned(),
            languages: Vec::new(),
            index_ignored: false,
        };
        let options = crate::Config::default().build_options(false);
        let cancel = CancelToken::new();
        let (outcome, watcher) =
            Engine::build_watched(tree, &options, languages, Some(&cancel)).unwrap();
        served_watched(&request, outcome, &cancel, watcher)
    }

    /// A watcher that dropped changes, with the batch it hands over for them: the whole tree. It watches `elsewhere`,
    /// so a server that kept it would follow nothing of the tree it stands in for, as on Linux it would follow no
    /// directory made while it dropped them.
    fn dropped_changes(elsewhere: &Path) -> (Watcher, Result<Vec<String>, WatchError>) {
        let (_, watcher) = served_from(elsewhere, Vec::new());
        (watcher.unwrap(), Ok(vec![String::new()]))
    }

    #[test]
    fn the_published_index_holds_a_directory_made_while_it_was_built_and_follows_it() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("a.txt"), "a").unwrap();
        let languages =
            WritesMidWalk::languages(tree.path(), "a.txt", vec!["late/b.txt".to_owned()]);

        let (served, watcher) = served_from(tree.path(), languages);

        let indexed = served.read().unwrap();
        assert_eq!(indexed.watch, Watch::Following);
        assert!(indexed.engine.index().entry("late/b.txt").is_some());
        drop(indexed);
        let mut watcher = watcher.unwrap();
        std::fs::write(tree.path().join("late/c.txt"), "c").unwrap();
        let seen = seen_soon(&mut watcher, "late/c.txt");
        assert!(seen.iter().any(|path| path == "late/c.txt"), "{seen:?}");
    }

    #[test]
    fn a_restarted_watch_takes_in_what_changed_while_it_was_down_or_restarting_and_follows_it() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("a.txt"), "a").unwrap();
        let languages = WritesMidWalk::languages(
            tree.path(),
            "late/b.txt",
            vec!["late/deeper/c.txt".to_owned()],
        );
        let (served, watcher) = served_from(tree.path(), languages);
        drop(watcher);
        std::fs::create_dir(tree.path().join("late")).unwrap();
        std::fs::write(tree.path().join("late/b.txt"), "b").unwrap();

        let mut watcher = served.refresh_watched().unwrap();
        let batch = watcher.drain();
        let mut watcher = crate::watch::caught_up(&served, watcher, batch).unwrap();

        let indexed = served.read().unwrap();
        assert!(indexed.engine.index().entry("late/b.txt").is_some());
        assert!(indexed.engine.index().entry("late/deeper/c.txt").is_some());
        drop(indexed);
        std::fs::write(tree.path().join("late/deeper/d.txt"), "d").unwrap();
        let seen = seen_soon(&mut watcher, "late/deeper/d.txt");
        assert!(
            seen.iter().any(|path| path == "late/deeper/d.txt"),
            "{seen:?}"
        );
    }

    #[test]
    fn a_directory_made_while_the_watch_dropped_changes_or_restarted_is_published_and_followed() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("a.txt"), "a").unwrap();
        let languages = WritesMidWalk::languages(
            tree.path(),
            "late/b.txt",
            vec!["late/deeper/c.txt".to_owned()],
        );
        let (served, _) = served_from(tree.path(), languages);
        std::fs::create_dir(tree.path().join("late")).unwrap();
        std::fs::write(tree.path().join("late/b.txt"), "b").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let (dropped, batch) = dropped_changes(elsewhere.path());

        let watcher = crate::watch::caught_up(&served, dropped, batch);

        let indexed = served.read().unwrap();
        assert!(indexed.engine.index().entry("late/b.txt").is_some());
        assert!(indexed.engine.index().entry("late/deeper/c.txt").is_some());
        drop(indexed);
        let mut watcher = watcher.unwrap();
        std::fs::write(tree.path().join("late/c.txt"), "c").unwrap();
        let seen = seen_soon(&mut watcher, "late/c.txt");
        assert!(seen.iter().any(|path| path == "late/c.txt"), "{seen:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_directory_a_gitignore_edit_shows_is_followed_once_the_watch_takes_it_in() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join(".gitignore"), "out/\n").unwrap();
        std::fs::create_dir_all(tree.path().join("out/deep")).unwrap();
        std::fs::write(tree.path().join("out/deep/a.txt"), "a").unwrap();
        let (served, watcher) = served_from(tree.path(), Vec::new());
        let mut watcher = watcher.unwrap();

        std::fs::write(tree.path().join(".gitignore"), "").unwrap();
        let shown = watcher.drain();
        let mut watcher = crate::watch::caught_up(&served, watcher, shown).unwrap();
        std::fs::write(tree.path().join("out/deep/b.txt"), "b").unwrap();
        let changed = watcher.drain();
        crate::watch::caught_up(&served, watcher, changed).unwrap();

        let held = ["out/deep/a.txt", "out/deep/b.txt"]
            .map(|path| served.read().unwrap().engine.index().entry(path).is_some());
        assert_eq!(held, [true, true]);
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

    #[test]
    fn a_failure_names_its_path_from_the_root() {
        let root = Path::new("/repo");
        let read = BuildError::Read {
            native_path: root.join("src").join("a.rs"),
            source: std::io::Error::other("denied"),
        };
        let cache = BuildError::Cache {
            native_path: PathBuf::from("/cache"),
            source: std::io::Error::other("full"),
        };

        assert_eq!(Failure::new(&read, root).path.as_deref(), Some("src/a.rs"));
        assert_eq!(Failure::new(&cache, root).path, None);
    }
}
