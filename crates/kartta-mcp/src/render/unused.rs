use kartta::{Dead, DeadCode, Excluded};

use super::{LIMIT_CUT, empty_languages_note, kind, named, span};
use crate::llmxml::Element;

/// What a `find_unused` call asked, as its answer echoes it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnusedAsk<'a> {
    pub(crate) languages: Option<&'a str>,
    pub(crate) path_prefix: Option<&'a str>,
    pub(crate) limit: usize,
}

/// The `find_unused` answer: the dead definitions of every graph judged, in path order and cut to the limit, with
/// what was judged live and, by reason, what was not judged.
pub(crate) fn unused(ask: &UnusedAsk<'_>, judged: &[(&str, DeadCode<'_>)]) -> String {
    let total: usize = judged.iter().map(|(_, dead)| dead.total).sum();
    let live: usize = judged.iter().map(|(_, dead)| dead.live).sum();
    let mut found: Vec<(&str, &Dead<'_>)> = judged
        .iter()
        .flat_map(|(family, dead)| dead.found.iter().map(move |found| (*family, found)))
        .collect();
    found.sort_by_key(|(_, dead)| (dead.definition.file, dead.definition.line));
    found.truncate(ask.limit);

    let truncated = total > found.len();
    let element = Element::new("unused")
        .optional("languages", ask.languages)
        .optional("path_prefix", ask.path_prefix)
        .attr("count", found.len())
        .optional("total", truncated.then_some(total))
        .flag("truncated", truncated)
        .attr("live", live);
    let excluded = excluded(judged.iter().map(|(_, dead)| &dead.excluded));
    if !found.is_empty() {
        return element.children(
            excluded
                .into_iter()
                .chain(found.iter().map(|&(family, dead)| symbol(family, dead)))
                .chain(truncated.then(|| LIMIT_CUT.to_owned())),
        );
    }
    let note = empty_languages_note(ask.languages).unwrap_or_else(|| match (&excluded, live) {
        (None, 0) if ask.path_prefix.is_some() => {
            "no definition in a code graph lies under path_prefix - the paths it starts are in no graph, or define \
             nothing"
        }
        (None, 0) => "no code graph holds a definition",
        (None, _) => "no unused definition - every definition was judged, and a call or a use reaches each",
        (Some(_), _) => "no unused definition among those judged - what was not judged is counted in <excluded>",
    });
    match excluded {
        Some(excluded) => element.wrap(&format!("{excluded}\n{note}")),
        None => element.inline(note),
    }
}

/// The `<excluded/>` record: what the graphs did not judge, summed by reason, or `None` when they judged everything.
fn excluded<'e>(each: impl Iterator<Item = &'e Excluded>) -> Option<String> {
    let mut sum = Excluded::default();
    for excluded in each {
        sum.test += excluded.test;
        sum.generated += excluded.generated;
        sum.entry += excluded.entry;
        sum.vendored += excluded.vendored;
        sum.unobservable += excluded.unobservable;
        sum.uncertain += excluded.uncertain;
        sum.dispatched += excluded.dispatched;
        sum.reflected += excluded.reflected;
    }
    (sum != Excluded::default()).then(|| {
        Element::new("excluded")
            .count("test", sum.test)
            .count("generated", sum.generated)
            .count("entry", sum.entry)
            .count("vendored", sum.vendored)
            .count("unobservable", sum.unobservable)
            .count("uncertain", sum.uncertain)
            .count("dispatched", sum.dispatched)
            .count("reflected", sum.reflected)
            .close()
    })
}

/// One dead definition, marked when calls or uses spell its name and bind elsewhere.
fn symbol(family: &str, dead: &Dead<'_>) -> String {
    let definition = &dead.definition;
    named("symbol", definition)
        .attr("kind", kind(definition.kind))
        .attr("lang", family)
        .attr("loc", span(definition))
        .flag("name_used_elsewhere", dead.spelled)
        .close()
}
