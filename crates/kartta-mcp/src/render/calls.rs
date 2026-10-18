use kartta::{CallPath, DirectCalls, ReachBand, Reached, References, Segment, SymbolKind, Walk};

use super::{
    Suggestion, confidence, edge_role, empty_languages_note, file_element, flat, kind, loc, named,
    unresolved,
};
use crate::llmxml::Element;

/// The blind spot of a method's sites.
const METHOD_NOTE: &str = "a call through a value of the owner's type resolves only where the graph can name that \
                           type, so some of those calls can be missing";

/// The blind spot of a field's sites.
const FIELD_NOTE: &str =
    "the graph records no access to a field, so every use of it is missing from this answer";

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
    element = element
        .attr("loc", loc(symbol.file, symbol.line))
        .attr("kind", kind(symbol.kind))
        .attr("count", references.total);
    if references.total > references.sites.len() {
        element = element.attr("shown", references.sites.len());
    }
    element = element
        .count("below_confidence", references.below_confidence)
        .count("passed_over", references.passed_over);
    let member_note = match symbol.kind.label() {
        "method" => Some(METHOD_NOTE),
        "field" => Some(FIELD_NOTE),
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
    element.children(note.into_iter().chain(references.sites.iter().map(|site| {
        let enclosing = &site.site_of;
        let mut record = named("ref", enclosing)
            .attr("loc", loc(enclosing.file, enclosing.line))
            .attr("role", edge_role(enclosing.flags))
            .attr("site", if site.call { "call" } else { "use" })
            .attr("at", loc(enclosing.file, site.line));
        if let Some(band) = confidence(site.confidence) {
            record = record.attr("confidence", band);
        }
        record.close()
    })))
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

/// Why a symbol may have no callers, by what it is.
fn no_callers(of: SymbolKind, floored: bool) -> String {
    let label = of.label();
    let kind = kind(of);
    match label {
        "func" | "method" | "macro" => {
            "no callers - entry point, dead code, or dynamic dispatch".to_owned()
        }
        "field" | "variant" => format!(
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
