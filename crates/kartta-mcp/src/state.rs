use std::borrow::Cow;
use std::fmt;
use std::num::NonZeroUsize;
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

/// How long `index_root` waits for the build it started before it answers that the build is still running: under the
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
    /// The root this index was built from.
    pub(crate) fn root(&self) -> &Path {
        &self.request.root
    }

    /// The index to answer from, or the error that names the fix when a refresh panicked partway through it.
    pub(crate) fn read(&self) -> Result<RwLockReadGuard<'_, Indexed>, String> {
        self.indexed
            .read()
            .map_err(|poisoned| broken(&poisoned.into_inner()))
    }

    /// Brings the index up to the tree at `paths`, or across the whole tree when there are none, until an
    /// `index_root` that replaces the index cancels it.
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
                "a later index_root replaced or dropped this index before the refresh finished".to_owned()
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

/// The index follows its tree until an `index_root` replaces it, which cancels the token.
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

#[cfg(test)]
impl Served {
    /// Poisons the index's lock, as a refresh that panics while it writes the index does.
    pub(crate) fn poison(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _written = self.indexed.write();
            panic!("a refresh panicked");
        }));
    }
}

/// The error a query answers once a refresh panicked while it held the index.
fn broken(indexed: &Indexed) -> String {
    format!(
        "a refresh of {} panicked partway, so the index may hold half of it - call index_root to build it again",
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

/// Where the server keeps the roots it indexes, at most `bound` of them: for each, a build under way, the engine it
/// built, or why its build failed.
#[derive(Debug)]
pub(crate) struct Roots {
    table: Mutex<Table>,
    builds: AtomicU64,
    /// Held for the length of each build, so builds run one at a time.
    building: Mutex<()>,
    watch: bool,
    bound: NonZeroUsize,
}

/// The roots, in the order each was first asked to be built.
#[derive(Debug, Default)]
struct Table {
    roots: Vec<Root>,
    /// How many times a call named a held root, which stamps each root with its latest ask.
    asks: u64,
}

#[derive(Debug)]
struct Root {
    /// The root as `index_root` or the launch named it.
    path: PathBuf,
    state: State,
    /// The stamp of the latest call that named this root.
    asked: u64,
}

#[derive(Debug)]
enum State {
    Building(Build),
    Ready(Arc<Served>),
    Failed(String),
}

/// What a call finds in place of an index to answer from.
#[derive(Debug)]
pub(crate) enum Unready {
    Building {
        root: String,
        elapsed: Duration,
    },
    Failed {
        root: String,
        error: String,
    },
    /// The server holds no index of the root the call named; the miss says which roots it holds.
    Unheld(String),
}

/// The error a query answers in place of an index.
impl fmt::Display for Unready {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Building { root, elapsed } => write!(
                formatter,
                "the index of {root} is still building ({}s so far) - index_stats answers once it is ready, and \
                 index_root with the same root, languages and index_ignored waits for it, up to {}s",
                elapsed.as_secs(),
                INDEX_WAIT.as_secs()
            ),
            Self::Failed { root, error } => write!(
                formatter,
                "indexing {root} failed: {error} - call index_root again"
            ),
            Self::Unheld(miss) => formatter.write_str(miss),
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

impl Roots {
    /// No root yet, room for `bound` of them, and every built index following its tree when `watch` is set.
    pub(crate) fn new(watch: bool, bound: NonZeroUsize) -> Self {
        Self {
            table: Mutex::default(),
            builds: AtomicU64::default(),
            building: Mutex::new(()),
            watch,
            bound,
        }
    }

    /// Starts building `request` on a blocking thread and returns where its outcome arrives, or the error that says
    /// why the server takes no other root.
    ///
    /// A build of the same request already under way is joined rather than started again, and the index the same
    /// request built is answered as it is held, brought up to its tree first when it does not follow the tree, unless
    /// a refresh broke it. A build of another request for the same root is cancelled, and so is a refresh of the engine
    /// this one replaces. A root past the bound drops a root that holds no build, one that failed first and else the
    /// least recently asked, cancelled the same way; when every root builds, no root is dropped and the request is
    /// refused. The new build starts once every other build has ended, and once the engine it replaces or drops is
    /// gone, a cancelled build's included, and every query that was answering from it has answered, so the server
    /// never holds more engines than its bound.
    pub(crate) fn start(
        self: &Arc<Self>,
        request: Request,
        options: BuildOptions,
    ) -> Result<watch::Receiver<Option<Outcome>>, String> {
        let mut table = self.lock();
        let at = table.position(&request.root);
        if let Some(at) = at {
            match &table.ask(at).state {
                State::Building(build) if build.request == request => {
                    return Ok(build.outcome.clone());
                }
                State::Ready(served)
                    if served.request == request && !served.indexed.is_poisoned() =>
                {
                    return Ok(held(Arc::clone(served)));
                }
                State::Building(_) | State::Ready(_) | State::Failed(_) => {}
            }
        }
        let dropped = match at {
            Some(_) => None,
            None => table.room(self.bound)?,
        };
        let id = self.builds.fetch_add(1, Ordering::Relaxed);
        let (sender, outcome) = watch::channel(None);
        let cancel = CancelToken::new();
        let starting = State::Building(Build {
            id,
            request: request.clone(),
            started: Instant::now(),
            cancel: cancel.clone(),
            outcome: outcome.clone(),
        });
        let replaced = table.take(&request.root, at, starting).or(dropped);
        drop(table);
        if let Some(replaced) = &replaced {
            replaced.cancel();
        }

        let roots = Arc::clone(self);
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            if let Some(engine) = replaced.and_then(|replaced| replaced.into_engine(&runtime)) {
                release(engine);
            }
            let built = {
                let _alone = roots
                    .building
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                build(&request, &options, &cancel, roots.watch)
            };
            let (built, follower) = match built {
                Ok((served, follower)) => (Ok(Arc::new(served)), follower),
                Err(error) => (Err(error), None),
            };
            let ready = roots.lock().finish(id, &built);
            match (&built, follower) {
                (Ok(served), _) if !ready => {
                    served.unwatch("a later index_root replaced this index".to_owned());
                }
                (Ok(served), Some(follower)) => drop(follower.follow(served)),
                _ => {}
            }
            sender.send_replace(Some(built));
        });
        Ok(outcome)
    }

    /// The engine to answer a query about `root` from, or the error that says why there is none.
    pub(crate) fn current(&self, root: &str) -> Result<Arc<Served>, String> {
        self.served(root).map_err(|unready| unready.to_string())
    }

    /// The engine to answer a query about `root` from, or what the server holds in its place, the root stamped as
    /// asked now.
    pub(crate) fn served(&self, root: &str) -> Result<Arc<Served>, Unready> {
        let mut table = self.lock();
        let Some(at) = table.position(Path::new(root)) else {
            return Err(Unready::Unheld(table.missed(root)));
        };
        table.ask(at).served()
    }

    /// What each root holds, in the order the server was first asked to build them, none of them stamped as asked.
    pub(crate) fn listed(&self) -> Vec<Result<Arc<Served>, Unready>> {
        self.lock().roots.iter().map(Root::served).collect()
    }

    /// What the server answers while it builds `request`, or `None` once its root holds anything else.
    pub(crate) fn building(&self, request: &Request) -> Option<Unready> {
        let table = self.lock();
        let at = table.position(&request.root)?;
        match &table.roots.get(at)?.state {
            State::Building(build) if build.request == *request => Some(build.unready()),
            State::Building(_) | State::Ready(_) | State::Failed(_) => None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Table {
    fn position(&self, root: &Path) -> Option<usize> {
        self.roots.iter().position(|held| held.path == root)
    }

    /// Stamps the root at `at` with a new ask, and returns it.
    fn ask(&mut self, at: usize) -> &Root {
        self.asks += 1;
        let root = &mut self.roots[at];
        root.asked = self.asks;
        root
    }

    /// Puts `state` in the root at `at`, or in a new root at `path` when there is none, stamped as asked now, and
    /// answers the state it replaced.
    fn take(&mut self, path: &Path, at: Option<usize>, state: State) -> Option<State> {
        if let Some(at) = at {
            return Some(std::mem::replace(&mut self.roots[at].state, state));
        }
        self.roots.push(Root {
            path: path.to_owned(),
            state,
            asked: 0,
        });
        self.ask(self.roots.len() - 1);
        None
    }

    /// Makes room for one more root under `bound`: drops a root that holds no build, one that failed before one that
    /// is ready and else the least recently asked, and answers its state; or the error that names the roots when every
    /// one of them builds.
    fn room(&mut self, bound: NonZeroUsize) -> Result<Option<State>, String> {
        if self.roots.len() < bound.get() {
            return Ok(None);
        }
        let Some(at) = (0..self.roots.len())
            .filter(|&at| !matches!(self.roots[at].state, State::Building(_)))
            .min_by_key(|&at| {
                let root = &self.roots[at];
                (matches!(root.state, State::Ready(_)), root.asked)
            })
        else {
            return Err(format!(
                "this server holds KARTTA_MAX_ROOTS={bound} roots, each still building: {} - call index_root again \
                 once index_stats shows one built",
                self.listing()
            ));
        };
        Ok(Some(self.roots.remove(at).state))
    }

    /// Settles the root of build `id` on its outcome, unless a later build replaced it or the bound dropped its root,
    /// and answers whether it did.
    fn finish(&mut self, id: u64, outcome: &Outcome) -> bool {
        let Some(root) = self
            .roots
            .iter_mut()
            .find(|root| matches!(&root.state, State::Building(running) if running.id == id))
        else {
            return false;
        };
        root.state = match outcome {
            Ok(served) => State::Ready(Arc::clone(served)),
            Err(error) => State::Failed(error.clone()),
        };
        true
    }

    /// Why no index answers a call that named `asked`: it lies inside a held root, named as [`Root::listed`] names
    /// it, or the server holds the roots listed, or none.
    fn missed(&self, asked: &str) -> String {
        if let Some(within) = self
            .roots
            .iter()
            .find(|root| Path::new(asked).starts_with(&root.path))
        {
            return format!(
                "{asked:?} is inside the root {} this server holds - pass root {:?} with paths relative to it, or \
                 call index_root with root {asked:?} to index it apart",
                within.listed(),
                within.path.display().to_string()
            );
        }
        let holds = if self.roots.is_empty() {
            "holds no root".to_owned()
        } else {
            format!("holds {}", self.listing())
        };
        format!(
            "this server has not indexed {asked:?} - it {holds}; call index_root with root {asked:?} to index it"
        )
    }

    /// The roots as a miss lists them, each as [`Root::listed`] names it.
    fn listing(&self) -> String {
        let listed: Vec<String> = self.roots.iter().map(Root::listed).collect();
        listed.join(", ")
    }
}

impl Root {
    /// The root as a miss names it: quoted, and marked as building or failed when it is.
    fn listed(&self) -> String {
        let path = self.path.display().to_string();
        match self.state {
            State::Building(_) => format!("{path:?} (building)"),
            State::Failed(_) => format!("{path:?} (failed)"),
            State::Ready(_) => format!("{path:?}"),
        }
    }

    /// The engine this root holds, or what it holds in its place.
    fn served(&self) -> Result<Arc<Served>, Unready> {
        match &self.state {
            State::Ready(served) => Ok(Arc::clone(served)),
            State::Building(build) => Err(build.unready()),
            State::Failed(error) => Err(Unready::Failed {
                root: self.path.display().to_string(),
                error: error.clone(),
            }),
        }
    }
}

impl State {
    /// Stops what this state runs: its build, or its engine's refreshes and follow.
    fn cancel(&self) {
        match self {
            Self::Building(build) => build.cancel.cancel(),
            Self::Ready(served) => served.cancel.cancel(),
            Self::Failed(_) => {}
        }
    }

    /// The engine this state holds, a build's once it has ended, or `None` when it holds none.
    fn into_engine(self, runtime: &tokio::runtime::Handle) -> Option<Arc<Served>> {
        match self {
            Self::Building(mut build) => runtime
                .block_on(build.outcome.wait_for(Option::is_some))
                .ok()
                .and_then(|outcome| outcome.clone())
                .and_then(Result::ok),
            Self::Ready(served) => Some(served),
            Self::Failed(_) => None,
        }
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
            "a later index_root replaced this build before it finished".to_owned()
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
    fn a_building_root_names_every_field_a_waiting_index_root_matches() {
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
        let roots = Arc::new(Roots::new(true, NonZeroUsize::MIN));
        let options = crate::Config::default().build_options(false);
        let request = |root: &Path| Request {
            root: root.to_owned(),
            languages: vec!["go"],
            index_ignored: false,
        };

        let mut built = roots.start(request(first.path()), options.clone()).unwrap();
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
        let mut rebuilt = roots.start(request(second.path()), options).unwrap();
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

    /// A Go file's tree, a runtime entered for the roots' blocking work, and the request of that tree.
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
        let roots = Arc::new(Roots::new(true, NonZeroUsize::MIN));
        let options = crate::Config::default().build_options(false);

        let first = Arc::downgrade(
            &settled(
                &runtime,
                roots.start(request.clone(), options.clone()).unwrap(),
            )
            .unwrap(),
        );
        let again = settled(&runtime, roots.start(request, options).unwrap()).unwrap();

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
        let roots = Arc::new(Roots::new(false, NonZeroUsize::MIN));
        let options = crate::Config::default().build_options(false);

        let first = Arc::downgrade(
            &settled(
                &runtime,
                roots.start(request.clone(), options.clone()).unwrap(),
            )
            .unwrap(),
        );
        std::fs::write(tree.path().join("b.go"), "package a\n\nfunc B() {}\n").unwrap();
        let again = settled(&runtime, roots.start(request, options).unwrap()).unwrap();

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
        let roots = Arc::new(Roots::new(false, NonZeroUsize::MIN));
        let options = crate::Config::default().build_options(false);
        let broken = settled(
            &runtime,
            roots.start(request.clone(), options.clone()).unwrap(),
        )
        .unwrap();
        let first = Arc::downgrade(&broken);
        std::thread::spawn(move || {
            let _held = broken.indexed.write().unwrap();
            panic!("a refresh panics while it holds the index");
        })
        .join()
        .unwrap_err();

        let again = settled(&runtime, roots.start(request, options).unwrap()).unwrap();

        assert!(
            first.upgrade().is_none(),
            "the broken index outlived the rebuild"
        );
        assert_eq!(again.read().unwrap().files(), 1);
    }

    /// A request for every file of `root` in Go.
    fn request(root: &Path) -> Request {
        Request {
            root: root.to_owned(),
            languages: vec!["go"],
            index_ignored: false,
        }
    }

    /// Builds `root` and waits for its engine.
    fn built(roots: &Arc<Roots>, runtime: &tokio::runtime::Runtime, root: &Path) -> Arc<Served> {
        let options = crate::Config::default().build_options(false);
        settled(runtime, roots.start(request(root), options).unwrap()).unwrap()
    }

    /// The engine `roots` answers a call naming `root` from.
    fn asked(roots: &Roots, root: &Path) -> Result<Arc<Served>, String> {
        roots.current(&root.display().to_string())
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    #[test]
    fn a_second_root_leaves_the_first_served() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let runtime = runtime();
        let _entered = runtime.enter();
        let roots = Arc::new(Roots::new(false, NonZeroUsize::new(2).unwrap()));

        let served = Arc::downgrade(&built(&roots, &runtime, first.path()));
        let other = built(&roots, &runtime, second.path());

        assert!(
            served
                .upgrade()
                .is_some_and(|served| Arc::ptr_eq(&asked(&roots, first.path()).unwrap(), &served)),
            "the first index was dropped or built again"
        );
        assert!(Arc::ptr_eq(&asked(&roots, second.path()).unwrap(), &other));
    }

    #[test]
    fn a_root_past_the_bound_drops_the_least_recently_asked_other() {
        let trees = [(); 3].map(|()| tempfile::tempdir().unwrap());
        let [first, second, third] = trees.each_ref().map(tempfile::TempDir::path);
        let runtime = runtime();
        let _entered = runtime.enter();
        let roots = Arc::new(Roots::new(false, NonZeroUsize::new(2).unwrap()));

        drop(built(&roots, &runtime, first));
        let dropped = Arc::downgrade(&built(&roots, &runtime, second));
        drop(asked(&roots, first).unwrap());
        drop(built(&roots, &runtime, third));

        assert!(dropped.upgrade().is_none(), "the dropped index outlived it");
        assert_eq!(
            asked(&roots, second).unwrap_err(),
            format!(
                "this server has not indexed {second:?} - it holds {first:?}, {third:?}; call index_root with root \
                 {second:?} to index it",
                second = second.display().to_string(),
                first = first.display().to_string(),
                third = third.display().to_string(),
            )
        );
        assert!(asked(&roots, first).is_ok());
    }

    #[test]
    fn a_root_past_the_bound_drops_a_failed_root_before_a_built_one() {
        let (built_tree, third) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let gone = tempfile::tempdir().unwrap();
        let missing = gone.path().join("missing");
        let runtime = runtime();
        let _entered = runtime.enter();
        let roots = Arc::new(Roots::new(false, NonZeroUsize::new(2).unwrap()));
        let options = crate::Config::default().build_options(false);

        let kept = Arc::downgrade(&built(&roots, &runtime, built_tree.path()));
        assert!(settled(&runtime, roots.start(request(&missing), options).unwrap()).is_err());
        drop(built(&roots, &runtime, third.path()));

        assert!(
            kept.upgrade()
                .is_some_and(|kept| Arc::ptr_eq(&asked(&roots, built_tree.path()).unwrap(), &kept)),
            "the built root was dropped before the failed one"
        );
        assert!(
            asked(&roots, &missing)
                .unwrap_err()
                .starts_with("this server has not indexed")
        );
    }

    #[test]
    fn a_build_waits_for_the_build_under_way() {
        let tree = tempfile::tempdir().unwrap();
        let runtime = runtime();
        let _entered = runtime.enter();
        let roots = Arc::new(Roots::new(false, NonZeroUsize::MIN));
        let options = crate::Config::default().build_options(false);
        let alone = roots.building.lock().unwrap();

        let building = roots.start(request(tree.path()), options).unwrap();
        let waited = (0..200).all(|_| {
            std::thread::sleep(Duration::from_millis(10));
            roots.building(&request(tree.path())).is_some()
        });
        drop(alone);

        assert!(waited, "the build ran beside the one under way");
        assert!(settled(&runtime, building).is_ok());
    }

    #[test]
    fn a_root_past_a_bound_of_building_roots_is_refused_and_drops_none() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let runtime = runtime();
        let _entered = runtime.enter();
        let roots = Arc::new(Roots::new(false, NonZeroUsize::MIN));
        let options = crate::Config::default().build_options(false);
        let alone = roots.building.lock().unwrap();

        let building = roots.start(request(first.path()), options.clone()).unwrap();
        let refused = roots.start(request(second.path()), options).unwrap_err();
        drop(alone);

        assert_eq!(
            refused,
            format!(
                "this server holds KARTTA_MAX_ROOTS=1 roots, each still building: {:?} (building) - call \
                 index_root again once index_stats shows one built",
                first.path().display().to_string()
            )
        );
        assert!(settled(&runtime, building).is_ok());
        assert!(asked(&roots, first.path()).is_ok());
    }

    #[test]
    fn a_root_inside_a_held_root_is_named_with_the_root_that_holds_it() {
        let tree = tempfile::tempdir().unwrap();
        let runtime = runtime();
        let _entered = runtime.enter();
        let roots = Arc::new(Roots::new(false, NonZeroUsize::MIN));
        drop(built(&roots, &runtime, tree.path()));

        let inner = tree.path().join("pkg");
        let error = asked(&roots, &inner).unwrap_err();

        assert!(
            error.starts_with(&format!(
                "{:?} is inside the root {:?} this server holds - pass root",
                inner.display().to_string(),
                tree.path().display().to_string()
            )),
            "{error}"
        );
        let failed = Arc::new(Roots::new(false, NonZeroUsize::MIN));
        let missing = tree.path().join("missing");
        let options = crate::Config::default().build_options(false);
        assert!(settled(&runtime, failed.start(request(&missing), options).unwrap()).is_err());
        assert!(
            asked(&failed, &missing.join("pkg"))
                .unwrap_err()
                .contains(&format!(
                    "is inside the root {:?} (failed) this server holds",
                    missing.display().to_string()
                )),
            "a failed root reads as indexed"
        );
        assert_eq!(
            asked(&Roots::new(false, NonZeroUsize::MIN), &inner).unwrap_err(),
            format!(
                "this server has not indexed {0:?} - it holds no root; call index_root with root {0:?} to index it",
                inner.display().to_string()
            )
        );
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
