use std::borrow::Cow;
use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kartta::{
    BinaryExtensions, BuildError, BuildOptions, ContentIndexStorage, Engine, IgnoreSources, Index,
    Language, LanguageFiles, PrunedNames, Stats, Watch,
};
use serde::{Deserialize, Serialize, Serializer};

use super::graph::{Wired, is_zero};
use crate::built::StoppedBuild;
use crate::error::{Error, ErrorCode};

const ERRORS_LISTED: usize = 20;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct BuildParams {
    pub(crate) id: Option<u64>,
    pub(crate) binding_id: Option<u64>,
    languages: Option<Vec<String>>,
    threads: Option<NonZeroUsize>,
    ignore: Ignore,
    pruned_names: Option<SetChange>,
    binary_extensions: Option<SetChange>,
    content_index: Option<ContentIndex>,
    content_cache: Option<PathBuf>,
    content_cache_ttl_secs: u64,
    stamps: Option<bool>,
    pub(crate) watch: bool,
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
        options.stamps = self.stamps.unwrap_or(self.watch);
        if self.watch {
            Engine::check_watched(&options).map_err(|error| {
                invalid(format!(
                    "{error}; leave stamps out, which watch sets, or set it true"
                ))
            })?;
        }
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
    pub(crate) by: By,
}

/// The name of a build, a grep or a refresh that a cancel compares its ids with: the `id` the call's caller gives, or
/// the `binding_id` a binding over the wire gives each call it runs. An id by one name never stops a call by the other,
/// so the ids a binding picks for cancels of its own never meet those its callers give.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum By {
    #[default]
    Id,
    BindingId,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    message: String,
}

impl StoredErrors {
    /// The failures of a walk of the tree at `root`.
    pub(crate) fn new<'e>(root: &Path, errors: impl IntoIterator<Item = &'e BuildError>) -> Self {
        Self(
            errors
                .into_iter()
                .map(|error| {
                    let path = error.native_path();
                    StoredError {
                        kind: error.label(),
                        path: path.map(|path| path.to_string_lossy().into_owned()),
                        file: path.and_then(|path| in_tree(path, root)),
                        message: error.to_string(),
                    }
                })
                .collect(),
        )
    }

    /// Takes in `failed`, the failures of a refresh that walked again the parts at the root-relative paths `walked`, as
    /// [`kartta::replace_failures`] does.
    pub(crate) fn replace(&mut self, walked: &[String], failed: Self) {
        kartta::replace_failures(&mut self.0, walked, failed.0, |error| {
            (error.file.as_deref().map(Cow::Borrowed), &error.message)
        });
    }
}

/// The root-relative forward-slash path of `path`, when it is in the tree under `root`.
fn in_tree(path: &Path, root: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(
        relative
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
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

#[derive(Serialize)]
pub(crate) struct StatsAnswer<'e> {
    entries: usize,
    threads: usize,
    languages: Vec<Wired<LanguageFiles<'e>>>,
    graphs: Vec<GraphStats<'e>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_index: Option<ContentIndexView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_index_failure: Option<&'e str>,
    errors: &'e StoredErrors,
    watch: WatchView<'e>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_build: Option<StoppedView>,
}

/// What a file query's answer reads from when a cancel stopped the last build: the entries that build's index holds,
/// and what its walk turned away, a floor on what the index lacks.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct StoppedView {
    pub(crate) entries: usize,
    pub(crate) unvisited: usize,
}

/// Whether the engine follows its tree: `following` while a watcher refreshes it from each change, else `reason` says
/// why the watch a build asked for is off, and a build that asked for none carries no reason. `restarts` counts the
/// watchers started over, absent when none was.
#[derive(Serialize)]
pub(crate) struct WatchView<'e> {
    following: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'e str>,
    #[serde(skip_serializing_if = "is_zero")]
    restarts: usize,
}

impl<'e> WatchView<'e> {
    /// What `watch` reports, `None` for a build that asked for no watch.
    pub(crate) fn new(watch: Option<&'e Watch>) -> Self {
        Self {
            following: watch.is_some_and(|watch| watch.off.is_none()),
            reason: watch.and_then(|watch| watch.off.as_deref()),
            restarts: watch.map_or(0, |watch| watch.restarts),
        }
    }
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
    #[serde(skip_serializing_if = "is_zero")]
    unindexed: usize,
    trigrams: usize,
    bytes: u64,
}

impl<'e> StatsAnswer<'e> {
    pub(crate) fn new(
        engine: &'e Engine,
        errors: &'e StoredErrors,
        watch: Option<&'e Watch>,
    ) -> Self {
        let stats = engine.stats();
        Self {
            entries: engine.index().len(),
            threads: engine.index().threads(),
            languages: stats.languages.into_iter().map(Wired).collect(),
            graphs: stats
                .families
                .into_iter()
                .map(|(name, stats)| GraphStats {
                    name,
                    stats: Wired(stats),
                })
                .collect(),
            content_index: ContentIndexView::of(engine.index()),
            content_index_failure: engine.index().content_index_failure(),
            errors,
            watch: WatchView::new(watch),
            stopped_build: None,
        }
    }

    /// What `stats` answers from the index of a build a cancel stopped, which holds no language and no graph.
    pub(crate) fn stopped(stopped: &'e StoppedBuild) -> Self {
        Self {
            entries: stopped.index.len(),
            threads: stopped.index.threads(),
            languages: Vec::new(),
            graphs: Vec::new(),
            content_index: ContentIndexView::of(&stopped.index),
            content_index_failure: stopped.index.content_index_failure(),
            errors: &stopped.errors,
            watch: WatchView::new(None),
            stopped_build: Some(stopped.view()),
        }
    }
}

impl ContentIndexView {
    fn of(index: &Index) -> Option<Self> {
        index.content_index_stats().map(|index| Self {
            files: index.files(),
            unindexed: index.unindexed(),
            trigrams: index.trigrams(),
            bytes: index.bytes(),
        })
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

    #[test]
    fn a_watch_answers_why_it_is_off_and_how_often_it_restarted_and_no_watch_says_neither() {
        let mut off = Watch::default();
        off.off = Some("the watch stopped: gone".to_owned());
        off.restarts = 2;
        let mut following = Watch::default();
        following.restarts = 1;
        let answer =
            |watch: Option<&Watch>| serde_json::to_value(WatchView::new(watch)).expect("JSON");

        assert_eq!(
            answer(Some(&off)),
            serde_json::json!({ "following": false, "reason": "the watch stopped: gone", "restarts": 2 })
        );
        assert_eq!(
            answer(Some(&following)),
            serde_json::json!({ "following": true, "restarts": 1 })
        );
        assert_eq!(answer(None), serde_json::json!({ "following": false }));
    }

    fn stored(failures: &[(Option<&str>, &str)]) -> StoredErrors {
        StoredErrors(
            failures
                .iter()
                .map(|&(path, message)| StoredError {
                    kind: "read",
                    path: path.map(|path| format!("/tree/{path}")),
                    file: path.map(str::to_owned),
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

        errors.replace(&[String::new()], stored(&[(Some("b.rs"), "b unread")]));

        assert_eq!(messages(&errors), ["b unread"]);
    }

    #[test]
    fn a_failure_in_the_tree_reads_from_the_root_however_the_root_is_spelled() {
        let file = |root: &str, path: &str| in_tree(Path::new(path), Path::new(root));

        assert_eq!(file("/tree", "/tree/a/b.rs").as_deref(), Some("a/b.rs"));
        assert_eq!(file("/tree/", "/tree/a/b.rs").as_deref(), Some("a/b.rs"));
        assert_eq!(file(".", "./a/b.rs").as_deref(), Some("a/b.rs"));
        assert_eq!(file("/tree", "/tree").as_deref(), Some(""));
        assert_eq!(file("/tree", "/cache/pack"), None);
        assert_eq!(file("/tree", "/treetop/a.rs"), None);
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
