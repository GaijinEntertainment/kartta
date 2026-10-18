use std::io;

use kartta::{
    Blast, CallPath, Change, CutLine, DeadCode, Definition, Definitions, Dependents, DirectCalls,
    Implementations, Miss, Nearness, ReachBand, Reached, References, Resolution, Search, Segment,
    Source, SymbolFlags, Walk,
};
use serde::{Serialize, Serializer};

/// A definition as every answer spells it. The flags appear only when one is set.
#[derive(Debug, Serialize)]
pub(crate) struct DefinitionView<'g> {
    name: &'g str,
    qualified_name: &'g str,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'g str>,
    kind: &'static str,
    file: &'g str,
    line: u32,
    end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<&'g str>,
    exported: bool,
    #[serde(skip_serializing_if = "Flags::is_empty")]
    flags: Flags,
}

/// A definition's flags, spelled as the list of those set: `test`, `entry`, `generated`, `vendored`.
#[derive(Debug)]
struct Flags(SymbolFlags);

impl Flags {
    const fn set(&self) -> [(&'static str, bool); 4] {
        [
            ("test", self.0.is_test()),
            ("entry", self.0.is_entry()),
            ("generated", self.0.is_generated()),
            ("vendored", self.0.is_vendored()),
        ]
    }

    fn is_empty(&self) -> bool {
        self.set().iter().all(|&(_, set)| !set)
    }
}

impl Serialize for Flags {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(
            self.set()
                .into_iter()
                .filter_map(|(name, set)| set.then_some(name)),
        )
    }
}

impl<'g> From<&Definition<'g>> for DefinitionView<'g> {
    fn from(definition: &Definition<'g>) -> Self {
        Self {
            name: definition.name,
            qualified_name: definition.qualified_name,
            owner: definition.owner,
            kind: definition.kind.label(),
            file: definition.file,
            line: definition.line,
            end_line: definition.end_line,
            signature: definition.signature,
            exported: definition.exported,
            flags: Flags(definition.flags),
        }
    }
}

fn definitions<'g>(definitions: &[Definition<'g>]) -> Vec<DefinitionView<'g>> {
    definitions.iter().map(DefinitionView::from).collect()
}

/// A symbol a walk over calls reached, and the call that reached it.
#[derive(Debug, Serialize)]
pub(crate) struct ReachedView<'g> {
    symbol: DefinitionView<'g>,
    depth: u32,
    confidence: u8,
    line: u32,
}

impl<'g> From<&Reached<'g>> for ReachedView<'g> {
    fn from(reached: &Reached<'g>) -> Self {
        Self {
            symbol: DefinitionView::from(&reached.symbol),
            depth: reached.depth,
            confidence: reached.confidence,
            line: reached.line,
        }
    }
}

fn reached<'g>(reached: &[Reached<'g>]) -> Vec<ReachedView<'g>> {
    reached.iter().map(ReachedView::from).collect()
}

/// The answer of `callers` and `callees` in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct WalkView<'g> {
    root: DefinitionView<'g>,
    passed_over: usize,
    reached: Vec<ReachedView<'g>>,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
    below_confidence: usize,
    unresolved: usize,
}

impl<'g> From<Walk<'g>> for WalkView<'g> {
    fn from(walk: Walk<'g>) -> Self {
        Self {
            root: DefinitionView::from(&walk.root),
            passed_over: walk.passed_over,
            reached: reached(&walk.reached),
            truncated: walk.truncated,
            turned_away: walk.turned_away,
            unexpanded: walk.unexpanded,
            below_confidence: walk.below_confidence,
            unresolved: walk.unresolved,
        }
    }
}

/// The answer of `call_path` in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct CallPathView<'g> {
    from: DefinitionView<'g>,
    to: DefinitionView<'g>,
    found: bool,
    hops: Vec<ReachedView<'g>>,
    truncated: bool,
    turned_away: usize,
    at_depth_bound: usize,
    below_confidence: usize,
    passed_over: PassedOver,
}

#[derive(Debug, Serialize)]
struct PassedOver {
    from: usize,
    to: usize,
}

impl<'g> From<CallPath<'g>> for CallPathView<'g> {
    fn from(path: CallPath<'g>) -> Self {
        Self {
            from: DefinitionView::from(&path.from),
            to: DefinitionView::from(&path.to),
            found: path.found,
            hops: reached(&path.hops),
            truncated: path.truncated,
            turned_away: path.turned_away,
            at_depth_bound: path.at_depth_bound,
            below_confidence: path.below_confidence,
            passed_over: PassedOver {
                from: path.passed_over.0,
                to: path.passed_over.1,
            },
        }
    }
}

/// The answer of `references` in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct ReferencesView<'g> {
    symbol: DefinitionView<'g>,
    passed_over: usize,
    sites: Vec<SiteView<'g>>,
    total: usize,
    below_confidence: usize,
}

#[derive(Debug, Serialize)]
struct SiteView<'g> {
    site_of: DefinitionView<'g>,
    line: u32,
    confidence: u8,
    call: bool,
}

impl<'g> From<References<'g>> for ReferencesView<'g> {
    fn from(references: References<'g>) -> Self {
        Self {
            symbol: DefinitionView::from(&references.symbol),
            passed_over: references.passed_over,
            sites: references
                .sites
                .iter()
                .map(|site| SiteView {
                    site_of: DefinitionView::from(&site.site_of),
                    line: site.line,
                    confidence: site.confidence,
                    call: site.call,
                })
                .collect(),
            total: references.total,
            below_confidence: references.below_confidence,
        }
    }
}

/// The answer of `definitions`, and with each definition's text, of `sources`, in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct DefinitionsView<'g> {
    total: usize,
    found: Vec<FoundView<'g>>,
}

#[derive(Debug, Serialize)]
struct FoundView<'g> {
    definition: DefinitionView<'g>,
    nearness: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<SourceView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SourceView {
    doc_line: u32,
    first_line: u32,
    lines: u32,
    text: String,
    cut_lines: Vec<CutLineView>,
    resized: bool,
}

#[derive(Debug, Serialize)]
struct CutLineView {
    line: u32,
    bytes: usize,
}

impl<'g> DefinitionsView<'g> {
    /// The view of `found`, each definition with the text `sources` holds for it, when one is given.
    pub(crate) fn new(found: &Definitions<'g>, sources: Option<Vec<io::Result<Source>>>) -> Self {
        let mut sources = sources.map(Vec::into_iter);
        Self {
            total: found.total,
            found: found
                .found
                .iter()
                .map(|found| {
                    let (source, error) = match sources.as_mut().and_then(Iterator::next) {
                        Some(Ok(source)) => (Some(SourceView::from(source)), None),
                        Some(Err(error)) => (None, Some(error.to_string())),
                        None => (None, None),
                    };
                    FoundView {
                        definition: DefinitionView::from(&found.definition),
                        nearness: nearness(found.nearness),
                        source,
                        error,
                    }
                })
                .collect(),
        }
    }
}

impl From<Source> for SourceView {
    fn from(source: Source) -> Self {
        Self {
            doc_line: source.doc_line,
            first_line: source.first_line,
            lines: source.lines,
            text: source.text,
            cut_lines: source
                .cut_lines
                .iter()
                .map(|&CutLine { line, bytes, .. }| CutLineView { line, bytes })
                .collect(),
            resized: source.resized,
        }
    }
}

const fn nearness(nearness: Nearness) -> &'static str {
    match nearness {
        Nearness::SameDirectory => "same_directory",
        _ => "elsewhere",
    }
}

/// The answer of `search` in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct SearchView<'g> {
    hits: Vec<HitView<'g>>,
    total: usize,
    named: usize,
    unmatched_words: Vec<String>,
    dropped_words: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    fuzzy_total: Option<usize>,
}

#[derive(Debug, Serialize)]
struct HitView<'g> {
    definition: DefinitionView<'g>,
    score: f64,
    exact: bool,
    fuzzy: bool,
}

impl<'g> From<Search<'g>> for SearchView<'g> {
    fn from(search: Search<'g>) -> Self {
        Self {
            hits: search
                .hits
                .iter()
                .map(|hit| HitView {
                    definition: DefinitionView::from(&hit.definition),
                    score: hit.score,
                    exact: hit.exact,
                    fuzzy: hit.fuzzy,
                })
                .collect(),
            total: search.total,
            named: search.named,
            unmatched_words: search.unmatched_words,
            dropped_words: search.dropped_words,
            fuzzy_total: search.fuzzy_total,
        }
    }
}

/// The answer of `implementations` in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct ImplementationsView<'g> {
    root: DefinitionView<'g>,
    passed_over: usize,
    found: Vec<ImplementorView<'g>>,
    total: usize,
    unexpanded: usize,
}

#[derive(Debug, Serialize)]
struct ImplementorView<'g> {
    symbol: DefinitionView<'g>,
    depth: u32,
}

impl<'g> From<Implementations<'g>> for ImplementationsView<'g> {
    fn from(found: Implementations<'g>) -> Self {
        Self {
            root: DefinitionView::from(&found.root),
            passed_over: found.passed_over,
            found: found
                .found
                .iter()
                .map(|implementor| ImplementorView {
                    symbol: DefinitionView::from(&implementor.symbol),
                    depth: implementor.depth,
                })
                .collect(),
            total: found.total,
            unexpanded: found.unexpanded,
        }
    }
}

/// The answer of a query about one file: the graph that holds it, if any, and what that graph answered.
#[derive(Debug, Serialize)]
pub(crate) struct FileView<'a, T> {
    file: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph: Option<&'a str>,
    held: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    answer: Option<T>,
}

impl<'a, T> FileView<'a, T> {
    pub(crate) const fn new(file: &'a str, graph: Option<&'a str>, answer: Option<T>) -> Self {
        Self {
            file,
            graph,
            held: answer.is_some(),
            answer,
        }
    }
}

/// The definitions of one file, as `outline` answers them.
#[derive(Debug, Serialize)]
pub(crate) struct OutlineView<'g> {
    symbols: Vec<DefinitionView<'g>>,
}

impl<'g> From<Vec<Definition<'g>>> for OutlineView<'g> {
    fn from(outline: Vec<Definition<'g>>) -> Self {
        Self {
            symbols: definitions(&outline),
        }
    }
}

/// The files that include a file, as `dependents` answers them.
#[derive(Debug, Serialize)]
pub(crate) struct DependentsView<'g> {
    found: Vec<DependentView<'g>>,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
}

#[derive(Debug, Serialize)]
struct DependentView<'g> {
    file: &'g str,
    depth: u32,
}

impl<'g> From<Dependents<'g>> for DependentsView<'g> {
    fn from(dependents: Dependents<'g>) -> Self {
        Self {
            found: dependents
                .found
                .iter()
                .map(|dependent| DependentView {
                    file: dependent.file,
                    depth: dependent.depth,
                })
                .collect(),
            truncated: dependents.truncated,
            turned_away: dependents.turned_away,
            unexpanded: dependents.unexpanded,
        }
    }
}

/// What a change to a range of a file's lines reaches, as `analyze_segment` answers it.
#[derive(Debug, Serialize)]
pub(crate) struct SegmentView<'g> {
    start_line: u32,
    end_line: u32,
    symbols: Vec<SegmentSymbolView<'g>>,
    includes: Vec<&'g str>,
    included_by: Vec<&'g str>,
}

#[derive(Debug, Serialize)]
struct SegmentSymbolView<'g> {
    definition: DefinitionView<'g>,
    callers: DirectCallsView<'g>,
    callees: DirectCallsView<'g>,
    unresolved: usize,
    blast: BlastView,
}

#[derive(Debug, Serialize)]
struct DirectCallsView<'g> {
    listed: Vec<ReachedView<'g>>,
    total: usize,
    below_confidence: usize,
}

impl<'g> From<&DirectCalls<'g>> for DirectCallsView<'g> {
    fn from(calls: &DirectCalls<'g>) -> Self {
        Self {
            listed: reached(&calls.listed),
            total: calls.total,
            below_confidence: calls.below_confidence,
        }
    }
}

#[derive(Debug, Serialize)]
struct BlastView {
    reach: usize,
    by_depth: Vec<usize>,
    affected_files: usize,
    band: &'static str,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
    below_confidence: usize,
}

impl From<&Blast> for BlastView {
    fn from(blast: &Blast) -> Self {
        Self {
            reach: blast.reach,
            by_depth: blast.by_depth.clone(),
            affected_files: blast.affected_files,
            band: band(blast.band),
            truncated: blast.truncated,
            turned_away: blast.turned_away,
            unexpanded: blast.unexpanded,
            below_confidence: blast.below_confidence,
        }
    }
}

const fn band(band: ReachBand) -> &'static str {
    match band {
        ReachBand::Low => "low",
        ReachBand::Medium => "medium",
        ReachBand::High => "high",
        _ => "critical",
    }
}

impl<'g> From<Segment<'g>> for SegmentView<'g> {
    fn from(segment: Segment<'g>) -> Self {
        Self {
            start_line: *segment.lines.start(),
            end_line: *segment.lines.end(),
            symbols: segment
                .symbols
                .iter()
                .map(|symbol| SegmentSymbolView {
                    definition: DefinitionView::from(&symbol.definition),
                    callers: DirectCallsView::from(&symbol.callers),
                    callees: DirectCallsView::from(&symbol.callees),
                    unresolved: symbol.unresolved,
                    blast: BlastView::from(&symbol.blast),
                })
                .collect(),
            includes: segment.includes,
            included_by: segment.included_by,
        }
    }
}

/// What a change to regions across files reaches, as `analyze_change` answers it.
#[derive(Debug, Serialize)]
pub(crate) struct ChangeView<'g> {
    changed_symbols: usize,
    changed_files: usize,
    unheld_regions: usize,
    regions_outside_symbols: usize,
    footprint: usize,
    footprint_files: usize,
    transitive_sum: usize,
    hotspots: Vec<HotspotView<'g>>,
    total_hotspots: usize,
    seeds: Vec<SeedView<'g>>,
    truncated_seeds: usize,
}

#[derive(Debug, Serialize)]
struct HotspotView<'g> {
    symbol: DefinitionView<'g>,
    hit_count: usize,
    min_depth: u32,
    also_changed: bool,
}

#[derive(Debug, Serialize)]
struct SeedView<'g> {
    symbol: DefinitionView<'g>,
    reach: usize,
    band: &'static str,
    caller_count: usize,
    consumers: Vec<ReachedView<'g>>,
    consumer_dirs: Vec<DirCountView<'g>>,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
    below_confidence: usize,
}

#[derive(Debug, Serialize)]
struct DirCountView<'g> {
    dir: &'g str,
    count: usize,
}

impl<'g> From<Change<'g>> for ChangeView<'g> {
    fn from(change: Change<'g>) -> Self {
        Self {
            changed_symbols: change.changed_symbols,
            changed_files: change.changed_files,
            unheld_regions: change.unheld_regions,
            regions_outside_symbols: change.regions_outside_symbols,
            footprint: change.footprint,
            footprint_files: change.footprint_files,
            transitive_sum: change.transitive_sum,
            hotspots: change
                .hotspots
                .iter()
                .map(|hotspot| HotspotView {
                    symbol: DefinitionView::from(&hotspot.symbol),
                    hit_count: hotspot.hit_count,
                    min_depth: hotspot.min_depth,
                    also_changed: hotspot.also_changed,
                })
                .collect(),
            total_hotspots: change.total_hotspots,
            seeds: change
                .seeds
                .iter()
                .map(|seed| SeedView {
                    symbol: DefinitionView::from(&seed.symbol),
                    reach: seed.reach,
                    band: band(seed.band),
                    caller_count: seed.caller_count,
                    consumers: reached(&seed.consumers),
                    consumer_dirs: seed
                        .consumer_dirs
                        .iter()
                        .map(|dir| DirCountView {
                            dir: dir.dir,
                            count: dir.count,
                        })
                        .collect(),
                    truncated: seed.truncated,
                    turned_away: seed.turned_away,
                    unexpanded: seed.unexpanded,
                    below_confidence: seed.below_confidence,
                })
                .collect(),
            truncated_seeds: change.truncated_seeds,
        }
    }
}

/// The answer of `dead_code` in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct DeadCodeView<'g> {
    found: Vec<DeadView<'g>>,
    total: usize,
    live: usize,
    excluded: ExcludedView,
}

#[derive(Debug, Serialize)]
struct DeadView<'g> {
    definition: DefinitionView<'g>,
    spelled: bool,
}

#[derive(Debug, Serialize)]
struct ExcludedView {
    kind: usize,
    test: usize,
    generated: usize,
    entry: usize,
    vendored: usize,
    unobservable: usize,
    uncertain: usize,
    dispatched: usize,
}

impl<'g> From<DeadCode<'g>> for DeadCodeView<'g> {
    fn from(dead: DeadCode<'g>) -> Self {
        let excluded = dead.excluded;
        Self {
            found: dead
                .found
                .iter()
                .map(|found| DeadView {
                    definition: DefinitionView::from(&found.definition),
                    spelled: found.spelled,
                })
                .collect(),
            total: dead.total,
            live: dead.live,
            excluded: ExcludedView {
                kind: excluded.kind,
                test: excluded.test,
                generated: excluded.generated,
                entry: excluded.entry,
                vendored: excluded.vendored,
                unobservable: excluded.unobservable,
                uncertain: excluded.uncertain,
                dispatched: excluded.dispatched,
            },
        }
    }
}

/// The answer of `resolution` in one graph.
#[derive(Debug, Serialize)]
pub(crate) struct ResolutionView<'g> {
    sites: usize,
    bound: Vec<BoundView>,
    edges: usize,
    misses: MissesView,
    unbound: Vec<UnboundView<'g>>,
    unlisted: usize,
}

#[derive(Debug, Serialize)]
struct BoundView {
    confidence: u8,
    sites: usize,
}

#[derive(Debug, Serialize)]
struct MissesView {
    external_import: usize,
    not_exported: usize,
    undefined: usize,
    not_admitted: usize,
    ambiguous: usize,
    through_a_receiver: usize,
}

#[derive(Debug, Serialize)]
struct UnboundView<'g> {
    caller: DefinitionView<'g>,
    line: u32,
    callee: &'g str,
    #[serde(skip_serializing_if = "Option::is_none")]
    receiver: Option<&'g str>,
    miss: &'static str,
}

const fn miss(miss: Miss) -> &'static str {
    match miss {
        Miss::ExternalImport => "external_import",
        Miss::NotExported => "not_exported",
        Miss::Undefined => "undefined",
        Miss::NotAdmitted => "not_admitted",
        _ => "ambiguous",
    }
}

impl<'g> From<Resolution<'g>> for ResolutionView<'g> {
    fn from(resolution: Resolution<'g>) -> Self {
        let misses = resolution.misses;
        Self {
            sites: resolution.sites,
            bound: resolution
                .bound
                .iter()
                .map(|&(confidence, sites)| BoundView { confidence, sites })
                .collect(),
            edges: resolution.edges,
            misses: MissesView {
                external_import: misses.external_import,
                not_exported: misses.not_exported,
                undefined: misses.undefined,
                not_admitted: misses.not_admitted,
                ambiguous: misses.ambiguous,
                through_a_receiver: misses.through_a_receiver,
            },
            unbound: resolution
                .unbound
                .iter()
                .map(|unbound| UnboundView {
                    caller: DefinitionView::from(&unbound.caller),
                    line: unbound.line,
                    callee: unbound.callee,
                    receiver: unbound.receiver,
                    miss: miss(unbound.miss),
                })
                .collect(),
            unlisted: resolution.unlisted,
        }
    }
}
