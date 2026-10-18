//! The methods that ask a finished build, one function each: decode the params, ask the engine, answer the view.

use kartta::{Region, SourceOptions};

use crate::built::Built;
use crate::error::Error;
use crate::wire::files::{
    FuzzyParams, FuzzyView, GlobParams, GlobView, GrepParams, GrepView, ListParams, ListView,
};
use crate::wire::graph::{
    CallPathView, ChangeView, DeadCodeView, DefinitionsView, DependentsView, FileView,
    ImplementationsView, OutlineView, ReferencesView, ResolutionView, SearchView, SegmentView,
    WalkView,
};
use crate::wire::params::{
    CALLEES_DEPTH, CALLERS_DEPTH, CallPathParams, ChangeParams, DeadCodeParams, DefinitionsParams,
    DependentsParams, ImplementationsParams, OutlineParams, ReferencesParams, ResolutionParams,
    SearchParams, SegmentParams, SourcesParams, WalkParams,
};
use crate::wire::{self, NoParams, StatsAnswer};

type Query = fn(&Built, &[u8]) -> Result<Vec<u8>, Error>;

/// Every method that asks a finished build, by name.
pub(crate) const QUERIES: &[(&str, Query)] = &[
    ("analyze_change", analyze_change),
    ("analyze_segment", analyze_segment),
    ("call_path", call_path),
    ("callees", callees),
    ("callers", callers),
    ("dead_code", dead_code),
    ("definitions", definitions),
    ("dependents", dependents),
    ("fuzzy", fuzzy),
    ("glob", glob),
    ("grep", grep),
    ("implementations", implementations),
    ("list", list),
    ("outline", outline),
    ("references", references),
    ("resolution", resolution),
    ("search", search),
    ("sources", sources),
    ("stats", stats),
];

fn stats(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let NoParams {} = wire::params(params)?;
    wire::ok(&StatsAnswer::new(&built.engine, &built.errors))
}

fn callers(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: WalkParams = wire::params(params)?;
    let limits = params.limits(CALLERS_DEPTH);
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        graph.callers(&params.name, limits).map(WalkView::from)
    })?)
}

fn callees(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: WalkParams = wire::params(params)?;
    let limits = params.limits(CALLEES_DEPTH);
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        graph.callees(&params.name, limits).map(WalkView::from)
    })?)
}

fn call_path(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: CallPathParams = wire::params(params)?;
    let limits = params.limits();
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        graph
            .call_path(&params.from, &params.to, limits)
            .map(CallPathView::from)
    })?)
}

fn references(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ReferencesParams = wire::params(params)?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        graph
            .references(&params.name, params.query())
            .map(ReferencesView::from)
    })?)
}

fn implementations(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ImplementationsParams = wire::params(params)?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        graph
            .implementations(&params.name, params.depth(), params.limit())
            .map(ImplementationsView::from)
    })?)
}

fn definitions(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DefinitionsParams = wire::params(params)?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        let found = graph.definitions(&params.name, params.from.as_deref(), params.limit());
        (found.total > 0).then(|| DefinitionsView::new(&found, None))
    })?)
}

fn sources(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SourcesParams = wire::params(params)?;
    let options = SourceOptions::default()
        .max_lines(params.max_lines)
        .max_line_bytes(params.max_line_bytes);
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        let found = graph.definitions(&params.name, params.from.as_deref(), params.limit());
        if found.total == 0 {
            return None;
        }
        let asked: Vec<_> = found.found.iter().map(|found| found.definition).collect();
        let sources = built.engine.sources(&asked, options);
        Some(DefinitionsView::new(&found, Some(sources)))
    })?)
}

fn search(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SearchParams = wire::params(params)?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        let found = graph.search(&params.query, params.limit());
        (found.total > 0).then(|| SearchView::from(found))
    })?)
}

fn dead_code(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DeadCodeParams = wire::params(params)?;
    let kinds = params.kinds()?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        Some(DeadCodeView::from(graph.dead_code(
            &kinds,
            params.path_prefix.as_deref(),
            params.limit(),
        )))
    })?)
}

fn resolution(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ResolutionParams = wire::params(params)?;
    let index = built.engine.index();
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        Some(ResolutionView::from(
            index.on_threads(|| graph.resolution(params.limit())),
        ))
    })?)
}

fn outline(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: OutlineParams = wire::params(params)?;
    let graph = built.graph_of(&params.file);
    let outline = graph.and_then(|(_, graph)| graph.outline(&params.file));
    wire::ok(&FileView::new(
        &params.file,
        graph.map(|(name, _)| name),
        outline.map(OutlineView::from),
    ))
}

fn dependents(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DependentsParams = wire::params(params)?;
    let graph = built.graph_of(&params.file);
    let found = graph
        .and_then(|(_, graph)| graph.dependents(&params.file, params.depth(), params.max_files()));
    wire::ok(&FileView::new(
        &params.file,
        graph.map(|(name, _)| name),
        found.map(DependentsView::from),
    ))
}

fn analyze_segment(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SegmentParams = wire::params(params)?;
    let graph = built.graph_of(&params.file);
    let segment = graph.and_then(|(_, graph)| {
        graph.analyze_segment(&params.file, params.lines(), params.limits())
    });
    wire::ok(&FileView::new(
        &params.file,
        graph.map(|(name, _)| name),
        segment.map(SegmentView::from),
    ))
}

fn analyze_change(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ChangeParams = wire::params(params)?;
    let regions: Vec<Region<'_>> = params
        .regions
        .iter()
        .map(|region| Region::new(&region.file, region.lines()))
        .collect();
    wire::ok(&ChangeView::from(
        built.engine.analyze_change(&regions, params.limits()),
    ))
}

fn list(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ListParams = wire::params(params)?;
    wire::ok(&ListView::new(built.engine.index(), &params)?)
}

fn glob(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: GlobParams = wire::params(params)?;
    wire::ok(&GlobView::new(built.engine.index(), &params)?)
}

fn fuzzy(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: FuzzyParams = wire::params(params)?;
    wire::ok(&FuzzyView::new(built.engine.index(), &params)?)
}

fn grep(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: GrepParams = wire::params(params)?;
    let index = built.engine.index();
    let matches = params.search(index)?;
    wire::ok(&GrepView::new(index, &matches))
}
