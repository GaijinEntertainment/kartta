use kartta::{Region, Selector, SourceOptions, ask_reading, is_parent, is_referent};

use crate::built::{Built, Matches};
use crate::error::Error;
use crate::wire::files::{FuzzyParams, FuzzyView, GlobParams, GlobView, ListParams, ListView};
use crate::wire::graph::{DefinitionsView, FileView, OutlineView, ReferencesView, Wired};
use crate::wire::params::{
    CALLEES_DEPTH, CALLERS_DEPTH, CallPathParams, ChangeParams, DeadCodeParams, DefinitionsParams,
    DependentsParams, ImplementationsParams, OutlineParams, ReferencesParams, ResolutionParams,
    SearchParams, SegmentParams, SourcesParams, WalkParams,
};
use crate::wire::{self, NoParams, StatsAnswer};

type Query = fn(&Built, &[u8]) -> Result<Vec<u8>, Error>;

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
    let (found, read_as) = ask_reading(
        built.graphs(params.lang.as_deref())?,
        [params.selector()],
        |_| true,
        |(name, graph), [named]| Some((name, Wired(graph.callers(named, limits)?))),
    );
    wire::ok(&Matches::read(found, read_as))
}

fn callees(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: WalkParams = wire::params(params)?;
    let limits = params.limits(CALLEES_DEPTH);
    let (found, read_as) = ask_reading(
        built.graphs(params.lang.as_deref())?,
        [params.selector()],
        |_| true,
        |(name, graph), [named]| Some((name, Wired(graph.callees(named, limits)?))),
    );
    wire::ok(&Matches::read(found, read_as))
}

fn call_path(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: CallPathParams = wire::params(params)?;
    let limits = params.limits();
    let (found, read_as) = ask_reading(
        built.graphs(params.lang.as_deref())?,
        [params.from(), params.to()],
        |_| true,
        |(name, graph), [from, to]| Some((name, Wired(graph.call_path(from, to, limits)?))),
    );
    wire::ok(&Matches::read_ends(found, read_as))
}

fn references(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ReferencesParams = wire::params(params)?;
    let (found, read_as) = ask_reading(
        built.graphs(params.lang.as_deref())?,
        [params.selector()],
        is_referent,
        |(name, graph), [named]| {
            let found = graph.references(named, params.query())?;
            Some((
                name,
                ReferencesView::new(found, graph.conventions().type_uses),
            ))
        },
    );
    wire::ok(&Matches::read(found, read_as))
}

fn implementations(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ImplementationsParams = wire::params(params)?;
    let (found, read_as) = ask_reading(
        built.graphs(params.lang.as_deref())?,
        [params.selector()],
        is_parent,
        |(name, graph), [named]| {
            let found = graph.implementations(named, params.depth(), params.limit())?;
            Some((name, Wired(found)))
        },
    );
    wire::ok(&Matches::read(found, read_as))
}

fn definitions(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DefinitionsParams = wire::params(params)?;
    let (found, read_as) = ask_reading(
        built.graphs(params.lang.as_deref())?,
        [Selector::new(&params.name, None)],
        |_| true,
        |(name, graph), [named]| {
            let found = graph.definitions(named.name, params.from.as_deref(), params.limit());
            (found.total > 0).then(|| (name, DefinitionsView::new(&found, None)))
        },
    );
    wire::ok(&Matches::read(found, read_as))
}

fn sources(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SourcesParams = wire::params(params)?;
    let options = SourceOptions::default()
        .max_lines(params.max_lines)
        .max_line_bytes(params.max_line_bytes);
    let (found, read_as) = ask_reading(
        built.graphs(params.lang.as_deref())?,
        [Selector::new(&params.name, None)],
        |_| true,
        |(name, graph), [named]| {
            let found = graph.definitions(named.name, params.from.as_deref(), params.limit());
            if found.total == 0 {
                return None;
            }
            let asked: Vec<_> = found.found.iter().map(|found| found.definition).collect();
            let sources = built.engine.sources(&asked, options);
            Some((name, DefinitionsView::new(&found, Some(sources))))
        },
    );
    wire::ok(&Matches::read(found, read_as))
}

fn search(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SearchParams = wire::params(params)?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        let found = graph.search(&params.query, params.limit());
        (found.total > 0).then_some(Wired(found))
    })?)
}

fn dead_code(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DeadCodeParams = wire::params(params)?;
    let kinds = params.kinds()?;
    wire::ok(&built.each_graph(params.lang.as_deref(), |graph| {
        Some(Wired(graph.dead_code(
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
        Some(Wired(index.on_threads(|| graph.resolution(params.limit()))))
    })?)
}

fn outline(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: OutlineParams = wire::params(params)?;
    let graph = built.engine.graph_of(&params.file);
    let outline = graph.and_then(|(_, graph)| graph.outline(&params.file));
    wire::ok(&FileView::new(
        &params.file,
        graph.map(|(name, _)| name),
        outline.map(|symbols| OutlineView { symbols }),
    ))
}

fn dependents(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: DependentsParams = wire::params(params)?;
    let graph = built.engine.graph_of(&params.file);
    let found = graph
        .and_then(|(_, graph)| graph.dependents(&params.file, params.depth(), params.max_files()));
    wire::ok(&FileView::new(
        &params.file,
        graph.map(|(name, _)| name),
        found.map(Wired),
    ))
}

fn analyze_segment(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: SegmentParams = wire::params(params)?;
    let graph = built.engine.graph_of(&params.file);
    let segment = graph.and_then(|(_, graph)| {
        graph.analyze_segment(&params.file, params.lines(), params.limits())
    });
    wire::ok(&FileView::new(
        &params.file,
        graph.map(|(name, _)| name),
        segment.map(Wired),
    ))
}

fn analyze_change(built: &Built, params: &[u8]) -> Result<Vec<u8>, Error> {
    let params: ChangeParams = wire::params(params)?;
    let regions: Vec<Region<'_>> = params
        .regions
        .iter()
        .map(|region| Region::new(&region.file, region.lines()))
        .collect();
    wire::ok(&Wired(
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
