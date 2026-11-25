use kartta::{
    Graph, Index, Region, Selector, SourceOptions, SymbolKind, ask_reading, is_parent, is_referent,
    selects_as_callers, unmatched,
};

use crate::built::{Built, Matches};
use crate::error::Error;
use crate::wire::files::{FuzzyParams, FuzzyView, GlobParams, GlobView, ListParams, ListView};
use crate::wire::graph::{
    CallersView, ChangeView, DefinitionsView, DependentsView, FileView, OutlineView,
    ReferencesView, Wired,
};
use crate::wire::params::{
    CallPathParams, ChangeParams, DeadCodeParams, DefinitionsParams, DependentsParams,
    ImplementationsParams, OutlineParams, ReferencesParams, ResolutionParams, SearchParams,
    SegmentParams, SourcesParams, WalkParams,
};
use crate::wire::{self, StoppedView};

type Query = fn(&Built, &[u8]) -> Result<Vec<u8>, Error>;
type FileQuery = fn(&Index, Option<StoppedView>, &[u8]) -> Result<Vec<u8>, Error>;

/// How many of the definitions a name that selected nothing names instead an answer lists.
const UNMATCHED_LISTED: usize = 5;

pub(crate) const QUERIES: &[(&str, Query)] = &[
    ("analyze_change", analyze_change),
    ("analyze_segment", analyze_segment),
    ("call_path", call_path),
    ("callees", callees),
    ("callers", callers),
    ("dead_code", dead_code),
    ("definitions", definitions),
    ("dependents", dependents),
    ("implementations", implementations),
    ("outline", outline),
    ("references", references),
    ("resolution", resolution),
    ("search", search),
    ("sources", sources),
];

/// The queries an index alone answers, the index of a build a cancel stopped among them.
pub(crate) const FILE_QUERIES: &[(&str, FileQuery)] =
    &[("fuzzy", fuzzy), ("glob", glob), ("list", list)];

/// What the `searched` graphs answer for `named`, as [`ask_reading`] asks them, and, where none answers, what the name
/// names instead among the definitions of the kinds `selects` takes.
fn ask_named<'b, T>(
    searched: impl Iterator<Item = (&'b str, &'b Graph)> + Clone,
    named: Selector<'_>,
    selects: fn(SymbolKind) -> bool,
    ask: impl Fn((&'b str, &'b Graph), Selector<'_>) -> Option<(&'b str, T)>,
) -> Matches<'b, T> {
    let (found, read_as) = ask_reading(searched.clone(), [named], selects, |graph, [named]| {
        ask(graph, named)
    });
    let unmatched = if found.is_empty() {
        let [read] = &read_as;
        let read = named.with_name(read.as_deref().unwrap_or(named.name));
        unmatched(searched, read, selects, UNMATCHED_LISTED)
    } else {
        None
    };
    Matches::read(found, read_as, unmatched)
}

fn callers(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: WalkParams = wire::params(params)?;
    let limits = params.limits();
    wire::ok(&ask_named(
        built.graphs(params.lang.as_deref())?,
        params.selector(),
        |_| true,
        |(name, graph), named| Some((name, CallersView::new(graph.callers(named, limits)?))),
    ))
}

fn callees(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: WalkParams = wire::params(params)?;
    let limits = params.limits();
    wire::ok(&ask_named(
        built.graphs(params.lang.as_deref())?,
        params.selector(),
        |_| true,
        |(name, graph), named| Some((name, Wired(graph.callees(named, limits)?))),
    ))
}

fn call_path(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: CallPathParams = wire::params(params)?;
    let limits = params.limits();
    let searched = built.graphs(params.lang.as_deref())?;
    let written = [params.from(), params.to()];
    let (found, read_as) = ask_reading(
        searched.clone(),
        written,
        |_| true,
        |(name, graph), [from, to]| Some((name, Wired(graph.call_path(from, to, limits)?))),
    );
    let unmatched_ends = if found.is_empty() {
        std::array::from_fn(|at| {
            let end = written[at].with_name(read_as[at].as_deref().unwrap_or(written[at].name));
            if selects_as_callers(searched.clone(), end) {
                return None;
            }
            unmatched(searched.clone(), end, |_| true, UNMATCHED_LISTED)
        })
    } else {
        [None, None]
    };
    wire::ok(&Matches::read_ends(found, read_as, unmatched_ends))
}

fn references(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ReferencesParams = wire::params(params)?;
    wire::ok(&ask_named(
        built.graphs(params.lang.as_deref())?,
        params.selector(),
        is_referent,
        |(name, graph), named| {
            let found = graph.references(named, params.query())?;
            Some((name, ReferencesView::new(found, graph.conventions())))
        },
    ))
}

fn implementations(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ImplementationsParams = wire::params(params)?;
    wire::ok(&ask_named(
        built.graphs(params.lang.as_deref())?,
        params.selector(),
        is_parent,
        |(name, graph), named| {
            let found = graph.implementations(named, params.depth, params.limit)?;
            Some((name, Wired(found)))
        },
    ))
}

fn definitions(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DefinitionsParams = wire::params(params)?;
    wire::ok(&ask_named(
        built.graphs(params.lang.as_deref())?,
        Selector::new(&params.name, None),
        |_| true,
        |(name, graph), named| {
            let found = graph.definitions(named.name, params.from.as_deref(), params.limit);
            (found.total > 0).then(|| (name, DefinitionsView::new(&found, None)))
        },
    ))
}

fn sources(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SourcesParams = wire::params(params)?;
    let options = SourceOptions::default()
        .max_lines(params.max_lines)
        .max_line_bytes(params.max_line_bytes);
    wire::ok(&ask_named(
        built.graphs(params.lang.as_deref())?,
        Selector::new(&params.name, None),
        |_| true,
        |(name, graph), named| {
            let found = graph.definitions(named.name, params.from.as_deref(), params.limit);
            if found.total == 0 {
                return None;
            }
            let asked: Vec<_> = found.found.iter().map(|found| found.definition).collect();
            let sources = built.engine.sources(&asked, options);
            Some((name, DefinitionsView::new(&found, Some(sources))))
        },
    ))
}

fn search(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SearchParams = wire::params(params)?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        let found = graph.search(&params.query, params.limit);
        (!found.hits.is_empty() || found.total > 0).then_some(Wired(found))
    })?)
}

fn dead_code(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DeadCodeParams = wire::params(params)?;
    let kinds = params.kinds()?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        Some(Wired(graph.dead_code(
            &kinds,
            params.path_prefix.as_deref(),
            params.limit,
        )))
    })?)
}

fn resolution(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ResolutionParams = wire::params(params)?;
    let index = built.engine.index();
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        Some(Wired(index.on_threads(|| graph.resolution(params.limit))))
    })?)
}

fn outline(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: OutlineParams = wire::params(params)?;
    wire::ok(&FileView::new(&built.engine, &params.file, |graph| {
        graph
            .outline(&params.file)
            .map(|symbols| OutlineView { symbols })
    }))
}

fn dependents(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DependentsParams = wire::params(params)?;
    wire::ok(&FileView::new(&built.engine, &params.file, |graph| {
        graph
            .dependents(&params.file, params.depth, params.max_files)
            .map(|dependents| DependentsView::new(dependents, graph.conventions()))
    }))
}

fn analyze_segment(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SegmentParams = wire::params(params)?;
    wire::ok(&FileView::new(&built.engine, &params.file, |graph| {
        graph
            .analyze_segment(&params.file, params.lines(), params.query())
            .map(Wired)
    }))
}

fn analyze_change(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ChangeParams = wire::params(params)?;
    let regions: Vec<Region<'_>> = params
        .regions
        .iter()
        .map(|region| Region::new(&region.file, region.lines()))
        .collect();
    wire::ok(&ChangeView::new(
        built.engine.analyze_change(&regions, params.query()),
    ))
}

fn list(index: &Index, stopped: Option<StoppedView>, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ListParams = wire::params(params)?;
    wire::ok(&ListView::new(index, &params, stopped)?)
}

fn glob(index: &Index, stopped: Option<StoppedView>, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: GlobParams = wire::params(params)?;
    wire::ok(&GlobView::new(index, &params, stopped)?)
}

fn fuzzy(index: &Index, stopped: Option<StoppedView>, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: FuzzyParams = wire::params(params)?;
    wire::ok(&FuzzyView::new(index, &params, stopped)?)
}
