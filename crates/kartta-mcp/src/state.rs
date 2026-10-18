use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use kartta::{BuildError, BuildOptions, CancelToken, Engine};
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
    pub(crate) errors: Vec<String>,
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

/// A built index as the server shares it: every query reads it under one lock.
#[derive(Debug)]
pub(crate) struct Served {
    indexed: RwLock<Indexed>,
}

impl Served {
    /// The index to answer from.
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, Indexed> {
        self.indexed.read().unwrap_or_else(PoisonError::into_inner)
    }
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
    /// cancelled. The new build starts once the engine it replaces is gone, the cancelled build's included, and once
    /// every query that was answering from it has answered, so a rebuild never holds two engines.
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
    let errors = outcome.errors().iter().map(ToString::to_string).collect();
    Ok(Served {
        indexed: RwLock::new(Indexed {
            root: request.root.display().to_string(),
            gitignore: !request.index_ignored,
            engine: outcome.into_engine(),
            errors,
        }),
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
