//! The files that include a file: transitively, nearest first, and what the depth bound and the cap left out.

use kartta_graph::{Dependents, Graph};
use kartta_lang::Extraction;

use crate::{graph, including};

fn file(path: &str, includes: &[&str]) -> Extraction {
    including(crate::file(path, Vec::new()), includes)
}

fn found(dependents: &Dependents<'_>) -> Vec<(String, u32)> {
    dependents
        .found
        .iter()
        .map(|dependent| (dependent.file.to_owned(), dependent.depth))
        .collect()
}

fn tree() -> Graph {
    graph(&[
        file("base.h", &[]),
        file("z/direct.h", &["base.h"]),
        file("a/direct.cpp", &["base.h"]),
        file("mid.h", &["z/direct.h"]),
        file("top.cpp", &["mid.h", "base.h"]),
    ])
}

#[test]
fn includers_come_nearest_first_then_in_path_order() {
    let graph = tree();

    let dependents = graph
        .dependents("base.h", 3, 0)
        .expect("the graph holds base.h");

    assert_eq!(
        found(&dependents),
        [
            ("a/direct.cpp".to_owned(), 1),
            ("top.cpp".to_owned(), 1),
            ("z/direct.h".to_owned(), 1),
            ("mid.h".to_owned(), 2),
        ]
    );
    assert!(!dependents.truncated);
    assert_eq!(dependents.unexpanded, 0);
}

#[test]
fn the_depth_bound_counts_the_files_whose_includers_it_did_not_look_at() {
    let graph = tree();

    let direct = graph.dependents("base.h", 1, 0).expect("held");
    let none = graph.dependents("base.h", 0, 0).expect("held");

    assert_eq!(direct.found.len(), 3);
    assert_eq!(direct.unexpanded, 3);
    assert_eq!(found(&none), []);
    assert_eq!(none.unexpanded, 1);
    for uncapped in [&direct, &none] {
        assert!(!uncapped.truncated);
        assert_eq!(uncapped.turned_away, 0);
    }
}

#[test]
fn the_cap_keeps_the_nearest_files_and_counts_what_it_turned_away() {
    let graph = tree();

    let capped = graph.dependents("base.h", 3, 2).expect("held");
    let filled = graph.dependents("base.h", 3, 3).expect("held");

    assert_eq!(
        found(&capped),
        [("a/direct.cpp".to_owned(), 1), ("top.cpp".to_owned(), 1)]
    );
    assert!(capped.truncated);
    assert_eq!(capped.turned_away, 1);
    assert_eq!(capped.unexpanded, 2);
    assert_eq!(filled.found.len(), 3);
    assert!(filled.truncated);
    assert_eq!((filled.turned_away, filled.unexpanded), (1, 0));
}

#[test]
fn a_file_the_graph_does_not_hold_answers_none_and_one_nobody_includes_answers_empty() {
    let graph = tree();

    assert_eq!(graph.dependents("absent.h", 3, 0), None);
    let alone = graph.dependents("top.cpp", 3, 0).expect("held");
    assert_eq!(found(&alone), []);
    assert_eq!(alone.unexpanded, 0);
}

#[test]
fn an_include_cycle_lists_each_file_once_and_never_the_file_asked_about() {
    let graph = graph(&[
        file("a.h", &["b.h"]),
        file("b.h", &["c.h"]),
        file("c.h", &["a.h"]),
    ]);

    let dependents = graph.dependents("a.h", 5, 0).expect("held");

    assert_eq!(
        found(&dependents),
        [("c.h".to_owned(), 1), ("b.h".to_owned(), 2)]
    );
}

#[test]
fn an_includer_inserted_before_the_file_it_includes_counts() {
    let graph = graph(&[file("user.cpp", &["late.h"]), file("late.h", &[])]);

    let dependents = graph.dependents("late.h", 1, 0).expect("held");

    assert_eq!(found(&dependents), [("user.cpp".to_owned(), 1)]);
}

#[test]
fn a_file_replaced_or_removed_stops_counting_as_an_includer() {
    let mut graph = tree();

    graph.insert(&file("top.cpp", &["mid.h"]));
    graph.remove("a/direct.cpp");
    let dependents = graph.dependents("base.h", 1, 0).expect("held");

    assert_eq!(found(&dependents), [("z/direct.h".to_owned(), 1)]);
}
