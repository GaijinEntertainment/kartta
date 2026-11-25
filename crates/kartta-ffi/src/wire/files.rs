use std::num::NonZeroUsize;

use kartta::{
    CancelToken, Case, EntryKind, Filter, FuzzyOptions, GlobOptions, GrepOptions, Index, LineKind,
    ListError, ListOptions, Listing, Matches, Patterns,
};
use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorCode};
use crate::wire::StoppedView;

fn filter(globs: Option<&[String]>) -> Result<Option<Filter>, Error> {
    globs
        .map(|globs| {
            Filter::new(globs)
                .map_err(|error| Error::new(ErrorCode::InvalidGlob, error.to_string()))
        })
        .transpose()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct ListParams {
    path: String,
    depth: usize,
    limit: usize,
    collapse: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobParams {
    globs: Vec<String>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    #[serde(default)]
    limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FuzzyParams {
    query: String,
    #[serde(default)]
    globs: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    #[serde(default)]
    limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrepParams {
    patterns: Vec<String>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    globs: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    #[serde(default)]
    context: usize,
    #[serde(default)]
    limit: usize,
    #[serde(default)]
    count_all: bool,
    #[serde(default)]
    max_line_bytes: usize,
    #[serde(default)]
    max_file_bytes: u64,
    #[serde(default)]
    pub(crate) id: Option<u64>,
    #[serde(default)]
    pub(crate) binding_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ListView<'a> {
    path: &'a str,
    entries: Vec<ListedView<'a>>,
    truncated: bool,
    excluded: usize,
    total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_build: Option<StoppedView>,
}

#[derive(Debug, Serialize)]
struct ListedView<'a> {
    path: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    binary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unlisted: Option<usize>,
}

pub(crate) const fn kind(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

impl<'a> ListView<'a> {
    pub(crate) fn new(
        index: &'a Index,
        params: &'a ListParams,
        stopped: Option<StoppedView>,
    ) -> Result<Self, Error> {
        let collapse = filter(params.collapse.as_deref())?;
        let exclude = filter(params.exclude.as_deref())?;
        let mut options = ListOptions::default()
            .depth(params.depth)
            .limit(params.limit);
        if let Some(collapse) = &collapse {
            options = options.collapse(collapse);
        }
        if let Some(exclude) = &exclude {
            options = options.exclude(exclude);
        }
        let listing = index
            .list(&params.path, &options)
            .map_err(|error| list_error(&error, stopped))?;
        Ok(Self::of(&params.path, &listing, stopped))
    }

    fn of(path: &'a str, listing: &Listing<'a>, stopped: Option<StoppedView>) -> Self {
        Self {
            path,
            entries: listing
                .items()
                .iter()
                .map(|listed| {
                    let entry = listed.entry();
                    ListedView {
                        path: entry.path().to_owned(),
                        kind: kind(entry.kind()),
                        size: listed.size(),
                        binary: listed.is_binary(),
                        target: entry.target(),
                        unlisted: listed.unlisted(),
                    }
                })
                .collect(),
            truncated: listing.truncated(),
            excluded: listing.excluded(),
            total: listing.total(),
            stopped_build: stopped,
        }
    }
}

fn list_error(error: &ListError, stopped: Option<StoppedView>) -> Error {
    let cut_short = stopped.is_some_and(|stopped| stopped.unvisited > 0);
    let note = if cut_short {
        "; a cancel stopped the walk short, so the tree may hold it"
    } else {
        ""
    };
    Error::new(ErrorCode::Listing(error.label()), format!("{error}{note}"))
}

#[derive(Debug, Serialize)]
pub(crate) struct GlobView {
    paths: Vec<String>,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    excluded: Option<NonZeroUsize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_build: Option<StoppedView>,
}

impl GlobView {
    pub(crate) fn new(
        index: &Index,
        params: &GlobParams,
        stopped: Option<StoppedView>,
    ) -> Result<Self, Error> {
        let exclude = filter(params.exclude.as_deref())?;
        let filter = Filter::new(&params.globs)
            .map_err(|error| Error::new(ErrorCode::InvalidGlob, error.to_string()))?;
        let mut options = GlobOptions::default().limit(params.limit);
        if let Some(exclude) = &exclude {
            options = options.exclude(exclude);
        }
        let found = index.glob(&filter, &options);
        let mut paths = index.paths();
        Ok(Self {
            paths: found
                .items()
                .iter()
                .map(|&at| paths.get(at).to_owned())
                .collect(),
            truncated: found.truncated(),
            excluded: NonZeroUsize::new(found.excluded()),
            stopped_build: stopped,
        })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FuzzyView {
    matches: Vec<FuzzyMatchView>,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_build: Option<StoppedView>,
}

#[derive(Debug, Serialize)]
struct FuzzyMatchView {
    path: String,
    score: i32,
}

impl FuzzyView {
    pub(crate) fn new(
        index: &Index,
        params: &FuzzyParams,
        stopped: Option<StoppedView>,
    ) -> Result<Self, Error> {
        let exclude = filter(params.exclude.as_deref())?;
        let filter = filter(params.globs.as_deref())?;
        let mut options = FuzzyOptions::default().limit(params.limit);
        if let Some(filter) = &filter {
            options = options.filter(filter);
        }
        if let Some(exclude) = &exclude {
            options = options.exclude(exclude);
        }
        let ranked = index.fuzzy(&params.query, &options);
        let mut paths = index.paths();
        Ok(Self {
            matches: ranked
                .items()
                .iter()
                .map(|found| FuzzyMatchView {
                    path: paths.get(found.position).to_owned(),
                    score: found.score,
                })
                .collect(),
            truncated: ranked.truncated(),
            matched: ranked.matched(),
            stopped_build: stopped,
        })
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct GrepView<'a> {
    files: Vec<GrepFileView<'a>>,
    lines: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_files: Option<usize>,
    cancelled: bool,
    candidates: usize,
    excluded: usize,
    symlinks: usize,
    searched: usize,
    binary: usize,
    oversized: usize,
    ruled_out: usize,
    unsearched: usize,
    failures: Vec<GrepFailureView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_build: Option<StoppedView>,
}

#[derive(Debug, Serialize)]
struct GrepFileView<'a> {
    path: String,
    lines: Vec<GrepLineView<'a>>,
}

#[derive(Debug, Serialize)]
struct GrepLineView<'a> {
    number: u64,
    kind: &'static str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cut: Option<CutView>,
}

#[derive(Debug, Serialize)]
struct CutView {
    before: usize,
    after: usize,
}

#[derive(Debug, Serialize)]
struct GrepFailureView {
    path: String,
    message: String,
}

impl GrepParams {
    pub(crate) fn search<'i>(
        &self,
        index: &'i Index,
        cancel: &CancelToken,
    ) -> Result<Matches<'i>, Error> {
        let case = if self.case_insensitive {
            Case::Insensitive
        } else {
            Case::Sensitive
        };
        let patterns = Patterns::new(&self.patterns, case)
            .map_err(|error| Error::new(ErrorCode::InvalidPattern, error.to_string()))?;
        let globs = filter(self.globs.as_deref())?;
        let exclude = filter(self.exclude.as_deref())?;
        let mut options = GrepOptions::default()
            .context(self.context)
            .limit(self.limit)
            .max_line_bytes(self.max_line_bytes)
            .max_file_bytes(self.max_file_bytes)
            .cancel(cancel);
        if self.count_all {
            options = options.count_all();
        }
        if let Some(globs) = &globs {
            options = options.filter(globs);
        }
        if let Some(exclude) = &exclude {
            options = options.exclude(exclude);
        }
        Ok(index.grep(&patterns, &options))
    }
}

impl<'a> GrepView<'a> {
    pub(crate) fn new(
        index: &Index,
        matches: &'a Matches<'_>,
        stopped: Option<StoppedView>,
    ) -> Self {
        let mut paths = index.paths();
        Self {
            files: matches
                .files()
                .iter()
                .map(|file| GrepFileView {
                    path: paths.get(file.position()).to_owned(),
                    lines: file
                        .lines()
                        .iter()
                        .map(|line| GrepLineView {
                            number: line.number(),
                            kind: match line.kind() {
                                LineKind::Match => "match",
                                LineKind::Context => "context",
                            },
                            text: line.text(),
                            cut: line.cut().map(|cut| CutView {
                                before: cut.before,
                                after: cut.after,
                            }),
                        })
                        .collect(),
                })
                .collect(),
            lines: matches.lines(),
            truncated: matches.truncated(),
            total_lines: matches.total_lines(),
            total_files: matches.total_files(),
            cancelled: matches.cancelled(),
            candidates: matches.candidates(),
            excluded: matches.excluded(),
            symlinks: matches.symlinks(),
            searched: matches.searched(),
            binary: matches.binary(),
            oversized: matches.oversized(),
            ruled_out: matches.ruled_out(),
            unsearched: matches.unsearched(),
            failures: matches
                .failures()
                .iter()
                .map(|failure| GrepFailureView {
                    path: paths.get(failure.position()).to_owned(),
                    message: failure.error().to_string(),
                })
                .collect(),
            stopped_build: stopped,
        }
    }
}
