//! The graph's contract: what it answers about the files it holds, and how a file going in or out changes that.

use kartta_graph::{Definitions, Graph, Nearness};
use kartta_lang::{Conventions, Extraction, Symbol, SymbolFlags, SymbolKind};

fn symbol(name: &str, qualified: &str, line: u32, end_line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Function, line, end_line);
    qualified.clone_into(&mut symbol.qualified_name);
    symbol
}

fn file(path: &str, symbols: Vec<Symbol>) -> Extraction {
    let mut extraction = Extraction::default();
    path.clone_into(&mut extraction.file);
    extraction.symbols = symbols;
    extraction
}

fn graph(files: &[Extraction]) -> Graph {
    let mut graph = Graph::new(Conventions::default());
    for extraction in files {
        graph.insert(extraction);
    }
    graph
}

fn places(definitions: &Definitions<'_>) -> Vec<String> {
    definitions
        .found
        .iter()
        .map(|found| format!("{}:{}", found.definition.file, found.definition.line))
        .collect()
}

#[test]
fn an_outline_orders_definitions_by_line_an_enclosing_one_first() {
    let graph = graph(&[file(
        "a.go",
        vec![
            symbol("after", "after", 20, 22),
            symbol("method", "T.method", 5, 8),
            symbol("T", "T", 5, 30),
        ],
    )]);

    let outline = graph.outline("a.go").expect("the graph holds a.go");

    let names: Vec<&str> = outline.iter().map(|definition| definition.name).collect();
    assert_eq!(names, ["T", "method", "after"]);
}

#[test]
fn an_outline_tells_a_file_with_no_definitions_from_one_the_graph_does_not_hold() {
    let graph = graph(&[file("empty.go", Vec::new())]);

    assert_eq!(
        graph.outline("empty.go").map(|outline| outline.len()),
        Some(0)
    );
    assert_eq!(graph.outline("absent.go"), None);
}

#[test]
fn a_definition_beside_the_asking_file_ranks_first_and_test_code_last() {
    let mut in_test = symbol("Parse", "Parse", 1, 2);
    in_test.flags = SymbolFlags::empty().with_test(true);
    let graph = graph(&[
        file("other/parse.go", vec![symbol("Parse", "Parse", 3, 4)]),
        file("lib/parse.go", vec![symbol("Parse", "Parse", 7, 8)]),
        file("aaa/parse_test.go", vec![in_test]),
    ]);

    let near_lib = graph.definitions("Parse", Some("lib/main.go"), 10);
    let from_nowhere = graph.definitions("Parse", None, 10);

    assert_eq!(
        places(&near_lib),
        ["lib/parse.go:7", "other/parse.go:3", "aaa/parse_test.go:1"]
    );
    assert_eq!(near_lib.found[0].nearness, Nearness::SameDirectory);
    assert_eq!(near_lib.found[1].nearness, Nearness::Elsewhere);
    assert_eq!(places(&from_nowhere)[0], "lib/parse.go:7");
}

#[test]
fn vendored_code_ranks_after_the_repository_s_own_code_and_before_test_code() {
    let mut in_test = symbol("Parse", "Parse", 1, 2);
    in_test.flags = SymbolFlags::empty().with_test(true);
    let mut vendored = symbol("Parse", "Parse", 5, 6);
    vendored.flags = SymbolFlags::empty().with_vendored(true);
    let graph = graph(&[
        file("aaa/parse_test.go", vec![in_test]),
        file("vendor/parse.go", vec![vendored]),
        file("zzz/parse.go", vec![symbol("Parse", "Parse", 3, 4)]),
    ]);

    let found = graph.definitions("Parse", Some("lib/main.go"), 10);

    assert_eq!(
        places(&found),
        ["zzz/parse.go:3", "vendor/parse.go:5", "aaa/parse_test.go:1"]
    );
}

#[test]
fn a_limit_keeps_the_nearest_definitions_in_rank_order() {
    let graph = graph(&[
        file("a/parse.go", vec![symbol("Parse", "Parse", 1, 1)]),
        file("b/parse.go", vec![symbol("Parse", "Parse", 1, 1)]),
        file("lib/parse.go", vec![symbol("Parse", "Parse", 9, 9)]),
        file("lib/more.go", vec![symbol("Parse", "Parse", 4, 4)]),
    ]);

    let nearest = graph.definitions("Parse", Some("lib/main.go"), 3);

    assert_eq!(
        places(&nearest),
        ["lib/more.go:4", "lib/parse.go:9", "a/parse.go:1"]
    );
    assert_eq!(nearest.total, 4);
}

#[test]
fn a_qualified_name_selects_definitions_it_ends_at_a_separator() {
    let graph = graph(&[file(
        "a.go",
        vec![
            symbol("parse", "pkg.Owner.parse", 1, 1),
            symbol("parse", "pkg.XOwner.parse", 2, 2),
            symbol("parse", "Owner::parse", 3, 3),
            symbol("parse", "parse", 4, 4),
        ],
    )]);

    let dotted = graph.definitions("Owner.parse", None, 10);
    let longer = graph.definitions("ns::Owner::parse", None, 10);

    assert_eq!(places(&dotted), ["a.go:1"]);
    assert_eq!(places(&longer), ["a.go:3"]);
}

#[test]
fn the_total_counts_what_the_limit_left_out() {
    let graph = graph(&[
        file("a.go", vec![symbol("Run", "Run", 1, 1)]),
        file("b.go", vec![symbol("Run", "Run", 1, 1)]),
        file("c.go", vec![symbol("Run", "Run", 1, 1)]),
    ]);

    let definitions = graph.definitions("Run", None, 2);
    let counted = graph.definitions("Run", None, 0);

    assert_eq!(definitions.total, 3);
    assert_eq!(definitions.found.len(), 2);
    assert_eq!((counted.total, counted.found.len()), (3, 0));
}

#[test]
fn a_file_inserted_again_replaces_what_it_defined() {
    let mut graph = graph(&[file("a.go", vec![symbol("Old", "Old", 1, 1)])]);

    graph.insert(&file("a.go", vec![symbol("New", "New", 1, 1)]));

    assert_eq!(graph.definitions("Old", None, 10).total, 0);
    assert_eq!(graph.definitions("New", None, 10).total, 1);
    assert_eq!((graph.file_count(), graph.symbol_count()), (1, 1));
}

#[test]
fn a_removed_file_is_forgotten_and_its_slot_serves_the_next_file() {
    let mut graph = graph(&[
        file("a.go", vec![symbol("Shared", "Shared", 1, 1)]),
        file("b.go", vec![symbol("Shared", "Shared", 2, 2)]),
    ]);

    let removed = graph.remove("a.go");
    graph.insert(&file("c.go", vec![symbol("Fresh", "Fresh", 3, 3)]));

    assert!(removed);
    assert!(!graph.remove("a.go"));
    assert_eq!(graph.outline("a.go"), None);
    assert_eq!(places(&graph.definitions("Shared", None, 10)), ["b.go:2"]);
    assert_eq!(places(&graph.definitions("Fresh", None, 10)), ["c.go:3"]);
    assert_eq!((graph.file_count(), graph.symbol_count()), (2, 2));
}
