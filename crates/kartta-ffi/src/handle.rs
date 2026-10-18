use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use kartta::{BuildError, CancelToken, Engine};
use serde::Serialize;

use crate::ABI_VERSION;
use crate::built::Built;
use crate::error::{Error, ErrorCode};
use crate::query::QUERIES;
use crate::wire::{
    self, BuildParams, CancelAnswer, CancelParams, NoParams, OpenParams, StatsAnswer, StoredErrors,
};

/// One tree, and the engine its last finished build made of it: what a C handle points to.
///
/// [`Kartta::open`] only names the tree. The `build` method walks it and builds the file index and every graph in one
/// pass, and every other method that queries asks the engine that build left. Queries run concurrently with each other
/// and with `cancel`; builds run one at a time, and a build drops the engine the handle held before it walks, so the
/// handle never holds two and a query during the walk answers the code `building`. A query that was already
/// running keeps the engine it asked until it returns. A build that fails or is cancelled leaves the handle without an
/// engine, the file index included.
#[derive(Debug)]
pub struct Kartta {
    root: PathBuf,
    built: RwLock<Option<Arc<Built>>>,
    builds: Mutex<Builds>,
    one_build_at_a_time: Mutex<()>,
}

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
    methods: Vec<&'static str>,
    languages: &'static [&'static str],
}

impl Kartta {
    /// Opens a handle on the tree that `params`, a JSON object `{"root": <path>}`, names. Nothing is walked until a
    /// build.
    ///
    /// # Errors
    ///
    /// The code `invalid_request` or `invalid_params` when `params` is not that object, and `invalid_root` when the
    /// root is not a directory this process can read the metadata of.
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
    /// documents; an unknown method answers the code `unknown_method`, listing the known ones.
    ///
    /// A panic answers the code `internal` and never unwinds out of the call, so a caller on a thread of its own or
    /// behind a C boundary needs no guard, and the handle stays usable.
    #[must_use]
    pub fn call(&self, method: &str, params: &[u8]) -> Vec<u8> {
        panic::catch_unwind(AssertUnwindSafe(|| self.answer(method, params)))
            .unwrap_or_else(|payload| Err(Error::panicked(payload.as_ref())))
            .unwrap_or_else(|error| wire::err(&error))
    }

    fn answer(&self, method: &str, params: &[u8]) -> Result<Vec<u8>, Error> {
        match method {
            "build" => self.build(&wire::params(params)?),
            "cancel" => wire::ok(&self.cancel(&wire::params(params)?)),
            "version" => {
                let NoParams {} = wire::params(params)?;
                wire::ok(&About {
                    abi: ABI_VERSION,
                    version: env!("CARGO_PKG_VERSION"),
                    methods: methods(),
                    languages: kartta_languages::NAMES,
                })
            }
            _ => {
                let Some(&(_, query)) = QUERIES.iter().find(|(name, _)| *name == method) else {
                    return Err(Error::new(
                        ErrorCode::UnknownMethod,
                        format!(
                            "no method {method:?}; the methods are {}",
                            methods().join(", ")
                        ),
                    ));
                };
                query(self.built()?.as_ref(), params)
            }
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
                errors: StoredErrors::new(&outcome.errors),
                engine: outcome.engine,
                family_of_language,
            })),
            Ok(Err(error)) => Err(Error::new(ErrorCode::BuildFailed, error.to_string())),
            Err(payload) => Err(Error::panicked(payload.as_ref())),
        }?;
        *self.built.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&built));
        drop(builds);
        wire::ok(&StatsAnswer::new(&built.engine, &built.errors))
    }

    fn cancel(&self, params: &CancelParams) -> CancelAnswer {
        CancelAnswer {
            stopped: lock(&self.builds).cancel(params.build),
        }
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

fn methods() -> Vec<&'static str> {
    let mut methods: Vec<&'static str> = ["build", "cancel", "version"]
        .into_iter()
        .chain(QUERIES.iter().map(|&(name, _)| name))
        .collect();
    methods.sort_unstable();
    methods
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
