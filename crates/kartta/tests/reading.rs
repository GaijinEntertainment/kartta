//! What a name that selected no symbol names instead, across the graphs a query searched.

use kartta::{
    Conventions, Extraction, Graph, Selector, Symbol, SymbolKind, UnmatchedKind, is_parent,
    selects_as_callers, unmatched,
};

fn graph(files: &[(&str, Vec<Symbol>)]) -> Graph {
    let mut graph = Graph::new(Conventions::default());
    for (path, symbols) in files {
        let mut extraction = Extraction::default();
        (*path).clone_into(&mut extraction.file);
        extraction.symbols.clone_from(symbols);
        graph.insert(&extraction);
    }
    graph
}

fn qualified(name: &str, qualified_name: &str, kind: SymbolKind) -> Symbol {
    let mut symbol = Symbol::new(name, kind, 1, 2);
    qualified_name.clone_into(&mut symbol.qualified_name);
    symbol
}

fn close(qualified_name: &str) -> Symbol {
    let mut symbol = qualified("Close", qualified_name, SymbolKind::Method);
    symbol.owner = Some("Store".to_owned());
    symbol
}

#[test]
fn a_suffix_several_definitions_end_with_names_them_all() {
    let go = graph(&[
        ("a/store.go", vec![close("a.Store.Close")]),
        ("b/store.go", vec![close("b.Store.Close")]),
    ]);
    let searched = [("go", &go)];

    let named = unmatched(
        searched.into_iter(),
        Selector::new("Store.Close", None),
        |_| true,
        5,
    )
    .expect("two definitions end with the name");

    assert_eq!(named.kind, UnmatchedKind::Several);
    assert_eq!(named.total, 2);
    let listed: Vec<_> = named
        .listed
        .iter()
        .map(|(family, definition)| (*family, definition.qualified_name))
        .collect();
    assert_eq!(listed, [("go", "a.Store.Close"), ("go", "b.Store.Close")]);
}

#[test]
fn a_name_defined_only_outside_its_path_prefix_counts_the_definitions_in_every_graph() {
    let go = graph(&[(
        "lib/parse.go",
        vec![qualified("parse", "lib.parse", SymbolKind::Function)],
    )]);
    let python = graph(&[
        (
            "tools/parse.py",
            vec![qualified("parse", "tools.parse", SymbolKind::Function)],
        ),
        (
            "tools/more.py",
            vec![qualified("parse", "more.parse", SymbolKind::Function)],
        ),
    ]);
    let searched = [("go", &go), ("python", &python)];

    let named = unmatched(
        searched.into_iter(),
        Selector::new("parse", Some("app/")),
        |_| true,
        2,
    )
    .expect("the definitions lie outside app/");

    assert_eq!(named.kind, UnmatchedKind::OutsidePrefix);
    assert_eq!(named.total, 3, "every definition outside the prefix");
    let families: Vec<&str> = named.listed.iter().map(|(family, _)| *family).collect();
    assert_eq!(families, ["go", "python"], "the first two, graph by graph");
}

#[test]
fn several_definitions_under_the_prefix_come_before_those_outside_it() {
    let go = graph(&[
        ("a/store.go", vec![close("a.Store.Close")]),
        ("a/other.go", vec![close("other.Store.Close")]),
        ("b/store.go", vec![close("b.Store.Close")]),
    ]);

    let named = unmatched(
        [("go", &go)].into_iter(),
        Selector::new("Store.Close", Some("a/")),
        |_| true,
        5,
    )
    .expect("two definitions under a/ end with the name");

    assert_eq!(named.kind, UnmatchedKind::Several);
    assert_eq!(named.total, 2, "only those under the prefix");
    let files: Vec<&str> = named
        .listed
        .iter()
        .map(|(_, definition)| definition.file)
        .collect();
    assert_eq!(files, ["a/other.go", "a/store.go"]);
}

#[test]
fn a_name_that_names_one_definition_under_its_prefix_or_none_names_nothing_instead() {
    let go = graph(&[(
        "app/parse.go",
        vec![qualified("parse", "app.parse", SymbolKind::Function)],
    )]);
    let ask = |name, prefix| {
        unmatched(
            [("go", &go)].into_iter(),
            Selector::new(name, prefix),
            |_| true,
            5,
        )
    };

    assert_eq!(ask("parse", Some("app/")), None, "one under the prefix");
    assert_eq!(ask("parse", None), None, "one, with no prefix");
    assert_eq!(ask("absent", Some("app/")), None, "none at all");
}

#[test]
fn a_name_with_one_definition_under_its_prefix_and_others_outside_names_only_those_outside() {
    let go = graph(&[
        (
            "app/parse.go",
            vec![qualified("parse", "app.parse", SymbolKind::Function)],
        ),
        (
            "lib/parse.go",
            vec![qualified("parse", "lib.parse", SymbolKind::Function)],
        ),
    ]);

    let named = unmatched(
        [("go", &go)].into_iter(),
        Selector::new("parse", Some("app/")),
        |_| true,
        5,
    )
    .expect("one definition lies outside app/");

    assert_eq!(named.kind, UnmatchedKind::OutsidePrefix);
    assert_eq!(named.total, 1, "the one under the prefix is not counted");
    let files: Vec<&str> = named
        .listed
        .iter()
        .map(|(_, definition)| definition.file)
        .collect();
    assert_eq!(files, ["lib/parse.go"]);
}

#[test]
fn a_definition_of_a_kind_the_query_does_not_select_is_not_counted() {
    let go = graph(&[
        (
            "lib/a.go",
            vec![qualified("Node", "a.Node", SymbolKind::Function)],
        ),
        (
            "lib/b.go",
            vec![qualified("Node", "b.Node", SymbolKind::Struct)],
        ),
    ]);

    let named = unmatched(
        [("go", &go)].into_iter(),
        Selector::new("Node", Some("app/")),
        is_parent,
        5,
    )
    .expect("the struct lies outside app/");

    assert_eq!(named.total, 1, "the function is no parent");
    assert_eq!(named.listed[0].1.qualified_name, "b.Node");
}

#[test]
fn a_call_end_selects_where_a_walk_over_callers_would_start() {
    let go = graph(&[
        (
            "a/store.go",
            vec![
                close("a.Store.Close"),
                qualified("parse", "a.parse", SymbolKind::Function),
            ],
        ),
        (
            "b/store.go",
            vec![
                close("b.Store.Close"),
                qualified("parse", "b.parse", SymbolKind::Function),
            ],
        ),
    ]);
    let selects =
        |name, prefix| selects_as_callers([("go", &go)].into_iter(), Selector::new(name, prefix));

    assert!(
        selects("parse", None),
        "a bare name shared by two picks one"
    );
    assert!(selects("Store.Close", Some("b/")), "the prefix picks one");
    assert!(!selects("Store.Close", None), "a suffix of two picks none");
    assert!(!selects("parse", Some("c/")), "none under the prefix");
}
