use std::ops::RangeInclusive;

use kartta::{Access, ChangeQuery, Limits, SegmentQuery, Selector, SiteQuery, SymbolKind};
use serde::Deserialize;

use crate::error::{Error, ErrorCode};

const fn limits(depth: u32, min_confidence: u8, max_symbols: usize) -> Limits {
    Limits::new(depth)
        .with_min_confidence(min_confidence)
        .with_max_symbols(max_symbols)
}

fn selector<'s>(
    name: &'s str,
    path_prefix: Option<&'s str>,
    line: Option<u32>,
    candidate_limit: Option<usize>,
) -> Selector<'s> {
    let selector = Selector::new(name, path_prefix).with_line(line);
    candidate_limit.map_or(selector, |limit| selector.with_candidate_limit(limit))
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
    depth: u32,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: usize,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl WalkParams {
    pub(crate) fn selector(&self) -> Selector<'_> {
        selector(
            &self.name,
            self.path_prefix.as_deref(),
            self.line,
            self.candidate_limit,
        )
    }

    pub(crate) const fn limits(&self) -> Limits {
        limits(self.depth, self.min_confidence, self.max_symbols)
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
    depth: u32,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: usize,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl CallPathParams {
    pub(crate) fn from(&self) -> Selector<'_> {
        selector(
            &self.from,
            self.from_path_prefix.as_deref(),
            self.from_line,
            self.candidate_limit,
        )
    }

    pub(crate) fn to(&self) -> Selector<'_> {
        selector(
            &self.to,
            self.to_path_prefix.as_deref(),
            self.to_line,
            self.candidate_limit,
        )
    }

    pub(crate) const fn limits(&self) -> Limits {
        limits(self.depth, self.min_confidence, self.max_symbols)
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
    limit: usize,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl ReferencesParams {
    pub(crate) fn selector(&self) -> Selector<'_> {
        selector(
            &self.name,
            self.path_prefix.as_deref(),
            self.line,
            self.candidate_limit,
        )
    }

    pub(crate) const fn query(&self) -> SiteQuery {
        let query = SiteQuery::new(self.limit).with_min_confidence(self.min_confidence);
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
    pub(crate) depth: u32,
    pub(crate) limit: usize,
    #[serde(default)]
    candidate_limit: Option<usize>,
}

impl ImplementationsParams {
    pub(crate) fn selector(&self) -> Selector<'_> {
        selector(
            &self.name,
            self.path_prefix.as_deref(),
            self.line,
            self.candidate_limit,
        )
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
    pub(crate) limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourcesParams {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    pub(crate) from: Option<String>,
    pub(crate) limit: usize,
    #[serde(default)]
    pub(crate) max_lines: usize,
    #[serde(default)]
    pub(crate) max_line_bytes: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchParams {
    pub(crate) query: String,
    #[serde(default)]
    pub(crate) lang: Option<String>,
    pub(crate) limit: usize,
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
    pub(crate) depth: u32,
    pub(crate) max_files: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SegmentParams {
    pub(crate) file: String,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: u32,
    depth: u32,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: usize,
    #[serde(default)]
    call_limit: Option<usize>,
}

impl SegmentParams {
    pub(crate) fn lines(&self) -> RangeInclusive<u32> {
        lines(self.start_line, self.end_line)
    }

    pub(crate) fn query(&self) -> SegmentQuery {
        let query = SegmentQuery::new(limits(self.depth, self.min_confidence, self.max_symbols));
        self.call_limit
            .map_or(query, |limit| query.with_call_limit(limit))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangeParams {
    pub(crate) regions: Vec<RegionParams>,
    depth: u32,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: usize,
    #[serde(default)]
    consumer_limit: Option<usize>,
    #[serde(default)]
    seed_limit: Option<usize>,
    #[serde(default)]
    hotspot_limit: Option<usize>,
}

impl ChangeParams {
    pub(crate) const fn query(&self) -> ChangeQuery {
        let mut query = ChangeQuery::new(limits(self.depth, self.min_confidence, self.max_symbols));
        if let Some(limit) = self.consumer_limit {
            query = query.with_consumer_limit(limit);
        }
        if let Some(limit) = self.seed_limit {
            query = query.with_seed_limit(limit);
        }
        if let Some(limit) = self.hotspot_limit {
            query = query.with_hotspot_limit(limit);
        }
        query
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeadCodeParams {
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    pub(crate) limit: usize,
}

impl DeadCodeParams {
    pub(crate) fn kinds(&self) -> Result<Vec<SymbolKind>, Error> {
        self.kinds
            .iter()
            .map(|label| kind_labelled(label))
            .collect()
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolutionParams {
    #[serde(default)]
    pub(crate) lang: Option<String>,
    pub(crate) limit: usize,
}
