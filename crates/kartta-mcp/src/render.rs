//! The answers agents read: an envelope per tool, built with [`Element`], and the plain path lists of the path tools.

mod calls;
mod files;
mod symbols;
mod unused;

use std::fmt::Display;
use std::time::Duration;

use kartta::{Definition, Hit, Resolution, SymbolFlags, SymbolKind};

pub(crate) use self::calls::{
    CallersAsk, PathAsk, call_path, callees, callers, change, impact, references,
};
pub(crate) use self::files::{dir, globbed, grep, paths};
pub(crate) use self::symbols::{
    Found, NameDefinitions, Read, definitions, hits, implementations, importers, outline,
};
pub(crate) use self::unused::{UnusedAsk, unused};
use crate::llmxml::Element;
use crate::state::{Indexed, Refreshed, Watch};

/// How an answer echoes an empty `languages` filter.
pub(crate) const EMPTY_LANGUAGES: &str = "[]";

/// How many close alternatives an unknown name answers with.
pub(crate) const SUGGESTIONS: usize = 5;

/// How many of the build's errors the index answer lists before it counts the rest.
const LISTED_ERRORS: usize = 5;

/// A signature on one line, so a multi-line declaration does not leak `\n` escapes into its attribute.
fn flat(signature: &str) -> String {
    signature.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn loc(file: &str, line: u32) -> String {
    format!("{file}:{line}")
}

/// A definition's span, `file:start-end`, or `file:line` for one on a single line.
fn span(definition: &Definition<'_>) -> String {
    let (file, line) = (definition.file, definition.line);
    if definition.end_line > line {
        format!("{file}:{line}-{}", definition.end_line)
    } else {
        loc(file, line)
    }
}

fn file_element(path: &str) -> String {
    Element::new("file").attr("path", path).close()
}

/// The kind as answers spell it, its short label written out: `func` is `function`, `iface` is `interface`.
fn kind(kind: SymbolKind) -> &'static str {
    match kind.label() {
        "func" => "function",
        "ns" => "namespace",
        "iface" => "interface",
        "var" => "variable",
        "const" => "constant",
        "alias" => "type-alias",
        "variant" => "enum-variant",
        "type" => "defined-type",
        label => label,
    }
}

/// The mark of a weakly resolved call: none at 80 or above, where a call binds by structure, `likely` from 50, where
/// it binds by the module layout, and `guess` below, where it binds by its name alone.
const fn confidence(confidence: u8) -> Option<&'static str> {
    match confidence {
        80.. => None,
        50..=79 => Some("likely"),
        _ => Some("guess"),
    }
}

/// Why a symbol is a reach artifact rather than code under review: test code before an entry point before generated
/// code, or `None` for ordinary code.
const fn role(flags: SymbolFlags) -> Option<&'static str> {
    if flags.is_test() {
        Some("test")
    } else if flags.is_entry() {
        Some("entry")
    } else if flags.is_generated() {
        Some("generated")
    } else {
        None
    }
}

/// The `role=` a caller, a callee or a site carries, `consumer` for ordinary code.
const fn edge_role(flags: SymbolFlags) -> &'static str {
    match role(flags) {
        Some(role) => role,
        None => "consumer",
    }
}

/// Starts an element naming a definition: `name`, and `qn` when the qualified name says more.
fn named(tag: &'static str, definition: &Definition<'_>) -> Element {
    Element::new(tag)
        .attr("name", definition.name)
        .optional("qn", qn(definition))
}

/// A definition's qualified name, when it says more than its name.
fn qn<'g>(definition: &Definition<'g>) -> Option<&'g str> {
    (definition.qualified_name != definition.name).then_some(definition.qualified_name)
}

/// A close alternative to a name nothing selects, found by a search of one family's graph.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Suggestion<'g> {
    pub(crate) family: &'g str,
    pub(crate) hit: Hit<'g>,
}

fn suggestion(suggestion: &Suggestion<'_>) -> String {
    let definition = &suggestion.hit.definition;
    named("suggestion", definition)
        .optional("role", role(definition.flags))
        .attr("kind", kind(definition.kind))
        .attr("lang", suggestion.family)
        .attr("loc", loc(definition.file, definition.line))
        .close()
}

fn empty_languages_note(languages: Option<&str>) -> Option<&'static str> {
    (languages == Some(EMPTY_LANGUAGES)).then_some(
        "no language selected - an empty languages list searches no graph; omit languages to search every indexed \
         language",
    )
}

/// What a name that selected no symbol does match, in the graphs a query searched.
#[derive(Debug, Clone)]
pub(crate) enum Unmatched<'g> {
    /// Several definitions the name ends with, none of which it picks: `total` of them, the first listed.
    Several {
        total: usize,
        listed: Vec<(&'g str, Definition<'g>)>,
    },
    /// Definitions of the name outside the path prefix asked: `total` of them, the first listed.
    Elsewhere {
        total: usize,
        listed: Vec<(&'g str, Definition<'g>)>,
    },
    /// The close alternatives a search found.
    Suggested(Vec<Suggestion<'g>>),
}

/// Starts a `<candidate>`: a symbol the name also selects, and where it is.
fn candidate(definition: &Definition<'_>) -> Element {
    named("candidate", definition)
        .attr("kind", kind(definition.kind))
        .attr("loc", loc(definition.file, definition.line))
}

/// An element holding `text`, then the `<candidate>` records, on one line when there are none.
fn noted(element: Element, text: &str, candidates: Vec<String>) -> String {
    if candidates.is_empty() {
        return element.inline(text);
    }
    element.children(std::iter::once(text.to_owned()).chain(candidates))
}

/// The body of an answer whose name selected nothing: the empty filter's note, what the name matches instead, or the
/// alternatives a search found. `prefix` names the parameter that took the path prefix.
fn unresolved(
    element: Element,
    languages: Option<&str>,
    prefix: &str,
    unmatched: &Unmatched<'_>,
) -> String {
    if let Some(note) = empty_languages_note(languages) {
        return element.inline(note);
    }
    let (text, listed) = match unmatched {
        Unmatched::Several { total, listed } => (
            format!(
                "{total} definitions end with this name and it picks none of them - a longer name, or {prefix} with \
                 one's file, picks one:"
            ),
            listed,
        ),
        Unmatched::Elsewhere { total, listed } => (
            format!("no definition of this name is under {prefix} - {total} are elsewhere:"),
            listed,
        ),
        Unmatched::Suggested(suggestions) if suggestions.is_empty() => {
            return element.inline("no symbol matches - find_symbols can pin the exact name");
        }
        Unmatched::Suggested(suggestions) => {
            let mut lines = vec!["no symbol matches - did you mean:".to_owned()];
            lines.extend(suggestions.iter().map(suggestion));
            return element.wrap(&lines.join("\n"));
        }
    };
    let candidates = listed
        .iter()
        .map(|(family, definition)| candidate(definition).attr("lang", *family).close())
        .collect();
    noted(element, &text, candidates)
}

/// The `index_repo` and `index_stats` answer: the index's shape, one `<language/>` per graph that holds a file, how
/// the calls of each of those graphs bind when `coverage` holds its resolution, and the failures the build collected.
pub(crate) fn index(indexed: &Indexed, coverage: &[(&str, Resolution<'_>)]) -> String {
    let graphs = indexed.graphs();
    let element = Element::new("index")
        .attr("root", &indexed.root)
        .attr("files", indexed.files())
        .attr(
            "gitignore",
            if indexed.gitignore {
                "respected"
            } else {
                "ignored"
            },
        )
        .attr(
            "watch",
            match &indexed.watch {
                Watch::Following => "following".to_owned(),
                Watch::Off(reason) => format!("off: {reason}"),
            },
        )
        .attr(
            "symbols",
            graphs.iter().map(|row| row.symbols).sum::<usize>(),
        )
        .attr("calls", graphs.iter().map(|row| row.calls).sum::<usize>());
    if graphs.is_empty() && indexed.errors.is_empty() {
        return element.inline("the code graph is empty");
    }

    let languages = graphs.iter().map(|row| {
        Element::new("language")
            .attr("name", &row.family)
            .attr("files", row.files)
            .attr("symbols", row.symbols)
            .attr("calls", row.calls)
            .attr("uses", row.uses)
            .attr("includes", row.includes)
            .count("files_without_symbols", row.files_without_symbols)
            .count("unheld", row.unheld)
            .close()
    });
    let errors = failures(
        indexed.errors.len(),
        indexed.errors.iter().map(|failure| &failure.text),
    );
    element.children(
        languages
            .chain(
                coverage
                    .iter()
                    .filter(|(family, _)| graphs.iter().any(|row| row.family == *family))
                    .map(|(family, resolution)| bound(family, resolution)),
            )
            .chain(errors),
    )
}

/// The `<errors>` block of `count` failures: the first few `failed` yields, and a count of the rest, or `None` when
/// there are none.
fn failures(count: usize, failed: impl Iterator<Item = impl Display>) -> Option<String> {
    if count == 0 {
        return None;
    }
    let mut listed: Vec<String> = failed
        .take(LISTED_ERRORS)
        .map(|failure| crate::llmxml::line(&failure.to_string()).into_owned())
        .collect();
    if count > LISTED_ERRORS {
        listed.push(format!("(+{} more)", count - LISTED_ERRORS));
    }
    Some(
        Element::new("errors")
            .attr("count", count)
            .wrap(&listed.join("\n")),
    )
}

/// The `refresh` answer: what the refresh walked again, then each count of what changed that is not zero, how long it
/// took, and the failures it collected.
pub(crate) fn refresh(refreshed: &Refreshed) -> String {
    let refresh = &refreshed.refresh;
    let outcome = &refresh.index;
    let walked = &outcome.walked;
    let mut element = Element::new("refresh");
    element = if walked.iter().any(String::is_empty) {
        element.attr("walked", "tree")
    } else {
        element.attr("walked", walked.len())
    };
    element = element
        .count("added", outcome.added.len())
        .count("removed", outcome.removed.len())
        .count("extracted", refresh.extracted)
        .count("resolved_again", refresh.resolved_again)
        .count("dropped", refresh.dropped.unwrap_or(0))
        .flag("rebuilt", refresh.rebuilt)
        .optional(
            "reconfigured",
            (!refresh.reconfigured.is_empty()).then(|| refresh.reconfigured.join(", ")),
        )
        .optional("widened", refresh.widened.map(|widened| widened.named))
        .optional("limit", refresh.widened.map(|widened| widened.limit))
        .attr("elapsed_ms", refreshed.elapsed.as_millis());
    if let Some(errors) = failures(refresh.errors().count(), refresh.errors()) {
        return element.wrap(&errors);
    }
    let changed = outcome.added.len()
        + outcome.removed.len()
        + refresh.extracted
        + refresh.resolved_again
        + refresh.dropped.unwrap_or(0);
    if changed == 0 && !refresh.rebuilt && refresh.reconfigured.is_empty() {
        element.inline("nothing changed - the index already matched the tree")
    } else {
        element.close()
    }
}

/// How one graph's call sites bind: the sites bound, by the bands an answer marks a call with, the caller and callee
/// pairs they make, and why the others bound none.
fn bound(family: &str, resolution: &Resolution<'_>) -> String {
    let mut bands = [0_usize; 3];
    for &(confidence, sites) in &resolution.bound {
        let band = match self::confidence(confidence) {
            None => 0,
            Some("likely") => 1,
            Some(_) => 2,
        };
        bands[band] += sites;
    }
    let misses = &resolution.misses;
    Element::new("coverage")
        .attr("lang", family)
        .attr("sites", resolution.sites)
        .attr("bound", bands.iter().sum::<usize>())
        .count("certain", bands[0])
        .count("likely", bands[1])
        .count("guess", bands[2])
        .attr("edges", resolution.edges)
        .count("external_import", misses.external_import)
        .count("not_exported", misses.not_exported)
        .count("undefined", misses.undefined)
        .count("not_admitted", misses.not_admitted)
        .count("ambiguous", misses.ambiguous)
        .count("through_receiver", misses.through_a_receiver)
        .close()
}

/// The `index_stats` answer while a build runs.
pub(crate) fn building(root: &str, elapsed: Duration) -> String {
    Element::new("index")
        .attr("root", root)
        .attr("status", "building")
        .attr("elapsed_secs", elapsed.as_secs())
        .inline(
            "still building - every other tool answers once it is ready, and index_repo with this root waits for it",
        )
}
