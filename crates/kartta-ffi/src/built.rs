use kartta::{Engine, Graph};
use serde::Serialize;

use crate::error::{Error, ErrorCode};
use crate::wire::StoredErrors;

#[derive(Debug)]
pub(crate) struct Built {
    pub(crate) engine: Engine,
    pub(crate) errors: StoredErrors,
}

#[derive(Debug, Serialize)]
pub(crate) struct Graphed<'b, T> {
    graph: &'b str,
    #[serde(flatten)]
    answer: T,
}

#[derive(Debug, Serialize)]
pub(crate) struct Matches<'b, T> {
    matches: Vec<Graphed<'b, T>>,
}

impl Built {
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

    pub(crate) fn graph_of(&self, file: &str) -> Option<(&str, &Graph)> {
        let claiming = self.engine.graph_of(file)?;
        self.engine
            .graphs()
            .find(|(_, graph)| std::ptr::eq(*graph, claiming))
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
