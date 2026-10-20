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
    #[serde(rename = "matches")]
    parts: Vec<Graphed<'b, T>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_read_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_read_as: Option<String>,
}

impl<'b, T> Matches<'b, T> {
    pub(crate) fn read(found: Vec<(&'b str, T)>, [read_as]: [Option<String>; 1]) -> Self {
        Self {
            read_as,
            ..Self::new(found)
        }
    }

    pub(crate) fn read_ends(
        found: Vec<(&'b str, T)>,
        [from_read_as, to_read_as]: [Option<String>; 2],
    ) -> Self {
        Self {
            from_read_as,
            to_read_as,
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
        }
    }
}

impl Built {
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
