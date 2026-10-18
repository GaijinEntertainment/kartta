use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use kartta::{BuildError, BuildOptions, CancelToken, Engine, EngineRefresh};
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

    pub(crate) fn graphs(&self) -> Vec<GraphRow> {
        graph_rows(&self.engine)
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
        let mut indexed = self
            .indexed
            .write()
            .map_err(|poisoned| broken(&poisoned.into_inner()))?;
        let started = Instant::now();
        let cancel = Some(&self.cancel);
        let refreshed = if paths.len() == 0 {
            indexed.engine.refresh(cancel)
        } else {
            indexed.engine.refresh_paths(paths, cancel)
        };
        let refresh = refreshed.map_err(|error| match error {
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
        replace_failures(&mut indexed.errors, refresh.index().walked(), failed);
        drop(indexed);
        Ok(Refreshed {
            refresh,
            elapsed: started.elapsed(),
        })
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
#[derive(Debug, Default)]
pub(crate) struct Slot {
    state: Mutex<State>,
    builds: AtomicU64,
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

/// The slot as one answer sees it.
#[derive(Debug)]
pub(crate) enum Status {
    Empty,
    Building { root: String, elapsed: Duration },
    Ready(Arc<Served>),
    Failed { root: String, error: String },
}

impl Slot {
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
            let built = build(&request, &options, &cancel).map(Arc::new);
            slot.finish(id, &request, &built);
            sender.send_replace(Some(built));
        });
        outcome
    }

    /// The engine to answer a query from, or the error that says why there is none.
    pub(crate) fn current(&self) -> Result<Arc<Served>, String> {
        match self.status() {
            Status::Ready(served) => Ok(served),
            Status::Empty => Err("no repository indexed: call index_repo first".to_owned()),
            Status::Building { root, elapsed } => Err(format!(
                "the index of {root} is still building ({}s so far) - index_stats answers once it is ready, and \
                 index_repo with the same root waits for it",
                elapsed.as_secs()
            )),
            Status::Failed { root, error } => Err(format!(
                "indexing {root} failed: {error} - call index_repo again"
            )),
        }
    }

    pub(crate) fn status(&self) -> Status {
        match &*self.lock() {
            State::Empty => Status::Empty,
            State::Building(build) => Status::Building {
                root: build.request.root.display().to_string(),
                elapsed: build.started.elapsed(),
            },
            State::Ready(served) => Status::Ready(Arc::clone(served)),
            State::Failed { root, error } => Status::Failed {
                root: root.clone(),
                error: error.clone(),
            },
        }
    }

    fn finish(&self, id: u64, request: &Request, outcome: &Outcome) {
        let mut state = self.lock();
        if !matches!(&*state, State::Building(running) if running.id == id) {
            return;
        }
        *state = match outcome {
            Ok(served) => State::Ready(Arc::clone(served)),
            Err(error) => State::Failed {
                root: request.root.display().to_string(),
                error: error.clone(),
            },
        };
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

fn build(
    request: &Request,
    options: &BuildOptions,
    cancel: &CancelToken,
) -> Result<Served, String> {
    let languages =
        kartta_languages::select(&request.languages).map_err(|error| error.to_string())?;
    let outcome = Engine::build(&request.root, options, languages, Some(cancel)).map_err(
        |error| match error {
            BuildError::Cancelled => {
                "a later index_repo replaced this build before it finished".to_owned()
            }
            error => format!("cannot index {}: {error}", request.root.display()),
        },
    )?;
    let root = outcome.engine().index().root();
    let errors = outcome
        .errors()
        .iter()
        .map(|error| Failure::new(error, root))
        .collect();
    Ok(Served {
        indexed: RwLock::new(Indexed {
            root: request.root.display().to_string(),
            gitignore: !request.index_ignored,
            engine: outcome.into_engine(),
            errors,
        }),
        cancel: cancel.clone(),
    })
}

/// A row for each family whose graph holds a file, or whose languages claimed one.
fn graph_rows(engine: &Engine) -> Vec<GraphRow> {
    let stats = engine.stats();
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

#[cfg(test)]
mod tests {
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
