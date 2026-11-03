use kartta_graph::{Definition, Graph, Selector};
use kartta_lang::SymbolKind;

/// The definitions that say why a query selected no symbol by a name: `total` of them, the first `listed`.
///
/// They are of the kinds the query selects, and the name names them, or names one edit from it do; or, for
/// [`UnmatchedKind::OtherKind`], the name names them and none is of such a kind.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Unmatched<'g> {
    /// Why the name selected none of them.
    pub kind: UnmatchedKind,
    /// How many there are.
    pub total: usize,
    /// The first of them, each with the name of the graph that holds it, in the order of the graphs asked and, within
    /// each, of [`Graph::definitions`], or of [`Graph::definitions_one_edit_from`] for [`UnmatchedKind::OneEdit`].
    pub listed: Vec<(&'g str, Definition<'g>)>,
    /// For [`UnmatchedKind::OtherKind`], how many of them are of each kind, the kinds in the order their first
    /// definition comes; empty for every other kind.
    pub kinds: Vec<(SymbolKind, usize)>,
}

/// Why a name selected none of the definitions an [`Unmatched`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnmatchedKind {
    /// Several definitions the selector looks at end with the name, and a query picks none of them: a longer name, or
    /// a path prefix and a line that look at only one of them, picks one.
    Several,
    /// The name names definitions outside the path prefix, none under it, or one that the selector looks at.
    OutsidePrefix,
    /// The name names definitions under the path prefix that start off the line, and at most one the selector looks
    /// at: the line, not the prefix, keeps them out. The count and the list hold those outside the prefix too.
    OffLine,
    /// The name names no definition, and names one edit from it do, as [`Graph::definitions_one_edit_from`] edits
    /// one: one character swapped with the next, dropped, replaced or added.
    OneEdit,
    /// The name names definitions and none is of a kind the query selects: the count and the list hold every one of
    /// them, wherever it lies.
    OtherKind,
}

impl UnmatchedKind {
    /// The word for the kind in an answer: `several`, `outside_prefix`, `off_line`, `one_edit` or `other_kind`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Several => "several",
            Self::OutsidePrefix => "outside_prefix",
            Self::OffLine => "off_line",
            Self::OneEdit => "one_edit",
            Self::OtherKind => "other_kind",
        }
    }
}

/// What `named` names instead, for a name by which a query selected no symbol in the `searched` graphs.
///
/// Among the definitions of the kinds `selects` takes, it answers several under the name's path prefix and on its line,
/// else some outside them, of [`UnmatchedKind::OffLine`] where one of those lies under the prefix; else, for a name
/// whose every definition is of another kind, those definitions, [`UnmatchedKind::OtherKind`]; else, for a name that
/// names no definition of any kind, those of the names one edit from it wherever they lie, listing at most `limit`;
/// and `None` when none holds. The name is looked up once in each graph, as [`Graph::definitions`] reads it,
/// and a name that names nothing once more for each string one edit from it, as
/// [`Graph::definitions_one_edit_from`] looks them up; nothing is searched.
pub fn unmatched<'g>(
    searched: impl Iterator<Item = (&'g str, &'g Graph)> + Clone,
    named: Selector<'_>,
    selects: fn(SymbolKind) -> bool,
    limit: usize,
) -> Option<Unmatched<'g>> {
    let (mut under, mut outside) = (
        tally(UnmatchedKind::Several),
        tally(UnmatchedKind::OutsidePrefix),
    );
    let mut other = tally(UnmatchedKind::OtherKind);
    let on_any_line = named.with_line(None);
    for (family, graph) in searched.clone() {
        let found = graph.definitions(named.name, None, usize::MAX).found;
        for definition in found.into_iter().map(|found| found.definition) {
            if !selects(definition.kind) {
                match other
                    .kinds
                    .iter_mut()
                    .find(|(kind, _)| *kind == definition.kind)
                {
                    Some((_, count)) => *count += 1,
                    None => other.kinds.push((definition.kind, 1)),
                }
                hold(&mut other, (family, definition), limit);
                continue;
            }
            let admitted = named.admits(definition.file, definition.line);
            if !admitted && on_any_line.admits(definition.file, definition.line) {
                outside.kind = UnmatchedKind::OffLine;
            }
            let held = if admitted { &mut under } else { &mut outside };
            hold(held, (family, definition), limit);
        }
    }
    if under.total > 1 {
        return Some(under);
    }
    if outside.total > 0 {
        return Some(outside);
    }
    if under.total > 0 {
        return None;
    }
    if other.total > 0 {
        return Some(other);
    }
    let mut near = tally(UnmatchedKind::OneEdit);
    for (family, graph) in searched {
        let found = graph.definitions_one_edit_from(named.name);
        for definition in found
            .into_iter()
            .filter(|definition| selects(definition.kind))
        {
            near.total += 1;
            if near.listed.len() < limit {
                near.listed.push((family, definition));
            }
        }
    }
    (near.total > 0).then_some(near)
}

const fn tally<'g>(kind: UnmatchedKind) -> Unmatched<'g> {
    Unmatched {
        kind,
        total: 0,
        listed: Vec::new(),
        kinds: Vec::new(),
    }
}

/// Counts `listed` in `held`, and lists it among the first `limit`.
fn hold<'g>(held: &mut Unmatched<'g>, listed: (&'g str, Definition<'g>), limit: usize) {
    held.total += 1;
    if held.listed.len() < limit {
        held.listed.push(listed);
    }
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
            .map_or(named[at], |read| named[at].with_name(read))
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
        .find(|read| selects_any(searched.clone(), named.with_name(read), selects))
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
                && named.admits(found.definition.file, found.definition.line)
        })
    })
}
