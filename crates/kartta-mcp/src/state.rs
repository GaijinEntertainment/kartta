use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use kartta::{
    BuildError, BuildOptions, CancelToken, Engine, EngineOutcome, EngineRefresh, Followed,
    Follower, Watch, WatchError, Watcher,
};
use kartta_fs::EntryKind;
use tokio::sync::watch;

/// How long `index_repo` waits for the build it started before it answers that the build is still running: under the
/// 60 s a client's request waits by default.
pub(crate) const INDEX_WAIT: Duration = Duration::from_secs(30);

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
    /// The languages a `languages` filter limited the build to, as answers echo them, or `None` when it built every
    /// one.
    pub(crate) languages: Option<String>,
    pub(crate) gitignore: Gitignore,
    pub(crate) engine: Engine,
    pub(crate) errors: Vec<Failure>,
    pub(crate) watch: Watch,
}

/// Which `.gitignore` files a build applied.
#[derive(Debug)]
pub(crate) enum Gitignore {
    /// None: the build indexed what they hide.
    Ignored,
    /// Those inside the root. `outer_top` is the top of the repository the root sits inside, when that top is above
    /// the root: git applies its `.gitignore` files above the root, and the build read none of them.
    InTree { outer_top: Option<String> },
}

impl Gitignore {
    /// What a build of `request` applied. The top is the nearest directory, the root or one above it, that holds a
    /// `.git`, as git finds it: one look in each, on the path with its links resolved, as git reads the directory it
    /// runs in.
    fn of(request: &Request) -> Self {
        if request.index_ignored {
            return Self::Ignored;
        }
        let outer_top = std::fs::canonicalize(&request.root).ok().and_then(|root| {
            let top = root
                .ancestors()
                .find(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())?;
            (top != root).then(|| top.display().to_string())
        });
        Self::InTree { outer_top }
    }
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
    request: Request,
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
        let cancel = Some(&self.cancel);
        let (refreshed, ()) = self.timed_refresh(|engine| {
            let refresh = if paths.len() == 0 {
                engine.refresh(cancel)
            } else {
                engine.refresh_paths(paths, cancel)
            };
            Ok((refresh?, ()))
        })?;
        Ok(refreshed)
    }

    fn timed_refresh<T>(
        &self,
        refresh: impl FnOnce(&mut Engine) -> Result<(EngineRefresh, T), BuildError>,
    ) -> Result<(Refreshed, T), String> {
        let mut indexed = self
            .indexed
            .write()
            .map_err(|poisoned| broken(&poisoned.into_inner()))?;
        let started = Instant::now();
        let refreshed = refresh(&mut indexed.engine);
        let (refresh, walked) = refreshed.map_err(|error| match error {
            BuildError::Cancelled => {
                "a later index_repo replaced this index before the refresh finished".to_owned()
            }
            BuildError::Misspelled { path, fault } => format!(
                "{path:?} {fault} - pass paths relative to the repository root, with / between names, as \
                 find_files prints them"
            ),
            error => format!("cannot refresh {}: {error}", indexed.root),
        })?;
        let root = indexed.engine.index().root();
        let failed = refresh
            .errors()
            .map(|error| Failure::new(error, root))
            .collect();
        kartta::replace_failures(
            &mut indexed.errors,
            &refresh.index.walked,
            failed,
            |failure| (failure.path.as_deref().map(Cow::Borrowed), &failure.text),
        );
        drop(indexed);
        let refreshed = Refreshed {
            refresh,
            elapsed: started.elapsed(),
        };
        Ok((refreshed, walked))
    }

    /// Records that the index no longer follows its tree, and why.
    fn unwatch(&self, reason: String) {
        let mut watch = Watch::default();
        watch.off = Some(reason);
        self.watched(watch);
    }
}

/// The index follows its tree until an `index_repo` replaces it, which cancels the token.
impl Followed for Served {
    fn token(&self) -> &CancelToken {
        &self.cancel
    }

    fn refreshed<T>(
        &self,
        refresh: impl FnOnce(&mut Engine) -> Result<(EngineRefresh, T), BuildError>,
    ) -> Result<T, String> {
        let (_, walked) = self.timed_refresh(refresh)?;
        Ok(walked)
    }

    fn watched(&self, watch: Watch) {
        self.indexed
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .watch = watch;
    }
}

/// The error a query answers once a refresh panicked while it held the index.
fn broken(indexed: &Indexed) -> String {
    format!(
        "a refresh of {} panicked partway, so the index may hold half of it - call index_repo to build it again",
        indexed.root
    )
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

/// What a slot that holds no index to answer from holds instead.
#[derive(Debug)]
pub(crate) enum Unready {
    Empty,
    Building { root: String, elapsed: Duration },
    Failed { root: String, error: String },
}

/// The error a query answers while the slot is unready.
impl fmt::Display for Unready {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("no repository indexed: call index_repo first"),
            Self::Building { root, elapsed } => write!(
                formatter,
                "the index of {root} is still building ({}s so far) - index_stats answers once it is ready, and \
                 index_repo with the same root, languages and index_ignored waits for it, up to {}s",
                elapsed.as_secs(),
                INDEX_WAIT.as_secs()
            ),
            Self::Failed { root, error } => write!(
                formatter,
                "indexing {root} failed: {error} - call index_repo again"
            ),
        }
    }
}

#[derive(Debug)]
struct Build {
    id: u64,
    request: Request,
    started: Instant,
    cancel: CancelToken,
    outcome: watch::Receiver<Option<Outcome>>,
}

impl Build {
    fn unready(&self) -> Unready {
        Unready::Building {
            root: self.request.root.display().to_string(),
            elapsed: self.started.elapsed(),
        }
    }
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
    /// A build of the same request already under way is joined rather than started again, and the index the same
    /// request built is answered as it is held, brought up to its tree first when it does not follow the tree, unless
    /// a refresh broke it. A build of another one is cancelled, and so is a refresh of the engine this one replaces.
    /// The new build starts once the engine it replaces is gone, the cancelled build's included, and once every query
    /// that was answering from it has answered, so a rebuild never holds two engines.
    pub(crate) fn start(
        self: &Arc<Self>,
        request: Request,
        options: BuildOptions,
    ) -> watch::Receiver<Option<Outcome>> {
        let mut state = self.lock();
        match &*state {
            State::Building(build) if build.request == request => return build.outcome.clone(),
            State::Building(build) => build.cancel.cancel(),
            State::Ready(served) if served.request == request && !served.indexed.is_poisoned() => {
                return held(Arc::clone(served));
            }
            State::Empty | State::Ready(_) | State::Failed { .. } => {}
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
            let (built, follower) = match build(&request, &options, &cancel, slot.watch) {
                Ok((served, follower)) => (Ok(Arc::new(served)), follower),
                Err(error) => (Err(error), None),
            };
            let ready = slot.finish(id, &request, &built);
            match (&built, follower) {
                (Ok(served), _) if !ready => {
                    served.unwatch("a later index_repo replaced this index".to_owned());
                }
                (Ok(served), Some(follower)) => drop(follower.follow(served)),
                _ => {}
            }
            sender.send_replace(Some(built));
        });
        outcome
    }

    /// The engine to answer a query from, or the error that says why there is none.
    pub(crate) fn current(&self) -> Result<Arc<Served>, String> {
        self.served().map_err(|unready| unready.to_string())
    }

    /// The engine to answer a query from, or what the slot holds instead.
    pub(crate) fn served(&self) -> Result<Arc<Served>, Unready> {
        match &*self.lock() {
            State::Ready(served) => Ok(Arc::clone(served)),
            State::Empty => Err(Unready::Empty),
            State::Building(build) => Err(build.unready()),
            State::Failed { root, error } => Err(Unready::Failed {
                root: root.clone(),
                error: error.clone(),
            }),
        }
    }

    /// What the slot answers while it builds `request`, or `None` once it holds anything else.
    pub(crate) fn building(&self, request: &Request) -> Option<Unready> {
        match &*self.lock() {
            State::Building(build) if build.request == *request => Some(build.unready()),
            State::Empty | State::Building(_) | State::Ready(_) | State::Failed { .. } => None,
        }
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

/// Answers `served` on a blocking thread: at once while it follows its tree, and after a whole refresh while it does
/// not.
fn held(served: Arc<Served>) -> watch::Receiver<Option<Outcome>> {
    let (sender, outcome) = watch::channel(None);
    tokio::task::spawn_blocking(move || {
        let following = served.read().map(|indexed| indexed.watch.off.is_none());
        let held = match following {
            Ok(true) => Ok(served),
            Ok(false) => served.refresh(std::iter::empty()).map(|_| served),
            Err(error) => Err(error),
        };
        sender.send_replace(Some(held));
    });
    outcome
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

/// Builds the engine `request` asks for, and with `watch` the follower of a watcher started before the build walked,
/// which follows the tree once the index holds what changed while it was built.
fn build(
    request: &Request,
    options: &BuildOptions,
    cancel: &CancelToken,
    watch: bool,
) -> Result<(Served, Option<Follower>), String> {
    let languages =
        kartta_languages::select(&request.languages).map_err(|error| error.to_string())?;
    let failed = |error| match error {
        BuildError::Stopped(_) => {
            "a later index_repo replaced this build before it finished".to_owned()
        }
        error => error.to_string(),
    };
    if !watch {
        let outcome =
            Engine::build(&request.root, options, languages, Some(cancel)).map_err(failed)?;
        let mut off = Watch::default();
        off.off = Some("turned off in the server configuration".to_owned());
        return Ok((Served::new(request, outcome, cancel, off), None));
    }
    let (outcome, watcher) =
        Engine::build_watched(&request.root, options, languages, Some(cancel)).map_err(failed)?;
    Ok(served_watched(request, outcome, cancel, watcher))
}

/// The engine `outcome` holds as served, with the changes `watcher` holds taken in, so the index answers for what
/// changed while it was built, and the follower that follows the tree from there, or none once the watch is off.
fn served_watched(
    request: &Request,
    outcome: EngineOutcome,
    cancel: &CancelToken,
    watcher: Result<Watcher, WatchError>,
) -> (Served, Option<Follower>) {
    let served = Served::new(request, outcome, cancel, Watch::default());
    let follower = Follower::caught_up(&served, watcher);
    (served, follower)
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
                languages: (request.languages.len() < kartta_languages::NAMES.len()).then(|| {
                    match request.languages.as_slice() {
                        [] => crate::render::EMPTY_LANGUAGES.to_owned(),
                        built => built.join(", "),
                    }
                }),
                gitignore: Gitignore::of(request),
                engine: outcome.engine,
                errors,
                watch,
            }),
            cancel: cancel.clone(),
            request: request.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_building_slot_names_every_field_a_waiting_index_repo_matches() {
        let building = Unready::Building {
            root: "/repo".into(),
            elapsed: Duration::from_secs(3),
        };

        for answer in [building.to_string(), crate::render::unready(&building)] {
            assert!(
                answer.contains("root, languages and index_ignored waits"),
                "{answer}"
            );
        }
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

    /// The outcome a build sends, once it sends one.
    fn settled(
        runtime: &tokio::runtime::Runtime,
        mut outcome: watch::Receiver<Option<Outcome>>,
    ) -> Outcome {
        runtime
            .block_on(outcome.wait_for(Option::is_some))
            .unwrap()
            .clone()
            .unwrap()
    }

    /// A Go file's tree, a runtime entered for the slot's blocking work, and the request of that tree.
    fn go_tree() -> (tempfile::TempDir, tokio::runtime::Runtime, Request) {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("a.go"), "package a\n\nfunc A() {}\n").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let request = Request {
            root: tree.path().to_owned(),
            languages: vec!["go"],
            index_ignored: false,
        };
        (tree, runtime, request)
    }

    #[test]
    fn a_repeat_of_the_request_held_answers_the_followed_index() {
        let (_tree, runtime, request) = go_tree();
        let _entered = runtime.enter();
        let slot = Arc::new(Slot::new(true));
        let options = crate::Config::default().build_options(false);

        let first = Arc::downgrade(
            &settled(&runtime, slot.start(request.clone(), options.clone())).unwrap(),
        );
        let again = settled(&runtime, slot.start(request, options)).unwrap();

        assert!(
            first
                .upgrade()
                .is_some_and(|first| Arc::ptr_eq(&first, &again)),
            "the repeat built again"
        );
    }

    #[test]
    fn a_repeat_of_the_request_held_refreshes_the_unwatched_index() {
        let (tree, runtime, request) = go_tree();
        let _entered = runtime.enter();
        let slot = Arc::new(Slot::new(false));
        let options = crate::Config::default().build_options(false);

        let first = Arc::downgrade(
            &settled(&runtime, slot.start(request.clone(), options.clone())).unwrap(),
        );
        std::fs::write(tree.path().join("b.go"), "package a\n\nfunc B() {}\n").unwrap();
        let again = settled(&runtime, slot.start(request, options)).unwrap();

        assert!(
            first
                .upgrade()
                .is_some_and(|first| Arc::ptr_eq(&first, &again)),
            "the repeat built again"
        );
        assert_eq!(again.read().unwrap().files(), 2, "the refresh took in b.go");
    }

    #[test]
    fn a_repeat_of_the_request_held_rebuilds_a_broken_index() {
        let (_tree, runtime, request) = go_tree();
        let _entered = runtime.enter();
        let slot = Arc::new(Slot::new(false));
        let options = crate::Config::default().build_options(false);
        let broken = settled(&runtime, slot.start(request.clone(), options.clone())).unwrap();
        let first = Arc::downgrade(&broken);
        std::thread::spawn(move || {
            let _held = broken.indexed.write().unwrap();
            panic!("a refresh panics while it holds the index");
        })
        .join()
        .unwrap_err();

        let again = settled(&runtime, slot.start(request, options)).unwrap();

        assert!(
            first.upgrade().is_none(),
            "the broken index outlived the rebuild"
        );
        assert_eq!(again.read().unwrap().files(), 1);
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
