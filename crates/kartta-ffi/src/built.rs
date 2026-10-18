use kartta::{Engine, Graph};
use serde::Serialize;

use crate::error::{Error, ErrorCode};
use crate::wire::StoredErrors;

/// What a finished build left: the engine, which language each graph's family holds, and what the build failed on.
#[derive(Debug)]
pub(crate) struct Built {
    pub(crate) engine: Engine,
    pub(crate) family_of_language: Vec<(String, String)>,
    pub(crate) errors: StoredErrors,
}

/// One graph's part of an answer that asks every graph, or the one a `lang` names.
#[derive(Debug, Serialize)]
pub(crate) struct Graphed<'b, T> {
    graph: &'b str,
    #[serde(flatten)]
    answer: T,
}

/// An answer of one part for each graph that answered, in the order of the engine's graphs.
#[derive(Debug, Serialize)]
pub(crate) struct Matches<'b, T> {
    matches: Vec<Graphed<'b, T>>,
}

impl Built {
    /// The answer of `ask` from every graph, or from the one `lang` names by a language or a family, leaving out the
    /// graphs that answer `None`.
    pub(crate) fn each_graph<'b, T>(
        &'b self,
        lang: Option<&str>,
        ask: impl Fn(&'b Graph) -> Option<T>,
    ) -> Result<Matches<'b, T>, Error> {
        let family = lang.map(|lang| self.family_named(lang)).transpose()?;
        Ok(Matches {
            matches: self
                .engine
                .graphs()
                .filter(|(name, _)| family.is_none_or(|family| family == *name))
                .filter_map(|(graph, held)| {
                    Some(Graphed {
                        graph,
                        answer: ask(held)?,
                    })
                })
                .collect(),
        })
    }

    /// The name and graph of the family whose language claims `file`, or `None` when no language does.
    pub(crate) fn graph_of(&self, file: &str) -> Option<(&str, &Graph)> {
        let claiming = self.engine.graph_of(file)?;
        self.engine
            .graphs()
            .find(|(_, graph)| std::ptr::eq(*graph, claiming))
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
