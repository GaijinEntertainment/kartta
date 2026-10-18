use std::collections::HashMap;
use std::iter;

use kartta::{
    Access, CallPath, Change, Definition, DirectCalls, Hotspot, ReachBand, Reached, Reference,
    References, Seed, Segment, SymbolKind, Walk,
};

use super::{
    Suggestion, confidence, edge_role, empty_languages_note, file_element, flat, kind, loc, named,
    role, span, unresolved,
};
use crate::llmxml::Element;

/// How many of a changed symbol's direct callers, or of their directories, an `impact_of_change` answer lists.
const SEED_CALLERS: usize = 10;

/// The blind spot of a method's sites.
const METHOD_NOTE: &str = "a call through a value of the owner's type resolves only where the graph can name that \
                           type, so some of those calls can be missing";

/// The blind spot of a field's sites in a language whose extraction records no access to a field.
const FIELD_NOTE: &str = "this language's extraction records no access to a field yet, so every use of it is missing \
                          from this answer";

/// The families whose extraction records field accesses and reads of package-level values.
const RECORDS_DATA_ACCESS: &[&str] = &["go", "jsts", "python"];

/// What a field's sites leave out in a family that records field accesses, where it leaves out more than the
/// `ambiguous` count says.
const PARTIAL_DATA_ACCESS: &[(&str, &str)] = &[
    (
        "jsts",
        "a member access on a value whose declaration spells no one type binds nothing; ambiguous counts those",
    ),
    (
        "python",
        "an attribute of a value whose class the extraction cannot name binds only where one field of the name is in \
         its own file; ambiguous counts the rest",
    ),
];

/// What a `find_call_path` call asked, as its answer echoes it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathAsk<'a> {
    pub(crate) from: &'a str,
    pub(crate) to: &'a str,
    pub(crate) languages: &'a str,
    pub(crate) hops: u32,
    pub(crate) min_confidence: u8,
}

/// A caller, a callee or a hop: where the symbol is defined, its role, how surely its call binds.
fn edge(tag: &'static str, reached: &Reached<'_>) -> Element {
    let symbol = &reached.symbol;
    let element = named(tag, symbol)
        .attr("loc", loc(symbol.file, symbol.line))
        .attr("role", edge_role(symbol.flags));
    match confidence(reached.confidence) {
        Some(band) => element.attr("confidence", band),
        None => element,
    }
}

/// The `find_call_path` answer: one `<match>` per family graph both names select a symbol in.
pub(crate) fn call_path(ask: &PathAsk<'_>, paths: &[(&str, CallPath<'_>)]) -> String {
    let mut element = Element::new("call_path")
        .attr("from", ask.from)
        .attr("to", ask.to);
    if !ask.languages.is_empty() {
        element = element.attr("languages", ask.languages);
    }
    element = element.attr("matches", paths.len());
    if paths.is_empty() {
        return element.inline(empty_languages_note(ask.languages).unwrap_or(
            "no symbol matches from or to in one graph - find_symbols can pin the exact names",
        ));
    }
    element.children(
        paths
            .iter()
            .map(|(family, path)| path_match(ask, family, path)),
    )
}

fn path_match(ask: &PathAsk<'_>, family: &str, path: &CallPath<'_>) -> String {
    let (passed_over_from, passed_over_to) = path.passed_over;
    let element = Element::new("match")
        .attr("lang", family)
        .attr("from", path.from.qualified_name)
        .attr("to", path.to.qualified_name)
        .attr("hops", path.hops.len())
        .attr("max_hops", ask.hops)
        .flag("truncated", path.truncated)
        .count("turned_away", path.turned_away)
        .count("at_depth_bound", path.at_depth_bound)
        .count("below_confidence", path.below_confidence)
        .count("passed_over_from", passed_over_from)
        .count("passed_over_to", passed_over_to);
    if !path.found && path.truncated {
        return element.attr("found", false).inline(&format!(
            "the node cap stopped the search before it covered {} calls, so a chain can still exist in the graph - a \
             smaller hops value or a higher min_confidence narrows the search",
            ask.hops
        ));
    }
    if !path.found {
        let floor = match confidence(ask.min_confidence) {
            _ if ask.min_confidence == 0 => String::new(),
            band => format!(
                " whose every call resolves at confidence {} or stronger",
                band.unwrap_or("certain")
            ),
        };
        return element.attr("found", false).inline(&format!(
            "no call chain within {} calls{floor} - the graph misses dynamic and unresolved calls, so this does not \
             show that the code holds none",
            ask.hops
        ));
    }
    element.children(path.hops.iter().map(|hop| {
        edge("ref", hop)
            .attr("call", loc(hop.symbol.file, hop.line))
            .close()
    }))
}

/// The `find_references` answer: one `<match>` per family graph the name selects a symbol in.
pub(crate) fn references(
    ask: &CallersAsk<'_>,
    found: &[(&str, References<'_>)],
    suggestions: &[Suggestion<'_>],
) -> String {
    let mut element = Element::new("references").attr("of", ask.name);
    if !ask.languages.is_empty() {
        element = element.attr("languages", ask.languages);
    }
    element = element.attr("matches", found.len());
    if found.is_empty() {
        return unresolved(element, ask.languages, suggestions);
    }
    element.children(
        found
            .iter()
            .map(|(family, references)| reference_match(ask.floored, family, references)),
    )
}

fn reference_match(floored: bool, family: &str, references: &References<'_>) -> String {
    let symbol = &references.symbol;
    let mut element = Element::new("match")
        .attr("lang", family)
        .attr("of", symbol.name);
    if symbol.qualified_name != symbol.name {
        element = element.attr("qn", symbol.qualified_name);
    }
    let counts = &references.counts;
    element = element
        .attr("loc", loc(symbol.file, symbol.line))
        .attr("kind", kind(symbol.kind))
        .attr("count", references.total);
    if references.total > references.sites.len() {
        element = element.attr("shown", references.sites.len());
    }
    let reads = if is_type(symbol.kind) {
        "types"
    } else {
        "reads"
    };
    element = element
        .count("calls", counts.calls)
        .count(reads, counts.reads)
        .count("writes", counts.writes)
        .count("updates", counts.updates)
        .count("ambiguous", references.ambiguous)
        .count("below_confidence", references.below_confidence)
        .count("passed_over", references.passed_over);
    let member_note = match symbol.kind.label() {
        "method" => Some(METHOD_NOTE),
        "field" if !RECORDS_DATA_ACCESS.contains(&family) => Some(FIELD_NOTE),
        "field" => PARTIAL_DATA_ACCESS
            .iter()
            .find(|(partial, _)| *partial == family)
            .map(|&(_, note)| note),
        _ => None,
    };
    if references.sites.is_empty() {
        let floor = if floored {
            " at this confidence floor"
        } else {
            ""
        };
        let mut text = format!(
            "no recorded site{floor} - resolved calls can omit dynamic or unresolved ones, and uses other than calls \
             are recorded only where the graph records them, so this does not show that nothing uses the symbol"
        );
        if let Some(note) = member_note {
            text.push_str("; ");
            text.push_str(note);
        }
        return element.inline(&text);
    }
    let note = member_note.map(|note| Element::new("note").inline(note));
    let records = site_records(symbol.kind, &references.sites);
    element.children(note.into_iter().chain(records.iter().map(|record| {
        let enclosing = &record.enclosing;
        let lines: Vec<String> = record.lines.iter().map(u32::to_string).collect();
        let element = named("ref", enclosing)
            .attr("loc", loc(enclosing.file, enclosing.line))
            .attr("role", edge_role(enclosing.flags))
            .attr("site", record.site)
            .attr("at", format!("{}:{}", enclosing.file, lines.join(",")));
        match record.band {
            Some(band) => element.attr("confidence", band),
            None => element,
        }
        .close()
    })))
}

/// The sites one enclosing symbol holds of one kind and confidence band, on the lines they are on.
struct SiteRecord<'g> {
    enclosing: Definition<'g>,
    site: &'static str,
    band: Option<&'static str>,
    lines: Vec<u32>,
}

fn site_records<'g>(of: SymbolKind, sites: &[Reference<'g>]) -> Vec<SiteRecord<'g>> {
    let mut records: Vec<SiteRecord<'g>> = Vec::new();
    let mut held: HashMap<(Definition<'g>, &'static str, Option<&'static str>), usize> =
        HashMap::new();
    for site in sites {
        let kind = site_kind(of, site);
        let band = confidence(site.confidence);
        let key = (site.site_of, kind, band);
        if let Some(&at) = held.get(&key) {
            records[at].lines.push(site.line);
            continue;
        }
        held.insert(key, records.len());
        records.push(SiteRecord {
            enclosing: site.site_of,
            site: kind,
            band,
            lines: vec![site.line],
        });
    }
    records
}

const fn site_kind(of: SymbolKind, site: &Reference<'_>) -> &'static str {
    match site.access {
        _ if site.call => "call",
        _ if is_type(of) => "type",
        Access::Read => "read",
        Access::Write => "write",
        Access::Update => "update",
    }
}

const fn is_type(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class
            | SymbolKind::Struct
            | SymbolKind::Interface
            | SymbolKind::Enum
            | SymbolKind::TypeAlias
            | SymbolKind::Trait
            | SymbolKind::DefinedType
    )
}

/// The `impact_of_edit` answer: a `<symbol>` per definition the lines overlap, with its direct callers and callees
/// and the reach of a change to it, and the files the file includes and is included by.
pub(crate) fn impact(file: &str, lines: (u32, u32), segment: Option<&Segment<'_>>) -> String {
    const NO_SYMBOLS: &str = "no symbols defined in this range - it may cover only file-scope code (imports, \
                              comments); zero symbols is not proof the edit is safe";
    let (start_line, end_line) = lines;
    let end = if end_line == u32::MAX {
        "end".to_owned()
    } else {
        end_line.to_string()
    };
    let element = Element::new("impact")
        .attr("of", file)
        .attr("start_line", start_line)
        .attr("end_line", end);
    let Some(segment) = segment else {
        return element.attr("symbols", 0).inline(
            "not in a code graph - an unrecognized extension, a language not indexed, or a path the index does not \
             hold: pass a repo-relative path as printed by find_files",
        );
    };
    let element = element.attr("symbols", segment.symbols.len());
    let has_includes = !segment.includes.is_empty() || !segment.included_by.is_empty();
    if segment.symbols.is_empty() && !has_includes {
        return element.inline(NO_SYMBOLS);
    }
    let mut parts: Vec<String> = if segment.symbols.is_empty() {
        vec![NO_SYMBOLS.to_owned()]
    } else {
        segment
            .symbols
            .iter()
            .map(|symbol| {
                let definition = &symbol.definition;
                let blast = &symbol.blast;
                let mut record = named("symbol", definition)
                    .attr("kind", kind(definition.kind))
                    .attr(
                        "lines",
                        format!("{}-{}", definition.line, definition.end_line),
                    )
                    .attr("callers", symbol.callers.total)
                    .attr("callees", symbol.callees.total)
                    .attr("reach", blast.reach)
                    .attr("reach_files", blast.affected_files)
                    .attr("reach_band", blast.band.label())
                    .flag("truncated", blast.truncated)
                    .count("unexpanded", blast.unexpanded)
                    .count("unresolved", symbol.unresolved);
                if let Some(signature) = definition.signature {
                    record = record.attr("sig", flat(signature));
                }
                record.children(
                    [
                        direct("callers", &symbol.callers, None),
                        direct("callees", &symbol.callees, Some(definition.file)),
                    ]
                    .into_iter()
                    .flatten(),
                )
            })
            .collect()
    };
    parts.push(
        Element::new("includes")
            .attr("count", segment.includes.len())
            .children(segment.includes.iter().map(|path| file_element(path))),
    );
    parts.push(
        Element::new("included-by")
            .attr("count", segment.included_by.len())
            .children(segment.included_by.iter().map(|path| file_element(path))),
    );
    element.children(parts)
}

/// The `impact_of_change` answer: what the change reaches in all, then the changed symbols, widest reach first, and
/// the symbols where their reaches meet, each list cut to `limit` and saying so as `shown` of `of`.
pub(crate) fn change(regions: usize, change: &Change<'_>, limit: usize) -> String {
    let element = Element::new("change")
        .attr("regions", regions)
        .attr("changed_symbols", change.changed_symbols)
        .attr("changed_files", change.changed_files)
        .attr("footprint", change.footprint)
        .attr("footprint_files", change.footprint_files)
        .count("truncated_walks", change.truncated_seeds)
        .count("unheld_regions", change.unheld_regions)
        .count("regions_outside_symbols", change.regions_outside_symbols);
    if change.changed_symbols == 0 {
        return element.inline(
            "no symbols in these regions - they cover only file-scope code (imports, comments) or files no code graph \
             holds (counted in unheld_regions); zero symbols is not proof the change is safe, and find_importers lists \
             the files that include a changed file",
        );
    }
    let seeds = Element::new("seeds")
        .attr("shown", change.seeds.len().min(limit))
        .attr("of", change.changed_symbols)
        .children(change.seeds.iter().take(limit).map(seed));
    let hotspots = (change.total_hotspots > 0).then(|| {
        Element::new("hotspots")
            .attr("shown", change.hotspots.len().min(limit))
            .attr("of", change.total_hotspots)
            .children(change.hotspots.iter().take(limit).map(hotspot))
    });
    element.children(iter::once(seeds).chain(hotspots))
}

/// A changed symbol: how far a change to it reaches, what its walk left out, and its direct callers, or their
/// directories when there are many.
fn seed(seed: &Seed<'_>) -> String {
    let symbol = &seed.symbol;
    let mut element = named("seed", symbol);
    if let Some(role) = role(symbol.flags) {
        element = element.attr("role", role);
    }
    element = element
        .attr("kind", kind(symbol.kind))
        .attr("loc", span(symbol.file, symbol.line, symbol.end_line))
        .attr("callers", seed.caller_count)
        .attr("reach", seed.reach)
        .attr("reach_band", seed.band.label())
        .flag("truncated", seed.truncated)
        .count("turned_away", seed.turned_away)
        .count("unexpanded", seed.unexpanded);
    let callers = (!seed.consumers.is_empty()).then(|| {
        Element::new("callers")
            .attr("shown", seed.consumers.len().min(SEED_CALLERS))
            .attr("of", seed.consumers.len())
            .children(seed.consumers.iter().take(SEED_CALLERS).map(|reached| {
                edge("ref", reached)
                    .attr("at", loc(reached.symbol.file, reached.line))
                    .close()
            }))
    });
    let dirs = (!seed.consumer_dirs.is_empty()).then(|| {
        Element::new("caller_dirs")
            .attr("shown", seed.consumer_dirs.len().min(SEED_CALLERS))
            .attr("of", seed.consumer_dirs.len())
            .children(seed.consumer_dirs.iter().take(SEED_CALLERS).map(|dir| {
                Element::new("dir")
                    .attr("path", dir.dir)
                    .attr("callers", dir.count)
                    .close()
            }))
    });
    element.children(callers.into_iter().chain(dirs))
}

/// A symbol two or more changed symbols reach: how many, and how near the nearest.
fn hotspot(hotspot: &Hotspot<'_>) -> String {
    let symbol = &hotspot.symbol;
    named("hotspot", symbol)
        .attr("kind", kind(symbol.kind))
        .attr("loc", loc(symbol.file, symbol.line))
        .attr("role", edge_role(symbol.flags))
        .attr("reached_by", hotspot.hit_count)
        .attr("hops", hotspot.min_depth)
        .flag("changed", hotspot.also_changed)
        .close()
}

/// One side of a symbol's direct calls, its listed refs disclosing the list's cap as `shown` of `of`. Each call is in
/// `calling_file`, or in the listed symbol's own file when that is the caller.
fn direct(
    tag: &'static str,
    calls: &DirectCalls<'_>,
    calling_file: Option<&str>,
) -> Option<String> {
    if calls.listed.is_empty() {
        return None;
    }
    Some(
        Element::new(tag)
            .attr("shown", calls.listed.len())
            .attr("of", calls.total)
            .count("below_confidence", calls.below_confidence)
            .children(calls.listed.iter().map(|reached| {
                let file = calling_file.unwrap_or(reached.symbol.file);
                edge("ref", reached)
                    .attr("at", loc(file, reached.line))
                    .close()
            })),
    )
}

/// Each caller role, and the `<match>` attribute that counts it when `find_callers` keeps one role.
const ROLE_COUNTS: [(&str, &str); 4] = [
    ("consumer", "consumers"),
    ("test", "tests"),
    ("entry", "entries"),
    ("generated", "generated"),
];

/// What a `find_callers` call asked, as its answer echoes it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallersAsk<'a> {
    pub(crate) name: &'a str,
    pub(crate) languages: &'a str,
    pub(crate) role: Option<&'a str>,
    pub(crate) floored: bool,
}

/// The `find_callers` answer: one `<match>` per family graph the name selects a symbol in, or the alternatives to a
/// name none does.
pub(crate) fn callers(
    ask: &CallersAsk<'_>,
    walks: &[(&str, Walk<'_>)],
    suggestions: &[Suggestion<'_>],
) -> String {
    let mut element = Element::new("callers").attr("of", ask.name);
    if !ask.languages.is_empty() {
        element = element.attr("languages", ask.languages);
    }
    if let Some(role) = ask.role {
        element = element.attr("role", role);
    }
    element = element.attr("matches", walks.len());
    if walks.is_empty() {
        return unresolved(element, ask.languages, suggestions);
    }
    element.children(
        walks
            .iter()
            .map(|(family, walk)| caller_match(ask, family, walk)),
    )
}

/// One graph's walk: the symbol, how many callers it reached and how widely that ranks, what the walk left out, and a
/// flat `<ref>` per caller, nearest first.
fn caller_match(ask: &CallersAsk<'_>, family: &str, walk: &Walk<'_>) -> String {
    let root = &walk.root;
    let reached = walk.reached.len();
    let mut element = Element::new("match")
        .attr("lang", family)
        .attr("of", root.name);
    if root.qualified_name != root.name {
        element = element.attr("qn", root.qualified_name);
    }
    element = element
        .attr("loc", loc(root.file, root.line))
        .attr("count", reached)
        .attr(
            "reach_band",
            ReachBand::of(reached, root.kind, root.exported).label(),
        )
        .flag("truncated", walk.truncated)
        .count("turned_away", walk.turned_away)
        .count("unexpanded", walk.unexpanded)
        .count("below_confidence", walk.below_confidence)
        .count("passed_over", walk.passed_over);
    if ask.role.is_some() {
        let mut callers = [0_usize; ROLE_COUNTS.len()];
        for caller in &walk.reached {
            let role = edge_role(caller.symbol.flags);
            if let Some(at) = ROLE_COUNTS.iter().position(|(counted, _)| *counted == role) {
                callers[at] += 1;
            }
        }
        for ((_, attribute), callers) in ROLE_COUNTS.iter().zip(callers) {
            element = element.attr(attribute, callers);
        }
    }
    if walk.reached.is_empty() {
        return element.inline(&no_callers(root.kind, ask.floored));
    }
    element.children(
        walk.reached
            .iter()
            .filter(|caller| {
                ask.role
                    .is_none_or(|role| edge_role(caller.symbol.flags) == role)
            })
            .map(caller),
    )
}

/// One caller: where it is defined, how far from the symbol, how surely its call binds, and the line of that call.
fn caller(reached: &Reached<'_>) -> String {
    let symbol = &reached.symbol;
    let mut element = named("ref", symbol)
        .attr("loc", loc(symbol.file, symbol.line))
        .attr("role", edge_role(symbol.flags));
    if let Some(band) = confidence(reached.confidence) {
        element = element.attr("confidence", band);
    }
    element
        .attr("hops", reached.depth)
        .attr("at", loc(symbol.file, reached.line))
        .close()
}

/// The `find_callees` answer: one `<match>` per family graph the name selects a symbol in, or the alternatives to a
/// name none does.
pub(crate) fn callees(
    ask: &CallersAsk<'_>,
    walks: &[(&str, Walk<'_>)],
    suggestions: &[Suggestion<'_>],
) -> String {
    let mut element = Element::new("callees").attr("of", ask.name);
    if !ask.languages.is_empty() {
        element = element.attr("languages", ask.languages);
    }
    element = element.attr("matches", walks.len());
    if walks.is_empty() {
        return unresolved(element, ask.languages, suggestions);
    }
    element.children(
        walks
            .iter()
            .map(|(family, walk)| callee_match(ask.floored, family, walk)),
    )
}

/// One graph's walk: the symbol, how many callees it reached, what the walk left out, and a flat `<ref>` per callee,
/// nearest first.
fn callee_match(floored: bool, family: &str, walk: &Walk<'_>) -> String {
    let root = &walk.root;
    let mut element = Element::new("match")
        .attr("lang", family)
        .attr("of", root.name);
    if root.qualified_name != root.name {
        element = element.attr("qn", root.qualified_name);
    }
    element = element
        .attr("loc", loc(root.file, root.line))
        .attr("count", walk.reached.len())
        .flag("truncated", walk.truncated)
        .count("turned_away", walk.turned_away)
        .count("unexpanded", walk.unexpanded)
        .count("below_confidence", walk.below_confidence)
        .count("unresolved", walk.unresolved)
        .count("passed_over", walk.passed_over);
    if walk.reached.is_empty() {
        let floor = if floored {
            " at this confidence floor"
        } else {
            ""
        };
        return element.inline(&format!(
            "no callees - no call in its body binds a definition the graph holds{floor}; a call into a library or a \
             builtin, through a function value, or by dynamic dispatch binds none"
        ));
    }
    element.children(walk.reached.iter().map(|reached| {
        let record = edge("ref", reached).attr("hops", reached.depth);
        if reached.depth == 1 {
            record.attr("at", loc(root.file, reached.line)).close()
        } else {
            record.close()
        }
    }))
}

/// Why a symbol may have no callers, by what it is.
fn no_callers(of: SymbolKind, floored: bool) -> String {
    let label = of.label();
    let kind = kind(of);
    match label {
        "func" | "method" | "macro" => {
            "no callers - entry point, dead code, or dynamic dispatch".to_owned()
        }
        "field" => "no callers - a field is read and written, not called; find_references lists its reads and \
                    writes"
            .to_owned(),
        "variant" => format!(
            "no callers - the graph records no call or other use of this {kind}; grep searches the text for its name"
        ),
        _ if floored => format!(
            "no callers - no call at this confidence floor names this {kind}; find_references lists its recorded uses"
        ),
        _ => format!(
            "no callers - the graph holds no call that names this {kind}; find_references lists its recorded uses"
        ),
    }
}
