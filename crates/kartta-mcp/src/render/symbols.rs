use std::fmt::Write;
use std::io;

use kartta::{
    Definition, Dependents, Hit, Implementations, Nearness, OutsideImplementations, Selector,
    Source, SymbolKind,
};

use super::{
    Asked, CallersAsk, Graphless, LIMIT_CUT, Unmatched, candidate, candidates_cut,
    empty_languages_note, fixed_cut, flat, kind, loc, named, noted, one_edit, push_cut_end, role,
    span, unresolved,
};
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
    /// The definitions named one edit from the query, when some word of it no searched graph holds and the query
    /// names nothing.
    pub(crate) one_edit: Option<kartta::Unmatched<'g>>,
    /// The languages a filtered build holds, as [`crate::state::Indexed::languages`] echoes them, which bound what
    /// the search could find.
    pub(crate) built: Option<&'g str>,
}

/// The `find_symbols` answer: the definitions named one edit from the query as `<suggestion/>` records, ranked
/// `<hit/>` records, and, when there is no hit, the searched languages whose files yielded no symbol at all and the
/// languages a filtered build holds.
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
        .count("dropped_words", found.dropped_words)
        .count(
            "one_edit",
            found.one_edit.as_ref().map_or(0, |near| near.total),
        );
    let suggested = found
        .one_edit
        .iter()
        .flat_map(|near| one_edit(near, languages));
    if found.hits.is_empty() {
        let element = element.optional("index_languages", found.built);
        let text = no_hits(languages, coverage);
        if found.one_edit.is_none() {
            return element.inline(&text);
        }
        return element.children(std::iter::once(text).chain(suggested));
    }
    element.children(suggested.chain(found.hits.iter().map(|(family, hit)| {
        let definition = &hit.definition;
        named("hit", definition)
            .optional("owner", definition.owner)
            .flag("fuzzy", hit.fuzzy)
            .optional("role", role(definition.flags))
            .attr("kind", kind(definition.kind))
            .attr("lang", family)
            .attr("loc", span(definition))
            .optional("sig", definition.signature.map(flat))
            .close()
    })))
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

/// One requested name of a `read_definitions` call: the reading it was answered for when it selects nothing as
/// written, how many definitions it selects, the nearest of them with their text, and the close alternatives to a
/// name that selects none.
#[derive(Debug)]
pub(crate) struct NameDefinitions<'g> {
    pub(crate) name: &'g str,
    pub(crate) read_as: Option<String>,
    pub(crate) total: usize,
    pub(crate) definitions: Vec<Read<'g>>,
    pub(crate) unmatched: Unmatched<'g>,
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
pub(crate) fn definitions(
    languages: Option<&str>,
    built: Option<&str>,
    answers: &[NameDefinitions<'_>],
) -> String {
    let element = Element::new("definitions")
        .optional("languages", languages)
        .attr("count", answers.len());
    element.children(answers.iter().map(|answer| {
        let name = Element::new("name")
            .attr("of", answer.name)
            .optional("read_as", answer.read_as.as_deref())
            .attr("total", answer.total);
        if answer.total == 0 {
            let asked = Asked {
                end: "",
                selector: Selector::new(answer.name, None),
                built,
            };
            return unresolved(name, languages, asked, &answer.unmatched);
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
        .attr("loc", span(definition))
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
        .flag("resized", source.resized)
        .count("cut_lines", source.cut_lines.len());
    if source.text.is_empty() {
        return element
            .optional("sig", definition.signature.map(flat))
            .close();
    }
    let mut body = String::new();
    let mut cuts = source.cut_lines.iter().peekable();
    for (line, number) in source.text.lines().zip(source.first_line..) {
        if !body.is_empty() {
            body.push('\n');
        }
        let _ = write!(body, "{number:>6}\t{line}");
        if let Some(cut) = cuts.next_if(|cut| cut.line == number) {
            push_cut_end(&mut body, cut.bytes);
        }
    }
    if !source.resized && answered_through < definition.end_line {
        let way = format!(
            "a read of lines {}-{} of its file",
            answered_through + 1,
            definition.end_line
        );
        body.push('\n');
        body.push_str(&fixed_cut("this body", Some(&way)));
    }
    element.wrap(&body)
}

/// Where a definition sits from the file a lookup asks from: its directory, vendored code, or elsewhere.
fn scope(nearness: Nearness, definition: &Definition<'_>) -> &'static str {
    match nearness.label() {
        "same_directory" => "same-directory",
        "elsewhere" if definition.flags.is_vendored() => "vendored",
        "elsewhere" => "other",
        label => label,
    }
}

/// The `find_implementations` answer: a `<type/>` per implementor, nearest first, `depth` marking one that implements
/// the type through another. Where the name selects several types, a `<root/>` names the one answered for, and
/// `<candidate/>` records the most implemented of the others.
pub(crate) fn implementations(
    ask: &CallersAsk<'_>,
    found: &[(&str, Implementations<'_>)],
    unmatched: &Unmatched<'_>,
) -> String {
    let languages = ask.languages;
    let count: usize = found.iter().map(|(_, found)| found.found.len()).sum();
    let total: usize = found.iter().map(|(_, found)| found.total).sum();
    let passed_over: usize = found.iter().map(|(_, found)| found.passed_over).sum();
    let element = Element::new("implementations")
        .attr("of", ask.selector.name)
        .optional("read_as", ask.read_as)
        .optional("languages", languages)
        .attr("count", count)
        .optional("total", (total > count).then_some(total))
        .count("passed_over", passed_over);
    if found.is_empty() {
        return unresolved(element, languages, ask.asked(), unmatched);
    }
    let selection: Vec<String> = found
        .iter()
        .filter(|(_, found)| found.passed_over > 0)
        .flat_map(|(family, found)| {
            let root = named("root", &found.root)
                .attr("lang", *family)
                .attr("loc", loc(found.root.file, found.root.line))
                .close();
            let others = found.candidates.iter().map(move |listed| {
                candidate(&listed.symbol)
                    .attr("lang", *family)
                    .attr("implementors", listed.count)
                    .close()
            });
            let cut = candidates_cut(
                found.passed_over > found.candidates.len(),
                found.passed_over + 1,
                "this name",
                Some(*family),
            );
            std::iter::once(root).chain(others).chain(cut)
        })
        .collect();
    if count == 0 {
        return noted(
            element,
            "the symbol is indexed but nothing implements or extends it",
            selection,
        );
    }
    let types = found.iter().flat_map(|(family, found)| {
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
    });
    let cut = (total > count).then(|| fixed_cut("this list", None));
    element.children(selection.into_iter().chain(types).chain(cut))
}

/// The `find_implementations` answer for a parent no searched graph holds a type of, `outside_graph` on its envelope:
/// a `<type/>` per type that names the parent, in path order, after a note that their own implementors were not
/// walked.
pub(crate) fn implementations_outside(
    ask: &CallersAsk<'_>,
    found: &[(&str, OutsideImplementations<'_>)],
) -> String {
    let count: usize = found.iter().map(|(_, found)| found.found.len()).sum();
    let total: usize = found.iter().map(|(_, found)| found.total).sum();
    let namesakes: usize = found.iter().map(|(_, found)| found.namesakes).sum();
    let element = Element::new("implementations")
        .attr("of", ask.selector.name)
        .optional("languages", ask.languages)
        .attr("count", count)
        .optional("total", (total > count).then_some(total))
        .flag("outside_graph", true)
        .count("namesakes", namesakes);
    let mut note = "no indexed type answers to this name - these types name it as a parent, and find_implementations \
                    of each lists its own implementors"
        .to_owned();
    if namesakes > 0 {
        note.push_str(
            "; namesakes= counts the other indexed types and namespaces of its last segment, which the lookup \
             reached from none of these parents, though one may still mean one through what the lookup does not \
             follow, as a C++ namespace alias",
        );
    }
    let types = found.iter().flat_map(|(family, found)| {
        found.found.iter().map(move |symbol| {
            named("type", symbol)
                .attr("lang", *family)
                .attr("loc", loc(symbol.file, symbol.line))
                .close()
        })
    });
    let cut = (total > count).then(|| fixed_cut("this list", None));
    element.children(std::iter::once(note).chain(types).chain(cut))
}

/// What a `find_importers` call asked: the file, and how many of its importers the answer lists.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImportersAsk<'a> {
    pub(crate) file: &'a str,
    pub(crate) listed: usize,
}

/// The `find_importers` answer: a `<file/>` per file that includes the file, nearest first, the first `listed` of
/// them, `depth` marking one that includes it through another. `graphless` says why a file no graph answers for has
/// none, and `counted` names the family of the graph that holds it with the
/// [includers](kartta_lang::Conventions::includers) its language counts.
pub(crate) fn importers(
    &ImportersAsk { file, listed }: &ImportersAsk<'_>,
    graphless: Graphless<'_>,
    dependents: Option<&Dependents<'_>>,
    counted: Option<(&str, &str)>,
) -> String {
    let element = Element::new("importers").attr("of", file);
    let Some(dependents) = dependents else {
        return element.attr("count", 0).inline(&graphless.note());
    };
    let cut = dependents.found.len() > listed;
    let element = element
        .attr("count", dependents.found.len())
        .optional("shown", cut.then_some(listed))
        .flag("truncated", dependents.truncated)
        .count("unexpanded", dependents.unexpanded);
    let counts =
        counted.map(|(family, includers)| format!("the {family} graph counts {includers}"));
    if dependents.found.is_empty() {
        return match counts {
            Some(counts) => element.inline(&counts),
            None => element.close(),
        };
    }
    let note = counts.map(|counts| Element::new("note").inline(&counts));
    let records = dependents.found.iter().take(listed).map(|dependent| {
        Element::new("file")
            .attr("path", dependent.file)
            .optional("depth", (dependent.depth > 1).then_some(dependent.depth))
            .close()
    });
    element.children(
        note.into_iter()
            .chain(records)
            .chain(cut.then(|| LIMIT_CUT.to_owned())),
    )
}

/// The `file_outline` answer: a `<symbol/>` per definition of the file in source order, the first `listed` of them,
/// `in` naming the definition that holds one without an owner. `graphless` says why a file no graph answers for has
/// none.
pub(crate) fn outline(
    file: &str,
    graphless: Graphless<'_>,
    outline: Option<&[Definition<'_>]>,
    listed: usize,
) -> String {
    let element = Element::new("outline").attr("of", file);
    match outline {
        None => element.attr("symbols", 0).inline(&graphless.note()),
        Some([]) => element.attr("symbols", 0).inline(
            "no definitions recorded - the graph holds this file but lists none: its language declined it, its parse \
             failed, or it declares none; index_stats reports whether the language has files without symbols",
        ),
        Some(definitions) => {
            let mut open = Vec::new();
            let cut = definitions.len() > listed;
            let records = definitions.iter().take(listed).map(|definition| {
                named("symbol", definition)
                    .optional("owner", definition.owner)
                    .optional("in", holder(&mut open, definition))
                    .attr("kind", kind(definition.kind))
                    .attr("lines", format!("{}-{}", definition.line, definition.end_line))
                    .optional("sig", definition.signature.map(flat))
                    .close()
            });
            element
                .attr("symbols", definitions.len())
                .optional("shown", cut.then_some(listed))
                .children(records.chain(cut.then(|| LIMIT_CUT.to_owned())))
        }
    }
}

/// The qualified name of the innermost definition of `open` that holds `definition`, when it has no owner; `open` then
/// drops the innermost definitions that end before its line, and opens `definition`. `open` holds earlier definitions
/// of one outline.
fn holder<'o, 'g>(
    open: &mut Vec<&'o Definition<'g>>,
    definition: &'o Definition<'g>,
) -> Option<&'g str> {
    while open
        .last()
        .is_some_and(|outer| outer.end_line < definition.line)
    {
        open.pop();
    }
    let holder = if definition.owner.is_none() {
        (open.iter().rev())
            .find(|outer| holds(outer, definition))
            .map(|outer| outer.qualified_name)
    } else {
        None
    };
    open.push(definition);
    holder
}

/// Whether `outer`, a definition that starts before or on the line `inner` starts on, holds `inner`: one starting
/// strictly inside its lines, or a field or a method starting on any of them when `outer` is a callable.
const fn holds(outer: &Definition<'_>, inner: &Definition<'_>) -> bool {
    let line = inner.line;
    (outer.line < line && line < outer.end_line)
        || (line <= outer.end_line
            && matches!(
                outer.kind,
                SymbolKind::Function | SymbolKind::Method | SymbolKind::Macro
            )
            && matches!(inner.kind, SymbolKind::Field | SymbolKind::Method))
}
