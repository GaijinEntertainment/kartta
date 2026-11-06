use std::io;
use std::ops::RangeInclusive;

use kartta::{
    Access, Blast, CallPath, Candidate, Change, Conventions, CutLine, Dead, DeadCode, Definition,
    Definitions, Dependent, Dependents, DirCount, DirectCalls, Engine, EngineChange, EntryKind,
    Excluded, FileScope, Graph, Hit, Hotspot, Implementations, Implementor, LanguageFiles, Miss,
    Misses, Nearness, ReachBand, Reached, Reference, References, Resolution, Search, Seed, Segment,
    SegmentSymbol, SelfCall, SiteCounts, Source, Stats, SymbolFlags, SymbolKind, Symlink, TypeUses,
    Unbound, Unmatched, UnmatchedKind, Walk,
};

use serde::{Serialize, Serializer};

pub(crate) trait Wire {
    fn wire<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;
}

impl<T: Wire> Wire for &T {
    fn wire<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (**self).wire(serializer)
    }
}

#[derive(Debug)]
pub(crate) struct Wired<T>(pub(crate) T);

impl<T: Wire> Serialize for Wired<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.wire(serializer)
    }
}

macro_rules! wire {
    ($($answer:ty => $definition:ident),* $(,)?) => {$(
        impl Wire for $answer {
            fn wire<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                $definition::serialize(self, serializer)
            }
        }
    )*};
}

wire!(
    Definition<'_> => DefinitionDef,
    Candidate<'_> => CandidateDef,
    Reached<'_> => ReachedDef,
    Walk<'_> => WalkDef,
    FileScope<'_> => FileScopeDef,
    SelfCall => SelfCallDef,
    CallPath<'_> => CallPathDef,
    Reference<'_> => ReferenceDef,
    References<'_> => ReferencesDef,
    Source => SourceDef,
    CutLine => CutLineDef,
    Hit<'_> => HitDef,
    Search<'_> => SearchDef,
    Implementor<'_> => ImplementorDef,
    Implementations<'_> => ImplementationsDef,
    Dependent<'_> => DependentDef,
    Dependents<'_> => DependentsDef,
    SegmentSymbol<'_> => SegmentSymbolDef,
    Segment<'_> => SegmentDef,
    Hotspot<'_> => HotspotDef,
    DirCount<'_> => DirCountDef,
    Seed<'_> => SeedDef,
    Change<'_> => ChangeDef,
    Dead<'_> => DeadDef,
    DeadCode<'_> => DeadCodeDef,
    Unbound<'_> => UnboundDef,
    Resolution<'_> => ResolutionDef,
    LanguageFiles<'_> => LanguageFilesDef,
    Stats => StatsDef,
    Unmatched<'_> => UnmatchedDef,
);

fn each<T: Wire, S: Serializer>(items: &[T], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(items.iter().map(Wired))
}

#[expect(
    clippy::ref_option,
    reason = "serde passes a field to serialize_with by reference"
)]
fn present<T: Wire, S: Serializer>(item: &Option<T>, serializer: S) -> Result<S::Ok, S::Error> {
    item.as_ref().map(Wired).serialize(serializer)
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a field to serialize_with and skip_serializing_if by reference"
)]
mod label {
    use kartta::{Access, Miss, Nearness, ReachBand, SymbolFlags, SymbolKind, UnmatchedKind};
    use serde::Serializer;

    pub(super) fn kind<S: Serializer>(kind: &SymbolKind, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(kind.label())
    }

    pub(super) fn kinds<S: Serializer>(
        kinds: &[(SymbolKind, usize)],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_map(kinds.iter().map(|(kind, count)| (kind.label(), count)))
    }

    pub(super) fn band<S: Serializer>(band: &ReachBand, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(band.label())
    }

    pub(super) fn access<S: Serializer>(access: &Access, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(access.label())
    }

    pub(super) fn nearness<S: Serializer>(
        nearness: &Nearness,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(nearness.label())
    }

    pub(super) fn miss<S: Serializer>(miss: &Miss, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(miss.label())
    }

    pub(super) fn unmatched<S: Serializer>(
        kind: &UnmatchedKind,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(kind.label())
    }

    const fn set(flags: SymbolFlags) -> [(&'static str, bool); 4] {
        [
            ("test", flags.is_test()),
            ("entry", flags.is_entry()),
            ("generated", flags.is_generated()),
            ("vendored", flags.is_vendored()),
        ]
    }

    pub(super) fn flags<S: Serializer>(
        flags: &SymbolFlags,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(
            set(*flags)
                .into_iter()
                .filter_map(|(name, set)| set.then_some(name)),
        )
    }

    pub(super) fn no_flags(flags: &SymbolFlags) -> bool {
        set(*flags).iter().all(|&(_, set)| !set)
    }
}

#[derive(Serialize)]
#[serde(remote = "Definition")]
struct DefinitionDef<'g> {
    name: &'g str,
    qualified_name: &'g str,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'g str>,
    #[serde(serialize_with = "label::kind")]
    kind: SymbolKind,
    file: &'g str,
    line: u32,
    end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<&'g str>,
    exported: bool,
    #[serde(
        skip_serializing_if = "label::no_flags",
        serialize_with = "label::flags"
    )]
    flags: SymbolFlags,
}

#[derive(Serialize)]
#[serde(remote = "Candidate")]
struct CandidateDef<'g> {
    #[serde(with = "DefinitionDef")]
    symbol: Definition<'g>,
    count: usize,
}

#[derive(Serialize)]
#[serde(remote = "Reached")]
struct ReachedDef<'g> {
    #[serde(with = "DefinitionDef")]
    symbol: Definition<'g>,
    depth: u32,
    confidence: u8,
    line: u32,
    #[serde(serialize_with = "present", skip_serializing_if = "Option::is_none")]
    through: Option<Definition<'g>>,
}

#[derive(Serialize)]
#[serde(remote = "Walk")]
struct WalkDef<'g> {
    #[serde(with = "DefinitionDef")]
    root: Definition<'g>,
    passed_over: usize,
    #[serde(serialize_with = "each", skip_serializing_if = "Vec::is_empty")]
    candidates: Vec<Candidate<'g>>,
    #[serde(serialize_with = "each")]
    reached: Vec<Reached<'g>>,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
    below_confidence: usize,
    unresolved: usize,
    #[serde(skip_serializing_if = "is_zero")]
    ambiguous: usize,
    #[serde(serialize_with = "each", skip_serializing_if = "Vec::is_empty")]
    file_scope: Vec<FileScope<'g>>,
    #[serde(serialize_with = "present", skip_serializing_if = "Option::is_none")]
    self_call: Option<SelfCall>,
}

#[derive(Serialize)]
pub(crate) struct CallersView<'g> {
    #[serde(flatten)]
    walk: Wired<Walk<'g>>,
    #[serde(serialize_with = "label::band")]
    band: ReachBand,
}

impl<'g> CallersView<'g> {
    pub(crate) fn new(walk: Walk<'g>) -> Self {
        Self {
            band: ReachBand::of(walk.reached.len(), walk.root.kind, walk.root.exported),
            walk: Wired(walk),
        }
    }
}

#[derive(Serialize)]
#[serde(remote = "SelfCall")]
struct SelfCallDef {
    confidence: u8,
    line: u32,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a field to skip_serializing_if by reference"
)]
pub(crate) const fn is_zero(count: &usize) -> bool {
    *count == 0
}

#[derive(Serialize)]
#[serde(remote = "FileScope")]
struct FileScopeDef<'g> {
    file: &'g str,
    confidence: u8,
}

#[derive(Serialize)]
#[serde(remote = "CallPath")]
struct CallPathDef<'g> {
    #[serde(with = "DefinitionDef")]
    from: Definition<'g>,
    #[serde(with = "DefinitionDef")]
    to: Definition<'g>,
    found: bool,
    #[serde(serialize_with = "each")]
    hops: Vec<Reached<'g>>,
    truncated: bool,
    turned_away: usize,
    at_depth_bound: usize,
    below_confidence: usize,
    #[serde(serialize_with = "passed_over")]
    passed_over: (usize, usize),
    #[serde(
        serialize_with = "path_candidates",
        skip_serializing_if = "no_path_candidates"
    )]
    candidates: PathCandidates<'g>,
}

fn passed_over<S: Serializer>(
    &(from, to): &(usize, usize),
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct PassedOver {
        from: usize,
        to: usize,
    }

    PassedOver { from, to }.serialize(serializer)
}

type PathCandidates<'g> = (Vec<Candidate<'g>>, Vec<Candidate<'g>>);

const fn no_path_candidates((from, to): &PathCandidates<'_>) -> bool {
    from.is_empty() && to.is_empty()
}

fn path_candidates<S: Serializer>(
    (from, to): &PathCandidates<'_>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Ends<'a, 'g> {
        #[serde(serialize_with = "each")]
        from: &'a [Candidate<'g>],
        #[serde(serialize_with = "each")]
        to: &'a [Candidate<'g>],
    }

    Ends { from, to }.serialize(serializer)
}

#[derive(Serialize)]
#[serde(remote = "References")]
struct ReferencesDef<'g> {
    #[serde(with = "DefinitionDef")]
    symbol: Definition<'g>,
    passed_over: usize,
    #[serde(serialize_with = "each", skip_serializing_if = "Vec::is_empty")]
    candidates: Vec<Candidate<'g>>,
    #[serde(serialize_with = "each")]
    sites: Vec<Reference<'g>>,
    total: usize,
    below_confidence: usize,
    #[serde(with = "SiteCountsDef")]
    counts: SiteCounts,
    ambiguous: usize,
    #[serde(serialize_with = "each", skip_serializing_if = "Vec::is_empty")]
    file_scope: Vec<FileScope<'g>>,
}

#[derive(Serialize)]
pub(crate) struct ReferencesView<'g> {
    #[serde(flatten)]
    references: Wired<References<'g>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    type_uses_unrecorded: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    data_uses_unrecorded: bool,
}

impl<'g> ReferencesView<'g> {
    pub(crate) fn new(references: References<'g>, conventions: &Conventions) -> Self {
        let kind = references.symbol.kind;
        Self {
            type_uses_unrecorded: conventions.type_uses == TypeUses::Nowhere && kind.is_type(),
            data_uses_unrecorded: conventions.unrecorded_data.contains(&kind),
            references: Wired(references),
        }
    }
}

#[derive(Serialize)]
#[serde(remote = "Reference")]
struct ReferenceDef<'g> {
    #[serde(with = "DefinitionDef")]
    site_of: Definition<'g>,
    line: u32,
    confidence: u8,
    call: bool,
    #[serde(serialize_with = "label::access")]
    access: Access,
}

#[derive(Serialize)]
#[serde(remote = "SiteCounts")]
struct SiteCountsDef {
    calls: usize,
    reads: usize,
    writes: usize,
    updates: usize,
    types: usize,
}

#[derive(Serialize)]
pub(crate) struct DefinitionsView<'g> {
    total: usize,
    found: Vec<FoundView<'g>>,
}

#[derive(Serialize)]
struct FoundView<'g> {
    #[serde(with = "DefinitionDef")]
    definition: Definition<'g>,
    #[serde(serialize_with = "label::nearness")]
    nearness: Nearness,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<Wired<Source>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(remote = "Source")]
struct SourceDef {
    doc_line: u32,
    first_line: u32,
    lines: u32,
    text: String,
    #[serde(serialize_with = "each")]
    cut_lines: Vec<CutLine>,
    resized: bool,
}

#[derive(Serialize)]
#[serde(remote = "CutLine")]
struct CutLineDef {
    line: u32,
    bytes: usize,
}

impl<'g> DefinitionsView<'g> {
    pub(crate) fn new(found: &Definitions<'g>, sources: Option<Vec<io::Result<Source>>>) -> Self {
        let mut sources = sources.map(Vec::into_iter);
        Self {
            total: found.total,
            found: found
                .found
                .iter()
                .map(|found| {
                    let (source, error) = match sources.as_mut().and_then(Iterator::next) {
                        Some(Ok(source)) => (Some(Wired(source)), None),
                        Some(Err(error)) => (None, Some(error.to_string())),
                        None => (None, None),
                    };
                    FoundView {
                        definition: found.definition,
                        nearness: found.nearness,
                        source,
                        error,
                    }
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(remote = "Search")]
struct SearchDef<'g> {
    #[serde(serialize_with = "each")]
    hits: Vec<Hit<'g>>,
    total: usize,
    named: usize,
    unmatched_words: Vec<String>,
    dropped_words: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    fuzzy_total: Option<usize>,
}

#[derive(Serialize)]
#[serde(remote = "Hit")]
struct HitDef<'g> {
    #[serde(with = "DefinitionDef")]
    definition: Definition<'g>,
    score: f64,
    exact: bool,
    fuzzy: bool,
}

#[derive(Serialize)]
#[serde(remote = "Implementations")]
struct ImplementationsDef<'g> {
    #[serde(with = "DefinitionDef")]
    root: Definition<'g>,
    passed_over: usize,
    #[serde(serialize_with = "each", skip_serializing_if = "Vec::is_empty")]
    candidates: Vec<Candidate<'g>>,
    #[serde(serialize_with = "each")]
    found: Vec<Implementor<'g>>,
    total: usize,
    unexpanded: usize,
}

#[derive(Serialize)]
#[serde(remote = "Implementor")]
struct ImplementorDef<'g> {
    #[serde(with = "DefinitionDef")]
    symbol: Definition<'g>,
    depth: u32,
}

#[derive(Debug, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wire answers `missing`, `directory` and `special` as keys of their own, and tests/reference.rs \
              reads a section's keys from these fields"
)]
pub(crate) struct FileView<'a, T> {
    file: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph: Option<&'a str>,
    held: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    missing: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    directory: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    special: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    symlink: Option<SymlinkView<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    declined: Option<DeclinedView<'a>>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    answer: Option<T>,
}

/// Where a held symlink points: its target as stored, and the held entry the target names.
#[derive(Debug, Serialize)]
struct SymlinkView<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<Box<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
}

impl<'a> SymlinkView<'a> {
    fn new(symlink: Symlink<'a>) -> Self {
        let (path, kind) = symlink
            .resolved
            .map(|entry| (entry.path().into(), super::files::kind(entry.kind())))
            .unzip();
        Self {
            target: symlink.target,
            path,
            kind,
        }
    }
}

/// The language that leaves a held file of its kind out of its graph, and the reason it states.
#[derive(Debug, Serialize)]
struct DeclinedView<'a> {
    language: &'a str,
    reason: &'a str,
}

impl<'a, T> FileView<'a, T> {
    /// What `ask` answers of `file` from the graph that claims it, or why no graph answers: the index holds no such
    /// path, the path is a directory, a symlink or a special file, a language leaves the held file out, or no language
    /// claims it.
    pub(crate) fn new(
        engine: &'a Engine,
        file: &'a str,
        ask: impl FnOnce(&'a Graph) -> Option<T>,
    ) -> Self {
        let claimed = engine.graph_of(file);
        let answer = claimed.and_then(|(_, graph)| ask(graph));
        let unheld = answer
            .is_none()
            .then(|| engine.index().entry(file).map(|entry| entry.kind()));
        let missing = unheld == Some(None);
        let directory = unheld == Some(Some(EntryKind::Dir));
        let symlink = (unheld == Some(Some(EntryKind::Symlink)))
            .then(|| engine.symlink(file))
            .flatten();
        let a_file = !missing && !directory;
        let declined = match claimed {
            None if a_file => engine.declined(file),
            _ => None,
        };
        Self {
            file,
            graph: claimed.filter(|_| a_file).map(|(name, _)| name),
            held: answer.is_some(),
            missing,
            directory,
            special: unheld == Some(Some(EntryKind::Other)),
            symlink: symlink.map(SymlinkView::new),
            declined: declined.map(|(language, reason)| DeclinedView { language, reason }),
            answer,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct OutlineView<'g> {
    #[serde(serialize_with = "each")]
    pub(crate) symbols: Vec<Definition<'g>>,
}

#[derive(Serialize)]
#[serde(remote = "Dependents")]
struct DependentsDef<'g> {
    #[serde(serialize_with = "each")]
    found: Vec<Dependent<'g>>,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
}

#[derive(Serialize)]
#[serde(remote = "Dependent")]
struct DependentDef<'g> {
    file: &'g str,
    depth: u32,
}

#[derive(Serialize)]
#[serde(remote = "Segment")]
struct SegmentDef<'g> {
    #[serde(flatten, serialize_with = "lines")]
    lines: RangeInclusive<u32>,
    #[serde(serialize_with = "each")]
    symbols: Vec<SegmentSymbol<'g>>,
    includes: Vec<&'g str>,
    included_by: Vec<&'g str>,
}

fn lines<S: Serializer>(lines: &RangeInclusive<u32>, serializer: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Lines {
        start_line: u32,
        end_line: u32,
    }

    Lines {
        start_line: *lines.start(),
        end_line: *lines.end(),
    }
    .serialize(serializer)
}

#[derive(Serialize)]
#[serde(remote = "SegmentSymbol")]
struct SegmentSymbolDef<'g> {
    #[serde(with = "DefinitionDef")]
    definition: Definition<'g>,
    #[serde(with = "DirectCallsDef")]
    callers: DirectCalls<'g>,
    #[serde(with = "DirectCallsDef", skip_serializing_if = "no_direct_calls")]
    users: DirectCalls<'g>,
    #[serde(skip_serializing_if = "is_zero")]
    ambiguous: usize,
    #[serde(with = "DirectCallsDef")]
    callees: DirectCalls<'g>,
    unresolved: usize,
    #[serde(with = "BlastDef")]
    blast: Blast,
}

#[derive(Serialize)]
#[serde(remote = "DirectCalls")]
struct DirectCallsDef<'g> {
    #[serde(serialize_with = "each")]
    listed: Vec<Reached<'g>>,
    total: usize,
    below_confidence: usize,
}

const fn no_direct_calls(calls: &DirectCalls<'_>) -> bool {
    calls.total == 0 && calls.below_confidence == 0
}

#[derive(Serialize)]
#[serde(remote = "Blast")]
struct BlastDef {
    reach: usize,
    by_depth: Vec<usize>,
    affected_files: usize,
    #[serde(serialize_with = "label::band")]
    band: ReachBand,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
    below_confidence: usize,
}

#[derive(Serialize)]
pub(crate) struct ChangeView<'e> {
    #[serde(flatten)]
    change: Wired<Change<'e>>,
    missing_regions: usize,
    symlink_regions: usize,
    directory_regions: usize,
    special_regions: usize,
    unclaimed_regions: usize,
    declined: Vec<DeclinedRegionsView<'e>>,
}

/// The regions of held files that one language leaves out of its graph for one reason.
#[derive(Serialize)]
struct DeclinedRegionsView<'e> {
    #[serde(flatten)]
    declined: DeclinedView<'e>,
    regions: usize,
}

impl<'e> ChangeView<'e> {
    pub(crate) fn new(answer: EngineChange<'e>) -> Self {
        Self {
            missing_regions: answer.missing_regions,
            symlink_regions: answer.symlink_regions,
            directory_regions: answer.directory_regions,
            special_regions: answer.special_regions,
            unclaimed_regions: answer.unclaimed_regions,
            declined: answer
                .declined
                .iter()
                .map(|declined| DeclinedRegionsView {
                    declined: DeclinedView {
                        language: declined.language,
                        reason: declined.reason,
                    },
                    regions: declined.regions,
                })
                .collect(),
            change: Wired(answer.change),
        }
    }
}

#[derive(Serialize)]
#[serde(remote = "Change")]
struct ChangeDef<'g> {
    changed_symbols: usize,
    changed_files: usize,
    unheld_regions: usize,
    regions_outside_symbols: usize,
    footprint: usize,
    footprint_files: usize,
    transitive_sum: usize,
    #[serde(serialize_with = "each")]
    hotspots: Vec<Hotspot<'g>>,
    total_hotspots: usize,
    #[serde(serialize_with = "each")]
    seeds: Vec<Seed<'g>>,
    truncated_seeds: usize,
}

#[derive(Serialize)]
#[serde(remote = "Hotspot")]
struct HotspotDef<'g> {
    #[serde(with = "DefinitionDef")]
    symbol: Definition<'g>,
    hit_count: usize,
    min_depth: u32,
    also_changed: bool,
}

#[derive(Serialize)]
#[serde(remote = "Seed")]
struct SeedDef<'g> {
    #[serde(with = "DefinitionDef")]
    symbol: Definition<'g>,
    reach: usize,
    #[serde(serialize_with = "label::band")]
    band: ReachBand,
    caller_count: usize,
    #[serde(skip_serializing_if = "is_zero")]
    user_count: usize,
    #[serde(skip_serializing_if = "is_zero")]
    ambiguous: usize,
    #[serde(serialize_with = "each")]
    consumers: Vec<Reached<'g>>,
    #[serde(serialize_with = "each")]
    consumer_dirs: Vec<DirCount<'g>>,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
    below_confidence: usize,
}

#[derive(Serialize)]
#[serde(remote = "DirCount")]
struct DirCountDef<'g> {
    dir: &'g str,
    count: usize,
}

#[derive(Serialize)]
#[serde(remote = "DeadCode")]
struct DeadCodeDef<'g> {
    #[serde(serialize_with = "each")]
    found: Vec<Dead<'g>>,
    total: usize,
    live: usize,
    #[serde(with = "ExcludedDef")]
    excluded: Excluded,
}

#[derive(Serialize)]
#[serde(remote = "Dead")]
struct DeadDef<'g> {
    #[serde(with = "DefinitionDef")]
    definition: Definition<'g>,
    spelled: bool,
}

#[derive(Serialize)]
#[serde(remote = "Excluded")]
struct ExcludedDef {
    kind: usize,
    test: usize,
    generated: usize,
    entry: usize,
    vendored: usize,
    unobservable: usize,
    uncertain: usize,
    dispatched: usize,
    reflected: usize,
}

#[derive(Serialize)]
#[serde(remote = "Resolution")]
struct ResolutionDef<'g> {
    sites: usize,
    #[serde(serialize_with = "bound")]
    bound: Vec<(u8, usize)>,
    edges: usize,
    #[serde(with = "MissesDef")]
    misses: Misses,
    #[serde(serialize_with = "each")]
    unbound: Vec<Unbound<'g>>,
    unlisted: usize,
}

fn bound<S: Serializer>(bound: &[(u8, usize)], serializer: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Bound {
        confidence: u8,
        sites: usize,
    }

    serializer.collect_seq(
        bound
            .iter()
            .map(|&(confidence, sites)| Bound { confidence, sites }),
    )
}

#[derive(Serialize)]
#[serde(remote = "Misses")]
struct MissesDef {
    external_import: usize,
    not_exported: usize,
    undefined: usize,
    not_admitted: usize,
    ambiguous: usize,
    #[serde(skip_serializing_if = "is_zero")]
    past_bound: usize,
    through_a_receiver: usize,
}

#[derive(Serialize)]
#[serde(remote = "Unbound")]
struct UnboundDef<'g> {
    #[serde(with = "DefinitionDef")]
    caller: Definition<'g>,
    line: u32,
    callee: &'g str,
    #[serde(skip_serializing_if = "Option::is_none")]
    receiver: Option<&'g str>,
    #[serde(serialize_with = "label::miss")]
    miss: Miss,
}

#[derive(Serialize)]
#[serde(remote = "LanguageFiles")]
struct LanguageFilesDef<'e> {
    name: &'e str,
    family: &'e str,
    claimed: usize,
    read: usize,
    cached: usize,
    held: usize,
}

#[derive(Serialize)]
#[serde(remote = "Stats")]
struct StatsDef {
    files: usize,
    files_without_symbols: usize,
    symbols: usize,
    #[serde(serialize_with = "label::kinds")]
    kinds: Vec<(SymbolKind, usize)>,
    calls: usize,
    local_calls: usize,
    uses: usize,
    reads: usize,
    writes: usize,
    updates: usize,
    types: usize,
    includes: usize,
}

#[derive(Serialize)]
#[serde(remote = "Unmatched")]
struct UnmatchedDef<'g> {
    #[serde(serialize_with = "label::unmatched")]
    kind: UnmatchedKind,
    total: usize,
    #[serde(serialize_with = "graphed")]
    listed: Vec<(&'g str, Definition<'g>)>,
    #[serde(serialize_with = "label::kinds", skip_serializing_if = "Vec::is_empty")]
    kinds: Vec<(SymbolKind, usize)>,
}

fn graphed<S: Serializer>(
    listed: &[(&str, Definition<'_>)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct GraphDefinition<'g> {
        graph: &'g str,
        #[serde(with = "DefinitionDef")]
        definition: Definition<'g>,
    }

    serializer.collect_seq(
        listed
            .iter()
            .map(|&(graph, definition)| GraphDefinition { graph, definition }),
    )
}
