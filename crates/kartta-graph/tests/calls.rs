//! Call resolution through the public queries: each tier of the cascade, and what each query answers and leaves out.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the query helpers \
              of an integration test crate are neither; a name that selects nothing is the failure"
)]

use kartta_graph::{Graph, Limits, Walk};
use kartta_lang::{
    Access, CallRef, Callable, CallableRole, CallsOnValues, Conventions, Extraction, ImportBinding,
    IncludeReach, IncludeRef, InheritedMembers, LocalCall, MemberCalls, OwnMembers, ReExport,
    Receiver, RefSite, Symbol, SymbolFlags, SymbolId, SymbolKind,
};

fn function(name: &str, line: u32) -> Symbol {
    Symbol::new(name, SymbolKind::Function, line, line + 1)
}

fn member(owner: &str, name: &str, kind: SymbolKind, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, kind, line, line + 1);
    symbol.qualified_name = format!("{owner}::{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

const fn exported(mut symbol: Symbol) -> Symbol {
    symbol.exported = true;
    symbol
}

fn call(from: u32, callee: &str, line: u32) -> CallRef {
    CallRef {
        caller_id: SymbolId(from),
        callee: callee.to_owned(),
        receiver: None,
        line,
    }
}

fn member_call(from: u32, receiver: &str, callee: &str, line: u32) -> CallRef {
    CallRef {
        receiver: Some(receiver.to_owned()),
        ..call(from, callee, line)
    }
}

fn file(path: &str, symbols: Vec<Symbol>, calls: Vec<CallRef>) -> Extraction {
    let mut extraction = Extraction::default();
    path.clone_into(&mut extraction.file);
    extraction.symbols = symbols;
    extraction.calls = calls;
    extraction
}

fn graph_of(conventions: Conventions, files: &[Extraction]) -> Graph {
    let mut graph = Graph::new(conventions);
    for extraction in files {
        graph.insert(extraction);
    }
    graph
}

fn graph(files: &[Extraction]) -> Graph {
    graph_of(Conventions::default(), files)
}

fn go() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.directory_packages = true;
    conventions
}

fn cpp() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::First;
    conventions.inherited_members = InheritedMembers::Found;
    conventions.values_are_call_targets = false;
    conventions.scope_separator = Some("::");
    conventions.include_reach = IncludeReach::Transitive;
    conventions.calls_on_values = CallsOnValues::NoNearestGuess;
    conventions
}

fn callers(graph: &Graph, name: &str) -> Vec<String> {
    let walk = graph
        .callers(name, Limits::new(1))
        .expect("the name selects a symbol");
    reached(&walk)
}

fn reached(walk: &Walk<'_>) -> Vec<String> {
    walk.reached
        .iter()
        .map(|reached| {
            format!(
                "{}:{}@{}",
                reached.symbol.file, reached.symbol.line, reached.confidence
            )
        })
        .collect()
}

fn callees(graph: &Graph, name: &str) -> Vec<String> {
    let walk = graph
        .callees(name, Limits::new(1))
        .expect("the name selects a symbol");
    reached(&walk)
}

#[test]
fn a_local_call_binds_the_local_its_scope_declares() {
    let mut extraction = file(
        "a.go",
        vec![
            function("outer", 1),
            Symbol::new("helper", SymbolKind::Variable, 2, 2),
        ],
        Vec::new(),
    );
    extraction.local_symbols = vec![SymbolId(1)];
    extraction.local_calls = vec![LocalCall {
        caller_id: SymbolId(0),
        local: SymbolId(1),
        line: 3,
    }];
    let elsewhere = file("b.go", vec![function("helper", 1)], Vec::new());

    let graph = graph(&[extraction, elsewhere]);

    assert_eq!(callees(&graph, "outer"), ["a.go:2@80"]);
}

#[test]
fn a_local_that_is_no_call_target_binds_no_call() {
    let mut extraction = file(
        "a.cpp",
        vec![
            function("outer", 1),
            Symbol::new("helper", SymbolKind::Variable, 2, 2),
        ],
        Vec::new(),
    );
    extraction.local_symbols = vec![SymbolId(1)];
    extraction.local_calls = vec![LocalCall {
        caller_id: SymbolId(0),
        local: SymbolId(1),
        line: 3,
    }];

    let graph = graph_of(cpp(), &[extraction]);

    assert_eq!(callees(&graph, "outer"), Vec::<String>::new());
}

fn importing(
    path: &str,
    alias: Option<&str>,
    target_dir: Option<&str>,
    calls: Vec<CallRef>,
) -> Extraction {
    let mut extraction = file(path, vec![function("main", 1)], calls);
    extraction.package = Some("main".to_owned());
    extraction.bindings = vec![ImportBinding {
        alias: alias.map(str::to_owned),
        target_dir: target_dir.map(str::to_owned),
        last_segment: "lib".to_owned(),
    }];
    extraction
}

fn package(path: &str, name: &str, symbols: Vec<Symbol>) -> Extraction {
    let mut extraction = file(path, symbols, Vec::new());
    extraction.package = Some(name.to_owned());
    extraction
}

#[test]
fn a_receiver_that_names_an_import_binds_the_export_of_the_imported_directory() {
    let graph = graph_of(
        go(),
        &[
            importing(
                "cmd/main.go",
                None,
                Some("pkg/lib"),
                vec![member_call(0, "lib", "Parse", 2)],
            ),
            package(
                "pkg/lib/parse.go",
                "lib",
                vec![exported(function("Parse", 1))],
            ),
            package("cmd/parse.go", "main", vec![function("Parse", 1)]),
        ],
    );

    assert_eq!(callees(&graph, "main"), ["pkg/lib/parse.go:1@95"]);
}

#[test]
fn an_import_binding_stops_the_cascade_where_it_binds_nothing() {
    let unexported = graph_of(
        go(),
        &[
            importing(
                "cmd/main.go",
                None,
                Some("pkg/lib"),
                vec![member_call(0, "lib", "parse", 2)],
            ),
            package("pkg/lib/parse.go", "lib", vec![function("parse", 1)]),
        ],
    );
    let external = graph_of(
        go(),
        &[
            importing(
                "cmd/main.go",
                Some("fmt"),
                None,
                vec![member_call(0, "fmt", "Errorf", 2)],
            ),
            package(
                "cmd/errors.go",
                "main",
                vec![exported(function("Errorf", 1))],
            ),
        ],
    );

    assert_eq!(callees(&unexported, "main"), Vec::<String>::new());
    assert_eq!(callees(&external, "main"), Vec::<String>::new());
}

#[test]
fn an_import_without_an_alias_binds_the_package_name_its_directory_declares() {
    let graph = graph_of(
        go(),
        &[
            importing(
                "cmd/main.go",
                None,
                Some("pkg/v2"),
                vec![member_call(0, "lib", "Parse", 2)],
            ),
            package(
                "pkg/v2/parse.go",
                "lib",
                vec![exported(function("Parse", 1))],
            ),
        ],
    );

    assert_eq!(callees(&graph, "main"), ["pkg/v2/parse.go:1@95"]);
}

#[test]
fn a_qualified_callee_binds_the_symbol_of_that_qualified_name() {
    let graph = graph(&[
        file(
            "a.cpp",
            vec![function("run", 1)],
            vec![call(0, "Widget::draw", 2)],
        ),
        file(
            "w.cpp",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "x.cpp",
            vec![member("Other", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&graph, "run"), ["w.cpp:1@90"]);
}

#[test]
fn a_qualified_callee_two_files_define_binds_the_first_in_path_order() {
    let widget = || vec![member("Widget", "draw", SymbolKind::Method, 1)];
    let graph = graph(&[
        file("b.cpp", widget(), Vec::new()),
        file("a.cpp", widget(), Vec::new()),
        file(
            "main.cpp",
            vec![function("run", 1)],
            vec![call(0, "Widget::draw", 2)],
        ),
    ]);

    assert_eq!(callees(&graph, "run"), ["a.cpp:1@90"]);
}

#[test]
fn a_callee_written_with_more_scopes_binds_its_trailing_owner_and_name() {
    let files = [
        file(
            "a.cpp",
            vec![function("run", 1)],
            vec![call(0, "ns::inner::Widget::draw", 2)],
        ),
        file(
            "w.cpp",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "x.cpp",
            vec![member("Other", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ];

    let scoped = graph_of(cpp(), &files);
    let unscoped = graph(&files);

    assert_eq!(callees(&scoped, "run"), ["w.cpp:1@88"]);
    assert_eq!(callees(&unscoped, "run"), Vec::<String>::new());
}

fn nested(scope: &str, owner: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = member(owner, name, SymbolKind::Struct, line);
    symbol.qualified_name = format!("{scope}::{owner}::{name}");
    symbol
}

#[test]
fn a_callee_written_from_the_owner_binds_a_member_of_a_longer_qualified_name() {
    let files = [
        including("a.cpp", &["w2.h"], vec![call(0, "Widget::Kind", 2)]),
        file("o.h", vec![nested("gfx", "Other", "Kind", 1)], Vec::new()),
        file("w1.h", vec![nested("gfx", "Widget", "Kind", 1)], Vec::new()),
        file("w2.h", vec![nested("ui", "Widget", "Kind", 1)], Vec::new()),
        file(
            "z.cpp",
            vec![function("go", 1)],
            vec![call(0, "Widget::Kind", 2)],
        ),
    ];

    let scoped = graph_of(cpp(), &files);
    let unscoped = graph(&files);

    assert_eq!(callees(&scoped, "run"), ["w2.h:1@88"]);
    assert_eq!(callees(&scoped, "go"), ["w1.h:1@88"]);
    assert_eq!(callees(&unscoped, "run"), Vec::<String>::new());
}

#[test]
fn a_bare_call_binds_its_own_file_before_any_other() {
    let graph = graph(&[
        file(
            "a.go",
            vec![function("run", 1), function("parse", 5)],
            vec![call(0, "parse", 2)],
        ),
        file("b.go", vec![function("parse", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["a.go:5@80"]);
}

#[test]
fn a_bare_call_binds_its_package_directory_outside_test_code() {
    let mut in_test = function("parse", 1);
    in_test.flags = SymbolFlags::empty().with_test(true);
    let graph = graph_of(
        go(),
        &[
            package("app/a_test.go", "app", vec![in_test]),
            package("app/main.go", "app", vec![function("run", 1)]),
            package("app/parse.go", "app", vec![function("parse", 1)]),
            package("lib/parse.go", "lib", vec![function("parse", 1)]),
        ],
    );
    let mut graph = graph;
    let mut caller = package("app/main.go", "app", vec![function("run", 1)]);
    caller.calls = vec![call(0, "parse", 2)];
    graph.insert(&caller);

    assert_eq!(callees(&graph, "run"), ["app/parse.go:1@75"]);
}

#[test]
fn a_dot_import_binds_its_one_export_of_the_name() {
    let graph = graph_of(
        go(),
        &[
            importing(
                "cmd/main.go",
                Some("."),
                Some("pkg/lib"),
                vec![call(0, "Parse", 2)],
            ),
            package(
                "pkg/lib/parse.go",
                "lib",
                vec![exported(function("Parse", 1))],
            ),
            package(
                "other/parse.go",
                "other",
                vec![exported(function("Parse", 1))],
            ),
        ],
    );

    assert_eq!(callees(&graph, "main"), ["pkg/lib/parse.go:1@95"]);
}

#[test]
fn a_dot_import_binds_before_the_package_of_the_caller_s_directory() {
    let mut external_test = package("app/app_test.go", "app_test", vec![function("TestRun", 1)]);
    external_test.bindings = vec![ImportBinding {
        alias: Some(".".to_owned()),
        target_dir: Some("lib".to_owned()),
        last_segment: "lib".to_owned(),
    }];
    external_test.calls = vec![call(0, "Helper", 2)];
    let graph = graph_of(
        go(),
        &[
            external_test,
            package(
                "app/helper.go",
                "app",
                vec![exported(function("Helper", 1))],
            ),
            package(
                "lib/helper.go",
                "lib",
                vec![exported(function("Helper", 1))],
            ),
        ],
    );

    assert_eq!(callees(&graph, "TestRun"), ["lib/helper.go:1@95"]);
}

fn including(path: &str, targets: &[&str], calls: Vec<CallRef>) -> Extraction {
    let mut extraction = file(path, vec![function("run", 1)], calls);
    extraction.includes = targets
        .iter()
        .map(|target| IncludeRef {
            target: (*target).to_owned(),
        })
        .collect();
    extraction
}

#[test]
fn a_bare_call_binds_the_one_included_file_that_defines_the_name() {
    let graph = graph(&[
        including(
            "src/a.cpp",
            &["inc/parse.h", "inc/other.h"],
            vec![call(0, "parse", 2)],
        ),
        file("inc/parse.h", vec![function("parse", 1)], Vec::new()),
        file("inc/other.h", vec![function("unrelated", 1)], Vec::new()),
        file("lib/parse.cpp", vec![function("parse", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["inc/parse.h:1@70"]);
}

#[test]
fn two_included_files_that_define_the_name_leave_the_call_to_later_tiers() {
    let graph = graph(&[
        including(
            "src/a.cpp",
            &["inc/one.h", "inc/two.h"],
            vec![call(0, "parse", 2)],
        ),
        file("inc/one.h", vec![function("parse", 1)], Vec::new()),
        file("inc/two.h", vec![function("parse", 1)], Vec::new()),
    ]);

    let walk = graph
        .callees("run", Limits::new(1))
        .expect("run is defined");

    assert_eq!(reached(&walk), Vec::<String>::new());
    assert_eq!(walk.unresolved, 1);
}

#[test]
fn an_included_file_that_defines_the_name_on_two_owners_leaves_the_call_to_later_tiers() {
    let graph = graph(&[
        including(
            "src/saver.cpp",
            &["inc/log.h", "inc/file.h"],
            vec![member_call(0, "", "close", 2)],
        ),
        file(
            "inc/log.h",
            vec![
                member("Log", "close", SymbolKind::Method, 1),
                member("Grpc", "close", SymbolKind::Method, 2),
            ],
            Vec::new(),
        ),
        file(
            "inc/file.h",
            vec![member("File", "close", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    let walk = graph
        .callees("run", Limits::new(1))
        .expect("run is defined");

    assert_eq!(reached(&walk), Vec::<String>::new());
    assert_eq!(walk.unresolved, 1);
}

fn header(path: &str, symbols: Vec<Symbol>, includes: &[&str]) -> Extraction {
    let mut extraction = file(path, symbols, Vec::new());
    extraction.includes = includes
        .iter()
        .map(|target| IncludeRef {
            target: (*target).to_owned(),
        })
        .collect();
    extraction
}

fn includes_through_headers(called: CallRef, defined: Symbol) -> [Extraction; 5] {
    [
        including("src/a.cpp", &["inc/scene.h"], vec![called]),
        header("inc/scene.h", Vec::new(), &["inc/core.h"]),
        header("inc/core.h", vec![defined.clone()], &["inc/scene.h"]),
        file("copy/core.h", vec![defined.clone()], Vec::new()),
        file("lib/parse.cpp", vec![defined], Vec::new()),
    ]
}

#[test]
fn a_bare_call_binds_the_one_file_its_includes_include_that_defines_the_name() {
    let graph = graph_of(
        cpp(),
        &includes_through_headers(call(0, "parse", 2), function("parse", 1)),
    );

    assert_eq!(callees(&graph, "run"), ["inc/core.h:1@70"]);
}

#[test]
fn a_file_that_an_include_includes_defines_nothing_the_caller_sees_where_includes_are_direct() {
    let graph = graph(&includes_through_headers(
        call(0, "parse", 2),
        function("parse", 1),
    ));

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_member_call_on_a_value_binds_the_one_member_its_includes_include() {
    let graph = graph_of(
        cpp(),
        &includes_through_headers(
            member_call(0, "", "parse", 2),
            member("Core", "parse", SymbolKind::Method, 1),
        ),
    );

    assert_eq!(callees(&graph, "run"), ["inc/core.h:1@70"]);
}

#[test]
fn a_member_call_on_a_value_binds_no_free_function_its_includes_include() {
    let graph = graph_of(
        cpp(),
        &includes_through_headers(member_call(0, "", "parse", 2), function("parse", 1)),
    );

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_name_more_than_32_definitions_share_binds_through_direct_includes_alone() {
    let mut files = includes_through_headers(call(0, "parse", 2), function("parse", 1)).to_vec();
    files.push(file(
        "lib/many.cpp",
        (0..30)
            .map(|line| function("parse", line * 2 + 1))
            .collect(),
        Vec::new(),
    ));
    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_name_32_definitions_share_binds_through_the_files_includes_include() {
    let mut files = includes_through_headers(call(0, "parse", 2), function("parse", 1)).to_vec();
    files.push(file(
        "lib/many.cpp",
        (0..29)
            .map(|line| function("parse", line * 2 + 1))
            .collect(),
        Vec::new(),
    ));
    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "run"), ["inc/core.h:1@70"]);
}

#[test]
fn a_directly_included_definition_binds_before_one_included_further_in() {
    let graph = graph_of(
        cpp(),
        &[
            including(
                "src/a.cpp",
                &["inc/near.h", "inc/scene.h"],
                vec![call(0, "parse", 2)],
            ),
            header("inc/near.h", vec![function("parse", 1)], &[]),
            header("inc/scene.h", Vec::new(), &["inc/core.h"]),
            header("inc/core.h", vec![function("parse", 1)], &[]),
        ],
    );

    assert_eq!(callees(&graph, "run"), ["inc/near.h:1@70"]);
}

#[test]
fn two_files_included_further_in_that_define_the_name_leave_the_call_to_later_tiers() {
    let graph = graph_of(
        cpp(),
        &[
            including("src/a.cpp", &["inc/scene.h"], vec![call(0, "parse", 2)]),
            header("inc/scene.h", Vec::new(), &["inc/one.h", "inc/two.h"]),
            header("inc/one.h", vec![function("parse", 1)], &[]),
            header("inc/two.h", vec![function("parse", 1)], &[]),
        ],
    );

    let walk = graph
        .callees("run", Limits::new(1))
        .expect("run is defined");

    assert_eq!(reached(&walk), Vec::<String>::new());
    assert_eq!(walk.unresolved, 1);
}

#[test]
fn a_re_export_chain_binds_the_renamed_definition() {
    let mut barrel = file("ui/index.ts", Vec::new(), Vec::new());
    barrel.re_exports = vec![ReExport {
        local_name: "Button".to_owned(),
        source_name: "BaseButton".to_owned(),
        source_path: "./button".to_owned(),
        source_file: Some("ui/button.ts".to_owned()),
    }];
    let graph = graph(&[
        including("app/main.ts", &["ui/index.ts"], vec![call(0, "Button", 2)]),
        barrel,
        file("ui/button.ts", vec![function("BaseButton", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["ui/button.ts:1@65"]);
    assert_eq!(callers(&graph, "BaseButton"), ["app/main.ts:1@65"]);
}

#[test]
fn a_caller_reaches_its_callee_through_every_rename_of_a_re_export_chain() {
    let graph = graph(&[
        including("app/main.ts", &["ui/a.ts"], vec![call(0, "E", 2)]),
        barrel("ui/a.ts", vec![re_export("E", "D", "ui/b.ts")]),
        barrel("ui/b.ts", vec![re_export("D", "C", "ui/c.ts")]),
        barrel("ui/c.ts", vec![re_export("C", "B", "ui/d.ts")]),
        barrel("ui/d.ts", vec![re_export("B", "A", "ui/base.ts")]),
        file("ui/base.ts", vec![function("A", 1)], Vec::new()),
    ]);

    assert_eq!(callers(&graph, "A"), ["app/main.ts:1@65"]);
}

fn re_export(local_name: &str, source_name: &str, source_file: &str) -> ReExport {
    ReExport {
        local_name: local_name.to_owned(),
        source_name: source_name.to_owned(),
        source_path: String::new(),
        source_file: Some(source_file.to_owned()),
    }
}

fn barrel(path: &str, re_exports: Vec<ReExport>) -> Extraction {
    let mut barrel = file(path, Vec::new(), Vec::new());
    barrel.re_exports = re_exports;
    barrel
}

#[test]
fn a_re_export_chain_binds_through_every_barrel_it_passes() {
    let graph = graph(&[
        including("app/main.ts", &["ui/index.ts"], vec![call(0, "Button", 2)]),
        barrel(
            "ui/index.ts",
            vec![re_export("Button", "Button", "ui/inner.ts")],
        ),
        barrel(
            "ui/inner.ts",
            vec![re_export("Button", "Base", "ui/base.ts")],
        ),
        file("ui/base.ts", vec![function("Base", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["ui/base.ts:1@65"]);
}

#[test]
fn a_re_export_chain_binds_however_many_barrels_rename_it() {
    let graph = graph(&[
        including("app/main.ts", &["ui/a.ts"], vec![call(0, "E", 2)]),
        barrel("ui/a.ts", vec![re_export("E", "D", "ui/b.ts")]),
        barrel("ui/b.ts", vec![re_export("D", "C", "ui/c.ts")]),
        barrel("ui/c.ts", vec![re_export("C", "B", "ui/d.ts")]),
        barrel("ui/d.ts", vec![re_export("B", "A", "ui/base.ts")]),
        file("ui/base.ts", vec![function("A", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["ui/base.ts:1@65"]);
}

#[test]
fn a_barrel_reached_under_one_name_is_searched_again_under_another() {
    let graph = graph(&[
        including(
            "app/main.ts",
            &["ui/a.ts", "ui/b.ts"],
            vec![call(0, "Y", 2)],
        ),
        barrel("ui/a.ts", vec![re_export("Y", "Missing", "ui/mid.ts")]),
        barrel("ui/b.ts", vec![re_export("*", "*", "ui/mid.ts")]),
        barrel("ui/mid.ts", vec![re_export("*", "*", "ui/src.ts")]),
        file("ui/src.ts", vec![function("Y", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["ui/src.ts:1@65"]);
}

#[test]
fn a_name_a_barrel_re_exports_by_name_is_not_looked_up_through_its_star_exports() {
    let graph = graph(&[
        including("app/main.ts", &["ui/index.ts"], vec![call(0, "Button", 2)]),
        barrel(
            "ui/index.ts",
            vec![
                re_export("Button", "Missing", "ui/x.ts"),
                re_export("*", "*", "ui/button.ts"),
            ],
        ),
        file("ui/x.ts", Vec::new(), Vec::new()),
        file("ui/button.ts", vec![function("Missing", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_call_whose_shape_admits_no_candidate_binds_nothing() {
    let graph = graph(&[
        file("a.go", vec![function("run", 1)], vec![call(0, "draw", 2)]),
        file(
            "b.go",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_member_call_with_no_member_of_the_name_takes_the_free_symbols() {
    let graph = graph(&[
        file(
            "a.go",
            vec![function("run", 1)],
            vec![member_call(0, "", "draw", 2)],
        ),
        file("b.go", vec![function("draw", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["b.go:1@60"]);
}

#[test]
fn a_receiver_that_names_an_owner_binds_before_the_caller_s_own_file() {
    let graph = graph(&[
        file(
            "a.go",
            vec![
                function("run", 1),
                member("Other", "draw", SymbolKind::Method, 5),
            ],
            vec![member_call(0, "Widget", "draw", 2)],
        ),
        file(
            "b.go",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);
    let alone = graph_of(
        Conventions::default(),
        &[
            file(
                "x/a.go",
                vec![function("run", 1)],
                vec![member_call(0, "Widget", "draw", 2)],
            ),
            file(
                "y/b.go",
                vec![member("Widget", "draw", SymbolKind::Method, 1)],
                Vec::new(),
            ),
        ],
    );

    assert_eq!(callees(&graph, "run"), ["b.go:1@85"]);
    assert_eq!(callees(&alone, "run"), ["y/b.go:1@85"]);
}

#[test]
fn a_receiver_that_names_an_owner_two_directories_declare_binds_the_one_beside_the_caller() {
    let put = || vec![member("Buffer", "put", SymbolKind::Method, 1)];
    let graph = graph(&[
        file("a/buffer.go", put(), Vec::new()),
        file("b/buffer.go", put(), Vec::new()),
        file(
            "b/write.go",
            vec![function("run", 1)],
            vec![member_call(0, "Buffer", "put", 2)],
        ),
    ]);

    assert_eq!(callees(&graph, "run"), ["b/buffer.go:1@85"]);
}

#[test]
fn a_bare_name_selects_a_callable_before_a_field_of_that_name() {
    let graph = graph(&[
        file(
            "a.go",
            vec![member("S", "draw", SymbolKind::Field, 1)],
            Vec::new(),
        ),
        file(
            "b.go",
            vec![member("T", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    let walk = graph
        .callees("draw", Limits::new(1))
        .expect("draw is defined");

    assert_eq!((walk.root.file, walk.passed_over), ("b.go", 0));
}

#[test]
fn a_receiver_that_names_an_owner_picks_that_owner_s_member() {
    let graph = graph(&[
        file(
            "a.go",
            vec![function("run", 1)],
            vec![member_call(0, "Widget", "draw", 2)],
        ),
        file(
            "b.go",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "c.go",
            vec![member("Other", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&graph, "run"), ["b.go:1@85"]);
}

#[test]
fn a_member_call_of_an_unknown_type_picks_a_member_of_the_caller_s_owner() {
    let mut caller = file(
        "a.go",
        vec![member("Widget", "run", SymbolKind::Method, 1)],
        vec![member_call(0, "w", "draw", 2)],
    );
    caller.symbols[0].qualified_name = "Widget.run".to_owned();
    let graph = graph(&[
        caller,
        file(
            "b.go",
            vec![member("Other", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "c.go",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&graph, "Widget.run"), ["c.go:1@80"]);
}

#[test]
fn the_one_member_of_its_name_on_the_caller_s_owner_binds_as_that_owner_s() {
    let mut caller = file(
        "a.go",
        vec![member("Widget", "run", SymbolKind::Method, 1)],
        vec![member_call(0, "w", "draw", 2)],
    );
    caller.symbols[0].qualified_name = "Widget.run".to_owned();
    let graph = graph(&[
        caller,
        file(
            "c.go",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&graph, "Widget.run"), ["c.go:1@80"]);
}

#[test]
fn a_name_that_selects_nothing_has_no_callees_and_no_callers() {
    let graph = graph(&[file("a.go", vec![function("run", 1)], Vec::new())]);

    assert!(graph.callees("missing", Limits::new(1)).is_none());
}

fn eighty_draws_each_in_its_own_directory() -> Vec<Extraction> {
    (0..80)
        .map(|at| {
            let owner = format!("T{at}");
            file(
                &format!("d{at}/draw.go"),
                vec![member(&owner, "draw", SymbolKind::Method, 1)],
                Vec::new(),
            )
        })
        .collect()
}

#[test]
fn a_name_with_many_definitions_resolves_as_one_with_few() {
    let mut files = eighty_draws_each_in_its_own_directory();
    files.push(file(
        "x/main.go",
        vec![function("run", 1)],
        vec![member_call(0, "T41", "draw", 2)],
    ));
    let mut caller = file(
        "d7/main.go",
        vec![member("T7", "run", SymbolKind::Method, 1)],
        Vec::new(),
    );
    caller.calls = vec![member_call(0, "w", "draw", 2)];
    files.push(caller);
    let mut near = file("d9/near.go", vec![function("near", 1)], Vec::new());
    near.calls = vec![member_call(0, "v", "draw", 2)];
    files.push(near);

    let graph = graph(&files);

    assert_eq!(callees(&graph, "run"), ["d41/draw.go:1@85"]);
    assert_eq!(callees(&graph, "T7::run"), ["d7/draw.go:1@80"]);
    assert_eq!(callees(&graph, "near"), ["d9/draw.go:1@50"]);
}

#[test]
fn a_local_of_a_name_with_many_definitions_binds_only_inside_its_holder() {
    let mut files = eighty_draws_each_in_its_own_directory();
    let mut holder = file(
        "y/holder.go",
        vec![
            member("Outer", "run", SymbolKind::Method, 1),
            member("Local", "draw", SymbolKind::Method, 3),
        ],
        vec![member_call(0, "x", "draw", 5)],
    );
    holder.local_symbols = vec![SymbolId(1)];
    files.push(holder);
    files.push(file(
        "z/other.go",
        vec![function("other", 1)],
        vec![member_call(0, "Local", "draw", 2)],
    ));

    let graph = graph(&files);

    assert_eq!(callees(&graph, "Outer::run"), ["y/holder.go:3@80"]);
    assert_eq!(callees(&graph, "other"), Vec::<String>::new());
}

#[test]
fn the_one_callable_of_a_name_binds_from_anywhere() {
    let graph = graph(&[
        file(
            "a/x.go",
            vec![function("run", 1)],
            vec![call(0, "parse", 2)],
        ),
        file("b/c/y.go", vec![function("parse", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), ["b/c/y.go:1@60"]);
}

#[test]
fn proximity_binds_the_one_callable_nearest_the_caller() {
    let same_dir = graph(&[
        file(
            "a/x.go",
            vec![function("run", 1)],
            vec![call(0, "parse", 2)],
        ),
        file("a/y.go", vec![function("parse", 1)], Vec::new()),
        file("b/y.go", vec![function("parse", 1)], Vec::new()),
    ]);
    let parent_dir = graph(&[
        file(
            "a/x/main.go",
            vec![function("run", 1)],
            vec![call(0, "parse", 2)],
        ),
        file("a/y/p.go", vec![function("parse", 1)], Vec::new()),
        file("b/y.go", vec![function("parse", 1)], Vec::new()),
    ]);
    let mut vendored = function("parse", 1);
    vendored.flags = SymbolFlags::empty().with_vendored(true);
    let first_party = graph(&[
        file(
            "a/x/main.go",
            vec![function("run", 1)],
            vec![call(0, "parse", 2)],
        ),
        file("vendor/y.go", vec![vendored], Vec::new()),
        file("b/y.go", vec![function("parse", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&same_dir, "run"), ["a/y.go:1@50"]);
    assert_eq!(callees(&parent_dir, "run"), ["a/y/p.go:1@30"]);
    assert_eq!(callees(&first_party, "run"), ["b/y.go:1@15"]);
}

fn members_alone() -> Conventions {
    let mut conventions = go();
    conventions.member_calls = MemberCalls::Members;
    conventions
}

const fn vendored(mut symbol: Symbol) -> Symbol {
    symbol.flags = SymbolFlags::empty().with_vendored(true);
    symbol
}

#[test]
fn a_member_call_takes_a_free_function_in_the_caller_s_directory_and_none_beyond_its_parent() {
    let near = graph(&[
        file(
            "a/x.js",
            vec![function("run", 1)],
            vec![member_call(0, "", "join", 2)],
        ),
        file("a/y.js", vec![function("join", 1)], Vec::new()),
        file("b/y.js", vec![function("join", 1)], Vec::new()),
    ]);
    let above = graph(&[
        file(
            "a/x/main.js",
            vec![function("run", 1)],
            vec![member_call(0, "", "join", 2)],
        ),
        file("a/y/p.js", vec![function("join", 1)], Vec::new()),
        file("b/y.js", vec![function("join", 1)], Vec::new()),
    ]);
    let far = |receiver: &str| {
        callees(
            &graph(&[
                file(
                    "a/x/main.js",
                    vec![function("run", 1)],
                    vec![member_call(0, receiver, "join", 2)],
                ),
                file(
                    "vendor/y.js",
                    vec![vendored(function("join", 1))],
                    Vec::new(),
                ),
                file("b/y.js", vec![function("join", 1)], Vec::new()),
            ]),
            "run",
        )
    };

    assert_eq!(callees(&near, "run"), ["a/y.js:1@50"]);
    assert_eq!(callees(&above, "run"), ["a/y/p.js:1@30"]);
    assert_eq!(far(""), Vec::<String>::new());
    assert_eq!(far("items"), Vec::<String>::new());
}

#[test]
fn a_member_call_takes_the_one_member_beyond_its_parent_directory() {
    let graph = graph(&[
        file(
            "a/x/main.js",
            vec![function("run", 1)],
            vec![member_call(0, "", "join", 2)],
        ),
        file(
            "vendor/y.js",
            vec![vendored(member("Url", "join", SymbolKind::Method, 1))],
            Vec::new(),
        ),
        file(
            "b/y.js",
            vec![member("Path", "join", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&graph, "run"), ["b/y.js:1@15"]);
}

#[test]
fn a_member_call_that_calls_members_alone_binds_no_free_function_at_any_tier() {
    let calling = |receiver: &str, mut caller_symbols: Vec<Symbol>, others: Vec<Extraction>| {
        caller_symbols.insert(0, function("run", 1));
        let mut files = vec![file(
            "a/x.go",
            caller_symbols,
            vec![member_call(0, receiver, "Join", 2)],
        )];
        files.extend(others);
        callees(&graph_of(members_alone(), &files), "run")
    };
    let elsewhere = |path: &str| file(path, vec![function("Join", 1)], Vec::new());

    for receiver in ["", "items"] {
        assert_eq!(
            calling(receiver, vec![function("Join", 5)], Vec::new()),
            Vec::<String>::new()
        );
        assert_eq!(
            calling(receiver, Vec::new(), vec![elsewhere("a/y.go")]),
            Vec::<String>::new()
        );
        assert_eq!(
            calling(
                receiver,
                Vec::new(),
                vec![elsewhere("a/y.go"), elsewhere("b/y.go")]
            ),
            Vec::<String>::new()
        );
    }
    assert_eq!(
        calling(
            "",
            Vec::new(),
            vec![file(
                "b/y.go",
                vec![member("Path", "Join", SymbolKind::Method, 1)],
                Vec::new()
            )]
        ),
        ["b/y.go:1@60"]
    );
}

#[test]
fn a_member_call_takes_the_one_member_near_the_caller() {
    let graph = graph(&[
        file(
            "a/x.js",
            vec![function("run", 1)],
            vec![member_call(0, "", "join", 2)],
        ),
        file(
            "a/y.js",
            vec![member("Path", "join", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "b/y.js",
            vec![member("Url", "join", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&graph, "run"), ["a/y.js:1@50"]);
}

#[test]
fn a_member_call_on_a_value_binds_no_nearby_member_where_the_language_guesses_none() {
    let mut conventions = Conventions::default();
    conventions.calls_on_values = CallsOnValues::NoNearestGuess;
    let caller = || {
        file(
            "a/x.cpp",
            vec![function("run", 1)],
            vec![member_call(0, "", "join", 2)],
        )
    };
    let nearby = graph_of(
        conventions,
        &[
            caller(),
            file(
                "a/y.cpp",
                vec![member("Path", "join", SymbolKind::Method, 1)],
                Vec::new(),
            ),
            file(
                "b/y.cpp",
                vec![member("Url", "join", SymbolKind::Method, 1)],
                Vec::new(),
            ),
        ],
    );
    let the_one = graph_of(
        conventions,
        &[
            caller(),
            file(
                "b/y.cpp",
                vec![member("Url", "join", SymbolKind::Method, 1)],
                Vec::new(),
            ),
        ],
    );

    assert_eq!(callees(&nearby, "run"), Vec::<String>::new());
    assert_eq!(callees(&the_one, "run"), ["b/y.cpp:1@60"]);
}

#[test]
fn a_member_call_binds_a_free_function_its_file_defines_or_the_one_of_its_name() {
    let same_file = graph(&[
        file(
            "a/x.js",
            vec![function("run", 1), function("join", 5)],
            vec![member_call(0, "", "join", 2)],
        ),
        file("a/y.js", vec![function("join", 1)], Vec::new()),
    ]);
    let the_one = graph(&[
        file(
            "a/x.js",
            vec![function("run", 1)],
            vec![member_call(0, "", "join", 2)],
        ),
        file("b/y.js", vec![function("join", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&same_file, "run"), ["a/x.js:5@80"]);
    assert_eq!(callees(&the_one, "run"), ["b/y.js:1@60"]);
}

#[test]
fn a_member_call_of_a_runtime_method_binds_a_member_and_no_free_function() {
    let mut conventions = Conventions::default();
    conventions.runtime_methods = &["push", "trim"];
    let calling = |site: CallRef, others: Vec<Extraction>| {
        let mut files = vec![file(
            "a/x.js",
            vec![function("run", 1), function("trim", 5)],
            vec![site],
        )];
        files.extend(others);
        callees(&graph_of(conventions, &files), "run")
    };
    let elsewhere = |path: &str, symbol: Symbol| file(path, vec![symbol], Vec::new());

    for receiver in ["", "line"] {
        assert_eq!(
            calling(member_call(0, receiver, "trim", 2), Vec::new()),
            Vec::<String>::new()
        );
        for path in ["a/y.js", "b/y.js"] {
            assert_eq!(
                calling(
                    member_call(0, receiver, "push", 2),
                    vec![elsewhere(path, function("push", 1))]
                ),
                Vec::<String>::new()
            );
        }
    }
    assert_eq!(
        calling(
            member_call(0, "", "push", 2),
            vec![elsewhere(
                "b/y.js",
                member("Stack", "push", SymbolKind::Method, 1)
            )]
        ),
        ["b/y.js:1@60"]
    );
    assert_eq!(calling(call(0, "trim", 2), Vec::new()), ["a/x.js:5@80"]);
    assert_eq!(
        calling(
            member_call(0, "", "join", 2),
            vec![elsewhere("a/y.js", function("join", 1))]
        ),
        ["a/y.js:1@60"]
    );
}

#[test]
fn a_member_call_of_a_runtime_method_binds_no_free_function_a_barrel_re_exports() {
    let mut conventions = Conventions::default();
    conventions.runtime_methods = &["trim"];
    let calling = |callee: &str| {
        let graph = graph_of(
            conventions,
            &[
                including(
                    "app/main.ts",
                    &["ui/index.ts"],
                    vec![member_call(0, "", callee, 2)],
                ),
                barrel("ui/index.ts", vec![re_export(callee, "base", "ui/base.ts")]),
                file("ui/base.ts", vec![function("base", 1)], Vec::new()),
            ],
        );
        callees(&graph, "run")
    };

    assert_eq!(calling("trim"), Vec::<String>::new());
    assert_eq!(calling("pad"), ["ui/base.ts:1@65"]);
}

#[test]
fn a_member_call_binds_no_free_value_and_a_bare_call_still_does() {
    let calling = |site: CallRef, elsewhere: SymbolKind| {
        callees(
            &graph(&[
                file(
                    "a/x.js",
                    vec![
                        function("run", 1),
                        Symbol::new("now", SymbolKind::Variable, 5, 5),
                    ],
                    vec![site],
                ),
                file(
                    "b/y.js",
                    vec![Symbol::new("max", elsewhere, 1, 1)],
                    Vec::new(),
                ),
            ]),
            "run",
        )
    };

    assert_eq!(
        calling(member_call(0, "Date", "now", 2), SymbolKind::Constant),
        Vec::<String>::new()
    );
    assert_eq!(
        calling(member_call(0, "", "now", 2), SymbolKind::Constant),
        Vec::<String>::new()
    );
    assert_eq!(
        calling(member_call(0, "Math", "max", 2), SymbolKind::Constant),
        Vec::<String>::new()
    );
    assert_eq!(
        calling(member_call(0, "Math", "max", 2), SymbolKind::Function),
        ["b/y.js:1@60"]
    );
    assert_eq!(
        calling(call(0, "now", 2), SymbolKind::Constant),
        ["a/x.js:5@80"]
    );
}

#[test]
fn several_callables_of_a_name_with_nothing_to_tell_them_apart_bind_nothing() {
    let graph = graph(&[
        file(
            "a/x.go",
            vec![function("run", 1)],
            vec![call(0, "parse", 2)],
        ),
        file("b/y.go", vec![function("parse", 1)], Vec::new()),
        file("c/z.go", vec![function("parse", 1)], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_call_written_from_the_root_binds_a_global_symbol_and_no_member() {
    let graph_calling = |callee: &str| {
        graph_of(
            cpp(),
            &[file(
                "a.cpp",
                vec![
                    member("Widget", "run", SymbolKind::Method, 1),
                    member("Widget", "draw", SymbolKind::Method, 5),
                    function("draw", 9),
                ],
                vec![call(0, callee, 2)],
            )],
        )
    };

    assert_eq!(
        callees(&graph_calling("::draw"), "Widget::run"),
        ["a.cpp:9@80"]
    );
    assert_eq!(
        callees(&graph_calling("draw"), "Widget::run"),
        ["a.cpp:5@80"]
    );
}

#[test]
fn a_name_that_is_also_a_member_is_ambiguous_to_the_counting_tiers() {
    let graph = graph_of(
        cpp(),
        &[
            file(
                "a.cpp",
                vec![
                    member("Widget", "run", SymbolKind::Method, 1),
                    member("Widget", "draw", SymbolKind::Method, 5),
                ],
                vec![call(0, "::draw", 2)],
            ),
            file("b.cpp", vec![function("draw", 1)], Vec::new()),
        ],
    );

    assert_eq!(callees(&graph, "Widget::run"), Vec::<String>::new());
}

#[test]
fn a_bare_call_inside_a_member_reaches_its_own_type_first_under_an_implicit_receiver() {
    let mut class = Symbol::new("Widget", SymbolKind::Class, 1, 20);
    class.qualified_name = "Widget".to_owned();
    let graph_with = |conventions| {
        graph_of(
            conventions,
            &[file(
                "w.cpp",
                vec![
                    class.clone(),
                    member("Widget", "run", SymbolKind::Method, 2),
                    member("Widget", "draw", SymbolKind::Method, 5),
                    function("draw", 30),
                ],
                vec![call(1, "draw", 3)],
            )],
        )
    };

    assert_eq!(callees(&graph_with(cpp()), "Widget::run"), ["w.cpp:5@80"]);
    assert_eq!(
        callees(&graph_with(Conventions::default()), "Widget::run"),
        ["w.cpp:30@80"]
    );
}

#[test]
fn a_bare_call_inside_a_member_reaches_a_member_of_a_base_the_lookup_finds() {
    let mut base = Symbol::new("Base", SymbolKind::Class, 1, 10);
    base.qualified_name = "Base".to_owned();
    let mut derived = Symbol::new("Derived", SymbolKind::Class, 1, 10);
    derived.parents = vec!["Base".to_owned()];
    let graph = graph_of(
        cpp(),
        &[
            including("d.cpp", &["base.h"], Vec::new()),
            file(
                "base.h",
                vec![base, member("Base", "draw", SymbolKind::Method, 2)],
                Vec::new(),
            ),
            file(
                "derived.h",
                vec![derived, member("Derived", "run", SymbolKind::Method, 2)],
                vec![call(1, "draw", 3)],
            ),
            file(
                "other.cpp",
                vec![member("Other", "draw", SymbolKind::Method, 1)],
                Vec::new(),
            ),
        ],
    );

    assert_eq!(callees(&graph, "Derived::run"), ["base.h:2@60"]);
}

#[test]
fn a_declaration_and_its_linked_definition_are_one_callable() {
    let mut header = file("w.h", vec![function("draw", 1)], Vec::new());
    header.callables = vec![Callable {
        symbol: SymbolId(0),
        key: "draw()".to_owned(),
        role: CallableRole::Declaration,
    }];
    let mut source = including("w.cpp", &["w.h"], Vec::new());
    source.symbols = vec![function("draw", 3)];
    source.callables = vec![Callable {
        symbol: SymbolId(0),
        key: "draw()".to_owned(),
        role: CallableRole::Definition,
    }];
    let caller = including("main.cpp", &["w.h"], vec![call(0, "draw", 2)]);

    let graph = graph_of(cpp(), &[header, source, caller]);

    let walk = graph
        .callers("draw", Limits::new(1))
        .expect("draw is defined");
    assert_eq!(walk.root.file, "w.cpp");
    assert_eq!(reached(&walk), ["main.cpp:1@70"]);
}

#[test]
fn a_caller_that_calls_twice_counts_once_at_the_first_line_of_its_most_confident_calls() {
    let graph = graph(&[file(
        "a.go",
        vec![function("run", 1), function("parse", 9)],
        vec![call(0, "parse", 4), call(0, "parse", 2)],
    )]);

    let walk = graph
        .callers("parse", Limits::new(1))
        .expect("parse is defined");

    assert_eq!(walk.reached.len(), 1);
    assert_eq!(walk.reached[0].line, 2);
}

#[test]
fn a_caller_that_calls_twice_counts_its_most_confident_call() {
    let caller = file(
        "a.go",
        vec![member("Widget", "run", SymbolKind::Method, 1)],
        vec![
            member_call(0, "w", "draw", 2),
            member_call(0, "Widget", "draw", 3),
        ],
    );
    let graph = graph(&[
        caller,
        file(
            "b.go",
            vec![member("Other", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "c.go",
            vec![member("Widget", "draw", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    let outward = graph
        .callees("Widget::run", Limits::new(1))
        .expect("Widget::run is defined");
    let inward = graph
        .callers("Widget::draw", Limits::new(1))
        .expect("Widget::draw is defined");

    let first = |walk: &Walk<'_>| (walk.reached[0].confidence, walk.reached[0].line);
    assert_eq!(outward.reached.len(), 1);
    assert_eq!(first(&outward), (85, 3));
    assert_eq!(first(&inward), (85, 3));
}

#[test]
fn a_bare_name_several_free_functions_share_selects_the_one_with_the_most_callers() {
    let graph = graph(&[
        file(
            "a.go",
            vec![
                function("parse", 1),
                function("first", 5),
                function("second", 9),
            ],
            vec![call(1, "parse", 6), call(2, "parse", 10)],
        ),
        file(
            "b.go",
            vec![function("parse", 1), function("third", 5)],
            vec![call(1, "parse", 6)],
        ),
    ]);

    let walk = graph
        .callers("parse", Limits::new(1))
        .expect("parse is defined");
    let references = graph
        .references("parse", 0, usize::MAX)
        .expect("parse is defined");

    assert_eq!((walk.root.file, walk.passed_over), ("a.go", 1));
    assert_eq!(reached(&walk), ["a.go:5@80", "a.go:9@80"]);
    assert_eq!((references.symbol.file, references.total), ("a.go", 2));
}

#[test]
fn free_functions_with_as_many_callers_select_the_first_in_path_order_however_the_files_arrived() {
    let defining = |path: &str| {
        file(
            path,
            vec![function("parse", 1), function("caller", 5)],
            vec![call(1, "parse", 6)],
        )
    };
    let graph = graph(&[defining("c.go"), defining("b.go"), defining("a.go")]);

    let walk = graph
        .callers("parse", Limits::new(1))
        .expect("parse is defined");

    assert_eq!((walk.root.file, walk.passed_over), ("a.go", 2));
    assert_eq!(reached(&walk), ["a.go:5@80"]);
}

fn chain() -> Graph {
    graph(&[file(
        "a.go",
        vec![
            function("a", 1),
            function("b", 10),
            function("c", 20),
            function("d", 30),
        ],
        vec![call(0, "b", 2), call(1, "c", 11), call(2, "d", 21)],
    )])
}

#[test]
fn a_walk_over_callers_goes_as_deep_as_its_limit_and_counts_what_it_did_not_follow() {
    let graph = chain();

    let shallow = graph.callers("d", Limits::new(1)).expect("d is defined");
    let deep = graph.callers("d", Limits::new(5)).expect("d is defined");

    assert_eq!(reached(&shallow), ["a.go:20@80"]);
    assert_eq!(shallow.unexpanded, 1);
    assert_eq!(reached(&deep), ["a.go:20@80", "a.go:10@80", "a.go:1@80"]);
    assert_eq!(
        deep.reached
            .iter()
            .map(|reached| reached.depth)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(deep.unexpanded, 0);
}

#[test]
fn a_walk_over_callees_follows_calls_outward() {
    let graph = chain();

    let walk = graph.callees("a", Limits::new(5)).expect("a is defined");

    assert_eq!(reached(&walk), ["a.go:10@80", "a.go:20@80", "a.go:30@80"]);
    assert_eq!(
        walk.reached
            .iter()
            .map(|reached| reached.depth)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn the_confidence_floor_counts_the_calls_it_left_out() {
    let graph = graph(&[
        file(
            "x/a.go",
            vec![function("run", 1)],
            vec![call(0, "parse", 2)],
        ),
        file(
            "y/b.go",
            vec![function("parse", 1), function("near", 5)],
            vec![call(1, "parse", 6)],
        ),
    ]);

    let walk = graph
        .callers("parse", Limits::new(1).with_min_confidence(70))
        .expect("parse is defined");

    assert_eq!(reached(&walk), ["y/b.go:5@80"]);
    assert_eq!(walk.below_confidence, 1);
}

#[test]
fn the_symbol_cap_reports_what_it_turned_away_and_left_unexpanded() {
    let graph = graph(&[file(
        "a.go",
        vec![
            function("target", 1),
            function("a", 10),
            function("b", 20),
            function("c", 30),
        ],
        vec![
            call(1, "target", 11),
            call(2, "target", 21),
            call(3, "target", 31),
        ],
    )]);

    let walk = graph
        .callers("target", Limits::new(3).with_max_symbols(3))
        .expect("target is defined");

    assert!(walk.truncated);
    assert_eq!(reached(&walk), ["a.go:10@80", "a.go:20@80"]);
    assert_eq!(walk.turned_away, 1);
    assert_eq!(walk.unexpanded, 2);
}

#[test]
fn a_cap_reached_only_by_symbols_already_visited_cuts_nothing() {
    let graph = graph(&[file(
        "a.go",
        vec![function("target", 1), function("a", 10), function("b", 20)],
        vec![
            call(1, "target", 11),
            call(2, "a", 21),
            call(2, "target", 22),
        ],
    )]);

    let walk = graph
        .callers("target", Limits::new(3).with_max_symbols(3))
        .expect("target is defined");

    assert_eq!(walk.reached.len(), 2);
    assert!(!walk.truncated);
    assert_eq!(walk.turned_away, 0);
}

#[test]
fn a_call_path_names_each_hop_from_the_start() {
    let graph = chain();

    let path = graph
        .call_path("a", "d", Limits::new(5))
        .expect("both are defined");
    let short = graph
        .call_path("a", "d", Limits::new(2))
        .expect("both are defined");
    let itself = graph
        .call_path("a", "a", Limits::new(5))
        .expect("a is defined");
    let no_depth = graph
        .call_path("a", "d", Limits::new(0))
        .expect("both are defined");

    assert!(path.found);
    let hops: Vec<(&str, u32, u32, u8)> = path
        .hops
        .iter()
        .map(|hop| (hop.symbol.name, hop.line, hop.depth, hop.confidence))
        .collect();
    assert_eq!(hops, [("a", 2, 0, 80), ("b", 11, 1, 80), ("c", 21, 2, 80)]);
    assert!(!short.found);
    assert!(!short.truncated);
    assert_eq!(short.at_depth_bound, 1);
    assert!(itself.found);
    assert_eq!(itself.hops.len(), 0);
    assert!(!no_depth.found);
    assert_eq!(no_depth.at_depth_bound, 0);
}

#[test]
fn a_call_path_takes_the_shorter_of_two_chains() {
    let graph = graph(&[file(
        "a.go",
        vec![
            function("a", 1),
            function("b", 10),
            function("c", 20),
            function("d", 30),
        ],
        vec![
            call(0, "b", 2),
            call(0, "d", 3),
            call(1, "c", 11),
            call(2, "d", 21),
        ],
    )]);

    let path = graph
        .call_path("a", "d", Limits::new(5))
        .expect("both are defined");

    let hops: Vec<(&str, u32)> = path
        .hops
        .iter()
        .map(|hop| (hop.symbol.name, hop.line))
        .collect();
    assert_eq!(hops, [("a", 3)]);
}

#[test]
fn a_call_path_found_at_the_depth_bound_counts_the_other_callers_it_reached_there() {
    let graph = graph(&[file(
        "a.go",
        vec![
            function("end", 1),
            function("middle", 10),
            function("other", 20),
            function("start", 30),
        ],
        vec![
            call(1, "end", 11),
            call(2, "middle", 21),
            call(3, "middle", 31),
        ],
    )]);

    let path = graph
        .call_path("start", "end", Limits::new(2))
        .expect("both are defined");

    assert!(path.found);
    assert_eq!(path.at_depth_bound, 1);
}

#[test]
fn a_call_path_counts_the_calls_its_confidence_floor_left_out() {
    let graph = graph(&[
        file(
            "x/a.go",
            vec![function("run", 1)],
            vec![call(0, "parse", 2)],
        ),
        file("y/b.go", vec![function("parse", 1)], Vec::new()),
    ]);

    let path = graph
        .call_path("run", "parse", Limits::new(1).with_min_confidence(70))
        .expect("both are defined");

    assert!(!path.found);
    assert_eq!(path.below_confidence, 1);
}

#[test]
fn a_call_path_search_the_cap_stops_says_so() {
    let graph = chain();

    let path = graph
        .call_path("a", "d", Limits::new(5).with_max_symbols(2))
        .expect("both are defined");

    assert!(!path.found);
    assert!(path.truncated);
    assert_eq!(path.turned_away, 1);
}

#[test]
fn a_call_path_counts_the_other_callables_each_name_selects() {
    let graph = graph(&[
        file(
            "a.go",
            vec![function("start", 1), function("end", 10)],
            vec![call(0, "end", 2)],
        ),
        file(
            "b.go",
            vec![function("start", 1), function("end", 10)],
            Vec::new(),
        ),
        file("c.go", vec![function("end", 1)], Vec::new()),
    ]);

    let path = graph
        .call_path("start", "end", Limits::new(3))
        .expect("both are defined");

    assert!(path.found);
    assert_eq!((path.from.file, path.to.file), ("a.go", "a.go"));
    assert_eq!(path.passed_over, (1, 2));
}

fn using(name: &str, qualifier: Option<&str>) -> RefSite {
    RefSite {
        site_of: SymbolId(0),
        name: name.to_owned(),
        receiver: qualifier.map_or(Receiver::Bare, |qualifier| {
            Receiver::Qualifier(qualifier.to_owned())
        }),
        access: Access::Read,
        line: 3,
    }
}

#[test]
fn references_bind_through_imports_the_file_and_the_package() {
    let mut caller = importing(
        "cmd/main.go",
        None,
        Some("pkg/lib"),
        vec![member_call(0, "lib", "Config", 2)],
    );
    caller.references = vec![using("Config", Some("lib"))];
    let mut same_file = package(
        "pkg/lib/config.go",
        "lib",
        vec![
            exported(Symbol::new("Config", SymbolKind::Struct, 1, 2)),
            function("load", 5),
        ],
    );
    same_file.references = vec![RefSite {
        site_of: SymbolId(1),
        ..using("Config", None)
    }];
    let mut same_package = package("pkg/lib/other.go", "lib", vec![function("other", 1)]);
    same_package.references = vec![using("Config", None)];

    let graph = graph_of(go(), &[caller, same_file, same_package]);

    let references = graph
        .references("Config", 0, usize::MAX)
        .expect("Config is defined");
    let found: Vec<String> = references
        .sites
        .iter()
        .map(|reference| {
            format!(
                "{}:{}@{}{}",
                reference.site_of.file,
                reference.line,
                reference.confidence,
                if reference.call { "c" } else { "" }
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            "cmd/main.go:2@95c",
            "cmd/main.go:3@95",
            "pkg/lib/config.go:3@80",
            "pkg/lib/other.go:3@75"
        ]
    );
    assert_eq!(references.total, 4);
}

#[test]
fn references_count_what_the_floor_and_the_limit_cut() {
    let mut defining = file(
        "a.go",
        vec![
            function("run", 1),
            Symbol::new("Config", SymbolKind::Struct, 5, 6),
        ],
        vec![call(0, "Config", 2)],
    );
    defining.references = vec![using("Config", None), using("Config", None)];

    let graph = graph(&[defining]);

    let limited = graph.references("Config", 0, 1).expect("Config is defined");
    let counted = graph.references("Config", 0, 0).expect("Config is defined");
    let floored = graph
        .references("Config", 90, usize::MAX)
        .expect("Config is defined");
    assert_eq!((limited.total, limited.sites.len()), (2, 1));
    assert_eq!(limited.sites[0].line, 2);
    assert_eq!((counted.total, counted.sites.len()), (2, 0));
    assert_eq!((floored.total, floored.below_confidence), (0, 2));
}

#[test]
fn references_select_the_symbol_with_the_most_calls_and_uses_together() {
    let mut used = file(
        "b.go",
        vec![function("parse", 1), function("take", 5)],
        Vec::new(),
    );
    used.references = vec![
        RefSite {
            site_of: SymbolId(1),
            ..using("parse", None)
        },
        RefSite {
            site_of: SymbolId(1),
            line: 6,
            ..using("parse", None)
        },
    ];
    let graph = graph(&[
        file(
            "a.go",
            vec![function("parse", 1), function("run", 5)],
            vec![call(1, "parse", 6)],
        ),
        used,
    ]);

    let references = graph
        .references("parse", 0, usize::MAX)
        .expect("parse is defined");
    let walk = graph
        .callers("parse", Limits::new(1))
        .expect("parse is defined");

    assert_eq!((references.symbol.file, references.total), ("b.go", 2));
    assert_eq!(walk.root.file, "a.go");
}

#[test]
fn references_select_among_every_symbol_a_use_can_name_where_values_are_no_call_targets() {
    let mut constant = file(
        "c.cpp",
        vec![
            Symbol::new("limit", SymbolKind::Constant, 1, 1),
            function("take", 5),
        ],
        Vec::new(),
    );
    constant.references = [6, 7, 8]
        .into_iter()
        .map(|line| RefSite {
            site_of: SymbolId(1),
            line,
            ..using("limit", None)
        })
        .collect();
    let graph = graph_of(
        cpp(),
        &[
            file("a.cpp", vec![function("run", 1)], vec![call(0, "limit", 2)]),
            file("b.cpp", vec![function("limit", 1)], Vec::new()),
            constant,
        ],
    );

    let references = graph
        .references("limit", 0, usize::MAX)
        .expect("limit is defined");

    assert_eq!(
        (
            references.symbol.file,
            references.symbol.kind,
            references.total
        ),
        ("c.cpp", SymbolKind::Constant, 3)
    );
    assert_eq!(references.passed_over, 1);
}

#[test]
fn a_qualified_reference_binds_an_imported_value_where_values_are_no_call_targets() {
    let mut caller = importing("cmd/main.go", None, Some("pkg/lib"), Vec::new());
    caller.references = vec![using("Limit", Some("lib"))];
    let graph = graph_of(
        cpp(),
        &[
            caller,
            package(
                "pkg/lib/limit.go",
                "lib",
                vec![exported(Symbol::new("Limit", SymbolKind::Constant, 1, 1))],
            ),
        ],
    );

    let references = graph
        .references("Limit", 0, usize::MAX)
        .expect("Limit is defined");

    assert_eq!(references.total, 1);
    let site = references.sites[0];
    assert_eq!(
        (site.site_of.file, site.site_of.name, site.line),
        ("cmd/main.go", "main", 3)
    );
    assert_eq!((site.confidence, site.call), (95, false));
}

#[test]
fn removing_a_file_takes_its_calls_and_its_callees_out_of_every_answer() {
    let mut graph = graph(&[
        file("a.go", vec![function("run", 1)], vec![call(0, "parse", 2)]),
        file("b.go", vec![function("parse", 1)], Vec::new()),
    ]);
    assert_eq!(callers(&graph, "parse"), ["a.go:1@60"]);

    graph.remove("a.go");
    assert_eq!(callers(&graph, "parse"), Vec::<String>::new());

    graph.insert(&file(
        "a.go",
        vec![function("run", 1)],
        vec![call(0, "parse", 2)],
    ));
    graph.remove("b.go");
    assert!(graph.callers("parse", Limits::new(1)).is_none());
    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_file_inserted_again_answers_with_its_new_uses() {
    let using_file = |line: u32| {
        let mut using_file = file("a.go", vec![function("run", 1)], Vec::new());
        using_file.references = vec![RefSite {
            line,
            ..using("Limit", None)
        }];
        using_file.package = Some("p".to_owned());
        using_file
    };
    let mut defining = file(
        "b.go",
        vec![Symbol::new("Limit", SymbolKind::Constant, 1, 1)],
        Vec::new(),
    );
    defining.package = Some("p".to_owned());
    let mut graph = graph_of(go(), &[using_file(3), defining]);
    let lines = |graph: &Graph| -> Vec<u32> {
        graph
            .references("Limit", 0, usize::MAX)
            .expect("Limit is defined")
            .sites
            .iter()
            .map(|site| site.line)
            .collect()
    };

    let before = lines(&graph);
    graph.insert(&using_file(7));
    let after = lines(&graph);

    assert_eq!(before, [3]);
    assert_eq!(after, [7]);
}

#[test]
fn a_name_only_a_field_defines_selects_the_field_and_one_a_variant_defines_selects_nothing() {
    let graph = graph(&[file(
        "a.go",
        vec![
            member("Config", "limit", SymbolKind::Field, 2),
            member("Kind", "Red", SymbolKind::EnumVariant, 4),
        ],
        Vec::new(),
    )]);

    let field = graph
        .references("limit", 0, usize::MAX)
        .map(|references| references.symbol.kind);
    assert_eq!(field, Some(SymbolKind::Field));
    assert!(graph.references("Red", 0, usize::MAX).is_none());
}

#[test]
fn a_file_inserted_again_answers_with_its_new_calls() {
    let mut graph = graph(&[
        file("a.go", vec![function("run", 1)], vec![call(0, "parse", 2)]),
        file(
            "b.go",
            vec![function("parse", 1), function("render", 5)],
            Vec::new(),
        ),
    ]);

    graph.insert(&file(
        "a.go",
        vec![function("run", 1)],
        vec![call(0, "render", 3)],
    ));

    assert_eq!(callees(&graph, "run"), ["b.go:5@60"]);
    assert_eq!(callers(&graph, "parse"), Vec::<String>::new());
    assert_eq!(callers(&graph, "render"), ["a.go:1@60"]);
}

#[test]
fn a_package_forgets_a_removed_file_s_names() {
    let mut graph = graph_of(
        go(),
        &[
            package("app/main.go", "app", vec![function("run", 1)]),
            package("app/parse.go", "app", vec![function("parse", 1)]),
            package("lib/parse.go", "lib", vec![function("parse", 1)]),
        ],
    );
    let mut caller = package("app/main.go", "app", vec![function("run", 1)]);
    caller.calls = vec![call(0, "parse", 2)];
    graph.insert(&caller);
    assert_eq!(callees(&graph, "run"), ["app/parse.go:1@75"]);

    graph.remove("app/parse.go");

    assert_eq!(
        callees(&graph, "run"),
        Vec::<String>::new(),
        "a bare call reaches no other package's function"
    );
}

#[test]
fn the_first_in_path_order_wins_however_the_files_arrived() {
    let caller = file(
        "z/main.go",
        vec![function("run", 1)],
        vec![member_call(0, "w", "draw", 2)],
    );
    let mut caller = caller;
    caller.symbols[0] = member("Widget", "run", SymbolKind::Method, 1);
    let early = file(
        "a/w.go",
        vec![member("Widget", "draw", SymbolKind::Method, 1)],
        Vec::new(),
    );
    let late = file(
        "b/w.go",
        vec![member("Widget", "draw", SymbolKind::Method, 1)],
        Vec::new(),
    );

    let in_order = graph(&[caller.clone(), early.clone(), late.clone()]);
    let reversed = graph(&[late, early, caller]);

    assert_eq!(callees(&in_order, "Widget::run"), ["a/w.go:1@80"]);
    assert_eq!(callees(&reversed, "Widget::run"), ["a/w.go:1@80"]);
}

#[test]
fn a_walk_over_callees_counts_what_its_floor_and_its_cap_left_out() {
    let graph = graph(&[
        file(
            "x/a.go",
            vec![function("run", 1), function("b", 10), function("c", 20)],
            vec![
                call(0, "b", 2),
                call(0, "c", 3),
                call(0, "parse", 4),
                call(0, "parse", 5),
            ],
        ),
        file("y/p.go", vec![function("parse", 1)], Vec::new()),
    ]);

    let floored = graph
        .callees("run", Limits::new(1).with_min_confidence(70))
        .expect("run is defined");
    let capped = graph
        .callees("run", Limits::new(2).with_max_symbols(2))
        .expect("run is defined");

    assert_eq!(reached(&floored), ["x/a.go:10@80", "x/a.go:20@80"]);
    assert_eq!(floored.below_confidence, 1);
    assert!(capped.truncated);
    assert_eq!(reached(&capped), ["x/a.go:10@80"]);
    assert_eq!((capped.turned_away, capped.unexpanded), (2, 1));
}

#[test]
fn a_call_of_a_declaration_reaches_the_definition_it_links_to() {
    let mut header = file("w.h", vec![function("draw", 1)], Vec::new());
    header.callables = vec![Callable {
        symbol: SymbolId(0),
        key: "draw()".to_owned(),
        role: CallableRole::Declaration,
    }];
    let mut source = including("w.cpp", &["w.h"], Vec::new());
    source.symbols = vec![function("draw", 3)];
    source.callables = vec![Callable {
        symbol: SymbolId(0),
        key: "draw()".to_owned(),
        role: CallableRole::Definition,
    }];
    let caller = including("main.cpp", &["w.h"], vec![call(0, "draw", 2)]);

    let graph = graph_of(cpp(), &[header, source, caller]);

    assert_eq!(callees(&graph, "run"), ["w.cpp:3@70"]);
}

#[test]
fn a_body_s_calls_are_found_wherever_the_extraction_lists_them() {
    let graph = graph(&[file(
        "a.go",
        vec![
            function("first", 1),
            function("second", 10),
            function("x", 20),
            function("y", 30),
        ],
        vec![call(1, "y", 12), call(0, "x", 2), call(1, "x", 11)],
    )]);

    assert_eq!(callees(&graph, "first"), ["a.go:20@80"]);
    assert_eq!(callees(&graph, "second"), ["a.go:20@80", "a.go:30@80"]);
}

#[test]
fn passed_over_counts_the_other_callables_the_name_selects() {
    let shared = graph(&[
        file("a.go", vec![function("parse", 1)], Vec::new()),
        file("b.go", vec![function("parse", 1)], Vec::new()),
    ]);
    let with_a_value = graph_of(
        cpp(),
        &[
            file("a.cpp", vec![function("parse", 1)], Vec::new()),
            file(
                "b.cpp",
                vec![Symbol::new("parse", SymbolKind::Variable, 1, 1)],
                Vec::new(),
            ),
        ],
    );

    let walk = shared
        .callees("parse", Limits::new(1))
        .expect("parse is defined");
    let beside_a_value = with_a_value
        .callees("parse", Limits::new(1))
        .expect("parse is defined");

    assert_eq!((walk.root.file, walk.passed_over), ("a.go", 1));
    assert_eq!(
        (beside_a_value.root.file, beside_a_value.passed_over),
        ("a.cpp", 0)
    );
}
