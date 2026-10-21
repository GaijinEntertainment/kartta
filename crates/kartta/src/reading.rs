use kartta_graph::{Definition, Graph, Selector};
use kartta_lang::SymbolKind;

/// The definitions a name a query selected no symbol by names, among those of the kinds the query selects, when they
/// say why it selected none: `total` of them, the first `listed`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Unmatched<'g> {
    /// Why the name selected none of them.
    pub kind: UnmatchedKind,
    /// How many definitions the name names so.
    pub total: usize,
    /// The first of them, each with the name of the graph that holds it, in the order of the graphs asked and of
    /// [`Graph::definitions`] within each.
    pub listed: Vec<(&'g str, Definition<'g>)>,
}

/// Why a name selected none of the definitions an [`Unmatched`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnmatchedKind {
    /// Several definitions under the path prefix end with the name, and a query picks none of them: a longer name, or
    /// a path prefix that holds only one of them, picks one.
    Several,
    /// The name names definitions outside the path prefix, and at most one under it.
    OutsidePrefix,
}

/// What `named` names instead, for a name by which a query selected no symbol in the `searched` graphs.
///
/// Among the definitions of the kinds `selects` takes, it answers several under the name's path prefix, else some
/// outside it, listing at most `limit`, and `None` when neither holds, as for a name that names no such definition.
/// The name is looked up once in each graph, as [`Graph::definitions`] reads it; nothing is searched.
pub fn unmatched<'g>(
    searched: impl Iterator<Item = (&'g str, &'g Graph)>,
    named: Selector<'_>,
    selects: fn(SymbolKind) -> bool,
    limit: usize,
) -> Option<Unmatched<'g>> {
    let tally = |kind| Unmatched {
        kind,
        total: 0,
        listed: Vec::new(),
    };
    let (mut under, mut outside) = (
        tally(UnmatchedKind::Several),
        tally(UnmatchedKind::OutsidePrefix),
    );
    for (family, graph) in searched {
        let found = graph.definitions(named.name, None, usize::MAX).found;
        for definition in found.into_iter().map(|found| found.definition) {
            if !selects(definition.kind) {
                continue;
            }
            let held = if named
                .path_prefix
                .is_none_or(|prefix| definition.file.starts_with(prefix))
            {
                &mut under
            } else {
                &mut outside
            };
            held.total += 1;
            if held.listed.len() < limit {
                held.listed.push((family, definition));
            }
        }
    }
    if under.total > 1 {
        return Some(under);
    }
    (outside.total > 0).then_some(outside)
}

/// Whether `named` selects a symbol in any of the `searched` graphs, as [`Graph::selects_as_callers`] answers for one:
/// an end of [`Graph::call_path`] that selects none is one to ask [`unmatched`] about.
pub fn selects_as_callers<'g>(
    mut searched: impl Iterator<Item = (&'g str, &'g Graph)>,
    named: Selector<'_>,
) -> bool {
    searched.any(|(_, graph)| graph.selects_as_callers(named))
}

/// What `ask` answers for `named` in each of the `searched` graphs that answers, or, when none does, what it answers
/// with each name read with the other owner separator, with the readings.
///
/// A name is read only when it selects no definition of the kinds `selects` takes, in the files it looks in, in any
/// of the graphs, and the reading does: each `.` read as `::`, or else each `::` as `.`, the first that selects. That
/// reads a qualified name spelled with another language's separator, `Index.refresh` for Rust's `Index::refresh` or
/// `pkg::New` for Go's `pkg.New`. A reading is `None` for a name that selects as written or that neither reading
/// selects, and every reading is `None` when `ask` answered the names as written, so a name that matched costs
/// nothing more.
///
/// `searched` yields the graphs a query asks, as [`Engine::graphs`](crate::Engine::graphs) does, `named` the names it
/// takes, `selects` the kinds of definition it selects, and `ask` its answer for one graph, `None` where the graph
/// answers nothing.
pub fn ask_reading<'g, T, const N: usize>(
    searched: impl Iterator<Item = (&'g str, &'g Graph)> + Clone,
    named: [Selector<'_>; N],
    selects: fn(SymbolKind) -> bool,
    ask: impl Fn((&'g str, &'g Graph), [Selector<'_>; N]) -> Option<T>,
) -> (Vec<T>, [Option<String>; N]) {
    let found: Vec<T> = searched
        .clone()
        .filter_map(|graph| ask(graph, named))
        .collect();
    if !found.is_empty() {
        return (found, [const { None }; N]);
    }
    let readings = named.map(|named| reading(&searched, named, selects));
    if readings.iter().all(Option::is_none) {
        return (found, readings);
    }
    let read = std::array::from_fn(|at| {
        readings[at]
            .as_deref()
            .map_or(named[at], |read| Selector::new(read, named[at].path_prefix))
    });
    let found = searched.filter_map(|graph| ask(graph, read)).collect();
    (found, readings)
}

/// The name of `named` read with the other owner separator, each `.` as `::` or else each `::` as `.`, when the name
/// selects no definition of the kinds `selects` takes in the files and graphs a query looks in and that reading
/// selects one.
fn reading<'g>(
    searched: &(impl Iterator<Item = (&'g str, &'g Graph)> + Clone),
    named: Selector<'_>,
    selects: fn(SymbolKind) -> bool,
) -> Option<String> {
    let name = named.name;
    if !(name.contains('.') || name.contains("::")) || selects_any(searched.clone(), named, selects)
    {
        return None;
    }
    [(".", "::"), ("::", ".")]
        .into_iter()
        .filter(|(written, _)| name.contains(written))
        .map(|(written, read)| name.replace(written, read))
        .find(|read| {
            selects_any(
                searched.clone(),
                Selector::new(read, named.path_prefix),
                selects,
            )
        })
}

/// Whether `named` selects a definition of the kinds `selects` takes, in the files it looks in, in the graphs a query
/// searched.
fn selects_any<'g>(
    mut searched: impl Iterator<Item = (&'g str, &'g Graph)>,
    named: Selector<'_>,
    selects: fn(SymbolKind) -> bool,
) -> bool {
    searched.any(|(_, graph)| {
        let selected = graph.definitions(named.name, None, usize::MAX).found;
        selected.iter().any(|found| {
            selects(found.definition.kind)
                && named
                    .path_prefix
                    .is_none_or(|prefix| found.definition.file.starts_with(prefix))
        })
    })
}
