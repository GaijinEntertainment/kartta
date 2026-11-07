use std::ops::RangeInclusive;

use kartta::{Access, ChangeQuery, Limits, SegmentQuery, Selector, SiteQuery, SymbolKind};
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
pub(crate) const SEGMENT_CALLS: usize = 50;
pub(crate) const CHANGE_CONSUMERS: usize = 30;
pub(crate) const CHANGE_SEEDS: usize = 300;
pub(crate) const CHANGE_HOTSPOTS: usize = 300;
pub(crate) const CANDIDATES: usize = 5;
pub(crate) const DEAD_CODE: usize = 100;
pub(crate) const UNBOUND: usize = 20;

fn limits(
    depth: Option<u32>,
    min_confidence: u8,
    max_symbols: Option<usize>,
    defaults: (u32, usize),
) -> Limits {
    Limits::new(depth.unwrap_or(defaults.0))
        .with_min_confidence(min_confidence)
        .with_max_symbols(max_symbols.unwrap_or(defaults.1))
}

fn lines(start_line: Option<u32>, end_line: u32) -> RangeInclusive<u32> {
    let end = if end_line == 0 { u32::MAX } else { end_line };
    start_line.map_or(1, |start| start.max(1))..=end
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WalkParams {
    pub(crate) name: String,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: Option<usize>,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl WalkParams {
    pub(crate) fn selector(&self) -> Selector<'_> {
        Selector::new(&self.name, self.path_prefix.as_deref())
            .with_line(self.line)
            .with_candidate_limit(self.candidate_limit.unwrap_or(CANDIDATES))
    }

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
    from: String,
    #[serde(default)]
    from_path_prefix: Option<String>,
    #[serde(default)]
    from_line: Option<u32>,
    to: String,
    #[serde(default)]
    to_path_prefix: Option<String>,
    #[serde(default)]
    to_line: Option<u32>,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: Option<usize>,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl CallPathParams {
    pub(crate) fn from(&self) -> Selector<'_> {
        Selector::new(&self.from, self.from_path_prefix.as_deref())
            .with_line(self.from_line)
            .with_candidate_limit(self.candidate_limit.unwrap_or(CANDIDATES))
    }

    pub(crate) fn to(&self) -> Selector<'_> {
        Selector::new(&self.to, self.to_path_prefix.as_deref())
            .with_line(self.to_line)
            .with_candidate_limit(self.candidate_limit.unwrap_or(CANDIDATES))
    }

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
    name: String,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    pub(crate) min_confidence: u8,
    #[serde(default)]
    access: Option<AccessParam>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl ReferencesParams {
    pub(crate) fn selector(&self) -> Selector<'_> {
        Selector::new(&self.name, self.path_prefix.as_deref())
            .with_line(self.line)
            .with_candidate_limit(self.candidate_limit.unwrap_or(CANDIDATES))
    }

    pub(crate) fn query(&self) -> SiteQuery {
        let query = SiteQuery::new(self.limit.unwrap_or(REFERENCES))
            .with_min_confidence(self.min_confidence);
        match self.access {
            Some(AccessParam::Read) => query.with_access(Access::Read),
            Some(AccessParam::Write) => query.with_access(Access::Write),
            Some(AccessParam::Update) => query.with_access(Access::Update),
            Some(AccessParam::Type) => query.with_access(Access::Type),
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
    Type,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImplementationsParams {
    name: String,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl ImplementationsParams {
    pub(crate) fn selector(&self) -> Selector<'_> {
        Selector::new(&self.name, self.path_prefix.as_deref())
            .with_line(self.line)
            .with_candidate_limit(self.candidate_limit.unwrap_or(CANDIDATES))
    }

    pub(crate) fn depth(&self) -> u32 {
        self.depth.unwrap_or(IMPLEMENTATIONS_DEPTH)
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit.unwrap_or(IMPLEMENTATIONS)
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
        self.limit.unwrap_or(DEFINITIONS)
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
        self.limit.unwrap_or(DEFINITIONS)
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
        self.limit.unwrap_or(SEARCH)
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
        self.depth.unwrap_or(DEPENDENTS_DEPTH)
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
    #[serde(default)]
    call_limit: Option<usize>,
}

impl SegmentParams {
    pub(crate) fn lines(&self) -> RangeInclusive<u32> {
        lines(self.start_line, self.end_line)
    }

    pub(crate) fn query(&self) -> SegmentQuery {
        let limits = limits(
            self.depth,
            self.min_confidence,
            self.max_symbols,
            (SEGMENT_DEPTH, WALK_MAX_SYMBOLS),
        );
        SegmentQuery::new(limits).with_call_limit(self.call_limit.unwrap_or(SEGMENT_CALLS))
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
    #[serde(default)]
    consumer_limit: Option<usize>,
    #[serde(default)]
    seed_limit: Option<usize>,
    #[serde(default)]
    hotspot_limit: Option<usize>,
}

impl ChangeParams {
    pub(crate) fn query(&self) -> ChangeQuery {
        let limits = limits(
            self.depth,
            self.min_confidence,
            self.max_symbols,
            (SEGMENT_DEPTH, WALK_MAX_SYMBOLS),
        );
        ChangeQuery::new(limits)
            .with_consumer_limit(self.consumer_limit.unwrap_or(CHANGE_CONSUMERS))
            .with_seed_limit(self.seed_limit.unwrap_or(CHANGE_SEEDS))
            .with_hotspot_limit(self.hotspot_limit.unwrap_or(CHANGE_HOTSPOTS))
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
        self.limit.unwrap_or(DEAD_CODE)
    }
}

fn kind_labelled(label: &str) -> Result<SymbolKind, Error> {
    SymbolKind::ALL
        .iter()
        .copied()
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
        self.limit.unwrap_or(UNBOUND)
    }
}
