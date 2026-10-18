use std::fmt::Write;
use std::io;

use kartta::{Definition, Dependents, Hit, Implementations, Nearness, Source};

use super::{Suggestion, empty_languages_note, flat, kind, loc, named, role, span, unresolved};
use crate::llmxml::Element;
use crate::state::GraphRow;

/// What a `find_symbols` search found across the graphs it searched.
#[derive(Debug)]
pub(crate) struct Found<'g> {
    /// The best hits, best first, at most the limit.
    pub(crate) hits: Vec<(&'g str, Hit<'g>)>,
    /// The definitions that matched before the limit, the fill-up's included.
    pub(crate) matched: usize,
    /// The query's words no searched graph holds.
    pub(crate) unmatched: Vec<String>,
    /// The query's distinct words past the eight the search looks for.
    pub(crate) dropped_words: usize,
}

/// The `find_symbols` answer: ranked `<hit/>` records, and, when there is none, the searched languages whose files
/// yielded no symbol at all.
pub(crate) fn hits(
    query: &str,
    languages: Option<&str>,
    found: &Found<'_>,
    coverage: &[&GraphRow],
) -> String {
    let truncated = found.matched > found.hits.len();
    let element = Element::new("symbols")
        .attr("of", query)
        .optional("languages", languages)
        .attr("count", found.hits.len())
        .optional("total", truncated.then_some(found.matched))
        .flag("truncated", truncated)
        .optional(
            "unmatched",
            (!found.unmatched.is_empty()).then(|| found.unmatched.join(", ")),
        )
        .count("dropped_words", found.dropped_words);
    if found.hits.is_empty() {
        return element.inline(&no_hits(languages, coverage));
    }
    element.children(found.hits.iter().map(|(family, hit)| {
        let definition = &hit.definition;
        named("hit", definition)
            .optional("owner", definition.owner)
            .flag("fuzzy", hit.fuzzy)
            .optional("role", role(definition.flags))
            .attr("kind", kind(definition.kind))
            .attr("lang", family)
            .attr(
                "loc",
                span(definition.file, definition.line, definition.end_line),
            )
            .optional("sig", definition.signature.map(flat))
            .close()
    }))
}

fn no_hits(languages: Option<&str>, coverage: &[&GraphRow]) -> String {
    if coverage.is_empty() {
        return empty_languages_note(languages)
            .unwrap_or("no match - if the language is absent from the index, index_stats lists what is indexed")
            .to_owned();
    }
    let named: Vec<String> = coverage
        .iter()
        .map(|row| {
            let noun = if row.files_without_symbols == 1 {
                "file"
            } else {
                "files"
            };
            format!("{} {} {noun}", row.files_without_symbols, row.family)
        })
        .collect();
    format!(
        "no match - note that {} in the searched set contributed no symbols at all (unsupported dialect or failed \
         parse), so a missing answer for those languages is coverage rather than absence; index_stats has the \
         per-language counts",
        named.join(", ")
    )
}

/// One requested name of a `read_definitions` call: how many definitions it selects, and the nearest of them with
/// their text.
#[derive(Debug)]
pub(crate) struct NameDefinitions<'g> {
    pub(crate) name: &'g str,
    pub(crate) total: usize,
    pub(crate) definitions: Vec<Read<'g>>,
}

/// One definition `read_definitions` answers, with its text as its file holds it now.
#[derive(Debug)]
pub(crate) struct Read<'g> {
    pub(crate) family: &'g str,
    pub(crate) definition: Definition<'g>,
    pub(crate) nearness: Nearness,
    pub(crate) source: io::Result<Source>,
}

/// The `read_definitions` answer: one `<name>` per requested name, in request order, each holding its definitions
/// nearest first with their text numbered from its first line.
pub(crate) fn definitions(languages: Option<&str>, answers: &[NameDefinitions<'_>]) -> String {
    let element = Element::new("definitions")
        .optional("languages", languages)
        .attr("count", answers.len());
    element.children(answers.iter().map(|answer| {
        let name = Element::new("name")
            .attr("of", answer.name)
            .attr("total", answer.total);
        if answer.total == 0 {
            return name.inline(
                empty_languages_note(languages)
                    .unwrap_or("no definition matches - find_symbols can locate the name"),
            );
        }
        name.optional(
            "shown",
            (answer.total > answer.definitions.len()).then_some(answer.definitions.len()),
        )
        .children(answer.definitions.iter().map(definition))
    }))
}

fn definition(read: &Read<'_>) -> String {
    let definition = &read.definition;
    let element = named("definition", definition)
        .optional("owner", definition.owner)
        .attr("kind", kind(definition.kind))
        .attr("lang", read.family)
        .attr(
            "loc",
            span(definition.file, definition.line, definition.end_line),
        )
        .attr("scope", scope(read.nearness, definition))
        .optional("role", role(definition.flags));
    let source = match &read.source {
        Ok(source) => source,
        Err(error) => {
            return element
                .attr("error", error)
                .optional("sig", definition.signature.map(flat))
                .close();
        }
    };
    let answered_through = source.first_line + source.lines.saturating_sub(1);
    let element = element
        .flag(
            "truncated",
            source.lines == 0 || answered_through < definition.end_line,
        )
        .flag("resized", source.resized);
    if source.text.is_empty() {
        return element
            .optional("sig", definition.signature.map(flat))
            .close();
    }
    let mut body = String::new();
    for (line, number) in source.text.lines().zip(source.first_line..) {
        if !body.is_empty() {
            body.push('\n');
        }
        let _ = write!(body, "{number:>6}\t{line}");
    }
    element.wrap(&body)
}

/// Where a definition sits from the file a lookup asks from: its directory, vendored code, or elsewhere.
const fn scope(nearness: Nearness, definition: &Definition<'_>) -> &'static str {
    match nearness {
        Nearness::SameDirectory => "same-directory",
        _ if definition.flags.is_vendored() => "vendored",
        _ => "other",
    }
}

/// The `find_implementations` answer: a `<type/>` per implementor, nearest first, `depth` marking one that implements
/// the type through another.
pub(crate) fn implementations(
    name: &str,
    languages: Option<&str>,
    found: &[(&str, Implementations<'_>)],
    suggestions: &[Suggestion<'_>],
) -> String {
    let count: usize = found.iter().map(|(_, found)| found.found.len()).sum();
    let total: usize = found.iter().map(|(_, found)| found.total).sum();
    let passed_over: usize = found.iter().map(|(_, found)| found.passed_over).sum();
    let element = Element::new("implementations")
        .attr("of", name)
        .optional("languages", languages)
        .attr("count", count)
        .optional("total", (total > count).then_some(total))
        .count("passed_over", passed_over);
    if found.is_empty() {
        return unresolved(element, languages, suggestions);
    }
    if count == 0 {
        return element.inline("the symbol is indexed but nothing implements or extends it");
    }
    element.children(found.iter().flat_map(|(family, found)| {
        found.found.iter().map(move |implementor| {
            let symbol = &implementor.symbol;
            named("type", symbol)
                .attr("lang", *family)
                .attr("loc", loc(symbol.file, symbol.line))
                .optional(
                    "depth",
                    (implementor.depth > 1).then_some(implementor.depth),
                )
                .close()
        })
    }))
}

/// The `find_importers` answer: a `<file/>` per file that includes the file, nearest first, `depth` marking one that
/// includes it through another.
pub(crate) fn importers(file: &str, dependents: Option<&Dependents<'_>>) -> String {
    let element = Element::new("importers").attr("of", file);
    let Some(dependents) = dependents else {
        return element.attr("count", 0).inline(
            "not in a code graph (an unrecognized extension, a language not indexed, or a path the index does not \
             hold) - pass a repo-relative path as printed by find_files",
        );
    };
    element
        .attr("count", dependents.found.len())
        .flag("truncated", dependents.truncated)
        .count("unexpanded", dependents.unexpanded)
        .children(dependents.found.iter().map(|dependent| {
            Element::new("file")
                .attr("path", dependent.file)
                .optional("depth", (dependent.depth > 1).then_some(dependent.depth))
                .close()
        }))
}

/// The `file_outline` answer: a `<symbol/>` per definition of the file in source order.
pub(crate) fn outline(file: &str, outline: Option<&[Definition<'_>]>) -> String {
    let element = Element::new("outline").attr("of", file);
    match outline {
        None => element.attr("symbols", 0).inline(
            "not in a code graph - an unrecognized extension, a language not indexed, or a path the index does not \
             hold: pass a repo-relative path as printed by find_files, or read or grep the file",
        ),
        Some([]) => element.attr("symbols", 0).inline(
            "no definitions recorded - the graph holds this file but lists none: its language declined it, its parse \
             failed, or it declares none; index_stats reports whether the language has files without symbols",
        ),
        Some(definitions) => element
            .attr("symbols", definitions.len())
            .children(definitions.iter().map(|definition| {
                named("symbol", definition)
                    .optional("owner", definition.owner)
                    .attr("kind", kind(definition.kind))
                    .attr("lines", format!("{}-{}", definition.line, definition.end_line))
                    .optional("sig", definition.signature.map(flat))
                    .close()
            })),
    }
}
