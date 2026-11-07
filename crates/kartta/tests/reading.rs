//! What a name that selected no symbol names instead, across the graphs a query searched.

use kartta::{
    Conventions, Extraction, Graph, Limits, Selector, Symbol, SymbolKind, UnmatchedKind,
    ask_reading, is_parent, selects_as_callers, unmatched,
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

fn overloads(qualified_name: &str) -> Graph {
    let at = |line| {
        let mut symbol = qualified("parse", qualified_name, SymbolKind::Function);
        (symbol.line, symbol.end_line) = (line, line + 1);
        symbol
    };
    graph(&[("app/parse.cpp", vec![at(1), at(5)])])
}

#[test]
fn a_line_no_definition_starts_on_names_the_definitions_of_its_file_as_off_the_line() {
    let cpp = overloads("app::parse");
    let ask = |line| {
        unmatched(
            [("cpp", &cpp)].into_iter(),
            Selector::new("parse", Some("app/parse.cpp")).with_line(line),
            |_| true,
            5,
        )
    };

    let off = ask(Some(9)).expect("two definitions lie off line 9");
    let beside = ask(Some(5)).expect("one definition lies off line 5");

    assert_eq!((off.kind, off.total), (UnmatchedKind::OffLine, 2));
    assert_eq!(beside.kind, UnmatchedKind::OffLine);
    assert_eq!((beside.total, beside.listed[0].1.line), (1, 1));
}

#[test]
fn a_line_with_no_path_prefix_misses_off_the_line() {
    let cpp = overloads("app::parse");

    let named = unmatched(
        [("cpp", &cpp)].into_iter(),
        Selector::new("parse", None).with_line(Some(9)),
        |_| true,
        5,
    )
    .expect("two definitions lie off line 9");

    assert_eq!(named.kind, UnmatchedKind::OffLine);
}

#[test]
fn a_line_with_every_definition_outside_the_prefix_misses_outside_it() {
    let cpp = overloads("app::parse");

    let named = unmatched(
        [("cpp", &cpp)].into_iter(),
        Selector::new("parse", Some("lib/")).with_line(Some(1)),
        |_| true,
        5,
    )
    .expect("two definitions lie outside lib/");

    assert_eq!((named.kind, named.total), (UnmatchedKind::OutsidePrefix, 2));
}

#[test]
fn a_definition_the_line_keeps_out_makes_the_miss_off_the_line_whatever_lies_outside_the_prefix() {
    let parse = qualified("parse", "parse", SymbolKind::Function);
    for files in [
        ["app/parse.cpp", "lib/parse.cpp"],
        ["lib/parse.cpp", "app/parse.cpp"],
    ] {
        let cpp = graph(&[
            (files[0], vec![parse.clone()]),
            (files[1], vec![parse.clone()]),
        ]);

        let named = unmatched(
            [("cpp", &cpp)].into_iter(),
            Selector::new("parse", Some("app/")).with_line(Some(9)),
            |_| true,
            5,
        )
        .expect("both definitions lie off line 9, one of them under app/");

        assert_eq!(named.kind, UnmatchedKind::OffLine, "{files:?}");
        assert_eq!(named.total, 2, "the one outside app/ still counts");
    }
}

#[test]
fn a_name_read_with_the_other_separator_keeps_its_line() {
    let cpp = overloads("app::parse");

    let (found, [read_as]) = ask_reading(
        [("cpp", &cpp)].into_iter(),
        [Selector::new("app.parse", Some("app/")).with_line(Some(5))],
        |_| true,
        |(_, graph), [named]| {
            graph
                .callers(named, Limits::new(1))
                .map(|walk| walk.root.line)
        },
    );

    assert_eq!(read_as.as_deref(), Some("app::parse"));
    assert_eq!(found, [5]);
}

#[test]
fn a_name_read_with_the_other_separator_keeps_its_candidate_limit() {
    let cpp = overloads("app::parse");

    let (found, [read_as]) = ask_reading(
        [("cpp", &cpp)].into_iter(),
        [Selector::new("app.parse", None).with_candidate_limit(0)],
        |_| true,
        |(_, graph), [named]| {
            graph
                .callers(named, Limits::new(1))
                .map(|walk| (walk.passed_over, walk.candidates.len()))
        },
    );

    assert_eq!(read_as.as_deref(), Some("app::parse"));
    assert_eq!(found, [(1, 0)]);
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

type Instead = (UnmatchedKind, usize, Vec<(String, String)>);

/// The kind and the count of what `name` names instead in `searched`, and the graph and the qualified name of each
/// definition listed.
fn named_instead(searched: &[(&str, &Graph)], name: &str) -> Option<Instead> {
    unmatched(
        searched.iter().copied(),
        Selector::new(name, None),
        |_| true,
        5,
    )
    .map(|named| {
        let listed = named
            .listed
            .iter()
            .map(|(family, definition)| {
                ((*family).to_owned(), definition.qualified_name.to_owned())
            })
            .collect();
        (named.kind, named.total, listed)
    })
}

#[test]
fn a_name_nothing_defines_names_the_definitions_one_edit_from_it() {
    let go = graph(&[(
        "walk.go",
        vec![qualified("handle", "walk.handle", SymbolKind::Function)],
    )]);
    let jsts = graph(&[(
        "kartta.ts",
        vec![
            qualified("#handle", "Kartta.#handle", SymbolKind::Field),
            qualified("#cache", "Kartta.#cache", SymbolKind::Field),
            qualified("#instance", "Kartta.#instance", SymbolKind::Field),
        ],
    )]);
    let searched = [("go", &go), ("jsts", &jsts)];
    let one = |family: &str, qualified_name: &str| {
        let listed = vec![(family.to_owned(), qualified_name.to_owned())];
        Some((UnmatchedKind::OneEdit, 1, listed))
    };

    for (name, edit) in [
        ("hadnle", "swapped"),
        ("hnadle", "swapped"),
        ("handel", "swapped"),
        ("hndle", "dropped"),
        ("hendle", "replaced"),
        ("Handle", "replaced"),
        ("handles", "added"),
    ] {
        assert_eq!(
            named_instead(&searched, name),
            one("go", "walk.handle"),
            "{name}: a character {edit}"
        );
    }
    for (name, field) in [
        ("#handel", "Kartta.#handle"),
        ("#cahce", "Kartta.#cache"),
        ("#insatnce", "Kartta.#instance"),
        ("Kartta.#handel", "Kartta.#handle"),
    ] {
        assert_eq!(named_instead(&searched, name), one("jsts", field), "{name}");
    }
}

#[test]
fn a_name_two_edits_from_every_definition_names_nothing_instead() {
    let go = graph(&[(
        "walk.go",
        vec![qualified("handle", "walk.handle", SymbolKind::Function)],
    )]);

    for name in ["hadnel", "hnadel", "andl", "hand"] {
        assert_eq!(
            unmatched(
                [("go", &go)].into_iter(),
                Selector::new(name, None),
                |_| true,
                5
            ),
            None,
            "{name}"
        );
    }
}

#[test]
fn a_name_held_only_as_other_kinds_lists_those_definitions_not_the_names_one_edit_from_it() {
    let go = graph(&[
        (
            "a.go",
            vec![
                qualified("Node", "a.Node", SymbolKind::Function),
                qualified("Node", "a.x.Node", SymbolKind::Variable),
            ],
        ),
        (
            "b.go",
            vec![qualified("Nodes", "b.Nodes", SymbolKind::Struct)],
        ),
        (
            "c.go",
            vec![qualified("Node", "c.Node", SymbolKind::Function)],
        ),
    ]);

    let named = unmatched(
        [("go", &go)].into_iter(),
        Selector::new("Node", Some("b.go")),
        is_parent,
        2,
    )
    .expect("Node is held, though no parent");

    assert_eq!(
        (named.kind, named.total),
        (UnmatchedKind::OtherKind, 3),
        "every definition, wherever it lies"
    );
    assert_eq!(
        named.kinds,
        [(SymbolKind::Function, 2), (SymbolKind::Variable, 1)],
        "counted past the limit"
    );
    let listed: Vec<&str> = named
        .listed
        .iter()
        .map(|(_, definition)| definition.qualified_name)
        .collect();
    assert_eq!(listed, ["a.Node", "a.x.Node"], "the first two");
}

#[test]
fn a_name_that_selects_one_definition_among_others_of_refused_kinds_names_nothing_instead() {
    let go = graph(&[
        (
            "a.go",
            vec![qualified("Node", "a.Node", SymbolKind::Function)],
        ),
        (
            "b.go",
            vec![qualified("Node", "b.Node", SymbolKind::Struct)],
        ),
    ]);

    let named = unmatched(
        [("go", &go)].into_iter(),
        Selector::new("Node", None),
        is_parent,
        5,
    );

    assert_eq!(named, None, "the struct is the one the query selects");
}

#[test]
fn the_definitions_one_edit_away_are_of_the_kinds_the_query_selects_and_count_past_the_limit() {
    let mut tested = qualified("Nodes", "test.Nodes", SymbolKind::Struct);
    tested.flags = tested.flags.with_test(true);
    let go = graph(&[
        (
            "a.go",
            vec![qualified("Nodes", "a.Nodes", SymbolKind::Function)],
        ),
        ("b.go", vec![tested]),
        (
            "c.go",
            vec![qualified("Nodes", "c.Nodes", SymbolKind::Struct)],
        ),
        (
            "d.go",
            vec![qualified("Mode", "d.Mode", SymbolKind::Interface)],
        ),
    ]);

    let named = unmatched(
        [("go", &go)].into_iter(),
        Selector::new("Node", None),
        is_parent,
        2,
    )
    .expect("Nodes and Mode are one edit from Node");

    assert_eq!(named.kind, UnmatchedKind::OneEdit);
    assert_eq!(named.total, 3, "the function is no parent");
    let listed: Vec<&str> = named
        .listed
        .iter()
        .map(|(_, definition)| definition.qualified_name)
        .collect();
    assert_eq!(
        listed,
        ["c.Nodes", "d.Mode"],
        "test code last, then by path"
    );
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
