use kartta_graph::{Graph, Selector};
use kartta_lang::SymbolKind;

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
