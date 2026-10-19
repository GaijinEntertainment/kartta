//! What a graph counts of what it holds, and how a file going in or out changes the counts.

use kartta_graph::Graph;
use kartta_lang::{
    Access, Conventions, Extraction, IncludeRef, LocalCall, NO_CALLER_ID, Receiver, Symbol,
    SymbolId, SymbolKind, UseRef,
};

use crate::call;

fn library() -> Extraction {
    let mut extraction = Extraction::default();
    "src/lib.c".clone_into(&mut extraction.file);
    extraction.symbols = vec![
        Symbol::new("run", SymbolKind::Function, 1, 9),
        Symbol::new("step", SymbolKind::Function, 2, 3),
        Symbol::new("Point", SymbolKind::Struct, 10, 12),
        Symbol::new("parse", SymbolKind::Function, 13, 15),
    ];
    extraction.local_symbols = vec![SymbolId(1)];
    extraction.calls = vec![
        call(0, "parse", 4),
        call(0, "print", 5),
        call(NO_CALLER_ID.0, "init", 20),
    ];
    extraction.local_calls = vec![LocalCall {
        caller_id: SymbolId(0),
        local: SymbolId(1),
        line: 6,
    }];
    let point = |access, line| UseRef {
        site_of: SymbolId(0),
        name: "Point".to_owned(),
        receiver: Receiver::Bare,
        access,
        line,
    };
    extraction.uses = vec![
        point(Access::Read, 7),
        point(Access::Write, 8),
        point(Access::Update, 8),
        point(Access::Write, 9),
    ];
    extraction.includes = vec![
        IncludeRef {
            target: "src/lib.h".to_owned(),
        },
        IncludeRef {
            target: "/usr/include/stdio.h".to_owned(),
        },
    ];
    extraction
}

fn empty(path: &str) -> Extraction {
    let mut extraction = Extraction::default();
    path.clone_into(&mut extraction.file);
    extraction
}

#[test]
fn the_counts_cover_every_file_definition_site_and_include_the_graph_holds() {
    let mut graph = Graph::new(Conventions::default());
    graph.insert(&library());
    graph.insert(&empty("src/lib.h"));

    let stats = graph.stats();

    assert_eq!(stats.files, 2);
    assert_eq!(stats.files_without_symbols, 1);
    assert_eq!(stats.symbols, 4);
    assert_eq!(
        stats.kinds,
        [(SymbolKind::Function, 3), (SymbolKind::Struct, 1)]
    );
    assert_eq!(stats.calls, 2, "the call at file scope is no site");
    assert_eq!(stats.local_calls, 1);
    assert_eq!(stats.uses, 4);
    assert_eq!(
        (stats.reads, stats.writes, stats.updates),
        (1, 2, 1),
        "each use counts once, by its access"
    );
    assert_eq!(
        stats.includes, 2,
        "an include the graph does not hold counts"
    );
}

#[test]
fn a_file_inserted_again_or_removed_leaves_the_counts_of_what_the_graph_still_holds() {
    let mut graph = Graph::new(Conventions::default());
    graph.insert(&library());
    graph.insert(&empty("src/lib.h"));

    let mut replaced = empty("src/lib.c");
    replaced.symbols = vec![Symbol::new("main", SymbolKind::Function, 1, 2)];
    graph.insert(&replaced);
    graph.remove("src/lib.h");

    let stats = graph.stats();
    assert_eq!(stats.files, 1);
    assert_eq!(stats.files_without_symbols, 0);
    assert_eq!(stats.symbols, 1);
    assert_eq!(stats.kinds, [(SymbolKind::Function, 1)]);
    assert_eq!(
        (stats.calls, stats.local_calls, stats.uses, stats.includes),
        (0, 0, 0, 0)
    );
}

#[test]
fn an_empty_graph_counts_nothing() {
    let stats = Graph::new(Conventions::default()).stats();

    assert_eq!(stats, kartta_graph::Stats::default());
}
