use kartta::{BuildError, CancelToken, Engine, Graph, Unmatched};
use serde::Serialize;

use crate::error::{Error, ErrorCode};
use crate::wire::graph::Wired;
use crate::wire::{self, RefreshAnswer, StoredErrors};

const NO_STAMPS: &str = concat!(
    "the build kept no stamps, so a refresh would read every file again and build every graph again while every ",
    r#"query waits, longer than a build; call build {"stamps": true} to refresh"#,
);

#[derive(Debug)]
pub(crate) struct Built {
    pub(crate) engine: Engine,
    pub(crate) errors: StoredErrors,
    pub(crate) stamps: bool,
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
        let refresh = refreshed.map_err(|error| match error {
            BuildError::Cancelled => Error::new(
                ErrorCode::RefreshCancelled,
                "a cancel stopped the refresh; the engine answers as it did before it",
            ),
            BuildError::OutsideTree { .. } => {
                Error::new(ErrorCode::InvalidParams, error.to_string())
            }
            error => Error::new(
                ErrorCode::RefreshFailed,
                format!("{error}; the engine answers as it did before the refresh"),
            ),
        })?;
        let failed = StoredErrors::new(refresh.errors());
        let answer = wire::ok(&RefreshAnswer::new(&refresh, &failed));
        self.errors
            .replace(self.engine.index().root(), &refresh.index.walked, failed);
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
