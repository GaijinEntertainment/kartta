//! The answers agents read: an envelope per tool, built with [`Element`], and the plain path lists of the path tools.

mod calls;
mod files;
mod symbols;
mod unused;

use std::fmt::{Display, Write};

use kartta::{
    Definition, Engine, EngineChange, Entry, EntryKind, Hit, Region, Resolution, Selector,
    SymbolFlags, SymbolKind, Symlink, UnmatchedKind,
};

pub(crate) use self::calls::{
    CallersAsk, EditAsk, PathAsk, call_path, callees, callers, change, impact, references,
};
pub(crate) use self::files::{Selection, dir, globbed, grep, paths};
pub(crate) use self::symbols::{
    Found, ImportersAsk, NameDefinitions, Read, definitions, hits, implementations,
    implementations_outside, importers, outline,
};
pub(crate) use self::unused::{UnusedAsk, unused};
use crate::languages::CATALOG;
use crate::llmxml::Element;
use crate::state::{Gitignore, Indexed, Refreshed, Unready};

/// How an answer echoes an empty `languages` filter.
pub(crate) const EMPTY_LANGUAGES: &str = "[]";

/// How many close alternatives an unknown name answers with.
pub(crate) const SUGGESTIONS: usize = 5;

/// The last line of a list a call's `limit` cut, after the records it lists.
const LIMIT_CUT: &str = "the limit cut this list - a higher limit lists the rest";

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

/// Appends the mark of a line cut before its end, with the bytes the cut left out.
fn push_cut_end(body: &mut String, bytes: usize) {
    let _ = write!(body, " [... omitted {bytes} bytes]");
}

/// Why no code graph answers for a file the index holds.
#[derive(Debug, Clone)]
pub(crate) enum Graphless<'e> {
    /// The index holds the path as a symlink, which no graph holds whatever it points at.
    Symlink(Symlink<'e>),
    /// The index holds the path as a socket, a fifo or a device node, which no graph holds whatever its name.
    Special,
    /// No language kartta ships claims the file or declines it.
    Unclaimed,
    /// `index_repo`'s `languages` filter left out of the build the language, one kartta ships, that claims the file.
    Unbuilt(&'static str),
    /// `language` leaves the file, one of its kind, out of its graph for `reason`.
    Declined { language: &'e str, reason: &'e str },
    /// The graph of this family claims the file and holds no record of it.
    Unheld(&'e str),
}

impl<'e> Graphless<'e> {
    /// Why no graph answers for `file`, which the graph of `family` claims, or no graph when `family` is `None`.
    pub(crate) fn of(engine: &'e Engine, file: &str, family: Option<&'e str>) -> Self {
        if let Some(symlink) = engine.symlink(file) {
            return Self::Symlink(symlink);
        }
        if engine
            .index()
            .entry(file)
            .is_some_and(|entry| entry.kind() == EntryKind::Other)
        {
            return Self::Special;
        }
        family.map_or_else(|| Self::unclaimed(engine, file), Self::Unheld)
    }

    /// Why no graph answers for `file`, a held file no indexed language claims: an indexed language declines it, a
    /// language the build left out claims or declines it, or no language kartta ships takes it.
    fn unclaimed(engine: &'e Engine, file: &str) -> Self {
        if let Some((language, reason)) = engine.declined(file) {
            return Self::Declined { language, reason };
        }
        if let Some(language) = CATALOG.claimant(file) {
            return Self::Unbuilt(language);
        }
        CATALOG
            .declined(file)
            .map_or(Self::Unclaimed, |(language, reason)| Self::Declined {
                language,
                reason,
            })
    }

    fn note(self) -> String {
        let why = match self {
            Self::Symlink(symlink) => return symlink_note(&symlink),
            Self::Special => {
                return "this path is a socket, a fifo or a device node, not a regular file, and no code graph \
                        holds one"
                    .to_owned();
            }
            Self::Unclaimed => {
                "no indexed language claims this file, so no code graph holds it".to_owned()
            }
            Self::Unbuilt(language) => {
                return format!(
                    "the {language} language claims this file, and index_repo's languages filter left it out of \
                     this index, so no code graph holds it - read or grep it, or index_repo with {language} in \
                     languages"
                );
            }
            Self::Declined { language, reason } => {
                format!("the {language} language leaves this file out of its graph: {reason}")
            }
            Self::Unheld(family) => format!(
                "the {family} graph claims this file and holds no record of it, one of the files index_stats counts \
                 in unheld"
            ),
        };
        format!("{why} - read or grep it")
    }
}

/// What the languages a build left out say of the regions an `impact_of_change` engine answer counts as unclaimed,
/// each list in the order the regions first name its entries.
#[derive(Debug, Default)]
pub(crate) struct Unbuilt {
    /// Per language that claims their files, the regions.
    pub(crate) claimed: Vec<(&'static str, usize)>,
    /// Per language that leaves their files out of its graph and the reason it states, the regions.
    pub(crate) declined: Vec<(&'static str, &'static str, usize)>,
}

impl Unbuilt {
    /// What the languages the build left out say of `change`'s unclaimed regions; nothing when it counts none.
    pub(crate) fn of(engine: &Engine, regions: &[Region<'_>], change: &EngineChange<'_>) -> Self {
        let mut unbuilt = Self::default();
        if change.unclaimed_regions == 0 {
            return unbuilt;
        }
        for region in regions {
            let file = region.path;
            let unclaimed = engine
                .index()
                .entry(file)
                .is_some_and(|entry| entry.kind() == EntryKind::File)
                && engine.graph_of(file).is_none()
                && engine.declined(file).is_none();
            if !unclaimed {
                continue;
            }
            if let Some(language) = CATALOG.claimant(file) {
                match unbuilt.claimed.iter_mut().find(|(met, _)| *met == language) {
                    Some((_, count)) => *count += 1,
                    None => unbuilt.claimed.push((language, 1)),
                }
            } else if let Some((language, reason)) = CATALOG.declined(file) {
                let met = unbuilt
                    .declined
                    .iter_mut()
                    .find(|(met, why, _)| *met == language && *why == reason);
                match met {
                    Some((_, _, count)) => *count += 1,
                    None => unbuilt.declined.push((language, reason, 1)),
                }
            }
        }
        unbuilt
    }

    /// How many unclaimed regions the languages the build left out claim or decline.
    pub(crate) fn regions(&self) -> usize {
        let claimed: usize = self.claimed.iter().map(|(_, regions)| regions).sum();
        let declined: usize = self.declined.iter().map(|(_, _, regions)| regions).sum();
        claimed + declined
    }
}

/// What a file answer says of a symlink: where it points and which path to pass instead.
fn symlink_note(symlink: &Symlink<'_>) -> String {
    const HOLDS_NONE: &str = "no code graph holds a symlink";
    let Some(target) = symlink.target else {
        return format!("this path is a symlink whose target could not be read, and {HOLDS_NONE}");
    };
    let (place, next) = symlink.resolved.as_ref().map_or_else(
        || {
            (
                "which names nothing the index holds".to_owned(),
                Some("read it".to_owned()),
            )
        },
        held_place,
    );
    let note = format!("this path is a symlink to {target:?}, {place}, and {HOLDS_NONE}");
    match next {
        Some(next) => format!("{note} - {next}"),
        None => note,
    }
}

/// The held entry a symlink points to, as its note names it, and the call to make instead, if any.
fn held_place(entry: &Entry<'_>) -> (String, Option<String>) {
    let path = entry.path();
    match entry.kind() {
        EntryKind::Dir => (
            format!("the indexed directory {path:?}"),
            Some("pass one of that directory's files, which list_dir lists".to_owned()),
        ),
        EntryKind::Other => (
            format!("the indexed {path:?}, which is neither a file nor a directory"),
            None,
        ),
        EntryKind::Symlink => (
            format!("the indexed symlink {path:?}"),
            Some(format!("pass {path:?}")),
        ),
        EntryKind::File => (
            format!("the indexed file {path:?}"),
            Some(format!("pass {path:?}")),
        ),
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

fn suggestion(family: &str, definition: &Definition<'_>) -> String {
    named("suggestion", definition)
        .optional("role", role(definition.flags))
        .attr("kind", kind(definition.kind))
        .attr("lang", family)
        .attr("loc", loc(definition.file, definition.line))
        .close()
}

/// The `<suggestion>` records of the definitions named one edit from a name nothing defines, where `near` lists them.
fn one_edit<'a>(near: &'a kartta::Unmatched<'_>) -> impl Iterator<Item = String> + 'a {
    near.listed
        .iter()
        .map(|(family, definition)| suggestion(family, definition))
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
    /// The definitions that say why the name selected none: those it names, or those named one edit from it.
    Named(kartta::Unmatched<'g>),
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

/// The selector a symbol tool was asked with, what its narrowing parameters' names start with: "" for `path_prefix`
/// and `line`, "from_" or "to_" for an end of `find_call_path`, and the languages a filtered build holds, as
/// [`Indexed::languages`] echoes them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Asked<'a> {
    pub(crate) end: &'a str,
    pub(crate) selector: Selector<'a>,
    pub(crate) built: Option<&'a str>,
}

/// The body of an answer whose name selected nothing, its envelope naming the languages a filtered build holds: the
/// empty filter's note, what the name matches instead, or the alternatives a search found.
fn unresolved(
    element: Element,
    languages: Option<&str>,
    asked: Asked<'_>,
    unmatched: &Unmatched<'_>,
) -> String {
    let element = element.optional("index_languages", asked.built);
    if let Some(note) = empty_languages_note(languages) {
        return element.inline(note);
    }
    let (text, listed) = match unmatched {
        Unmatched::Named(near) if near.kind == UnmatchedKind::OneEdit => {
            return did_you_mean(element.count("one_edit", near.total), one_edit(near));
        }
        Unmatched::Named(named) => (named_instead(asked, named), &named.listed),
        Unmatched::Suggested(suggestions) if suggestions.is_empty() => {
            return element.inline("no symbol matches - find_symbols can pin the exact name");
        }
        Unmatched::Suggested(suggestions) => {
            let records = suggestions
                .iter()
                .map(|suggested| suggestion(suggested.family, &suggested.hit.definition));
            return did_you_mean(element, records);
        }
    };
    let candidates = listed
        .iter()
        .map(|(family, definition)| candidate(definition).attr("lang", *family).close())
        .collect();
    noted(element, &text, candidates)
}

/// What a name that selected none of the definitions `named` lists says of them, naming the parameters that pick one:
/// a path prefix with one's file and the line it starts on, since a file can hold several of them.
fn named_instead(asked: Asked<'_>, named: &kartta::Unmatched<'_>) -> String {
    let (total, end) = (named.total, asked.end);
    if named.kind == UnmatchedKind::OtherKind {
        let held: Vec<String> = named
            .kinds
            .iter()
            .map(|&(held, count)| format!("{} ({count})", kind(held)))
            .collect();
        let those = if held.len() == 1 {
            "that kind"
        } else {
            "those kinds"
        };
        let listed = named.listed.len();
        let shown = if listed < total {
            format!(", the first {listed} of {total} listed")
        } else {
            String::new()
        };
        return format!(
            "the name is held only as {} - this tool takes no root of {those}{shown}:",
            held.join(", ")
        );
    }
    if named.kind == UnmatchedKind::Several {
        return format!(
            "{total} definitions end with this name and it picks none of them - a longer name, or {end}path_prefix \
             with one's file and {end}line with its line, picks one:"
        );
    }
    let Some(line) = asked.selector.line else {
        return format!(
            "no definition of this name is under {end}path_prefix - {total} are elsewhere:"
        );
    };
    let under = if asked.selector.path_prefix.is_some() {
        format!(" under {end}path_prefix")
    } else {
        String::new()
    };
    format!("no definition of this name{under} starts on {end}line {line} - {total} are elsewhere:")
}

/// `element` holding the miss and the `<suggestion>` records that answer it.
fn did_you_mean(element: Element, suggestions: impl Iterator<Item = String>) -> String {
    element.children(
        std::iter::once("no symbol matches - did you mean:".to_owned()).chain(suggestions),
    )
}

/// The `index_repo` and `index_stats` answer: the index's shape, one `<language/>` per graph that holds a file, how
/// the calls of each of those graphs bind when `coverage` holds its resolution, and the failures the build collected.
pub(crate) fn index(indexed: &Indexed, coverage: &[(&str, Resolution<'_>)]) -> String {
    let graphs = indexed.graphs();
    let element = Element::new("index")
        .attr("root", &indexed.root)
        .optional("languages", indexed.languages.as_deref())
        .attr("files", indexed.files())
        .attr(
            "gitignore",
            match &indexed.gitignore {
                Gitignore::Ignored => "ignored".to_owned(),
                Gitignore::InTree { outer_top: None } => "in-tree".to_owned(),
                Gitignore::InTree {
                    outer_top: Some(top),
                } => format!(
                    "in-tree: the root is inside the repository at {top}, whose .gitignore files above the root were \
                     not read"
                ),
            },
        )
        .attr(
            "watch",
            indexed
                .watch
                .off
                .as_ref()
                .map_or_else(|| "following".to_owned(), |reason| format!("off: {reason}")),
        )
        .attr(
            "symbols",
            graphs.iter().map(|row| row.symbols).sum::<usize>(),
        )
        .attr("calls", graphs.iter().map(|row| row.calls).sum::<usize>())
        .optional(
            "grep_index",
            indexed
                .engine
                .index()
                .content_index_failure()
                .map(|failure| format!("off: {failure}")),
        );
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
        .optional("parts", refresh.widened.map(|widened| widened.parts))
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
        .count("past_bound", misses.past_bound)
        .count("through_receiver", misses.through_a_receiver)
        .close()
}

/// The `index_stats` answer while the slot holds no index: nothing indexed, a build under way, or the failure of the
/// last build.
pub(crate) fn unready(unready: &Unready) -> String {
    let element = Element::new("index");
    match unready {
        Unready::Empty => element.attr("status", "empty").inline(
            "nothing is indexed - call index_repo with the repository's absolute root",
        ),
        Unready::Building { root, elapsed } => element
            .attr("root", root)
            .attr("status", "building")
            .attr("elapsed_secs", elapsed.as_secs())
            .inline(
                "still building - every other tool answers once it is ready, and index_repo with this root, \
                 languages and index_ignored waits for it",
            ),
        Unready::Failed { root, error } => element
            .attr("root", root)
            .attr("status", "failed")
            .inline(&format!("{error} - call index_repo again")),
    }
}
