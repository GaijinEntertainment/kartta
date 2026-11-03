use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use kartta::{
    BuildError, BuildOptions, CancelToken, Engine, EngineRefresh, Followed, Follower, Graph, Index,
    Language, PathFault, Stopped, Unmatched, Watch,
};
use serde::Serialize;

use crate::error::{Error, ErrorCode};
use crate::wire::graph::Wired;
use crate::wire::{self, RefreshAnswer, StoppedView, StoredErrors};

const NO_STAMPS: &str = concat!(
    "the build kept no stamps, so a refresh would read every file again and build every graph again while every ",
    r#"query waits, longer than a build; call build {"stamps": true} to refresh"#,
);

pub(crate) const BROKEN: &str =
    "a refresh panicked partway, so the engine may hold half of it; call build to build it again";

#[derive(Debug)]
pub(crate) struct Built {
    pub(crate) engine: Engine,
    pub(crate) errors: StoredErrors,
    pub(crate) stamps: bool,
    /// What the follow of a build that asked for one reports; `None` when the build asked for none.
    pub(crate) watch: Option<Watch>,
}

/// A built engine as the handle shares it: every query reads it under one lock and a refresh writes it under that lock,
/// so a query waits for a refresh under way. The follow of a build that asked for one refreshes it the same way until
/// the build after it or the handle's drop cancels the follow's own token, which `cancel` never reaches.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) built: RwLock<Built>,
    follow: CancelToken,
}

/// What the last build left on a handle: an engine, or the index of a build a cancel stopped.
#[derive(Debug, Clone)]
pub(crate) enum Held {
    Built(Arc<Shared>),
    Stopped(Arc<StoppedBuild>),
}

/// The index of a build a cancel stopped, which answers the file queries alone: it holds what the walk reached, and no
/// graph, no follow and nothing a refresh could carry over.
#[derive(Debug)]
pub(crate) struct StoppedBuild {
    pub(crate) index: Index,
    pub(crate) errors: StoredErrors,
    unvisited: usize,
}

impl StoppedBuild {
    pub(crate) fn new(stopped: Stopped) -> Self {
        let Stopped {
            index,
            errors,
            unvisited,
            ..
        } = stopped;
        Self {
            errors: StoredErrors::new(index.root(), &errors),
            index,
            unvisited,
        }
    }

    pub(crate) const fn view(&self) -> StoppedView {
        StoppedView {
            entries: self.index.len(),
            unvisited: self.unvisited,
        }
    }

    /// What the file queries answer from, as a failure's message says it.
    pub(crate) fn answers(&self) -> String {
        let StoppedView { entries, unvisited } = self.view();
        let reached = if unvisited == 0 {
            format!("the {entries} entries its walk reached")
        } else {
            format!("the {entries} entries its walk reached, and it turned {unvisited} away")
        };
        format!("glob, list, fuzzy, grep and stats answer from {reached}; call build for the rest")
    }
}

impl Shared {
    /// Builds `root` as [`Engine::build`] does.
    pub(crate) fn build(
        root: &Path,
        options: &BuildOptions,
        languages: Vec<Box<dyn Language>>,
        cancel: &CancelToken,
    ) -> Result<Self, BuildError> {
        let outcome = Engine::build(root, options, languages, Some(cancel))?;
        Ok(Self::new(outcome, options.stamps, None))
    }

    /// Builds `root` as [`Engine::build_watched`] does, with the changes its watcher holds taken in, and answers the
    /// follower that follows the tree from there, or none once the watch is off.
    pub(crate) fn build_watched(
        root: &Path,
        options: &BuildOptions,
        languages: Vec<Box<dyn Language>>,
        cancel: &CancelToken,
    ) -> Result<(Self, Option<Follower>), BuildError> {
        let (outcome, watcher) = Engine::build_watched(root, options, languages, Some(cancel))?;
        let shared = Self::new(outcome, options.stamps, Some(Watch::default()));
        let follower = Follower::caught_up(&shared, watcher);
        Ok((shared, follower))
    }

    fn new(outcome: kartta::EngineOutcome, stamps: bool, watch: Option<Watch>) -> Self {
        Self {
            built: RwLock::new(Built {
                errors: StoredErrors::new(outcome.engine.index().root(), &outcome.errors),
                engine: outcome.engine,
                stamps,
                watch,
            }),
            follow: CancelToken::new(),
        }
    }

    /// Ends the follow, if the build asked for one: a refresh of it under way stops before its walk ends, and the
    /// follow ends at its next look.
    pub(crate) fn unfollow(&self) {
        self.follow.cancel();
    }
}

impl Followed for Shared {
    fn token(&self) -> &CancelToken {
        &self.follow
    }

    fn refreshed<T>(
        &self,
        refresh: impl FnOnce(&mut Engine) -> Result<(EngineRefresh, T), BuildError>,
    ) -> Result<T, String> {
        let mut built = self.built.write().map_err(|_| BROKEN.to_owned())?;
        let (refresh, walked) = refresh(&mut built.engine).map_err(|error| error.to_string())?;
        let failed = StoredErrors::new(built.engine.index().root(), refresh.errors());
        built.errors.replace(&refresh.index.walked, failed);
        drop(built);
        Ok(walked)
    }

    fn watched(&self, watch: Watch) {
        self.built
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .watch = Some(watch);
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Graphed<'b, T> {
    graph: &'b str,
    #[serde(flatten)]
    answer: T,
}

#[derive(Debug, Serialize)]
pub(crate) struct Matches<'b, T> {
    #[serde(rename = "matches")]
    parts: Vec<Graphed<'b, T>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_read_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_read_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unmatched: Option<Wired<Unmatched<'b>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_unmatched: Option<Wired<Unmatched<'b>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_unmatched: Option<Wired<Unmatched<'b>>>,
}

impl<'b, T> Matches<'b, T> {
    pub(crate) fn read(
        found: Vec<(&'b str, T)>,
        [read_as]: [Option<String>; 1],
        unmatched: Option<Unmatched<'b>>,
    ) -> Self {
        Self {
            read_as,
            unmatched: unmatched.map(Wired),
            ..Self::new(found)
        }
    }

    pub(crate) fn read_ends(
        found: Vec<(&'b str, T)>,
        [from_read_as, to_read_as]: [Option<String>; 2],
        [from_unmatched, to_unmatched]: [Option<Unmatched<'b>>; 2],
    ) -> Self {
        Self {
            from_read_as,
            to_read_as,
            from_unmatched: from_unmatched.map(Wired),
            to_unmatched: to_unmatched.map(Wired),
            ..Self::new(found)
        }
    }

    fn new(found: impl IntoIterator<Item = (&'b str, T)>) -> Self {
        Self {
            parts: found
                .into_iter()
                .map(|(graph, answer)| Graphed { graph, answer })
                .collect(),
            read_as: None,
            from_read_as: None,
            to_read_as: None,
            unmatched: None,
            from_unmatched: None,
            to_unmatched: None,
        }
    }
}

/// The engine's refusal of the first of `paths` it refuses by its spelling, as the wire answers it.
///
/// A refresh asks this before it reads the handle's state, so a wrong path answers `invalid_params` before `not_built`
/// or `no_stamps` would, neither of which reaches the engine's own refusal.
pub(crate) fn misspelled(paths: Option<&[String]>) -> Result<(), Error> {
    let refused = paths.into_iter().flatten().find_map(|path| {
        Some(BuildError::Misspelled {
            path: path.clone(),
            fault: PathFault::of(path)?,
        })
    });
    refused.map_or(Ok(()), |refused| Err(refresh_error(refused)))
}

/// The wire's answer for a refresh that `error` stopped, which leaves the engine as it was.
fn refresh_error(error: BuildError) -> Error {
    match error {
        BuildError::Cancelled => Error::new(
            ErrorCode::RefreshCancelled,
            "a cancel stopped the refresh; the engine answers as it did before it",
        ),
        BuildError::Misspelled { .. } => Error::new(
            ErrorCode::InvalidParams,
            format!(
                "{error}; pass it from the root, with / between names, as the answers spell a path"
            ),
        ),
        error => Error::new(
            ErrorCode::RefreshFailed,
            format!("{error}; the engine answers as it did before the refresh"),
        ),
    }
}

impl Built {
    /// Brings the engine up to the tree at the root-relative `paths`, or across the whole tree for `None`, until
    /// `cancel` stops the walk, answers what the refresh did, and takes its failures into the stored ones.
    pub(crate) fn refresh(
        &mut self,
        paths: Option<&[String]>,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, Error> {
        if !self.stamps {
            return Err(Error::new(ErrorCode::NoStamps, NO_STAMPS));
        }
        let refreshed = match paths {
            None => self.engine.refresh(Some(cancel)),
            Some(paths) => self
                .engine
                .refresh_paths(paths.iter().map(String::as_str), Some(cancel)),
        };
        let refresh = refreshed.map_err(refresh_error)?;
        let failed = StoredErrors::new(self.engine.index().root(), refresh.errors());
        let answer = wire::ok(&RefreshAnswer::new(&refresh, &failed, self.watch.as_ref()));
        self.errors.replace(&refresh.index.walked, failed);
        answer
    }

    pub(crate) fn each_graph<'b, T>(
        &'b self,
        lang: Option<&str>,
        ask: impl Fn(&'b Graph) -> Option<T>,
    ) -> Result<Matches<'b, T>, Error> {
        let searched = self.graphs(lang)?;
        Ok(Matches::new(
            searched.filter_map(|(graph, held)| Some((graph, ask(held)?))),
        ))
    }

    pub(crate) fn graphs(
        &self,
        lang: Option<&str>,
    ) -> Result<impl Iterator<Item = (&str, &Graph)> + Clone, Error> {
        let family = lang.map(|lang| self.family_named(lang)).transpose()?;
        Ok(self
            .engine
            .graphs()
            .filter(move |(name, _)| family.is_none_or(|family| family == *name)))
    }

    fn family_named(&self, lang: &str) -> Result<&str, Error> {
        self.engine
            .languages()
            .find(|&(language, family)| language == lang || family == lang)
            .map(|(_, family)| family)
            .ok_or_else(|| {
                let held: Vec<&str> = self
                    .engine
                    .languages()
                    .map(|(language, _)| language)
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
