//! The answers agents read: an envelope per tool, built with [`Element`], and the plain path lists of the path tools.

mod calls;
mod files;

use std::time::Duration;

use kartta::{Definition, Hit, SymbolFlags, SymbolKind};

pub(crate) use self::calls::{CallersAsk, callers};
pub(crate) use self::files::paths;
use crate::llmxml::Element;
use crate::state::Indexed;

/// How an answer echoes an empty `languages` filter.
pub(crate) const EMPTY_LANGUAGES: &str = "[]";

/// How many close alternatives an unknown name answers with.
pub(crate) const SUGGESTIONS: usize = 5;

/// How many of the build's errors the index answer lists before it counts the rest.
const LISTED_ERRORS: usize = 5;

fn loc(file: &str, line: u32) -> String {
    format!("{file}:{line}")
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
    let element = Element::new(tag).attr("name", definition.name);
    if definition.qualified_name == definition.name {
        element
    } else {
        element.attr("qn", definition.qualified_name)
    }
}

/// A close alternative to a name nothing selects, found by a search of one family's graph.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Suggestion<'g> {
    pub(crate) family: &'g str,
    pub(crate) hit: Hit<'g>,
}

fn suggestion(suggestion: &Suggestion<'_>) -> String {
    let definition = &suggestion.hit.definition;
    let mut element = named("suggestion", definition);
    if let Some(role) = role(definition.flags) {
        element = element.attr("role", role);
    }
    element
        .attr("kind", kind(definition.kind))
        .attr("lang", suggestion.family)
        .attr("loc", loc(definition.file, definition.line))
        .close()
}

fn empty_languages_note(languages: &str) -> Option<&'static str> {
    (languages == EMPTY_LANGUAGES).then_some(
        "no language selected - an empty languages list searches no graph; omit languages to search every indexed \
         language",
    )
}

/// The body of an answer whose name selected nothing: the empty filter's note, or the alternatives a search found.
fn unresolved(element: Element, languages: &str, suggestions: &[Suggestion<'_>]) -> String {
    if let Some(note) = empty_languages_note(languages) {
        return element.inline(note);
    }
    if suggestions.is_empty() {
        return element.inline("no symbol matches - find_symbols can pin the exact name");
    }
    let mut lines = vec!["no symbol matches - did you mean:".to_owned()];
    lines.extend(suggestions.iter().map(suggestion));
    element.wrap(&lines.join("\n"))
}

/// The `index_repo` and `index_stats` answer: the index's shape, one `<language/>` per graph that holds a file, and the
/// failures the build collected.
pub(crate) fn index(indexed: &Indexed) -> String {
    let element = Element::new("index")
        .attr("root", &indexed.root)
        .attr("files", indexed.files)
        .attr(
            "gitignore",
            if indexed.gitignore {
                "respected"
            } else {
                "ignored"
            },
        )
        .attr(
            "symbols",
            indexed.graphs.iter().map(|row| row.symbols).sum::<usize>(),
        )
        .attr(
            "calls",
            indexed.graphs.iter().map(|row| row.calls).sum::<usize>(),
        );
    if indexed.graphs.is_empty() && indexed.errors.is_empty() {
        return element.inline("the code graph is empty");
    }

    let languages = indexed.graphs.iter().map(|row| {
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
    let errors = (!indexed.errors.is_empty()).then(|| {
        let mut listed: Vec<String> = indexed
            .errors
            .iter()
            .take(LISTED_ERRORS)
            .map(|error| crate::llmxml::line(error).into_owned())
            .collect();
        if indexed.errors.len() > LISTED_ERRORS {
            listed.push(format!("(+{} more)", indexed.errors.len() - LISTED_ERRORS));
        }
        Element::new("errors")
            .attr("count", indexed.errors.len())
            .wrap(&listed.join("\n"))
    });
    element.children(languages.chain(errors))
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
