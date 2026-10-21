//! Which definitions the names one edit from a name select, and in what order.

use kartta_graph::Definition;
use kartta_lang::{Symbol, SymbolFlags, SymbolKind};

use crate::{file, graph};

fn symbol(name: &str, qualified: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Function, line, line);
    qualified.clone_into(&mut symbol.qualified_name);
    symbol
}

fn qualified<'g>(found: &[Definition<'g>]) -> Vec<&'g str> {
    found
        .iter()
        .map(|definition| definition.qualified_name)
        .collect()
}

#[test]
fn a_name_a_character_swapped_dropped_replaced_or_added_from_a_definitions_selects_it() {
    let graph = graph(&[file("walk.go", vec![symbol("handle", "walk.handle", 3)])]);

    for name in [
        "hadnle", "hnadle", "handel", "hndle", "hendle", "Handle", "handles",
    ] {
        let found = graph.definitions_one_edit_from(name);

        assert_eq!(qualified(&found), ["walk.handle"], "for {name}");
    }
}

#[test]
fn a_name_two_edits_from_every_definitions_selects_none() {
    let graph = graph(&[file("walk.go", vec![symbol("handle", "walk.handle", 3)])]);

    for name in ["hadnel", "hnadel", "andl", "hand", "handle"] {
        let found = graph.definitions_one_edit_from(name);

        assert!(found.is_empty(), "for {name}: {:?}", qualified(&found));
    }
}

#[test]
fn a_qualified_name_one_edit_from_another_selects_only_what_that_one_selects() {
    let graph = graph(&[file(
        "parse.go",
        vec![
            symbol("parse", "Owner.parse", 1),
            symbol("parse", "Other.parse", 2),
            symbol("parse", "ns::Owner::parse", 3),
            symbol("parse", "parse", 4),
            symbol("parse", "Kartta.#parse", 5),
        ],
    )]);

    for (name, selected) in [
        ("Ownre.parse", &["Owner.parse"][..]),
        ("Owner.prase", &["Owner.parse"]),
        ("ns::Ownre::parse", &["ns::Owner::parse"]),
        ("pkg.Ownre.parse", &["Owner.parse"]),
        ("Kartta.#prase", &["Kartta.#parse"]),
        ("Katrta.#parse", &["Kartta.#parse"]),
        ("Otehr.parse", &["Other.parse"]),
    ] {
        let found = graph.definitions_one_edit_from(name);

        assert_eq!(qualified(&found), selected, "for {name}");
    }
}

#[test]
fn the_definitions_one_edit_away_come_outside_test_then_vendored_code_first_then_by_path() {
    let mut in_test = symbol("handle", "walk.handle", 1);
    in_test.flags = SymbolFlags::default().with_test(true);
    let mut vendored = symbol("handle", "walk.handle", 1);
    vendored.flags = SymbolFlags::default().with_vendored(true);
    let graph = graph(&[
        file("0_test.go", vec![in_test]),
        file("1_vendored.go", vec![vendored]),
        file("b.go", vec![symbol("handle", "walk.handle", 1)]),
        file("a.go", vec![symbol("handle", "walk.handle", 1)]),
    ]);

    let found = graph.definitions_one_edit_from("hadnle");

    let files: Vec<&str> = found.iter().map(|definition| definition.file).collect();
    assert_eq!(files, ["a.go", "b.go", "1_vendored.go", "0_test.go"]);
}

#[test]
fn the_definitions_one_edit_away_in_one_file_come_by_line_then_qualified_name() {
    let graph = graph(&[file(
        "walk.go",
        vec![
            symbol("handle", "x.handle", 5),
            symbol("handle", "z.handle", 2),
            symbol("handle", "y.handle", 2),
        ],
    )]);

    let found = graph.definitions_one_edit_from("hadnle");

    assert_eq!(qualified(&found), ["y.handle", "z.handle", "x.handle"]);
}

#[test]
fn a_name_past_256_characters_finds_nothing_one_edit_from_it() {
    let defined = format!("{}b", "a".repeat(255));
    let graph = graph(&[file("long.go", vec![symbol(&defined, &defined, 1)])]);

    let swapped = format!("{}ba", "a".repeat(254));
    let added = format!("{defined}c");

    assert_eq!(
        qualified(&graph.definitions_one_edit_from(&swapped)),
        [defined.as_str()]
    );
    assert_eq!(
        qualified(&graph.definitions_one_edit_from(&added)),
        Vec::<&str>::new()
    );
}

#[test]
fn a_definition_several_edits_reach_comes_once() {
    let graph = graph(&[file("a.go", vec![symbol("aa", "aa", 1)])]);

    let found = graph.definitions_one_edit_from("aaa");

    assert_eq!(qualified(&found), ["aa"]);
}
