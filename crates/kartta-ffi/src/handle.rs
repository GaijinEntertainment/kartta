use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use kartta::{BuildError, CancelToken, Engine, Graph, Limits, Walk};
use serde::Serialize;

use crate::ABI_VERSION;
use crate::error::{Error, ErrorCode, INTERNAL_ENCODE_FAILURE};
use crate::wire::{
    self, BuildParams, CancelAnswer, CancelParams, NoParams, OpenParams, StatsAnswer, StoredErrors,
    WalkParams, Walks,
};

/// Every method [`Kartta::call`] answers, as `version` lists them.
const METHODS: &[&str] = &["build", "callers", "cancel", "stats", "version"];

/// One tree, and the engine its last finished build made of it: what a C handle points to.
///
/// [`Kartta::open`] only names the tree. The `build` method walks it and builds the file index and every graph in one
/// pass, and every other method that queries asks the engine that build left. Queries run concurrently with each other
/// and with `cancel`; builds run one at a time, and a build drops the engine the handle held before it walks, so the
/// handle never holds two and a query during the walk answers [`ErrorCode::Building`]. A query that was already
/// running keeps the engine it asked until it returns. A build that fails or is cancelled leaves the handle without an
/// engine, the file index included.
#[derive(Debug)]
pub struct Kartta {
    root: PathBuf,
    built: RwLock<Option<Arc<Built>>>,
    builds: Mutex<Builds>,
    one_build_at_a_time: Mutex<()>,
}

#[derive(Debug)]
struct Built {
    engine: Engine,
    family_of_language: Vec<(String, String)>,
    errors: StoredErrors,
}

/// The builds that entered the library and have not finished, waiting their turn or running, and the ids cancels
/// named before their builds entered.
#[derive(Debug, Default)]
struct Builds {
    entered: u64,
    in_flight: Vec<InFlight>,
    cancelled_ahead: Vec<u64>,
}

#[derive(Debug)]
struct InFlight {
    ticket: u64,
    id: Option<u64>,
    token: CancelToken,
}

#[derive(Debug, Serialize)]
struct About {
    abi: u32,
    version: &'static str,
    methods: &'static [&'static str],
    languages: &'static [&'static str],
}

impl Kartta {
    /// Opens a handle on the tree that `params`, a JSON object `{"root": <path>}`, names. Nothing is walked until a
    /// build.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::InvalidRequest`] or [`ErrorCode::InvalidParams`] when `params` is not that object, and
    /// [`ErrorCode::InvalidRoot`] when the root is not a directory this process can read the metadata of.
    pub fn open(params: &[u8]) -> Result<Self, Error> {
        let OpenParams { root } = wire::params(params)?;
        let invalid_root = |reason: &dyn std::fmt::Display| {
            Error::new(
                ErrorCode::InvalidRoot,
                format!("{}: {reason}", root.display()),
            )
        };
        match fs::metadata(&root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(invalid_root(&"not a directory")),
            Err(error) => return Err(invalid_root(&error)),
        }
        Ok(Self {
            root,
            built: RwLock::new(None),
            builds: Mutex::default(),
            one_build_at_a_time: Mutex::new(()),
        })
    }

    /// Answers `method` with `params`, a JSON object or nothing, as the JSON `{"ok": <answer>}` or
    /// `{"err": {"code": <code>, "message": <message>}}`. The methods and their params are the wire the crate
    /// documents; an unknown method answers [`ErrorCode::UnknownMethod`], listing the known ones.
    #[must_use]
    pub fn call(&self, method: &str, params: &[u8]) -> Vec<u8> {
        self.answer(method, params)
            .unwrap_or_else(|error| answer_error(&error))
    }

    fn answer(&self, method: &str, params: &[u8]) -> Result<Vec<u8>, Error> {
        match method {
            "build" => self.build(&wire::params(params)?),
            "callers" => self.walk(&wire::params(params)?, Graph::callers),
            "cancel" => answer(&self.cancel(&wire::params(params)?)),
            "stats" => {
                let NoParams {} = wire::params(params)?;
                let built = self.built()?;
                answer(&StatsAnswer::new(&built.engine, &built.errors))
            }
            "version" => {
                let NoParams {} = wire::params(params)?;
                answer(&About {
                    abi: ABI_VERSION,
                    version: env!("CARGO_PKG_VERSION"),
                    methods: METHODS,
                    languages: kartta_languages::NAMES,
                })
            }
            _ => Err(Error::new(
                ErrorCode::UnknownMethod,
                format!(
                    "no method {method:?}; the methods are {}",
                    METHODS.join(", ")
                ),
            )),
        }
    }

    fn build(&self, params: &BuildParams) -> Result<Vec<u8>, Error> {
        let languages = params.languages()?;
        let options = params.options()?;
        let family_of_language = languages
            .iter()
            .map(|language| (language.name().to_owned(), language.family().to_owned()))
            .collect();

        let (ticket, token) = lock(&self.builds).enter(params.id);
        let _one_build = lock(&self.one_build_at_a_time);
        *self.built.write().unwrap_or_else(PoisonError::into_inner) = None;
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            Engine::build(&self.root, &options, languages, Some(&token))
        }));

        let mut builds = lock(&self.builds);
        builds.leave(ticket);
        let built = match outcome {
            Ok(Ok(_) | Err(BuildError::Cancelled)) if token.is_cancelled() => Err(Error::new(
                ErrorCode::BuildCancelled,
                "a cancel stopped the build; the handle holds no index until a build finishes",
            )),
            Ok(Ok(outcome)) => Ok(Arc::new(Built {
                errors: StoredErrors::new(outcome.errors()),
                engine: outcome.into_engine(),
                family_of_language,
            })),
            Ok(Err(error)) => Err(Error::new(ErrorCode::BuildFailed, error.to_string())),
            Err(payload) => Err(Error::panicked(payload.as_ref())),
        }?;
        *self.built.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&built));
        drop(builds);
        answer(&StatsAnswer::new(&built.engine, &built.errors))
    }

    fn cancel(&self, params: &CancelParams) -> CancelAnswer {
        CancelAnswer {
            stopped: lock(&self.builds).cancel(params.build),
        }
    }

    fn walk(
        &self,
        params: &WalkParams,
        walk: for<'g> fn(&'g Graph, &str, Limits) -> Option<Walk<'g>>,
    ) -> Result<Vec<u8>, Error> {
        let built = self.built()?;
        let mut walks = Walks::default();
        for (name, graph) in built.graphs(params.lang.as_deref())? {
            if let Some(found) = walk(graph, &params.name, params.limits()) {
                walks.push(name, &found);
            }
        }
        answer(&walks)
    }

    fn built(&self) -> Result<Arc<Built>, Error> {
        if let Some(built) = self.finished() {
            return Ok(built);
        }
        let idle = {
            let builds = lock(&self.builds);
            if let Some(built) = self.finished() {
                return Ok(built);
            }
            builds.in_flight.is_empty()
        };
        if idle {
            Err(Error::new(
                ErrorCode::NotBuilt,
                "no build has finished on this handle; call build first",
            ))
        } else {
            Err(Error::new(
                ErrorCode::Building,
                "a build is running; ask again once it finishes",
            ))
        }
    }

    fn finished(&self) -> Option<Arc<Built>> {
        self.built
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Built {
    fn graphs(&self, lang: Option<&str>) -> Result<impl Iterator<Item = (&str, &Graph)>, Error> {
        let family = match lang {
            None => None,
            Some(lang) => Some(self.family_named(lang)?),
        };
        Ok(self
            .engine
            .graphs()
            .filter(move |(name, _)| family.is_none_or(|family| family == *name)))
    }

    fn family_named(&self, lang: &str) -> Result<&str, Error> {
        self.family_of_language
            .iter()
            .find(|(language, family)| language == lang || family == lang)
            .map(|(_, family)| family.as_str())
            .ok_or_else(|| {
                let held: Vec<&str> = self
                    .family_of_language
                    .iter()
                    .map(|(language, _)| language.as_str())
                    .collect();
                Error::new(
                    ErrorCode::InvalidParams,
                    format!(
                        "the build holds no graph for {lang:?}; it built the languages {}",
                        held.join(", ")
                    ),
                )
            })
    }
}

impl Builds {
    fn enter(&mut self, id: Option<u64>) -> (u64, CancelToken) {
        let token = CancelToken::new();
        if let Some(id) = id
            && let Some(at) = self.cancelled_ahead.iter().position(|&ahead| ahead == id)
        {
            self.cancelled_ahead.remove(at);
            token.cancel();
        }
        self.entered += 1;
        self.in_flight.push(InFlight {
            ticket: self.entered,
            id,
            token: token.clone(),
        });
        (self.entered, token)
    }

    fn leave(&mut self, ticket: u64) {
        self.in_flight.retain(|build| build.ticket != ticket);
    }

    fn cancel(&mut self, id: Option<u64>) -> bool {
        let mut stopped = false;
        for build in &self.in_flight {
            if id.is_none() || build.id == id {
                build.token.cancel();
                stopped = true;
            }
        }
        if let Some(id) = id
            && !stopped
            && !self.cancelled_ahead.contains(&id)
        {
            self.cancelled_ahead.push(id);
        }
        stopped
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn answer(answer: &impl Serialize) -> Result<Vec<u8>, Error> {
    #[derive(Serialize)]
    struct Answered<'a, T> {
        ok: &'a T,
    }

    serde_json::to_vec(&Answered { ok: answer }).map_err(|error| {
        Error::new(
            ErrorCode::Internal,
            format!("the answer could not be encoded as JSON: {error}"),
        )
    })
}

/// The wire's `{"err": ...}` answer for `error`.
pub(crate) fn answer_error(error: &Error) -> Vec<u8> {
    #[derive(Serialize)]
    struct Failed<'a> {
        err: &'a Error,
    }

    serde_json::to_vec(&Failed { err: error })
        .unwrap_or_else(|_| [&br#"{"err":"#[..], INTERNAL_ENCODE_FAILURE, b"}"].concat())
}

#[cfg(test)]
mod tests {
    use super::Builds;

    #[test]
    fn a_cancel_repeated_before_its_build_enters_is_remembered_once() {
        let mut builds = Builds::default();

        builds.cancel(Some(7));
        builds.cancel(Some(7));

        assert_eq!(builds.cancelled_ahead, [7]);
    }
}
