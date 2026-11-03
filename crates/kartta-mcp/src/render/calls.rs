use std::collections::HashMap;
use std::iter;
use std::ops::Range;

use kartta::{
    CallPath, Candidate, Change, Conventions, Definition, DirectCalls, Hotspot, ReachBand, Reached,
    Reference, References, Seed, Segment, Selector, SelfCall, SymbolKind, TypeUses, Walk,
};

use super::{
    Asked, Unmatched, candidate, confidence, edge_role, empty_languages_note, file_element, flat,
    graphless, kind, loc, named, noted, qn, role, span, unresolved,
};
use crate::llmxml::Element;

/// How many of a changed symbol's direct callers, or of their directories, an `impact_of_change` answer lists.
const SEED_CALLERS: usize = 10;

/// The blind spot of a method's sites.
const METHOD_NOTE: &str = "a call through a value of the owner's type resolves only where the graph can name that \
                           type, so some of those calls can be missing";

/// What a type's sites leave out in a family whose graph records no use of a type, after the name of that graph.
const NO_TYPE_USES: &str = "graph records no use of a type where code names it as one, such as a declaration, a \
                            signature, a generic argument or an import";

/// What a field's sites leave out in a family that records some field accesses, where it leaves out more than the
/// `ambiguous` count says.
const PARTIAL_DATA_ACCESS: &[(&str, &str)] = &[
    (
        "cpp",
        "a member access on a value whose type the graph cannot name binds only where one field of the name is in the \
         file or the files it includes; ambiguous counts the rest",
    ),
    (
        "jsts",
        "a member access on a value whose declaration spells no one type binds nothing; ambiguous counts those",
    ),
    (
        "python",
        "an attribute of a value whose class the extraction cannot name binds only where one field of the name is in \
         its own file; ambiguous counts the rest",
    ),
    (
        "quirrel",
        "a field is recorded only where a method of its owner names it bare, so its uses through this.x or a value's \
         x are missing from this answer",
    ),
];

/// What a `find_call_path` call asked, as its answer echoes it, with the reading of each end that selects nothing as
/// written.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathAsk<'a> {
    pub(crate) from: &'a str,
    pub(crate) from_read_as: Option<&'a str>,
    pub(crate) to: &'a str,
    pub(crate) to_read_as: Option<&'a str>,
    pub(crate) languages: Option<&'a str>,
    pub(crate) hops: u32,
}

/// The `<candidate>` records of the symbols a name also selects, each with the count it ranked by under `count`.
fn candidates(candidates: &[Candidate<'_>], count: &str) -> Vec<String> {
    candidates
        .iter()
        .map(|listed| candidate(&listed.symbol).attr(count, listed.count).close())
        .collect()
}

/// A caller, a callee or a hop: where the symbol is defined, its role, how surely its call binds.
fn edge(tag: &'static str, reached: &Reached<'_>) -> Element {
    let symbol = &reached.symbol;
    named(tag, symbol)
        .attr("loc", loc(symbol.file, symbol.line))
        .attr("role", edge_role(symbol.flags))
        .optional("confidence", confidence(reached.confidence))
}

/// The `find_call_path` answer: one `<match>` per family graph both names select a symbol in. With none, each end
/// that selects no symbol in any graph searched lists what its name matches instead, under its own `<from>` or
/// `<to>`.
pub(crate) fn call_path(
    ask: &PathAsk<'_>,
    paths: &[(&str, CallPath<'_>)],
    unmatched_ends: &[(&'static str, Selector<'_>, Unmatched<'_>)],
) -> String {
    let element = Element::new("call_path")
        .attr("from", ask.from)
        .optional("from_read_as", ask.from_read_as)
        .attr("to", ask.to)
        .optional("to_read_as", ask.to_read_as)
        .optional("languages", ask.languages)
        .attr("matches", paths.len());
    if paths.is_empty() {
        if let Some(note) = empty_languages_note(ask.languages) {
            return element.inline(note);
        }
        if unmatched_ends.is_empty() {
            return element.inline(
                "no graph holds both a symbol from selects and one to selects - find_symbols can pin the exact \
                 names",
            );
        }
        return element.children(
            unmatched_ends
                .iter()
                .map(|&(end, selector, ref unmatched)| {
                    let parameters = format!("{end}_");
                    let asked = Asked {
                        end: &parameters,
                        selector,
                    };
                    unresolved(Element::new(end), None, asked, unmatched)
                }),
        );
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
    let (from_candidates, to_candidates) = &path.candidates;
    let passed_over: Vec<String> = [("from", from_candidates), ("to", to_candidates)]
        .into_iter()
        .flat_map(|(end, listed)| {
            listed.iter().map(move |listed| {
                candidate(&listed.symbol)
                    .attr("end", end)
                    .attr("callers", listed.count)
                    .close()
            })
        })
        .collect();
    if !path.found && path.truncated {
        let text = format!(
            "the node cap stopped the search before it covered {} calls, so a chain can still exist in the graph - a \
             smaller hops value or a higher min_confidence narrows the search",
            ask.hops
        );
        return noted(element.attr("found", false), &text, passed_over);
    }
    if !path.found {
        let text = if path.below_confidence > 0 {
            below_the_floor(
                &format!("no call chain within {} calls", ask.hops),
                path.below_confidence,
                ("caller binds", "callers bind"),
                "min_confidence \"any\" follows",
            )
        } else {
            format!(
                "no call chain within {} calls - the graph misses dynamic and unresolved calls, so this does not show \
                 that the code holds none",
                ask.hops
            )
        };
        return noted(element.attr("found", false), &text, passed_over);
    }
    let hops = path.hops.iter().map(|hop| {
        edge("ref", hop)
            .attr("call", loc(hop.symbol.file, hop.line))
            .close()
    });
    element.children(hops.chain(passed_over))
}

/// The `find_references` answer: one `<match>` per family graph the name selects a symbol in, with the conventions
/// that say which uses that family records. `keeps_one_access` says that the call kept the sites of one access.
pub(crate) fn references(
    ask: &CallersAsk<'_>,
    keeps_one_access: bool,
    found: &[(&str, &Conventions, References<'_>)],
    unmatched: &Unmatched<'_>,
) -> String {
    let element = Element::new("references")
        .attr("of", ask.selector.name)
        .optional("read_as", ask.read_as)
        .optional("languages", ask.languages)
        .attr("matches", found.len());
    if found.is_empty() {
        return unresolved(element, ask.languages, ask.asked(), unmatched);
    }
    element.children(found.iter().map(|(family, conventions, references)| {
        reference_match(keeps_one_access, family, conventions, references)
    }))
}

/// How many sites a `find_references` match lists one by one, in the files with the most sites.
const LISTED_SITES: usize = 100;

/// How many more files a `find_references` match names once its listed sites run out, each with its count of sites.
const COUNTED_FILES: usize = 50;

fn reference_match(
    keeps_one_access: bool,
    family: &str,
    conventions: &Conventions,
    references: &References<'_>,
) -> String {
    let symbol = &references.symbol;
    let counts = &references.counts;
    let listing = Listing::of(&references.sites);
    let shown = references.sites.len().min(LISTED_SITES);
    let element = match_of(family, symbol)
        .attr("kind", kind(symbol.kind))
        .attr("count", references.total)
        .optional("shown", (references.total > shown).then_some(shown))
        .count("files", listing.files)
        .optional(
            "files_shown",
            (listing.files > listing.records.len()).then_some(listing.records.len()),
        )
        .count("calls", counts.calls)
        .count("types", counts.types)
        .count("reads", counts.reads)
        .count("writes", counts.writes)
        .count("updates", counts.updates)
        .count("ambiguous", references.ambiguous)
        .count("below_confidence", references.below_confidence)
        .count("passed_over", references.passed_over)
        .count("file_scope", references.file_scope.len());
    let member_note = match symbol.kind.label() {
        "method" => Some(METHOD_NOTE),
        "field" => PARTIAL_DATA_ACCESS
            .iter()
            .find(|(partial, _)| *partial == family)
            .map(|&(_, note)| note),
        _ => None,
    };
    let unrecorded = Unrecorded::of(conventions, symbol.kind);
    let passed_over = candidates(&references.candidates, "sites");
    if references.sites.is_empty() && references.file_scope.is_empty() {
        let text = if references.below_confidence > 0 {
            let lists = if keeps_one_access {
                "min_confidence \"any\" without access lists"
            } else {
                "min_confidence \"any\" lists"
            };
            below_the_floor(
                "no recorded site",
                references.below_confidence,
                ("use binds", "uses bind"),
                lists,
            )
        } else {
            no_sites(family, unrecorded, member_note)
        };
        return noted(element, &text, passed_over);
    }
    let missing = unrecorded.map(|gap| {
        format!(
            "{}, so every such use is missing from this answer",
            gap.statement(family)
        )
    });
    let note = (missing.as_deref())
        .or(member_note)
        .map(|note| Element::new("note").inline(note));
    let file_scope = references.file_scope.iter().map(|user| {
        Element::new("file_scope")
            .attr("path", user.file)
            .optional("confidence", confidence(user.confidence))
            .close()
    });
    element.children(
        note.into_iter()
            .chain(listing.records)
            .chain(file_scope)
            .chain(passed_over),
    )
}

/// The uses of a symbol that its family's graph records nowhere, as the family's conventions state them.
#[derive(Clone, Copy)]
enum Unrecorded {
    /// A type's uses as one, where the graph records no use of a type.
    TypeUses,
    /// The reads and writes of a field, a variable or a constant, by the word for its kind, where the graph records
    /// none of that kind.
    DataUses(&'static str),
}

impl Unrecorded {
    fn of(conventions: &Conventions, of: SymbolKind) -> Option<Self> {
        if conventions.type_uses == TypeUses::Nowhere && of.is_type() {
            return Some(Self::TypeUses);
        }
        (conventions.unrecorded_data.contains(&of)).then(|| Self::DataUses(kind(of)))
    }

    /// The statement that the graph of `family` records none of these uses.
    fn statement(self, family: &str) -> String {
        match self {
            Self::TypeUses => format!("the {family} {NO_TYPE_USES}"),
            Self::DataUses(what) => {
                format!("the {family} graph records no read or write of a {what}")
            }
        }
    }

    const fn what(self) -> &'static str {
        match self {
            Self::TypeUses => "type",
            Self::DataUses(what) => what,
        }
    }
}

/// Why a match lists no site at any confidence: what the graph of `family` records none of, else the caveat that fits
/// any language with the `member_note` of a method's or a field's sites.
fn no_sites(family: &str, unrecorded: Option<Unrecorded>, member_note: Option<&str>) -> String {
    if let Some(gap) = unrecorded {
        return format!(
            "no recorded site - {}, so this does not show that nothing uses the {}; grep finds where code names it",
            gap.statement(family),
            gap.what()
        );
    }
    let mut text = "no recorded site - resolved calls can omit dynamic or unresolved ones, and uses other than calls \
                    are recorded only where the graph records them, so this does not show that nothing uses the symbol"
        .to_owned();
    if let Some(note) = member_note {
        text.push_str("; ");
        text.push_str(note);
    }
    text
}

/// The `<file>` records of a `find_references` match: the files with the most sites first, the others in path order,
/// their sites listed one by one up to [`LISTED_SITES`], then [`COUNTED_FILES`] more files by their count alone.
struct Listing {
    records: Vec<String>,
    files: usize,
}

impl Listing {
    fn of(sites: &[Reference<'_>]) -> Self {
        let files = by_file(sites);
        let (mut unlisted, mut counted) = (LISTED_SITES, 0);
        let mut records = Vec::new();
        for file in &files {
            let sites = &sites[file.clone()];
            let listed = sites.len().min(unlisted);
            counted += usize::from(listed == 0);
            if counted > COUNTED_FILES {
                break;
            }
            unlisted -= listed;
            records.push(file_record(sites, listed));
        }
        Self {
            records,
            files: files.len(),
        }
    }
}

fn file_record(sites: &[Reference<'_>], listed: usize) -> String {
    let element = Element::new("file")
        .attr("path", sites[0].site_of.file)
        .attr("sites", sites.len());
    if listed == 0 {
        return element.close();
    }
    element
        .optional("shown", (listed < sites.len()).then_some(listed))
        .children(site_records(&sites[..listed]).iter().map(site_record))
}

/// The ranges of `sites`, which come in file and line order, that each file holds: the files with the most sites
/// first, the others in path order.
fn by_file(sites: &[Reference<'_>]) -> Vec<Range<usize>> {
    let mut files: Vec<Range<usize>> = Vec::new();
    for (at, site) in sites.iter().enumerate() {
        match files.last_mut() {
            Some(file) if sites[file.start].site_of.file == site.site_of.file => file.end = at + 1,
            _ => files.push(at..at + 1),
        }
    }
    files.sort_by_key(|file| std::cmp::Reverse(file.len()));
    files
}

fn site_record(record: &SiteRecord<'_>) -> String {
    let enclosing = &record.enclosing;
    let lines: Vec<String> = record.lines.iter().map(u32::to_string).collect();
    named("ref", enclosing)
        .attr("line", enclosing.line)
        .attr("role", edge_role(enclosing.flags))
        .attr("site", record.site)
        .attr("at", lines.join(","))
        .optional("confidence", record.band)
        .close()
}

/// The sites one enclosing symbol holds of one kind and confidence band, on the lines they are on.
struct SiteRecord<'g> {
    enclosing: Definition<'g>,
    site: &'static str,
    band: Option<&'static str>,
    lines: Vec<u32>,
}

fn site_records<'g>(sites: &[Reference<'g>]) -> Vec<SiteRecord<'g>> {
    let mut records: Vec<SiteRecord<'g>> = Vec::new();
    let mut held: HashMap<(Definition<'g>, &'static str, Option<&'static str>), usize> =
        HashMap::new();
    for site in sites {
        let kind = if site.call {
            "call"
        } else {
            site.access.label()
        };
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

/// The `impact_of_edit` answer: a `<symbol>` per definition the lines overlap, with its direct callers and callees
/// and the reach of a change to it, and the files the file includes and is included by. A field, a variable or a
/// constant lists its users in place of its callers, because its users hold them. `family` names the graph that claims
/// a file no graph answers for.
pub(crate) fn impact(
    file: &str,
    lines: (u32, u32),
    family: Option<&str>,
    segment: Option<&Segment<'_>>,
) -> String {
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
        return element.attr("symbols", 0).inline(&graphless(family));
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
                named("symbol", definition)
                    .attr("kind", kind(definition.kind))
                    .attr(
                        "lines",
                        format!("{}-{}", definition.line, definition.end_line),
                    )
                    .attr("callers", symbol.callers.total)
                    .count("users", symbol.users.total)
                    .count("ambiguous", symbol.ambiguous)
                    .attr("callees", symbol.callees.total)
                    .attr("reach", blast.reach)
                    .attr("reach_files", blast.affected_files)
                    .attr("reach_band", blast.band.label())
                    .flag("truncated", blast.truncated)
                    .count("unexpanded", blast.unexpanded)
                    .count("unresolved", symbol.unresolved)
                    .optional("sig", definition.signature.map(flat))
                    .children(
                        [
                            (symbol.users.total == 0)
                                .then(|| direct("callers", &symbol.callers, None))
                                .flatten(),
                            direct("users", &symbol.users, None),
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

/// A changed symbol: how far a change to it reaches, what its walk left out, and its direct callers, or a field's, a
/// variable's or a constant's direct users, or their directories when there are many.
fn seed(seed: &Seed<'_>) -> String {
    let symbol = &seed.symbol;
    let element = named("seed", symbol)
        .optional("role", role(symbol.flags))
        .attr("kind", kind(symbol.kind))
        .attr("loc", span(symbol))
        .attr("callers", seed.caller_count)
        .count("users", seed.user_count)
        .count("ambiguous", seed.ambiguous)
        .attr("reach", seed.reach)
        .attr("reach_band", seed.band.label())
        .flag("truncated", seed.truncated)
        .count("turned_away", seed.turned_away)
        .count("unexpanded", seed.unexpanded);
    let (listed, dirs_tag) = if seed.user_count > 0 {
        ("users", "user_dirs")
    } else {
        ("callers", "caller_dirs")
    };
    let callers = (!seed.consumers.is_empty()).then(|| {
        Element::new(listed)
            .attr("shown", seed.consumers.len().min(SEED_CALLERS))
            .attr("of", seed.consumers.len())
            .children(seed.consumers.iter().take(SEED_CALLERS).map(|reached| {
                edge("ref", reached)
                    .attr("at", loc(reached.symbol.file, reached.line))
                    .close()
            }))
    });
    let dirs = (!seed.consumer_dirs.is_empty()).then(|| {
        Element::new(dirs_tag)
            .attr("shown", seed.consumer_dirs.len().min(SEED_CALLERS))
            .attr("of", seed.consumer_dirs.len())
            .children(seed.consumer_dirs.iter().take(SEED_CALLERS).map(|dir| {
                Element::new("dir")
                    .attr("path", dir.dir)
                    .attr(listed, dir.count)
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

/// What a call that names one symbol asked, as its answer echoes it, with the reading of a name that selects nothing
/// as written.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallersAsk<'a> {
    pub(crate) selector: Selector<'a>,
    pub(crate) read_as: Option<&'a str>,
    pub(crate) languages: Option<&'a str>,
    pub(crate) role: Option<&'a str>,
}

impl<'a> CallersAsk<'a> {
    /// The selector as written, which the parameters `path_prefix` and `line` narrowed.
    pub(crate) const fn asked(&self) -> Asked<'a> {
        Asked {
            end: "",
            selector: self.selector,
        }
    }
}

/// The `find_callers` answer: one `<match>` per family graph the name selects a symbol in, or the alternatives to a
/// name none does.
pub(crate) fn callers(
    ask: &CallersAsk<'_>,
    walks: &[(&str, Walk<'_>)],
    unmatched: &Unmatched<'_>,
) -> String {
    let element = Element::new("callers")
        .attr("of", ask.selector.name)
        .optional("read_as", ask.read_as)
        .optional("languages", ask.languages)
        .optional("role", ask.role)
        .attr("matches", walks.len());
    if walks.is_empty() {
        return unresolved(element, ask.languages, ask.asked(), unmatched);
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
    let mut element = match_of(family, root)
        .attr("count", reached)
        .attr(
            "reach_band",
            ReachBand::of(reached, root.kind, root.exported).label(),
        )
        .flag("truncated", walk.truncated)
        .count("turned_away", walk.turned_away)
        .count("unexpanded", walk.unexpanded)
        .count("ambiguous", walk.ambiguous)
        .count("below_confidence", walk.below_confidence)
        .count("passed_over", walk.passed_over)
        .count("file_scope", walk.file_scope.len());
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
    let passed_over = candidates(&walk.candidates, "callers");
    let itself = walk.self_call.map(|call| self_call(root, call));
    if walk.reached.is_empty() && walk.file_scope.is_empty() {
        let text = no_callers(
            root.kind,
            walk.ambiguous,
            walk.below_confidence,
            itself.is_some(),
        );
        return noted(
            element,
            &text,
            itself.into_iter().chain(passed_over).collect(),
        );
    }
    let refs = walk
        .reached
        .iter()
        .filter(|caller| {
            ask.role
                .is_none_or(|role| edge_role(caller.symbol.flags) == role)
        })
        .map(|caller| {
            edge("ref", caller)
                .attr("hops", caller.depth)
                .attr("at", loc(caller.symbol.file, caller.line))
                .close()
        });
    let file_scope = walk
        .file_scope
        .iter()
        .filter(|_| ask.role.is_none())
        .map(|caller| {
            Element::new("file_scope")
                .attr("path", caller.file)
                .optional("confidence", confidence(caller.confidence))
                .close()
        });
    element.children(
        itself
            .into_iter()
            .chain(refs)
            .chain(file_scope)
            .chain(passed_over),
    )
}

/// The `find_callees` answer: one `<match>` per family graph the name selects a symbol in, or the alternatives to a
/// name none does.
pub(crate) fn callees(
    ask: &CallersAsk<'_>,
    walks: &[(&str, Walk<'_>)],
    unmatched: &Unmatched<'_>,
) -> String {
    let element = Element::new("callees")
        .attr("of", ask.selector.name)
        .optional("read_as", ask.read_as)
        .optional("languages", ask.languages)
        .attr("matches", walks.len());
    if walks.is_empty() {
        return unresolved(element, ask.languages, ask.asked(), unmatched);
    }
    element.children(
        walks
            .iter()
            .map(|(family, walk)| callee_match(family, walk)),
    )
}

/// One graph's walk: the symbol, how many callees it reached, what the walk left out, and a flat `<ref>` per callee,
/// nearest first.
fn callee_match(family: &str, walk: &Walk<'_>) -> String {
    let root = &walk.root;
    let element = match_of(family, root)
        .attr("count", walk.reached.len())
        .flag("truncated", walk.truncated)
        .count("turned_away", walk.turned_away)
        .count("unexpanded", walk.unexpanded)
        .count("below_confidence", walk.below_confidence)
        .count("unresolved", walk.unresolved)
        .count("passed_over", walk.passed_over);
    let passed_over = candidates(&walk.candidates, "callers");
    let itself = walk.self_call.map(|call| self_call(root, call));
    if walk.reached.is_empty() {
        let (none, other) = if itself.is_some() {
            ("no callees but itself", "no other call")
        } else {
            ("no callees", "no call")
        };
        let text = if walk.below_confidence > 0 {
            below_the_floor(
                none,
                walk.below_confidence,
                ("callee binds", "callees bind"),
                "min_confidence \"any\" lists",
            )
        } else {
            format!(
                "{none} - {other} in its body binds a definition the graph holds; a call into a library or a builtin, \
                 through a function value, or by dynamic dispatch binds none"
            )
        };
        return noted(
            element,
            &text,
            itself.into_iter().chain(passed_over).collect(),
        );
    }
    let refs = walk.reached.iter().map(|reached| {
        edge("ref", reached)
            .attr("hops", reached.depth)
            .optional(
                "at",
                (reached.depth == 1).then(|| loc(root.file, reached.line)),
            )
            .close()
    });
    element.children(itself.into_iter().chain(refs).chain(passed_over))
}

/// Starts a `<match>`: the family graph, and the symbol a name selected in it with its place.
fn match_of(family: &str, symbol: &Definition<'_>) -> Element {
    Element::new("match")
        .attr("lang", family)
        .attr("of", symbol.name)
        .optional("qn", qn(symbol))
        .attr("loc", loc(symbol.file, symbol.line))
}

/// The note of an answer the confidence floor emptied: `empty` at the floor, then the `dropped` records that bind below
/// it, as `records` spells one and several, and `retry`, the call that lists or follows them.
fn below_the_floor(empty: &str, dropped: usize, records: (&str, &str), retry: &str) -> String {
    let (bind, them) = if dropped == 1 {
        (records.0, "it")
    } else {
        (records.1, "them")
    };
    format!("{empty} at this confidence floor - {dropped} {bind} below it, and {retry} {them}")
}

/// The `<self_call>` record of a symbol's call of itself, at the line of the call in the symbol's own file.
fn self_call(root: &Definition<'_>, call: SelfCall) -> String {
    Element::new("self_call")
        .attr("at", loc(root.file, call.line))
        .optional("confidence", confidence(call.confidence))
        .close()
}

/// Why a symbol may have no callers: the `below_confidence` callers the confidence floor dropped, else the `ambiguous`
/// calls of its name that may call it, else what it is; a symbol that calls `itself` has no caller but itself.
fn no_callers(of: SymbolKind, ambiguous: usize, below_confidence: usize, itself: bool) -> String {
    let none = if itself {
        "no callers but itself"
    } else {
        "no callers"
    };
    if below_confidence > 0 {
        return below_the_floor(
            none,
            below_confidence,
            ("caller binds", "callers bind"),
            "min_confidence \"any\" lists",
        );
    }
    let label = of.label();
    let kind = kind(of);
    if ambiguous > 0 {
        let otherwise = if matches!(label, "func" | "method" | "macro") {
            "if none of them calls it, it is an entry point, dead code, or reached by dynamic dispatch"
        } else {
            "find_references lists its recorded uses"
        };
        let binds = if itself {
            "no call but its own binds it"
        } else {
            "no call binds it"
        };
        return format!(
            "{binds} - ambiguous counts the calls of its name that bound none because several definitions of the name \
             may be meant, and any of them may call this {kind}; grep finds them, and {otherwise}"
        );
    }
    match label {
        "func" | "method" | "macro" => format!("{none} - entry point, dead code, or dynamic dispatch"),
        "field" => "no callers - a field is read and written, not called; find_references lists its reads and \
                    writes"
            .to_owned(),
        "variant" => format!(
            "no callers - the graph records no call or other use of this {kind}; grep searches the text for its name"
        ),
        _ => format!(
            "no callers - the graph holds no call that names this {kind}; find_references lists its recorded uses"
        ),
    }
}
