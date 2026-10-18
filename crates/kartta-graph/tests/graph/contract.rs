//! The graph's contract: what it answers about the files it holds, and how a file going in or out changes that.

use kartta_graph::{Definitions, Graph, Nearness};
use kartta_lang::{Extraction, IncludeRef, Symbol, SymbolFlags, SymbolKind};

use crate::{file, graph};

fn symbol(name: &str, qualified: &str, line: u32, end_line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Function, line, end_line);
    qualified.clone_into(&mut symbol.qualified_name);
    symbol
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
    assert_eq!((graph.stats().files, graph.stats().symbols), (1, 1));
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
    assert_eq!((graph.stats().files, graph.stats().symbols), (2, 2));
}

fn including(mut extraction: Extraction, target: &str) -> Extraction {
    extraction.includes.push(IncludeRef {
        target: target.to_owned(),
    });
    extraction
}

fn including_as_test(extraction: Extraction, target: &str) -> Extraction {
    let mut extraction = including(extraction, target);
    extraction.test_includes.push(target.to_owned());
    extraction
}

fn is_test_code(graph: &Graph, path: &str) -> Vec<bool> {
    graph
        .outline(path)
        .unwrap_or_default()
        .iter()
        .map(|definition| definition.flags.is_test())
        .collect()
}

#[test]
fn every_definition_of_a_file_only_test_modules_include_is_test_code() {
    let graph = graph(&[
        including_as_test(
            file("src/lib.rs", vec![symbol("run", "run", 1, 1)]),
            "src/tests.rs",
        ),
        file(
            "src/tests.rs",
            vec![symbol("helper", "tests::helper", 1, 1)],
        ),
    ]);

    assert_eq!(is_test_code(&graph, "src/tests.rs"), [true]);
    assert_eq!(is_test_code(&graph, "src/lib.rs"), [false]);
    let found = graph.definitions("helper", None, 10);
    assert!(found.found[0].definition.flags.is_test());
}

#[test]
fn a_file_a_plain_include_reaches_is_test_code_only_once_that_includer_is_removed() {
    let mut graph = graph(&[
        file("src/shared.rs", vec![symbol("util", "shared::util", 1, 1)]),
        including_as_test(file("src/lib.rs", Vec::new()), "src/shared.rs"),
        including(file("src/main.rs", Vec::new()), "src/shared.rs"),
    ]);
    let while_plain = is_test_code(&graph, "src/shared.rs");

    graph.remove("src/main.rs");
    let while_test_only = is_test_code(&graph, "src/shared.rs");
    graph.remove("src/lib.rs");

    assert_eq!(while_plain, [false]);
    assert_eq!(while_test_only, [true]);
    assert_eq!(is_test_code(&graph, "src/shared.rs"), [false]);
}

#[test]
fn a_module_only_test_modules_include_is_test_code_too() {
    let graph = graph(&[
        including(file("src/tests.rs", Vec::new()), "src/tests/helpers.rs"),
        including_as_test(file("src/lib.rs", Vec::new()), "src/tests.rs"),
        file(
            "src/tests/helpers.rs",
            vec![symbol("fixture", "tests::helpers::fixture", 1, 1)],
        ),
    ]);

    assert_eq!(is_test_code(&graph, "src/tests/helpers.rs"), [true]);
}

#[test]
fn a_test_include_the_file_does_not_include_marks_nothing() {
    let mut lib = file("src/lib.rs", Vec::new());
    lib.test_includes.push("src/tests.rs".to_owned());
    let graph = graph(&[
        lib,
        file(
            "src/tests.rs",
            vec![symbol("helper", "tests::helper", 1, 1)],
        ),
    ]);

    assert_eq!(is_test_code(&graph, "src/tests.rs"), [false]);
}
