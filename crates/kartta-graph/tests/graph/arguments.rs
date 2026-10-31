//! What a call's count of arguments binds, in each path a call's candidates come by.

use std::sync::Arc;

use kartta_graph::confidence::{FIRST_PARTY, PARENT_DIR, SAME_DIR, SAME_FILE};
use kartta_graph::{Graph, Limits};
use kartta_lang::{CallRef, Conventions, Extraction, Symbol, SymbolKind};

use crate::{call, file, function, graph, graph_of, member_call};

const fn taking(mut symbol: Symbol, most_arguments: u8) -> Symbol {
    symbol.most_arguments = most_arguments;
    symbol
}

const fn passing(mut call: CallRef, arguments: u8) -> CallRef {
    call.arguments = arguments;
    call
}

fn method(owner: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Method, line, line + 1);
    symbol.qualified_name = format!("{owner}.{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

fn calling(path: &str, caller: &str, calls: Vec<CallRef>) -> Extraction {
    let mut extraction = file(path, vec![function(caller, 1)]);
    extraction.calls = calls;
    extraction
}

fn callees(graph: &Graph, name: &str) -> Vec<String> {
    graph
        .callees(name, Limits::new(1))
        .expect("the name selects a symbol")
        .reached
        .iter()
        .map(|reached| format!("{}:{}", reached.symbol.file, reached.symbol.line))
        .collect()
}

fn unresolved(graph: &Graph, name: &str) -> usize {
    graph
        .callees(name, Limits::new(1))
        .expect("the name selects a symbol")
        .unresolved
}

#[test]
fn a_call_binds_no_definition_that_takes_fewer_than_it_passes_and_counts_as_unresolved() {
    let mut extraction = file(
        "a.das",
        vec![
            taking(function("two", 1), 2),
            function("over", 3),
            function("fits", 5),
        ],
    );
    extraction.calls = vec![passing(call(1, "two", 4), 3), passing(call(2, "two", 6), 2)];

    let graph = graph(&[extraction]);

    assert_eq!(callees(&graph, "over"), Vec::<String>::new());
    assert_eq!(unresolved(&graph, "over"), 1);
    assert_eq!(callees(&graph, "fits"), ["a.das:1"]);
}

#[test]
fn a_side_that_counts_nothing_turns_nothing_away() {
    let definitions = file(
        "a.das",
        vec![taking(function("none", 1), 0), function("any", 3)],
    );
    let caller = calling(
        "b.das",
        "run",
        vec![call(0, "none", 2), passing(call(0, "any", 3), u8::MAX)],
    );

    assert_eq!(
        callees(&graph(&[definitions, caller]), "run"),
        ["a.das:1", "a.das:3"]
    );
}

#[test]
fn a_method_takes_the_object_of_a_member_call_beside_its_parameters() {
    let definitions = file("a.das", vec![taking(method("Widget", "resize", 1), 1)]);
    let fits = calling(
        "b.das",
        "fits",
        vec![passing(member_call("Widget", "resize", 2), 2)],
    );
    let over = calling(
        "c.das",
        "over",
        vec![passing(member_call("Widget", "resize", 2), 3)],
    );

    let graph = graph(&[definitions, fits, over]);

    assert_eq!(callees(&graph, "fits"), ["a.das:1"]);
    assert_eq!(callees(&graph, "over"), Vec::<String>::new());
}

#[test]
fn a_method_that_takes_any_number_takes_the_most_a_member_call_can_pass() {
    let definitions = file("a.das", vec![method("Widget", "resize", 1)]);
    let caller = calling(
        "b.das",
        "run",
        vec![passing(member_call("Widget", "resize", 2), u8::MAX)],
    );

    assert_eq!(callees(&graph(&[definitions, caller]), "run"), ["a.das:1"]);
}

#[test]
fn a_free_function_takes_no_object_beside_its_parameters() {
    let definitions = file("a.das", vec![taking(function("scale", 1), 1)]);
    let caller = calling(
        "b.das",
        "run",
        vec![passing(member_call("widget", "scale", 2), 2)],
    );

    assert_eq!(
        callees(&graph(&[definitions, caller]), "run"),
        Vec::<String>::new()
    );
}

#[test]
fn a_name_whose_definitions_the_arguments_turned_away_binds_no_guess() {
    let fits = file("y/q/two.das", vec![taking(function("cvt", 1), 2)]);
    let short = file("z/r/one.das", vec![taking(function("cvt", 1), 1)]);
    let caller = calling("x/p/a.das", "run", vec![passing(call(0, "cvt", 2), 2)]);

    assert_eq!(
        callees(&graph(&[fits, short, caller]), "run"),
        Vec::<String>::new()
    );
}

fn confidences(graph: &Graph, name: &str) -> Vec<u8> {
    graph
        .callees(name, Limits::new(1))
        .expect("the name selects a symbol")
        .reached
        .iter()
        .map(|reached| reached.confidence)
        .collect()
}

#[test]
fn the_nearest_definition_of_a_name_the_arguments_narrowed_binds_as_a_guess() {
    for (path, nearest) in [("x/p/two.das", SAME_DIR), ("x/q/two.das", PARENT_DIR)] {
        let fits = file(path, vec![taking(function("cvt", 1), 2)]);
        let also = file("z/s/two.das", vec![taking(function("cvt", 1), 2)]);
        let short = file("z/r/one.das", vec![taking(function("cvt", 1), 1)]);
        let caller = calling("x/p/a.das", "run", vec![passing(call(0, "cvt", 2), 2)]);
        let narrowed = [fits.clone(), also.clone(), short, caller.clone()];

        assert_eq!(confidences(&graph(&[fits, also, caller]), "run"), [nearest]);
        assert_eq!(confidences(&graph(&narrowed), "run"), [FIRST_PARTY]);
    }
}

#[test]
fn the_definition_scope_binds_keeps_its_tier_where_the_arguments_narrowed_the_name() {
    let extraction = |defined_by: &str| {
        let mut extraction = file(
            defined_by,
            vec![
                taking(function("cvt", 1), 1),
                taking(function("cvt", 2), 2),
                function("run", 3),
            ],
        );
        extraction.calls = vec![passing(call(2, "cvt", 4), 2)];
        extraction
    };

    let graph = graph(&[extraction("a.das")]);

    assert_eq!(callees(&graph, "run"), ["a.das:2"]);
    assert_eq!(confidences(&graph, "run"), [SAME_FILE]);
}

#[test]
fn a_member_call_passes_an_object_no_namespace_takes_though_a_bare_call_of_its_name_binds_it() {
    let module = file(
        "z/r/log.das",
        vec![Symbol::new("log", SymbolKind::Namespace, 1, 9)],
    );
    let object = calling(
        "x/p/a.das",
        "run",
        vec![passing(member_call("t", "log", 2), 2)],
    );
    let bare = calling("x/p/a.das", "run", vec![passing(call(0, "log", 2), 1)]);

    assert_eq!(
        callees(&graph(&[module.clone(), object]), "run"),
        Vec::<String>::new()
    );
    assert_eq!(callees(&graph(&[module, bare]), "run"), ["z/r/log.das:1"]);
}

#[test]
fn a_name_with_enough_definitions_to_share_an_index_binds_no_guess_either() {
    let fits = file("y/q/two.das", vec![taking(function("cvt", 1), 2)]);
    let short = (0..64).map(|at| {
        file(
            &format!("z/r/{at}.das"),
            vec![taking(function("cvt", 1), 1)],
        )
    });
    let caller = calling("x/p/a.das", "run", vec![passing(call(0, "cvt", 2), 2)]);
    let files: Vec<Extraction> = [fits, caller].into_iter().chain(short).collect();

    assert_eq!(callees(&graph(&files), "run"), Vec::<String>::new());
}

#[test]
fn a_gate_keeps_what_the_arguments_turned_away() {
    let in_package = |mut extraction: Extraction, package: &str, dependencies: &str| {
        extraction.package = Some(package.to_owned());
        extraction.dependencies = Some(Arc::from(dependencies));
        extraction
    };
    let fits = in_package(
        file("lib/q/two.das", vec![taking(function("cvt", 1), 2)]),
        "lib",
        "",
    );
    let short = (0..64).map(|at| {
        in_package(
            file(
                &format!("lib/r/{at}.das"),
                vec![taking(function("cvt", 1), 1)],
            ),
            "lib",
            "",
        )
    });
    let caller = in_package(
        calling("app/p/a.das", "run", vec![passing(call(0, "cvt", 2), 2)]),
        "app",
        "lib",
    );
    let files: Vec<Extraction> = [fits, caller].into_iter().chain(short).collect();

    assert_eq!(
        callees(&graph_of(Conventions::default(), &files), "run"),
        Vec::<String>::new()
    );
}

#[test]
fn a_shared_index_holds_a_nearby_definition_for_the_count_it_takes_alone() {
    let nearby = file("x/def.das", vec![taking(function("stat", 1), 1)]);
    let others = (0..64).map(|at| {
        file(
            &format!("z/r/{at}.das"),
            vec![taking(function("stat", 1), 1)],
        )
    });
    let fits = calling("x/a.das", "fits", vec![passing(call(0, "stat", 2), 1)]);
    let over = calling("x/b.das", "over", vec![passing(call(0, "stat", 2), 2)]);
    let both = calling(
        "x/c.das",
        "both",
        vec![
            passing(call(0, "stat", 2), 2),
            passing(call(0, "stat", 3), 1),
        ],
    );
    let files: Vec<Extraction> = [nearby, fits, over, both]
        .into_iter()
        .chain(others)
        .collect();
    let graph = graph(&files);

    assert_eq!(callees(&graph, "fits"), ["x/def.das:1"]);
    assert_eq!(callees(&graph, "over"), Vec::<String>::new());
    assert_eq!(callees(&graph, "both"), ["x/def.das:1"]);
}

#[test]
fn a_shared_index_holds_a_nearby_method_for_a_call_that_passes_it_an_object_alone() {
    let nearby = file("x/def.das", vec![taking(method("Widget", "resize", 1), 1)]);
    let others = (0..64).map(|at| {
        file(
            &format!("z/r/{at}.das"),
            vec![taking(function("resize", 1), 2)],
        )
    });
    let bare = calling("x/a.das", "bare", vec![passing(call(0, "resize", 2), 2)]);
    let member = calling(
        "x/b.das",
        "member",
        vec![passing(member_call("widget", "resize", 2), 2)],
    );
    let both = calling(
        "x/c.das",
        "both",
        vec![
            passing(call(0, "resize", 2), 2),
            passing(member_call("widget", "resize", 3), 2),
        ],
    );
    let files: Vec<Extraction> = [nearby, bare, member, both]
        .into_iter()
        .chain(others)
        .collect();
    let graph = graph(&files);

    assert_eq!(callees(&graph, "bare"), Vec::<String>::new());
    assert_eq!(callees(&graph, "member"), ["x/def.das:1"]);
    assert_eq!(callees(&graph, "both"), ["x/def.das:1"]);
}
