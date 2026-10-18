use std::ops::RangeInclusive;

use kartta::{Access, Limits, SiteQuery, SymbolKind};
use serde::Deserialize;

use crate::error::{Error, ErrorCode};

pub(crate) const CALLERS_DEPTH: u32 = 3;
pub(crate) const CALLEES_DEPTH: u32 = 1;
pub(crate) const WALK_MAX_SYMBOLS: usize = 500;
pub(crate) const CALL_PATH_DEPTH: u32 = 8;
pub(crate) const CALL_PATH_MAX_SYMBOLS: usize = 20_000;
pub(crate) const REFERENCES: usize = 500;
pub(crate) const DEFINITIONS: usize = 10;
pub(crate) const SEARCH: usize = 20;
pub(crate) const IMPLEMENTATIONS_DEPTH: u32 = 3;
pub(crate) const IMPLEMENTATIONS: usize = 100;
pub(crate) const DEPENDENTS_DEPTH: u32 = 3;
pub(crate) const DEPENDENTS: usize = 100;
pub(crate) const SEGMENT_DEPTH: u32 = 3;
pub(crate) const DEAD_CODE: usize = 100;
pub(crate) const UNBOUND: usize = 20;

fn limits(
    depth: Option<u32>,
    min_confidence: u8,
    max_symbols: Option<usize>,
    defaults: (u32, usize),
) -> Limits {
    Limits::new(bound(depth, defaults.0))
        .with_min_confidence(min_confidence)
        .with_max_symbols(bound(max_symbols, defaults.1))
}

/// `value`, or `default` when it is left out or 0.
pub(crate) fn bound<T: Copy + Default + PartialEq>(value: Option<T>, default: T) -> T {
    value
        .filter(|&value| value != T::default())
        .unwrap_or(default)
}

fn lines(start_line: Option<u32>, end_line: u32) -> RangeInclusive<u32> {
    let end = if end_line == 0 { u32::MAX } else { end_line };
    bound(start_line, 1)..=end
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WalkParams {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: Option<usize>,
}

impl WalkParams {
    pub(crate) fn limits(&self, depth: u32) -> Limits {
        limits(
            self.depth,
            self.min_confidence,
            self.max_symbols,
            (depth, WALK_MAX_SYMBOLS),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallPathParams {
    pub(crate) from: String,
    pub(crate) to: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: Option<usize>,
}

impl CallPathParams {
    pub(crate) fn limits(&self) -> Limits {
        limits(
            self.depth,
            self.min_confidence,
            self.max_symbols,
            (CALL_PATH_DEPTH, CALL_PATH_MAX_SYMBOLS),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReferencesParams {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    pub(crate) min_confidence: u8,
    #[serde(default)]
    access: Option<AccessParam>,
    #[serde(default)]
    limit: Option<usize>,
}

impl ReferencesParams {
    pub(crate) fn query(&self) -> SiteQuery {
        let query =
            SiteQuery::new(bound(self.limit, REFERENCES)).with_min_confidence(self.min_confidence);
        match self.access {
            Some(AccessParam::Read) => query.with_access(Access::Read),
            Some(AccessParam::Write) => query.with_access(Access::Write),
            Some(AccessParam::Update) => query.with_access(Access::Update),
            None => query,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AccessParam {
    Read,
    Write,
    Update,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImplementationsParams {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    limit: Option<usize>,
}

impl ImplementationsParams {
    pub(crate) fn depth(&self) -> u32 {
        bound(self.depth, IMPLEMENTATIONS_DEPTH)
    }

    pub(crate) fn limit(&self) -> usize {
        bound(self.limit, IMPLEMENTATIONS)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DefinitionsParams {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    pub(crate) from: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

impl DefinitionsParams {
    pub(crate) fn limit(&self) -> usize {
        bound(self.limit, DEFINITIONS)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourcesParams {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    pub(crate) from: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    pub(crate) max_lines: usize,
    #[serde(default)]
    pub(crate) max_line_bytes: usize,
}

impl SourcesParams {
    pub(crate) fn limit(&self) -> usize {
        bound(self.limit, DEFINITIONS)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchParams {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

impl SearchParams {
    pub(crate) fn limit(&self) -> usize {
        bound(self.limit, SEARCH)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutlineParams {
    pub(crate) file: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DependentsParams {
    pub(crate) file: String,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    max_files: Option<usize>,
}

impl DependentsParams {
    pub(crate) fn depth(&self) -> u32 {
        bound(self.depth, DEPENDENTS_DEPTH)
    }

    pub(crate) fn max_files(&self) -> usize {
        self.max_files.unwrap_or(DEPENDENTS)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SegmentParams {
    pub(crate) file: String,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: u32,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: Option<usize>,
}

impl SegmentParams {
    pub(crate) fn lines(&self) -> RangeInclusive<u32> {
        lines(self.start_line, self.end_line)
    }

    pub(crate) fn limits(&self) -> Limits {
        limits(
            self.depth,
            self.min_confidence,
            self.max_symbols,
            (SEGMENT_DEPTH, WALK_MAX_SYMBOLS),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangeParams {
    pub(crate) regions: Vec<RegionParams>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: Option<usize>,
}

impl ChangeParams {
    pub(crate) fn limits(&self) -> Limits {
        limits(
            self.depth,
            self.min_confidence,
            self.max_symbols,
            (SEGMENT_DEPTH, WALK_MAX_SYMBOLS),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegionParams {
    pub(crate) file: String,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: u32,
}

impl RegionParams {
    pub(crate) fn lines(&self) -> RangeInclusive<u32> {
        lines(self.start_line, self.end_line)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct DeadCodeParams {
    pub(crate) lang: Option<String>,
    kinds: Vec<String>,
    pub(crate) path_prefix: Option<String>,
    limit: Option<usize>,
}

impl DeadCodeParams {
    pub(crate) fn kinds(&self) -> Result<Vec<SymbolKind>, Error> {
        self.kinds
            .iter()
            .map(|label| kind_labelled(label))
            .collect()
    }

    pub(crate) fn limit(&self) -> usize {
        bound(self.limit, DEAD_CODE)
    }
}

fn kind_labelled(label: &str) -> Result<SymbolKind, Error> {
    SymbolKind::ALL
        .into_iter()
        .find(|kind| kind.label() == label)
        .ok_or_else(|| {
            let known: Vec<&str> = SymbolKind::ALL.iter().map(|kind| kind.label()).collect();
            Error::new(
                ErrorCode::InvalidParams,
                format!(
                    "no symbol kind {label:?}; the kinds are {}",
                    known.join(", ")
                ),
            )
        })
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct ResolutionParams {
    pub(crate) lang: Option<String>,
    limit: Option<usize>,
}

impl ResolutionParams {
    pub(crate) fn limit(&self) -> usize {
        bound(self.limit, UNBOUND)
    }
}
