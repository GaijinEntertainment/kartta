//! How a graph's call sites bind when every one is resolved: the confidence each bound at, why the others bound
//! nothing, and which of those it lists.

use kartta_graph::{Graph, Miss, Resolution, confidence};
use kartta_lang::{
    CallRef, Conventions, Extraction, FILE_SCOPE, ImportBinding, LocalCall, Symbol, SymbolId,
    SymbolKind,
};

use crate::function;

fn call(from: u32, receiver: Option<&str>, callee: &str, line: u32) -> CallRef {
    CallRef {
        receiver: receiver.map(str::to_owned),
        ..crate::call(from, callee, line)
    }
}

fn file(path: &str, symbols: Vec<Symbol>, calls: Vec<CallRef>) -> Extraction {
    let mut extraction = crate::file(path, symbols);
    extraction.calls = calls;
    extraction
}

fn field(owner: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Field, line, line);
    symbol.qualified_name = format!("{owner}::{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

fn program() -> Graph {
    let mut main = file(
        "src/main.c",
        vec![function("main", 1), function("step", 2)],
        vec![
            call(0, None, "parse", 3),
            call(0, None, "parse", 4),
            call(0, None, "printf", 5),
            call(0, None, "size", 6),
            call(0, None, "helper", 7),
            call(0, Some("obj"), "missing", 8),
            call(FILE_SCOPE.0, None, "init", 20),
        ],
    );
    main.local_symbols = vec![SymbolId(1)];
    main.local_calls = vec![LocalCall {
        site_of: SymbolId(0),
        local: SymbolId(1),
        line: 9,
    }];
    let mut graph = Graph::new(Conventions::default());
    for extraction in [
        main,
        file(
            "src/parse.c",
            vec![
                function("parse", 1),
                Symbol::new("Point", SymbolKind::Struct, 5, 8),
                field("Point", "size", 6),
            ],
            Vec::new(),
        ),
        file("a/x/one.c", vec![function("helper", 1)], Vec::new()),
        file("b/y/two.c", vec![function("helper", 1)], Vec::new()),
    ] {
        graph.insert(&extraction);
    }
    graph
}

fn listed<'g>(resolution: &Resolution<'g>) -> Vec<(u32, &'g str, Miss)> {
    resolution
        .unbound
        .iter()
        .map(|unbound| (unbound.line, unbound.callee, unbound.miss))
        .collect()
}

#[test]
fn every_site_counts_once_under_the_confidence_that_bound_it_or_the_reason_it_bound_none() {
    let graph = program();

    let resolution = graph.resolution(usize::MAX);

    assert_eq!(resolution.sites, 7, "the call at file scope is no site");
    assert_eq!(
        resolution.bound,
        [(confidence::SAME_FILE, 1), (confidence::UNIQUE, 2)]
    );
    assert_eq!(
        resolution.edges, 2,
        "two calls of parse from main are one edge"
    );
    let misses = resolution.misses;
    assert_eq!(
        (misses.undefined, misses.not_admitted, misses.ambiguous),
        (2, 1, 1)
    );
    assert_eq!(misses.through_a_receiver, 1);
    assert_eq!(
        listed(&resolution),
        [
            (5, "printf", Miss::Undefined),
            (6, "size", Miss::NotAdmitted),
            (7, "helper", Miss::Ambiguous),
            (8, "missing", Miss::Undefined),
        ]
    );
    assert_eq!(resolution.unbound[3].receiver, Some("obj"));
    assert_eq!(resolution.unbound[3].caller.name, "main");
    assert_eq!(resolution.unlisted, 0);
}

#[test]
fn the_limit_lists_the_first_unbound_sites_in_path_order_and_counts_the_rest() {
    let graph = program();

    let resolution = graph.resolution(2);

    assert_eq!(
        listed(&resolution),
        [
            (5, "printf", Miss::Undefined),
            (6, "size", Miss::NotAdmitted)
        ]
    );
    assert_eq!(resolution.unlisted, 2);
    let counts = graph.resolution(0);
    assert_eq!(counts.unbound, []);
    assert_eq!((counts.unlisted, counts.misses), (4, resolution.misses));
}

#[test]
fn the_listed_sites_follow_path_order_across_files_resolved_apart() {
    let paths: Vec<String> = (0..500).map(|at| format!("src/f{at:03}.c")).collect();
    let mut graph = Graph::new(Conventions::default());
    for path in paths.iter().rev() {
        graph.insert(&file(
            path,
            vec![function("main", 1)],
            vec![call(0, None, "absent", 2)],
        ));
    }

    let resolution = graph.resolution(300);

    let files: Vec<&str> = resolution
        .unbound
        .iter()
        .map(|unbound| unbound.caller.file)
        .collect();
    assert_eq!(files, paths[..300]);
    assert_eq!(resolution.unlisted, 200);
}

#[test]
fn a_call_through_an_import_misses_outside_the_repository_or_where_the_directory_exports_none() {
    let mut app = file(
        "cmd/app.go",
        vec![function("main", 1)],
        vec![
            call(0, Some("ext"), "Run", 2),
            call(0, Some("lib"), "hidden", 3),
            call(0, Some("lib"), "Shown", 4),
        ],
    );
    app.bindings = vec![
        ImportBinding {
            alias: Some("ext".to_owned()),
            target_dir: None,
            last_segment: "ext".to_owned(),
        },
        ImportBinding {
            alias: None,
            target_dir: Some("lib".to_owned()),
            last_segment: "lib".to_owned(),
        },
    ];
    let mut shown = function("Shown", 5);
    shown.exported = true;
    let mut graph = Graph::new(Conventions::default());
    graph.insert(&app);
    graph.insert(&file(
        "lib/lib.go",
        vec![function("hidden", 1), shown],
        Vec::new(),
    ));

    let resolution = graph.resolution(10);

    assert_eq!(resolution.bound, [(confidence::IMPORT, 1)]);
    assert_eq!(
        listed(&resolution),
        [
            (2, "Run", Miss::ExternalImport),
            (3, "hidden", Miss::NotExported)
        ]
    );
    let misses = resolution.misses;
    assert_eq!((misses.external_import, misses.not_exported), (1, 1));
    assert_eq!(misses.through_a_receiver, 0, "an import receiver is traced");
}

#[test]
fn an_empty_graph_resolves_nothing() {
    let graph = Graph::new(Conventions::default());

    let resolution = graph.resolution(10);

    assert_eq!((resolution.sites, resolution.edges), (0, 0));
    assert_eq!(resolution.bound, []);
    assert_eq!(resolution.unbound, []);
}
