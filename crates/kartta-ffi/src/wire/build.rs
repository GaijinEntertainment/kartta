use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use kartta::{
    BinaryExtensions, BuildError, BuildOptions, ContentIndexStorage, Engine, IgnoreSources,
    Language, PrunedNames, SymbolKind,
};
use serde::{Deserialize, Serialize, Serializer};

use crate::error::{Error, ErrorCode};

const ERRORS_LISTED: usize = 20;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct BuildParams {
    pub(crate) id: Option<u64>,
    languages: Option<Vec<String>>,
    threads: Option<NonZeroUsize>,
    ignore: Ignore,
    pruned_names: Option<SetChange>,
    binary_extensions: Option<SetChange>,
    content_index: Option<ContentIndex>,
    content_cache: Option<PathBuf>,
    content_cache_ttl_secs: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ignore {
    #[default]
    InTree,
    None,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum SetChange {
    Extend(Vec<String>),
    Replace(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case", deny_unknown_fields)]
enum ContentIndex {
    Memory,
    File { dir: PathBuf },
    Mapped { dir: PathBuf },
}

impl BuildParams {
    pub(crate) fn languages(&self) -> Result<Vec<Box<dyn Language>>, Error> {
        self.languages.as_ref().map_or_else(
            || Ok(kartta_languages::all()),
            |names| kartta_languages::select(names).map_err(invalid),
        )
    }

    pub(crate) fn options(&self) -> Result<BuildOptions, Error> {
        let mut options = BuildOptions::default();
        options.threads = self.threads;
        options.ignore_sources = match self.ignore {
            Ignore::InTree => IgnoreSources::InTree,
            Ignore::None => IgnoreSources::None,
        };
        if let Some(change) = &self.pruned_names {
            options.pruned_names = change.apply(
                PrunedNames::builtin(),
                |names| PrunedNames::new(names),
                PrunedNames::union,
            )?;
        }
        if let Some(change) = &self.binary_extensions {
            options.binary_extensions = change.apply(
                BinaryExtensions::builtin(),
                |extensions| BinaryExtensions::new(extensions),
                BinaryExtensions::union,
            )?;
        }
        options.content_index = self.content_index.as_ref().map(|index| match index {
            ContentIndex::Memory => ContentIndexStorage::Memory,
            ContentIndex::File { dir } => ContentIndexStorage::File { dir: dir.clone() },
            ContentIndex::Mapped { dir } => ContentIndexStorage::Mapped { dir: dir.clone() },
        });
        options.content_cache.clone_from(&self.content_cache);
        options.content_cache_ttl = Duration::from_secs(self.content_cache_ttl_secs);
        Ok(options)
    }
}

impl SetChange {
    fn apply<S, E: Display>(
        &self,
        builtin: S,
        new: fn(&[String]) -> Result<S, E>,
        union: fn(S, &S) -> S,
    ) -> Result<S, Error> {
        match self {
            Self::Extend(names) => Ok(union(builtin, &new(names).map_err(invalid)?)),
            Self::Replace(names) => new(names).map_err(invalid),
        }
    }
}

fn invalid(error: impl Display) -> Error {
    Error::new(ErrorCode::InvalidParams, error.to_string())
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct CancelParams {
    pub(crate) build: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelAnswer {
    pub(crate) stopped: bool,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct StoredErrors {
    total: usize,
    listed: Vec<StoredError>,
}

#[derive(Debug, Serialize)]
struct StoredError {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    message: String,
}

impl StoredErrors {
    pub(crate) fn new(errors: &[BuildError]) -> Self {
        Self {
            total: errors.len(),
            listed: errors
                .iter()
                .take(ERRORS_LISTED)
                .map(|error| StoredError {
                    kind: kind_of(error),
                    path: error
                        .native_path()
                        .map(|path| path.to_string_lossy().into_owned()),
                    message: error.to_string(),
                })
                .collect(),
        }
    }
}

pub(crate) const fn kind_of(error: &BuildError) -> &'static str {
    match error {
        BuildError::Walk(_) => "walk",
        BuildError::Ignore { .. } => "ignore",
        BuildError::Name { .. } => "name",
        BuildError::Read { .. } => "read",
        BuildError::ContentIndex { .. } => "content_index",
        BuildError::Cancelled => "cancelled",
        BuildError::Cache { .. } => "cache",
        _ => "other",
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct StatsAnswer<'e> {
    entries: usize,
    languages: Vec<LanguageView<'e>>,
    graphs: Vec<GraphView<'e>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_index: Option<ContentIndexView>,
    errors: &'e StoredErrors,
}

#[derive(Debug, Serialize)]
struct LanguageView<'e> {
    name: &'e str,
    family: &'e str,
    claimed: usize,
    read: usize,
    cached: usize,
    held: usize,
}

#[derive(Debug, Serialize)]
struct GraphView<'e> {
    name: &'e str,
    files: usize,
    files_without_symbols: usize,
    symbols: usize,
    #[serde(serialize_with = "kinds")]
    kinds: Vec<(SymbolKind, usize)>,
    calls: usize,
    local_calls: usize,
    uses: usize,
    includes: usize,
}

#[derive(Debug, Serialize)]
struct ContentIndexView {
    files: usize,
    trigrams: usize,
    bytes: u64,
}

fn kinds<S: Serializer>(kinds: &[(SymbolKind, usize)], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_map(kinds.iter().map(|(kind, count)| (kind.label(), count)))
}

impl<'e> StatsAnswer<'e> {
    pub(crate) fn new(engine: &'e Engine, errors: &'e StoredErrors) -> Self {
        let stats = engine.stats();
        Self {
            entries: engine.index().len(),
            languages: stats
                .languages
                .iter()
                .map(|language| LanguageView {
                    name: language.name,
                    family: language.family,
                    claimed: language.claimed,
                    read: language.read,
                    cached: language.cached,
                    held: language.held,
                })
                .collect(),
            graphs: stats
                .families
                .into_iter()
                .map(|(name, stats)| GraphView {
                    name,
                    files: stats.files,
                    files_without_symbols: stats.files_without_symbols,
                    symbols: stats.symbols,
                    kinds: stats.kinds,
                    calls: stats.calls,
                    local_calls: stats.local_calls,
                    uses: stats.uses,
                    includes: stats.includes,
                })
                .collect(),
            content_index: engine
                .index()
                .content_index_stats()
                .map(|index| ContentIndexView {
                    files: index.files(),
                    trigrams: index.trigrams(),
                    bytes: index.bytes(),
                }),
            errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(params: &str) -> BuildOptions {
        serde_json::from_str::<BuildParams>(params)
            .expect("the params read")
            .options()
            .expect("the options build")
    }

    #[test]
    fn a_cache_time_to_live_is_read_in_seconds_and_left_out_is_zero() {
        assert_eq!(
            options(r#"{ "content_cache_ttl_secs": 3600 }"#).content_cache_ttl,
            Duration::from_hours(1)
        );
        assert_eq!(options("{}").content_cache_ttl, Duration::ZERO);
    }
}
