use std::collections::HashSet;
use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kartta::{
    BinaryExtensions, BuildError, BuildOptions, ContentIndexStorage, Engine, IgnoreSources,
    Language, LanguageFiles, PrunedNames, Stats,
};
use serde::{Deserialize, Serialize, Serializer};

use super::graph::Wired;
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
    stamps: bool,
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
        options.stamps = self.stamps;
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
    pub(crate) grep: Option<u64>,
    pub(crate) refresh: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelAnswer {
    pub(crate) stopped: bool,
}

/// Every failure a build or a refresh collected, answered as their `total` and the first of them `listed`.
#[derive(Debug, Default)]
pub(crate) struct StoredErrors(Vec<StoredError>);

#[derive(Debug, Serialize)]
struct StoredError {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    message: String,
}

impl StoredErrors {
    pub(crate) fn new<'e>(errors: impl IntoIterator<Item = &'e BuildError>) -> Self {
        Self(
            errors
                .into_iter()
                .map(|error| StoredError {
                    kind: kind_of(error),
                    path: error
                        .native_path()
                        .map(|path| path.to_string_lossy().into_owned()),
                    message: error.to_string(),
                })
                .collect(),
        )
    }

    /// Takes in `failed`, the failures of a refresh that walked again the parts at the root-relative paths `walked`,
    /// so these list what the tree under `root` fails now, each once: a failure under a part walked again, or one the
    /// refresh met again, gives way to the refresh's. A failure that names no path in the tree, such as an unwritable
    /// cache, stays until a refresh of the whole tree or until a refresh meets it again.
    pub(crate) fn replace(&mut self, root: &Path, walked: &[String], failed: Self) {
        let walked: HashSet<&str> = walked.iter().map(String::as_str).collect();
        let failed_at: Vec<Option<String>> =
            failed.0.iter().map(|error| error.in_tree(root)).collect();
        let failed_paths: HashSet<&str> = failed_at.iter().flatten().map(String::as_str).collect();
        let failed_pathless: HashSet<&str> = failed
            .0
            .iter()
            .zip(&failed_at)
            .filter_map(|(error, at)| at.is_none().then_some(error.message.as_str()))
            .collect();
        self.0.retain(|error| {
            error.in_tree(root).map_or_else(
                || !walked.contains("") && !failed_pathless.contains(error.message.as_str()),
                |path| {
                    !failed_paths.contains(path.as_str())
                        && !ancestors(&path).any(|part| walked.contains(part))
                },
            )
        });
        self.0.extend(failed.0);
    }
}

impl StoredError {
    /// The root-relative forward-slash path of what failed, when it is in the tree under `root`.
    fn in_tree(&self, root: &Path) -> Option<String> {
        let relative = Path::new(self.path.as_deref()?).strip_prefix(root).ok()?;
        Some(
            relative
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }
}

/// The root-relative path `path`, and every directory above it up to the root, the empty path.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    std::iter::once("")
        .chain(path.match_indices('/').filter_map(|(at, _)| path.get(..at)))
        .chain(std::iter::once(path))
}

impl Serialize for StoredErrors {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Listed<'e> {
            total: usize,
            listed: &'e [StoredError],
        }

        Listed {
            total: self.0.len(),
            listed: self.0.get(..ERRORS_LISTED).unwrap_or(&self.0),
        }
        .serialize(serializer)
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

#[derive(Serialize)]
pub(crate) struct StatsAnswer<'e> {
    entries: usize,
    languages: Vec<Wired<LanguageFiles<'e>>>,
    graphs: Vec<GraphStats<'e>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_index: Option<ContentIndexView>,
    errors: &'e StoredErrors,
}

#[derive(Serialize)]
struct GraphStats<'e> {
    name: &'e str,
    #[serde(flatten)]
    stats: Wired<Stats>,
}

#[derive(Debug, Serialize)]
struct ContentIndexView {
    files: usize,
    trigrams: usize,
    bytes: u64,
}

impl<'e> StatsAnswer<'e> {
    pub(crate) fn new(engine: &'e Engine, errors: &'e StoredErrors) -> Self {
        let stats = engine.stats();
        Self {
            entries: engine.index().len(),
            languages: stats.languages.into_iter().map(Wired).collect(),
            graphs: stats
                .families
                .into_iter()
                .map(|(name, stats)| GraphStats {
                    name,
                    stats: Wired(stats),
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

    #[test]
    fn a_build_keeps_stamps_only_when_asked() {
        assert!(options(r#"{ "stamps": true }"#).stamps);
        assert!(!options("{}").stamps);
    }

    fn stored(failures: &[(Option<&str>, &str)]) -> StoredErrors {
        StoredErrors(
            failures
                .iter()
                .map(|&(path, message)| StoredError {
                    kind: "read",
                    path: path.map(|path| format!("/tree/{path}")),
                    message: message.to_owned(),
                })
                .collect(),
        )
    }

    fn messages(errors: &StoredErrors) -> Vec<&str> {
        errors
            .0
            .iter()
            .map(|error| error.message.as_str())
            .collect()
    }

    #[test]
    fn a_refresh_replaces_the_failures_under_what_it_walked_and_those_it_met_again() {
        let mut errors = stored(&[
            (Some("src/a.rs"), "a unread"),
            (Some("src/deep/b.rs"), "b unread"),
            (Some("srcs/c.rs"), "c unread"),
            (Some("lib/d.rs"), "d unread"),
            (None, "cache unwritable"),
            (None, "cache unmergeable"),
        ]);

        errors.replace(
            Path::new("/tree"),
            &["src".to_owned()],
            stored(&[
                (Some("lib/d.rs"), "d unread again"),
                (None, "cache unmergeable"),
            ]),
        );

        assert_eq!(
            messages(&errors),
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
        let mut errors = stored(&[(Some("a.rs"), "a unread"), (None, "cache unwritable")]);

        errors.replace(
            Path::new("/tree"),
            &[String::new()],
            stored(&[(Some("b.rs"), "b unread")]),
        );

        assert_eq!(messages(&errors), ["b unread"]);
    }

    #[test]
    fn stored_errors_count_every_failure_and_list_the_first_twenty() {
        let paths: Vec<String> = (0..25).map(|at| format!("f{at}.rs")).collect();
        let failures: Vec<(Option<&str>, &str)> = paths
            .iter()
            .map(|path| (Some(path.as_str()), "unread"))
            .collect();

        let answer = serde_json::to_value(stored(&failures)).expect("JSON");

        assert_eq!(answer["total"], 25);
        assert_eq!(
            answer["listed"].as_array().map(Vec::len),
            Some(ERRORS_LISTED)
        );
    }
}
