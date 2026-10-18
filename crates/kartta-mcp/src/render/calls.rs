use kartta::{ReachBand, Reached, SymbolKind, Walk};

use super::{Suggestion, confidence, edge_role, kind, loc, named, unresolved};
use crate::llmxml::Element;

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
