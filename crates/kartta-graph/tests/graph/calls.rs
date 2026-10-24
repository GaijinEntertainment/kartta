//! Call resolution through the public queries: each tier of the cascade, and what each query answers and leaves out.

use kartta_graph::{Candidate, Graph, Limits, Selector, SiteQuery, Walk};
use kartta_lang::{
    Access, BareReceivers, CallRef, Callable, CallableRole, CallsOnValues, Conventions, Extraction,
    FILE_SCOPE, ImportBinding, IncludeReach, IncludeRef, IncludedRuntimeMembers, InheritedMembers,
    LocalCall, MemberCalls, NamespaceCalls, OwnMembers, ReExport, Receiver, Symbol, SymbolFlags,
    SymbolId, SymbolKind, UntypedOwners, UseRef,
};

use crate::{call, function, graph, graph_of};

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

fn member_call(from: u32, receiver: &str, callee: &str, line: u32) -> CallRef {
    CallRef {
        receiver: Some(receiver.to_owned()),
        ..call(from, callee, line)
    }
}

fn file(path: &str, symbols: Vec<Symbol>, calls: Vec<CallRef>) -> Extraction {
    let mut extraction = crate::file(path, symbols);
    extraction.calls = calls;
    extraction
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
        site_of: SymbolId(0),
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
        site_of: SymbolId(0),
        local: SymbolId(1),
        line: 3,
    }];

    let graph = graph_of(cpp(), &[extraction]);

    assert_eq!(callees(&graph, "outer"), Vec::<String>::new());
}

fn declaring_a_local_value(path: &str, name: &str) -> Extraction {
    let mut extraction = file(
        path,
        vec![
            function("body", 1),
            Symbol::new(name, SymbolKind::Constant, 2, 2),
        ],
        Vec::new(),
    );
    extraction.local_symbols = vec![SymbolId(1)];
    extraction
}

#[test]
fn a_value_a_body_declares_keeps_a_namesake_from_being_the_one_callable_of_its_name() {
    let caller = file(
        "app/admin.js",
        vec![function("remove", 1)],
        vec![call(0, "confirm", 2)],
    );
    let dialog = file("ui/dialog.ts", vec![function("confirm", 1)], Vec::new());
    let local = declaring_a_local_value("spec/dialog.test.ts", "confirm");

    let alone = graph(&[caller.clone(), dialog.clone()]);
    assert_eq!(callees(&alone, "remove"), ["ui/dialog.ts:1@60"]);
    let beside_the_local = graph(&[caller.clone(), dialog.clone(), local.clone()]);
    assert_eq!(callees(&beside_the_local, "remove"), Vec::<String>::new());
    let mut conventions = Conventions::default();
    conventions.values_are_call_targets = false;
    let without_value_targets = graph_of(conventions, &[caller.clone(), dialog.clone(), local]);
    assert_eq!(
        callees(&without_value_targets, "remove"),
        ["ui/dialog.ts:1@60"],
        "a value is no namesake where a call never targets one"
    );
    let many_locals =
        (0..64).map(|at| declaring_a_local_value(&format!("spec/{at}.ts"), "confirm"));
    let indexed: Vec<Extraction> = [caller, dialog].into_iter().chain(many_locals).collect();
    assert_eq!(
        callees(&graph(&indexed), "remove"),
        Vec::<String>::new(),
        "a name of many definitions holds its locals apart"
    );
}

#[test]
fn a_value_a_body_declares_keeps_the_nearness_step_that_holds_it_from_binding() {
    let caller = file(
        "app/admin.js",
        vec![function("remove", 1)],
        vec![call(0, "format", 2)],
    );
    let near = file("app/util.js", vec![function("format", 1)], Vec::new());
    let far = file("lib/format.js", vec![function("format", 1)], Vec::new());

    let nearest = graph(&[caller.clone(), near.clone(), far.clone()]);
    assert_eq!(callees(&nearest, "remove"), ["app/util.js:1@50"]);
    let local_elsewhere = declaring_a_local_value("lib/other.js", "format");
    let beside_a_far_local = graph(&[caller.clone(), near.clone(), far.clone(), local_elsewhere]);
    assert_eq!(callees(&beside_a_far_local, "remove"), ["app/util.js:1@50"]);
    let mut local_near = declaring_a_local_value("app/other.js", "format");
    local_near.symbols[1].kind = SymbolKind::Variable;
    let beside_a_near_local = graph(&[caller, near, far, local_near]);
    assert_eq!(
        callees(&beside_a_near_local, "remove"),
        Vec::<String>::new(),
        "a variable counts as a constant does"
    );
}

#[test]
fn a_value_a_body_declares_keeps_the_parent_directory_and_first_party_steps_from_binding() {
    let caller = file(
        "a/b/admin.js",
        vec![function("remove", 1)],
        vec![call(0, "format", 2)],
    );
    let cousin = file("a/c/util.js", vec![function("format", 1)], Vec::new());
    let far = file("lib/x/format.js", vec![function("format", 1)], Vec::new());
    let mut vendored_symbol = function("format", 1);
    vendored_symbol.flags = SymbolFlags::empty().with_vendored(true);
    let vendored = file("vendor/y/format.js", vec![vendored_symbol], Vec::new());

    let in_parent = [caller.clone(), cousin, far.clone()];
    assert_eq!(callees(&graph(&in_parent), "remove"), ["a/c/util.js:1@30"]);
    let mut beside_a_cousin_local = in_parent.to_vec();
    beside_a_cousin_local.push(declaring_a_local_value("a/d/other.js", "format"));
    assert_eq!(
        callees(&graph(&beside_a_cousin_local), "remove"),
        Vec::<String>::new()
    );

    let first_party = [caller, far, vendored];
    assert_eq!(
        callees(&graph(&first_party), "remove"),
        ["lib/x/format.js:1@15"]
    );
    let mut beside_a_vendored_local = first_party.to_vec();
    let mut vendored_local = declaring_a_local_value("vendor/z/other.js", "format");
    for symbol in &mut vendored_local.symbols {
        symbol.flags = SymbolFlags::empty().with_vendored(true);
    }
    beside_a_vendored_local.push(vendored_local);
    assert_eq!(
        callees(&graph(&beside_a_vendored_local), "remove"),
        ["lib/x/format.js:1@15"],
        "vendored code is outside the first-party step"
    );
    let mut beside_a_first_party_local = first_party.to_vec();
    beside_a_first_party_local.push(declaring_a_local_value("other/z/v.js", "format"));
    assert_eq!(
        callees(&graph(&beside_a_first_party_local), "remove"),
        Vec::<String>::new()
    );
}

#[test]
fn a_value_a_body_declares_leaves_a_macro_invocation_to_the_macro_of_its_name() {
    let mut invocation = call(0, "render", 2);
    invocation.invokes_a_macro = true;
    let caller = file("app/main.rs", vec![function("main", 1)], vec![invocation]);
    let mut macro_symbol = function("render", 1);
    macro_symbol.kind = SymbolKind::Macro;
    let defining = file("lib/macros.rs", vec![macro_symbol], Vec::new());
    let local = declaring_a_local_value("other/body.rs", "render");

    assert_eq!(
        callees(&graph(&[caller, defining, local]), "main"),
        ["lib/macros.rs:1@60"]
    );
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
fn a_bare_call_named_like_a_package_import_binds_no_export_of_the_package() {
    let graph = graph_of(
        go(),
        &[
            importing(
                "cmd/main.go",
                None,
                Some("pkg/lib"),
                vec![call(0, "lib", 2)],
            ),
            package("pkg/lib/lib.go", "lib", vec![exported(function("lib", 1))]),
            package("cmd/lib.go", "main", vec![function("lib", 1)]),
        ],
    );

    assert_eq!(callees(&graph, "main"), ["cmd/lib.go:1@75"]);
}

fn importing_module(path: &str, name: &str, module: &str, calls: Vec<CallRef>) -> Extraction {
    let mut extraction = file(path, vec![function("main", 1)], calls);
    extraction.bindings = vec![ImportBinding {
        alias: None,
        target_dir: Some(module.to_owned()),
        last_segment: name.to_owned(),
    }];
    extraction
}

#[test]
fn a_call_that_names_an_import_of_a_module_s_file_binds_the_file_s_export() {
    let module = file(
        "crates/a/src/m.rs",
        vec![exported(function("parse", 1))],
        Vec::new(),
    );
    let namesake = file(
        "crates/c/src/parse.rs",
        vec![exported(function("parse", 1))],
        Vec::new(),
    );
    let bare = graph(&[
        importing_module(
            "crates/b/src/lib.rs",
            "parse",
            "crates/a/src/m.rs",
            vec![call(0, "parse", 2)],
        ),
        module.clone(),
        namesake.clone(),
    ]);
    let through_the_module = graph(&[
        importing_module(
            "crates/b/src/lib.rs",
            "m",
            "crates/a/src/m.rs",
            vec![member_call(0, "m", "parse", 2)],
        ),
        module,
        namesake,
    ]);

    assert_eq!(callees(&bare, "main"), ["crates/a/src/m.rs:1@95"]);
    assert_eq!(
        callees(&through_the_module, "main"),
        ["crates/a/src/m.rs:1@95"]
    );
}

#[test]
fn a_module_s_file_without_a_top_level_export_of_the_name_leaves_its_import_to_later_tiers() {
    let importer = || {
        importing_module(
            "crates/b/src/lib.rs",
            "parse",
            "crates/a/src/m.rs",
            vec![call(0, "parse", 2)],
        )
    };
    let elsewhere = file(
        "crates/c/src/parse.rs",
        vec![exported(function("parse", 1))],
        Vec::new(),
    );
    let mut nested = exported(function("parse", 2));
    nested.qualified_name = "inner::parse".to_owned();

    let absent = graph(&[
        importer(),
        file("crates/a/src/m.rs", Vec::new(), Vec::new()),
        elsewhere.clone(),
    ]);
    let absent_through_the_module = graph(&[
        importing_module(
            "crates/b/src/lib.rs",
            "m",
            "crates/a/src/m.rs",
            vec![member_call(0, "m", "parse", 2)],
        ),
        file("crates/a/src/m.rs", Vec::new(), Vec::new()),
        elsewhere,
    ]);
    let private = graph(&[
        importer(),
        file("crates/a/src/m.rs", vec![function("parse", 1)], Vec::new()),
    ]);
    let in_an_inline_module = graph(&[
        importer(),
        file(
            "crates/a/src/m.rs",
            vec![
                exported(Symbol::new("inner", SymbolKind::Namespace, 1, 3)),
                nested,
            ],
            Vec::new(),
        ),
    ]);

    assert_eq!(callees(&absent, "main"), ["crates/c/src/parse.rs:1@60"]);
    assert_eq!(
        callees(&absent_through_the_module, "main"),
        ["crates/c/src/parse.rs:1@60"]
    );
    assert_eq!(callees(&private, "main"), ["crates/a/src/m.rs:1@60"]);
    assert_eq!(
        callees(&in_an_inline_module, "main"),
        ["crates/a/src/m.rs:2@60"]
    );
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

fn calling_member(path: &str, from: &str, targets: &[&str], call: CallRef) -> Extraction {
    let mut extraction = including(path, targets, vec![call]);
    extraction.symbols = vec![function(from, 1)];
    extraction
}

#[test]
fn a_cpp_operand_s_class_binds_its_member_where_the_caller_s_file_sees_the_class() {
    let files = [
        including(
            "main.cpp",
            &["vec.h"],
            vec![member_call(0, "vector", "begin", 2)],
        ),
        file(
            "vec.h",
            vec![member("vector", "begin", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "apex/PxVector.h",
            vec![
                member("vector", "begin", SymbolKind::Method, 1),
                member("vector", "end", SymbolKind::Method, 2),
            ],
            Vec::new(),
        ),
        calling_member("z.cpp", "unseen", &[], member_call(0, "vector", "end", 2)),
        calling_member("y.cpp", "undeclared", &[], member_call(0, "", "end", 2)),
        calling_member(
            "s.cpp",
            "typedef",
            &["str.h"],
            member_call(0, "string", "append", 2),
        ),
        calling_member(
            "t.cpp",
            "untyped",
            &["str.h"],
            member_call(0, "", "append", 2),
        ),
        file(
            "str.h",
            vec![member("basic_string", "append", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "run"), ["vec.h:1@85"]);
    assert_eq!(
        callees(&graph, "unseen"),
        callees(&graph, "undeclared"),
        "a class the caller's file does not see leaves the call as one on an operand of no declared class"
    );
    assert_eq!(
        callees(&graph, "typedef"),
        callees(&graph, "untyped"),
        "a name no class holding the member answers to, as a typedef, does the same"
    );
    assert_eq!(callees(&graph, "untyped"), ["str.h:1@70"]);
}

#[test]
fn a_cpp_operand_s_class_binds_the_member_of_the_class_its_file_includes_before_a_deeper_namesake()
{
    let mut hud = including("m/hud.h", &["a/xray.h"], Vec::new());
    hud.symbols.clear();
    let files = [
        calling_member(
            "hud.cpp",
            "draw",
            &["m/hud.h", "z/unit.h"],
            member_call(0, "Unit", "getWeap", 2),
        ),
        hud,
        file(
            "a/xray.h",
            vec![member("Unit", "getWeap", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file(
            "z/unit.h",
            vec![member("Unit", "getWeap", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "draw"), ["z/unit.h:1@85"]);
}

fn class_in(scope: &str, name: &str, line: u32, end_line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Class, line, end_line);
    symbol.qualified_name = if scope.is_empty() {
        name.to_owned()
    } else {
        format!("{scope}::{name}")
    };
    symbol
}

fn widget_in(scope: &str) -> Vec<Symbol> {
    vec![
        class_in(scope, "Widget", 2, 5),
        member("Widget", "draw", SymbolKind::Method, 4),
    ]
}

#[test]
fn a_cpp_operand_s_class_binds_the_namesake_its_declaration_s_scopes_name() {
    let widgets = ["a/widget.h", "b/widget.h"];
    let mut main = including(
        "app/main.cpp",
        &widgets,
        vec![
            member_call(0, "b::Widget", "draw", 4),
            member_call(1, "a::Widget", "draw", 6),
            member_call(2, "::b::Widget", "draw", 8),
            member_call(3, "Widget", "draw", 10),
        ],
    );
    main.symbols = vec![
        function("paint", 3),
        function("sketch", 5),
        function("rooted", 7),
        function("blind", 9),
    ];
    let mut frame = including(
        "app/frame.cpp",
        &widgets,
        vec![
            member_call(0, "Widget", "draw", 4),
            member_call(1, "Widget", "draw", 7),
        ],
    );
    frame.symbols = vec![
        namespaced("b", "frame", 3),
        namespaced("b::detail", "outline", 6),
    ];
    let files = [
        file("a/widget.h", widget_in("a"), Vec::new()),
        file("b/widget.h", widget_in("b"), Vec::new()),
        main,
        frame,
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "paint"), ["b/widget.h:4@85"]);
    assert_eq!(callees(&graph, "sketch"), ["a/widget.h:4@85"]);
    assert_eq!(callees(&graph, "rooted"), ["b/widget.h:4@85"]);
    assert_eq!(
        callees(&graph, "frame"),
        ["b/widget.h:4@85"],
        "a bare class name is looked up from the caller's namespace outward"
    );
    assert_eq!(
        callees(&graph, "outline"),
        ["b/widget.h:4@85"],
        "a namespace that holds no class of the name passes the lookup to the one around it"
    );
    assert_eq!(
        callees(&graph, "blind"),
        Vec::<String>::new(),
        "namesakes in two namespaces the caller is in neither of are ambiguous"
    );
    assert_eq!(graph.resolution(0).misses.ambiguous, 1);
}

#[test]
fn a_callee_its_scope_qualifies_binds_the_member_of_the_class_that_scope_names() {
    let widgets = ["a/widget.h", "b/widget.h", "outer/b/widget.h"];
    let mut main = including(
        "app/main.cpp",
        &widgets,
        vec![
            call(0, "b::Widget::draw", 4),
            call(1, "a::Widget::draw", 6),
            call(2, "b::Widget::draw", 8),
            call(3, "::b::Widget::draw", 10),
            call(4, "c::Widget::draw", 12),
        ],
    );
    main.symbols = vec![
        function("paint", 3),
        function("sketch", 5),
        namespaced("outer", "inner", 7),
        namespaced("outer", "rooted", 9),
        function("guess", 11),
    ];
    let files = [
        file(widgets[0], widget_in("a"), Vec::new()),
        file(widgets[1], widget_in("b"), Vec::new()),
        file(widgets[2], widget_in("outer::b"), Vec::new()),
        main,
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "paint"), ["b/widget.h:4@88"]);
    assert_eq!(callees(&graph, "sketch"), ["a/widget.h:4@88"]);
    assert_eq!(
        callees(&graph, "inner"),
        ["outer/b/widget.h:4@88"],
        "the class the innermost scope around the call holds under the spelling wins"
    );
    assert_eq!(
        callees(&graph, "rooted"),
        ["b/widget.h:4@88"],
        "a spelling from the root names the root's class alone"
    );
    assert_eq!(
        callees(&graph, "guess"),
        ["a/widget.h:4@88"],
        "a scope that names none of the classes keeps the first in path order"
    );
}

fn box_copy(dir: &str) -> [Extraction; 2] {
    let callable = |symbol, role| Callable {
        symbol: SymbolId(symbol),
        key: "ns::Box::fill()".to_owned(),
        role,
    };
    let mut header = file(
        &format!("{dir}/box.h"),
        vec![
            class_in("ns", "Box", 1, 3),
            member("Box", "fill", SymbolKind::Method, 2),
        ],
        Vec::new(),
    );
    header.callables = vec![callable(1, CallableRole::Declaration)];
    let mut source = including(
        &format!("{dir}/box.cpp"),
        &["x/box.h", "y/box.h"],
        Vec::new(),
    );
    source.symbols = vec![member("Box", "fill", SymbolKind::Method, 3)];
    source.callables = vec![callable(0, CallableRole::Definition)];
    [header, source]
}

#[test]
fn a_callee_its_scope_qualifies_keeps_the_first_member_where_the_graph_locates_no_class_of_it() {
    let [x_header, x_source] = box_copy("x");
    let [y_header, y_source] = box_copy("y");
    let caller = including("x/use.cpp", &["x/box.h"], vec![call(0, "ns::Box::fill", 2)]);

    let graph = graph_of(cpp(), &[x_header, x_source, y_header, y_source, caller]);

    assert_eq!(
        callees(&graph, "run"),
        ["x/box.cpp:3@88"],
        "a declaration in each copy of a library links neither definition, so no class of the first is located"
    );
}

fn field_of(owner: &str, name: &str, line: u32, declared: &str) -> Symbol {
    let mut symbol = member(owner, name, SymbolKind::Field, line);
    symbol.value_types = vec![declared.to_owned()];
    symbol
}

fn locks_and_cache() -> [Extraction; 2] {
    let locks = vec![
        class_in("", "Lock", 1, 3),
        member("Lock", "unlockRead", SymbolKind::Method, 2),
        class_in("", "SpinLock", 4, 6),
        member("SpinLock", "unlockRead", SymbolKind::Method, 5),
    ];
    let mut cache = file(
        "cache.h",
        vec![
            class_in("", "Cache", 1, 5),
            field_of("Cache", "lock", 2, "Lock"),
            field_of("Cache", "held", 3, "*Lock"),
        ],
        Vec::new(),
    );
    cache.includes = vec![IncludeRef {
        target: "lock.h".to_owned(),
    }];
    [file("lock.h", locks, Vec::new()), cache]
}

#[test]
fn a_cpp_call_on_a_field_of_the_caller_s_class_binds_the_member_of_the_class_it_is_declared_as() {
    let mut source = including(
        "cache.cpp",
        &["cache.h"],
        vec![
            member_call(0, ".lock", "unlockRead", 2),
            member_call(0, ".guard", "unlockRead", 3),
            member_call(1, "->held", "unlockRead", 6),
            member_call(1, ".held", "unlockRead", 7),
            member_call(2, ".lock", "unlockRead", 10),
            member_call(3, ".global", "unlockRead", 12),
        ],
    );
    source.symbols = vec![
        member("Cache", "release", SymbolKind::Method, 1),
        member("Cache", "read", SymbolKind::Method, 5),
        function("outside", 9),
        member("Cache", "write", SymbolKind::Method, 11),
    ];
    let [mut locks, mut cache] = locks_and_cache();
    locks.symbols.extend([
        class_in("sync", "Lock", 7, 9),
        member("Lock", "unlockRead", SymbolKind::Method, 8),
    ]);
    cache
        .symbols
        .push(field_of("Cache", "guard", 4, "sync::Lock"));

    let graph = graph_of(cpp(), &[locks, cache, source]);

    assert_eq!(
        callees(&graph, "Cache::release"),
        ["lock.h:2@85", "lock.h:8@85"],
        "a field's class keeps the scopes its declaration spells"
    );
    assert_eq!(
        callees(&graph, "Cache::read"),
        ["lock.h:2@85"],
        "a pointer field binds through `->`, and through `.` it is a call on a value of no declared class"
    );
    assert_eq!(
        callees(&graph, "outside"),
        Vec::<String>::new(),
        "outside a member a name is no field"
    );
    assert_eq!(
        callees(&graph, "Cache::write"),
        Vec::<String>::new(),
        "a name no field of the class answers to is a value of no declared class"
    );
    assert_eq!(graph.resolution(0).misses.ambiguous, 3);
}

#[test]
fn a_cpp_call_on_a_field_of_a_base_binds_the_member_of_the_class_it_is_declared_as() {
    let mut derived = class_in("", "Guard", 1, 4);
    derived.parents = vec!["Cache".to_owned()];
    let mut twin = class_in("", "Twin", 5, 7);
    twin.parents = vec!["Cache".to_owned(), "Other".to_owned()];
    let mut guard = file(
        "guard.h",
        vec![
            derived,
            field_of("Guard", "held", 2, "*SpinLock"),
            twin,
            class_in("", "Other", 8, 10),
            field_of("Other", "lock", 9, "Lock"),
        ],
        Vec::new(),
    );
    guard.includes = vec![IncludeRef {
        target: "cache.h".to_owned(),
    }];
    let root = file(
        "root.h",
        vec![
            class_in("", "Root", 1, 3),
            field_of("Root", "lock", 2, "*SpinLock"),
        ],
        Vec::new(),
    );
    let mut source = including(
        "guard.cpp",
        &["guard.h"],
        vec![
            member_call(0, ".lock", "unlockRead", 2),
            member_call(0, "->held", "unlockRead", 3),
            member_call(1, ".lock", "unlockRead", 6),
        ],
    );
    source.symbols = vec![
        member("Guard", "release", SymbolKind::Method, 1),
        member("Twin", "write", SymbolKind::Method, 5),
    ];
    let [locks, mut cache] = locks_and_cache();
    cache.symbols[0].parents = vec!["Root".to_owned()];
    cache.includes.push(IncludeRef {
        target: "root.h".to_owned(),
    });

    let graph = graph_of(cpp(), &[locks, root, cache, guard, source]);

    assert_eq!(
        callees(&graph, "Guard::release"),
        ["lock.h:2@85", "lock.h:5@85"],
        "a base's field binds where the class declares none of the name, hiding a farther base's, and the class's own \
         field hides the base's"
    );
    assert_eq!(
        callees(&graph, "Twin::write"),
        Vec::<String>::new(),
        "fields of one name in two bases leave the name ambiguous"
    );
}

#[test]
fn a_cpp_call_through_a_class_template_field_s_arrow_binds_the_member_of_the_class_it_reaches() {
    let mut owned = field_of("Holder", "owned", 2, "unique_ptr");
    owned.value_types.push("*Lock".to_owned());
    let mut holder = file(
        "holder.h",
        vec![
            class_in("", "Holder", 1, 3),
            owned,
            class_in("", "unique_ptr", 4, 6),
            member("unique_ptr", "reset", SymbolKind::Method, 5),
        ],
        Vec::new(),
    );
    holder.includes = vec![IncludeRef {
        target: "lock.h".to_owned(),
    }];
    let mut source = including(
        "holder.cpp",
        &["holder.h"],
        vec![
            member_call(0, "->owned", "unlockRead", 2),
            member_call(0, ".owned", "reset", 3),
        ],
    );
    source.symbols = vec![member("Holder", "release", SymbolKind::Method, 1)];
    let [locks, _] = locks_and_cache();

    let graph = graph_of(cpp(), &[locks, holder, source]);

    assert_eq!(
        callees(&graph, "Holder::release"),
        ["holder.h:5@85", "lock.h:2@85"],
        "`.` reaches the template's own member, `->` the class its first argument names"
    );
}

#[test]
fn a_cpp_operand_s_class_binds_the_first_of_namesakes_of_one_qualified_name() {
    let files = [
        file(
            "box.h",
            vec![
                class_in("", "Box", 1, 3),
                member("Box", "fill", SymbolKind::Method, 2),
                class_in("", "Box", 4, 6),
                member("Box", "fill", SymbolKind::Method, 5),
            ],
            Vec::new(),
        ),
        calling_member(
            "use.cpp",
            "pack",
            &["box.h"],
            member_call(0, "Box", "fill", 2),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(
        callees(&graph, "pack"),
        ["box.h:2@85"],
        "a template and its specialization share the qualified name, and the first binds as before"
    );
}

fn derived_in(scope: &str, name: &str, (line, end_line): (u32, u32), parents: &[&str]) -> Symbol {
    let mut symbol = class_in(scope, name, line, end_line);
    symbol.parents = parents.iter().map(|&parent| parent.to_owned()).collect();
    symbol
}

/// `cloud.h` declares, in `namespace draco`, `Base` and `Side` with a `Flush`, `Mid : Base` with its own, `Other` with
/// one and derived from neither, and classes that call `Flush` on `this` in one member and bare in the next:
/// `Derived : Base`, `Near : Mid`, `Both : Base, Side` and `Own : Base`, which declares `Flush` too; `shutdown` calls
/// it on a local of `Derived`.
fn flushing_classes() -> Extraction {
    let this_and_bare = |class: &str, line: u32| {
        [
            member_call(0, class, "Flush", line + 1),
            call(1, "Flush", line + 3),
        ]
    };
    let mut symbols = vec![
        class_in("draco", "Base", 1, 3),
        member("Base", "Flush", SymbolKind::Method, 2),
        derived_in("draco", "Mid", (4, 6), &["Base"]),
        member("Mid", "Flush", SymbolKind::Method, 5),
        class_in("draco", "Side", 7, 9),
        member("Side", "Flush", SymbolKind::Method, 8),
        class_in("draco", "Other", 10, 12),
        member("Other", "Flush", SymbolKind::Method, 11),
    ];
    let mut calls = Vec::new();
    for (class, parents, line) in [
        ("Derived", &["Base"][..], 20),
        ("Near", &["Mid"], 30),
        ("Both", &["Base", "Side"], 40),
    ] {
        let from = u32::try_from(symbols.len()).expect("a handful of symbols");
        symbols.extend([
            derived_in("draco", class, (line, line + 5), parents),
            member(class, "Close", SymbolKind::Method, line + 1),
            member(class, "Stop", SymbolKind::Method, line + 3),
        ]);
        calls.extend(this_and_bare(class, line).map(|call| CallRef {
            site_of: SymbolId(from + 1 + call.site_of.0),
            ..call
        }));
    }
    let own = u32::try_from(symbols.len()).expect("a handful of symbols");
    symbols.extend([
        derived_in("draco", "Own", (50, 55), &["Base"]),
        member("Own", "Flush", SymbolKind::Method, 51),
        member("Own", "Close", SymbolKind::Method, 52),
        namespaced("draco", "shutdown", 60),
    ]);
    calls.extend([
        member_call(own + 2, "Own", "Flush", 53),
        member_call(own + 3, "Derived", "Flush", 61),
    ]);
    file("cloud.h", symbols, calls)
}

#[test]
fn a_cpp_call_on_a_class_that_inherits_the_member_binds_what_a_bare_call_in_its_member_binds() {
    let namesake = file(
        "xray/base.h",
        vec![
            class_in("xray", "Base", 1, 3),
            member("Base", "Flush", SymbolKind::Method, 2),
        ],
        Vec::new(),
    );
    let graph = graph_of(cpp(), &[flushing_classes(), namesake]);
    let targets = |caller: &str| -> Vec<String> {
        callees(&graph, caller)
            .into_iter()
            .map(|reached| reached.split('@').next().unwrap_or_default().to_owned())
            .collect()
    };

    assert_eq!(
        callees(&graph, "Derived::Close"),
        ["cloud.h:2@85"],
        "a namesake of the base outside the base's body is another class's"
    );
    assert_eq!(
        callees(&graph, "draco::shutdown"),
        ["cloud.h:2@85"],
        "a local of the class reaches its bases as `this` does"
    );
    assert_eq!(
        callees(&graph, "Near::Close"),
        ["cloud.h:5@85"],
        "the nearest base's member hides a farther base's"
    );
    assert_eq!(
        callees(&graph, "Both::Close"),
        Vec::<String>::new(),
        "members of one name in two bases leave the call ambiguous"
    );
    assert_eq!(
        callees(&graph, "Own::Close"),
        ["cloud.h:51@85"],
        "the class's own member binds before a base's"
    );
    for class in ["Derived", "Near", "Both"] {
        assert_eq!(
            targets(&format!("{class}::Close")),
            targets(&format!("{class}::Stop")),
            "a call on `this` in {class} binds what a bare call binds"
        );
    }
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

fn namespaced(scope: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = function(name, line);
    symbol.qualified_name = format!("{scope}::{name}");
    symbol
}

fn calling_from(path: &str, from: Symbol, targets: &[&str], callee: &str) -> Extraction {
    let mut extraction = including(path, targets, vec![call(0, callee, 2)]);
    extraction.symbols = vec![from];
    extraction
}

#[test]
fn a_callee_written_inside_a_namespace_binds_the_definition_that_scope_names() {
    let files = [
        calling_from(
            "duration.cc",
            namespaced("absl", "FromChrono", 1),
            &["time.h", "other.h"],
            "time_internal::GetRepHi",
        ),
        calling_from(
            "other.cc",
            namespaced("other", "Convert", 1),
            &["time.h", "other.h"],
            "time_internal::GetRepHi",
        ),
        file(
            "time.h",
            vec![namespaced("absl::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
        file(
            "other.h",
            vec![namespaced("other::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "FromChrono"), ["time.h:1@88"]);
    assert_eq!(callees(&graph, "Convert"), ["other.h:1@88"]);
}

#[test]
fn the_callers_of_an_operator_whose_name_holds_a_separator_include_a_call_written_qualified() {
    let files = [
        file(
            "das/eval.cpp",
            vec![function("divide", 30), function("shrink", 40)],
            vec![
                call(0, "das::operator/", 31),
                call(1, "das::operator/=", 41),
            ],
        ),
        file(
            "das/math.h",
            vec![
                namespaced("das", "operator/", 12),
                namespaced("das", "operator/=", 20),
            ],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "divide"), ["das/math.h:12@90"]);
    assert_eq!(callers(&graph, "das::operator/"), ["das/eval.cpp:30@90"]);
    assert_eq!(callees(&graph, "shrink"), ["das/math.h:20@90"]);
    assert_eq!(callers(&graph, "das::operator/="), ["das/eval.cpp:40@90"]);
}

#[test]
fn a_callee_written_inside_a_namespace_binds_its_scope_s_definition_before_a_global_one() {
    let files = [
        calling_from(
            "duration.cc",
            namespaced("absl", "FromChrono", 1),
            &["time.h", "global.h"],
            "time_internal::GetRepHi",
        ),
        file(
            "time.h",
            vec![namespaced("absl::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
        file(
            "global.h",
            vec![namespaced("time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "FromChrono"), ["time.h:1@88"]);
}

#[test]
fn a_callee_written_inside_nested_namespaces_binds_the_innermost_scope_s_definition() {
    let files = [
        calling_from(
            "run.cc",
            namespaced("a::b", "Run", 1),
            &["outer.h", "inner.h"],
            "x::g",
        ),
        file("outer.h", vec![namespaced("a::x", "g", 1)], Vec::new()),
        file("inner.h", vec![namespaced("a::b::x", "g", 1)], Vec::new()),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "Run"), ["inner.h:1@88"]);
}

#[test]
fn a_callee_written_inside_a_namespace_binds_the_copy_its_file_sees() {
    let files = [
        calling_from(
            "duration.cc",
            namespaced("absl", "FromChrono", 1),
            &["b/time.h"],
            "time_internal::GetRepHi",
        ),
        file(
            "a/time.h",
            vec![namespaced("absl::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
        file(
            "b/time.h",
            vec![namespaced("absl::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "FromChrono"), ["b/time.h:1@88"]);
}

#[test]
fn a_callee_written_from_the_root_inside_a_namespace_binds_the_global_definition() {
    let files = [
        calling_from(
            "duration.cc",
            namespaced("absl", "FromChrono", 1),
            &["time.h", "global.h"],
            "::time_internal::GetRepHi",
        ),
        file(
            "time.h",
            vec![namespaced("absl::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
        file(
            "global.h",
            vec![namespaced("time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "FromChrono"), ["global.h:1@90"]);
}

#[test]
fn a_callee_no_enclosing_scope_names_binds_the_global_name_it_spells() {
    let files = [
        calling_from(
            "duration.cc",
            namespaced("absl", "FromChrono", 1),
            &["global.h"],
            "time_internal::GetRepHi",
        ),
        file(
            "global.h",
            vec![namespaced("time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "FromChrono"), ["global.h:1@90"]);
}

#[test]
fn a_callee_its_innermost_scope_does_not_name_binds_the_next_scope_out() {
    let files = [
        calling_from("run.cc", namespaced("a::b", "Run", 1), &["outer.h"], "x::g"),
        file("outer.h", vec![namespaced("a::x", "g", 1)], Vec::new()),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "Run"), ["outer.h:1@88"]);
}

#[test]
fn a_callee_written_outside_every_namespace_binds_no_namespaced_definition() {
    let files = [
        calling_from(
            "duration.cc",
            function("FromChrono", 1),
            &["time.h"],
            "time_internal::GetRepHi",
        ),
        file(
            "time.h",
            vec![namespaced("absl::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "FromChrono"), Vec::<String>::new());
}

#[test]
fn a_callee_written_inside_a_namespace_binds_none_its_file_does_not_see() {
    let files = [
        calling_from(
            "duration.cc",
            namespaced("absl", "FromChrono", 1),
            &[],
            "time_internal::GetRepHi",
        ),
        file(
            "time.h",
            vec![namespaced("absl::time_internal", "GetRepHi", 1)],
            Vec::new(),
        ),
    ];

    let graph = graph_of(cpp(), &files);

    assert_eq!(callees(&graph, "FromChrono"), Vec::<String>::new());
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
fn a_dot_import_binds_no_namespace_where_the_language_binds_none() {
    let files = [
        importing(
            "cmd/main.go",
            Some("."),
            Some("pkg/lib"),
            vec![call(0, "walk", 2)],
        ),
        package(
            "pkg/lib/walk.go",
            "lib",
            vec![exported(Symbol::new("walk", SymbolKind::Namespace, 1, 1))],
        ),
    ];
    let mut unbound = go();
    unbound.namespace_calls = NamespaceCalls::Unbound;

    assert_eq!(
        callees(&graph_of(go(), &files), "main"),
        ["pkg/lib/walk.go:1@95"]
    );
    assert_eq!(
        callees(&graph_of(unbound, &files), "main"),
        Vec::<String>::new()
    );
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

/// `3rdParty/sse2neon/sse2neon.h` defines the one `_mm_add_ps` of the tree; `engine/math/plain.cpp` includes each of
/// `includes` and calls it from `sum4`.
fn intrinsic_shim(includes: &[&str]) -> [Extraction; 2] {
    let mut caller = including(
        "engine/math/plain.cpp",
        includes,
        vec![call(0, "_mm_add_ps", 2)],
    );
    caller.symbols = vec![function("sum4", 1)];
    [
        file(
            "3rdParty/sse2neon/sse2neon.h",
            vec![function("_mm_add_ps", 4)],
            Vec::new(),
        ),
        caller,
    ]
}

#[test]
fn a_call_binds_no_one_callable_that_no_include_of_its_file_reaches_where_includes_paste_text() {
    let graph = graph_of(cpp(), &intrinsic_shim(&[]));

    let walk = graph
        .callees("sum4", Limits::new(1))
        .expect("sum4 is defined");

    assert_eq!(reached(&walk), Vec::<String>::new());
    assert_eq!(walk.unresolved, 1);
    assert_eq!(callers(&graph, "_mm_add_ps"), Vec::<String>::new());
}

#[test]
fn a_call_binds_the_one_callable_of_a_header_its_file_includes() {
    let graph = graph_of(cpp(), &intrinsic_shim(&["3rdParty/sse2neon/sse2neon.h"]));

    assert_eq!(
        callees(&graph, "sum4"),
        ["3rdParty/sse2neon/sse2neon.h:4@70"]
    );
}

#[test]
fn a_call_binds_the_one_callable_that_a_file_including_its_file_reaches() {
    let wrapper = file(
        "das/wrapper.inc",
        vec![function("fastcall", 1)],
        vec![call(0, "Rx", 2)],
    );
    let mut bind = including("das/bind.cpp", &["das/wrapper.inc"], Vec::new());
    bind.symbols = vec![function("Rx", 3)];

    let graph = graph_of(cpp(), &[wrapper, bind]);

    assert_eq!(callees(&graph, "fastcall"), ["das/bind.cpp:3@60"]);
}

#[test]
fn a_member_call_binds_the_one_member_whose_declaration_its_file_s_includes_reach() {
    let mut class_header = file(
        "inc/widget.h",
        vec![
            Symbol::new("Widget", SymbolKind::Class, 1, 3),
            member("Widget", "draw", SymbolKind::Method, 2),
        ],
        Vec::new(),
    );
    class_header.callables = vec![Callable {
        symbol: SymbolId(1),
        key: "Widget::draw()".to_owned(),
        role: CallableRole::Declaration,
    }];
    let mut source = including("src/widget.cpp", &["inc/widget.h"], Vec::new());
    source.symbols = vec![member("Widget", "draw", SymbolKind::Method, 3)];
    source.callables = vec![Callable {
        symbol: SymbolId(0),
        key: "Widget::draw()".to_owned(),
        role: CallableRole::Definition,
    }];
    let caller = including(
        "app/main.cpp",
        &["inc/all.h"],
        vec![member_call(0, "panel", "draw", 2)],
    );
    let umbrella = header("inc/all.h", Vec::new(), &["inc/widget.h"]);

    let graph = graph_of(cpp(), &[class_header, source, umbrella, caller]);

    assert_eq!(callees(&graph, "run"), ["src/widget.cpp:3@60"]);
}

#[test]
fn a_member_call_binds_no_one_member_that_no_include_of_its_file_reaches() {
    let caller = including(
        "app/main.cpp",
        &["inc/all.h"],
        vec![member_call(0, "panel", "draw", 2)],
    );
    let umbrella = header("inc/all.h", Vec::new(), &[]);
    let source = file(
        "src/widget.cpp",
        vec![member("Widget", "draw", SymbolKind::Method, 3)],
        Vec::new(),
    );

    let graph = graph_of(cpp(), &[source, umbrella, caller]);

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
}

#[test]
fn a_call_binds_the_one_callable_no_include_reaches_where_includes_name_modules() {
    let graph = graph(&intrinsic_shim(&[]));

    assert_eq!(
        callees(&graph, "sum4"),
        ["3rdParty/sse2neon/sse2neon.h:4@60"]
    );
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
fn a_receiver_that_names_an_owner_two_packages_declare_binds_the_one_of_the_caller_s_package() {
    let in_package = |mut extraction: Extraction, package: &str| {
        extraction.package = Some(package.to_owned());
        extraction
    };
    let put = || vec![member("Buffer", "put", SymbolKind::Method, 1)];
    let run = || vec![function("run", 1)];
    let call = || vec![member_call(0, "Buffer", "put", 2)];
    let graph = graph_of(
        Conventions::default(),
        &[
            in_package(file("a-1/src/raw/buffer.rs", put(), Vec::new()), "a-1"),
            in_package(file("a-2/src/raw/buffer.rs", put(), Vec::new()), "a-2"),
            in_package(file("a-2/src/write.rs", run(), call()), "a-2"),
            file("b/src/write.rs", run(), call()),
            in_package(file("c/src/buffer.rs", put(), Vec::new()), "c-1"),
            in_package(file("c/src/write.rs", run(), call()), "c-2"),
            in_package(file("c-2/buffer.rs", put(), Vec::new()), "c-2"),
        ],
    );
    let go_graph = graph_of(
        go(),
        &[
            in_package(file("a/buffer.go", put(), Vec::new()), "main"),
            in_package(file("b/buffer.go", put(), Vec::new()), "buffer"),
            in_package(file("c/write.go", run(), call()), "buffer"),
        ],
    );
    let callees_from = |graph: &Graph, path: &str| {
        let walk = graph
            .callees(Selector::new("run", Some(path)), Limits::new(1))
            .expect("the name selects a symbol");
        reached(&walk)
    };

    assert_eq!(
        callees_from(&graph, "a-2/src/write.rs"),
        ["a-2/src/raw/buffer.rs:1@85"]
    );
    assert_eq!(
        callees_from(&graph, "b/src/write.rs"),
        ["a-1/src/raw/buffer.rs:1@85"],
        "a caller of no package takes the first in path order"
    );
    assert_eq!(
        callees_from(&graph, "c/src/write.rs"),
        ["c/src/buffer.rs:1@85"],
        "the caller's directory comes before its package"
    );
    assert_eq!(
        callees_from(&go_graph, "c/write.go"),
        ["a/buffer.go:1@85"],
        "where packages are directories, a package of the same name elsewhere is another one"
    );
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

const fn in_test(mut symbol: Symbol) -> Symbol {
    symbol.flags = SymbolFlags::empty().with_test(true);
    symbol
}

#[test]
fn the_one_callable_of_a_name_binds_a_test_definition_only_for_test_code() {
    let graph = graph(&[
        file("a/x.c", vec![function("run", 1)], vec![call(0, "parse", 2)]),
        file("b/c/y.c", vec![in_test(function("parse", 1))], Vec::new()),
        file(
            "d/e/z.c",
            vec![in_test(function("check", 1))],
            vec![call(0, "parse", 2)],
        ),
    ]);

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
    assert_eq!(callees(&graph, "check"), ["b/c/y.c:1@60"]);
    assert_eq!(
        graph.resolution(0).misses.not_admitted,
        1,
        "the call from outside test code counts as one that binds no definition it may"
    );
}

#[test]
fn a_test_definition_counts_against_the_one_callable_of_its_name() {
    let graph = graph(&[
        file("a/x.c", vec![function("run", 1)], vec![call(0, "parse", 2)]),
        file("b/c/y.c", vec![function("parse", 1)], Vec::new()),
        file("d/e/z.c", vec![in_test(function("parse", 1))], Vec::new()),
    ]);

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
    assert_eq!(graph.resolution(0).misses.ambiguous, 1);
}

#[test]
fn a_call_outside_test_code_binds_the_one_type_that_a_test_type_stands_in_for() {
    let class = |name: &str| Symbol::new(name, SymbolKind::Class, 1, 3);
    let declared = graph(&[
        file(
            "a/x.c",
            vec![function("run", 1)],
            vec![call(0, "Point", 2), call(0, "parse", 3)],
        ),
        file(
            "b/c/point.h",
            vec![class("Point"), function("parse", 5)],
            Vec::new(),
        ),
        file(
            "d/e/stub.h",
            vec![in_test(class("Point")), in_test(function("parse", 5))],
            Vec::new(),
        ),
        file(
            "d/e/check.c",
            vec![in_test(function("check", 1))],
            vec![call(0, "Point", 2)],
        ),
    ]);

    assert_eq!(
        callees(&declared, "run"),
        ["b/c/point.h:1@60"],
        "a function that test code shares binds none"
    );
    assert_eq!(
        callees(&declared, "check"),
        ["d/e/stub.h:1@50"],
        "test code takes the type nearest it"
    );

    let constructed = graph(&[
        file("a/x.c", vec![function("run", 1)], vec![call(0, "Point", 2)]),
        file(
            "b/c/point.cpp",
            vec![member("Point", "Point", SymbolKind::Method, 1)],
            Vec::new(),
        ),
        file("d/e/stub.h", vec![in_test(class("Point"))], Vec::new()),
    ]);
    assert_eq!(
        callees(&constructed, "run"),
        ["b/c/point.cpp:1@60"],
        "a constructor outside test code stands for its type"
    );
}

#[test]
fn a_runtime_method_binds_no_test_definition_as_the_one_callable_for_a_call_outside_test_code() {
    let mut conventions = Conventions::default();
    conventions.runtime_methods = &["push"];
    let stack = in_test(member("Stack", "push", SymbolKind::Method, 1));
    let graph = graph_of(
        conventions,
        &[
            file(
                "a/x.js",
                vec![function("run", 1)],
                vec![member_call(0, "stack", "push", 2)],
            ),
            file("b/y.js", vec![stack], Vec::new()),
            file(
                "c/z.js",
                vec![in_test(function("check", 1))],
                vec![member_call(0, "stack", "push", 2)],
            ),
        ],
    );

    assert_eq!(callees(&graph, "run"), Vec::<String>::new());
    assert_eq!(callees(&graph, "check"), ["b/y.js:1@60"]);
}

#[test]
fn a_call_binds_no_namespace_where_the_language_binds_none_and_the_namespace_still_counts() {
    let files = [
        file(
            "a/x.rs",
            vec![
                function("run", 1),
                Symbol::new("draw", SymbolKind::Namespace, 5, 5),
                function("draw", 6),
            ],
            vec![call(0, "walk", 2), call(0, "draw", 3), call(0, "text", 4)],
        ),
        file(
            "b/lib.rs",
            vec![
                Symbol::new("walk", SymbolKind::Namespace, 1, 1),
                Symbol::new("text", SymbolKind::Namespace, 2, 2),
            ],
            Vec::new(),
        ),
        file("c/text.rs", vec![function("text", 1)], Vec::new()),
    ];
    let mut unbound = Conventions::default();
    unbound.namespace_calls = NamespaceCalls::Unbound;

    assert_eq!(
        callees(&graph(&files), "run"),
        ["a/x.rs:5@80", "b/lib.rs:1@60"]
    );
    assert_eq!(callees(&graph_of(unbound, &files), "run"), ["a/x.rs:6@80"]);
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

#[test]
fn nearness_binds_a_test_definition_only_for_test_code() {
    let calling = |nearest: &str, other: Extraction| {
        graph(&[
            file(
                "a/x/main.c",
                vec![function("run", 1)],
                vec![call(0, "parse", 2)],
            ),
            file(
                "a/x/check.c",
                vec![in_test(function("check", 1))],
                vec![call(0, "parse", 2)],
            ),
            file(nearest, vec![in_test(function("parse", 1))], Vec::new()),
            other,
        ])
    };
    let product = |path: &str| file(path, vec![function("parse", 1)], Vec::new());
    let vendored_parse = file(
        "vendor/y.c",
        vec![vendored(function("parse", 1))],
        Vec::new(),
    );
    let steps = [
        ("a/x/y.c", product("b/y.c"), "a/x/y.c:1@50"),
        ("a/y/p.c", product("b/y.c"), "a/y/p.c:1@30"),
        ("b/y.c", vendored_parse, "b/y.c:1@15"),
    ];

    for (nearest, other, bound) in steps {
        let graph = calling(nearest, other);
        assert_eq!(callees(&graph, "run"), Vec::<String>::new(), "{nearest}");
        assert_eq!(callees(&graph, "check"), [bound]);
        assert_eq!(
            graph.resolution(0).misses.ambiguous,
            1,
            "the call from outside test code counts as one that nearness does not settle"
        );
    }
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

fn through_modules(member_calls: MemberCalls) -> Conventions {
    let mut conventions = Conventions::default();
    conventions.owner_chain_separator = Some("::");
    conventions.own_type_receivers = Some(&["other"]);
    conventions.member_calls = member_calls;
    conventions
}

#[test]
fn a_member_call_that_calls_free_functions_through_modules_alone_takes_one_only_through_a_module() {
    let calling = |member_calls,
                   receiver: &str,
                   (mut caller_symbols, others): (Vec<Symbol>, Vec<Extraction>)| {
        caller_symbols.insert(0, function("run", 1));
        let mut files = vec![file(
            "a/x.rs",
            caller_symbols,
            vec![member_call(0, receiver, "join", 2)],
        )];
        files.extend(others);
        callees(&graph_of(through_modules(member_calls), &files), "run")
    };
    let elsewhere = |path: &str| file(path, vec![function("join", 1)], Vec::new());
    let layouts = || {
        [
            (vec![function("join", 5)], Vec::new()),
            (Vec::new(), vec![elsewhere("a/y.rs")]),
            (Vec::new(), vec![elsewhere("b/y.rs")]),
        ]
    };

    for receiver in ["", "other", "Index", "a::Widget"] {
        for layout in layouts() {
            assert_eq!(
                calling(MemberCalls::ModulePaths, receiver, layout),
                Vec::<String>::new(),
                "{receiver}"
            );
        }
    }
    for receiver in ["walk", "a::walk"] {
        for (layout, nearby) in layouts().into_iter().zip(layouts()).skip(1) {
            let nearby = calling(MemberCalls::NearbyFreeFunctions, receiver, nearby);
            assert_eq!(nearby.len(), 1, "{receiver}");
            assert_eq!(calling(MemberCalls::ModulePaths, receiver, layout), nearby);
        }
        assert_eq!(
            calling(
                MemberCalls::ModulePaths,
                receiver,
                (vec![function("join", 5)], Vec::new())
            ),
            Vec::<String>::new(),
            "{receiver}"
        );
    }
    assert_eq!(
        calling(
            MemberCalls::ModulePaths,
            "",
            (
                Vec::new(),
                vec![file(
                    "b/y.rs",
                    vec![member("Path", "join", SymbolKind::Method, 1)],
                    Vec::new()
                )]
            )
        ),
        ["b/y.rs:1@60"]
    );
}

#[test]
fn a_path_through_a_module_binds_in_the_caller_s_file_only_the_item_its_path_names() {
    let in_module = |chain: &str, line: u32| {
        let mut symbol = function("join", line);
        symbol.qualified_name = format!("{chain}::join");
        symbol
    };
    let calling = |receiver: &str, mut symbols: Vec<Symbol>| {
        symbols.insert(0, function("run", 1));
        let files = [
            file("a/x.rs", symbols, vec![member_call(0, receiver, "join", 2)]),
            file("b/y.rs", vec![function("join", 1)], Vec::new()),
        ];
        callees(
            &graph_of(through_modules(MemberCalls::ModulePaths), &files),
            "run",
        )
    };
    let namesakes = || {
        vec![
            function("join", 3),
            member("Path", "join", SymbolKind::Method, 4),
            in_module("inner", 5),
            in_module("a::walk", 6),
        ]
    };

    assert_eq!(calling("walk", namesakes()), ["a/x.rs:6@80"]);
    assert_eq!(calling("a::walk", namesakes()), ["a/x.rs:6@80"]);
    assert_eq!(calling("inner", namesakes()), ["a/x.rs:5@80"]);
    assert_eq!(calling("path", namesakes()), Vec::<String>::new());
    assert_eq!(
        calling("path", vec![member("path", "join", SymbolKind::Method, 4)]),
        ["a/x.rs:4@85"]
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
fn a_member_call_of_a_runtime_method_binds_no_free_function() {
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
fn a_member_call_of_a_runtime_method_binds_a_member_only_where_scope_reaches_it() {
    let mut conventions = Conventions::default();
    conventions.runtime_methods = &["push"];
    let push = |owner: &str, line: u32| member(owner, "push", SymbolKind::Method, line);
    let calling = |caller: Vec<Symbol>, others: Vec<Extraction>| {
        let mut files = vec![file("a/x.js", caller, vec![member_call(0, "", "push", 2)])];
        files.extend(others);
        graph_of(conventions, &files)
    };
    let run = || vec![function("run", 1)];

    let one_member_anywhere = calling(
        run(),
        vec![file("b/y.js", vec![push("Stack", 1)], Vec::new())],
    );
    assert_eq!(
        callees(&one_member_anywhere, "run"),
        Vec::<String>::new(),
        "the one member of the name is a namesake of the runtime's method, not what a value calls"
    );
    assert_eq!(one_member_anywhere.resolution(0).misses.not_admitted, 1);

    let one_member_nearby = calling(
        run(),
        vec![
            file("a/y.js", vec![push("Stack", 1)], Vec::new()),
            file("b/z.js", vec![push("Queue", 1)], Vec::new()),
        ],
    );
    assert_eq!(
        callees(&one_member_nearby, "run"),
        Vec::<String>::new(),
        "nor is the one member of the name in the caller's directory"
    );

    let mut own_file = run();
    own_file.push(push("Stack", 5));
    assert_eq!(
        callees(&calling(own_file, Vec::new()), "run"),
        ["a/x.js:5@80"],
        "a member the caller's own file defines still binds"
    );

    let included = graph_of(
        conventions,
        &[
            including(
                "a/x.js",
                &["lib/stack.js"],
                vec![member_call(0, "", "push", 2)],
            ),
            file("lib/stack.js", vec![push("Stack", 1)], Vec::new()),
        ],
    );
    assert_eq!(
        callees(&included, "run"),
        ["lib/stack.js:1@70"],
        "so does a member a file the caller includes defines"
    );

    let own_owner = graph_of(
        conventions,
        &[
            file(
                "a/x.js",
                vec![member("Stack", "run", SymbolKind::Method, 1)],
                vec![member_call(0, "items", "push", 2)],
            ),
            file("b/y.js", vec![push("Stack", 1)], Vec::new()),
            file("c/z.js", vec![push("Queue", 1)], Vec::new()),
        ],
    );
    assert_eq!(
        callees(&own_owner, "Stack::run"),
        ["b/y.js:1@80"],
        "and a member of the caller's own owner"
    );
}

#[test]
fn the_caller_s_owner_binds_the_copy_of_its_member_nearest_the_caller() {
    let mut conventions = Conventions::default();
    conventions.own_type_receivers = Some(&["cls", "other", "self"]);
    let graph = graph_of(
        conventions,
        &[
            file(
                "a/x.py",
                vec![member("A", "m", SymbolKind::Method, 1)],
                Vec::new(),
            ),
            file(
                "b/a.py",
                vec![member("A", "m", SymbolKind::Method, 1)],
                Vec::new(),
            ),
            file(
                "b/y.py",
                vec![
                    member("A", "f", SymbolKind::Method, 1),
                    member("A", "m", SymbolKind::Method, 3),
                    member("Inner", "m", SymbolKind::Method, 5),
                ],
                vec![member_call(0, "self", "m", 2)],
            ),
        ],
    );

    assert_eq!(
        callees(&graph, "A::f"),
        ["b/y.py:3@80"],
        "the caller's own file holds its owner's member, though another owner's member of the name leaves the file's \
         scope ambiguous"
    );
}

#[test]
fn the_caller_s_owner_binds_a_namesake_class_only_where_its_own_class_declares_a_base() {
    let qualified = |namespace: &str, name: &str, line: u32| {
        let mut symbol = member("App", name, SymbolKind::Method, line);
        symbol.qualified_name = format!("{namespace}\\App::{name}");
        symbol
    };
    let class = |parents: &[&str]| {
        let mut class = Symbol::new("App", SymbolKind::Class, 1, 10);
        class.qualified_name = "A\\App".to_owned();
        class.parents = parents.iter().map(|&parent| parent.to_owned()).collect();
        class
    };
    let calling = |parents: Option<&[&str]>, elsewhere: Vec<Symbol>| {
        let mut own = parents.map(class).into_iter().collect::<Vec<_>>();
        let caller = u32::try_from(own.len()).expect("one class at most");
        own.push(qualified("A", "f", 2));
        let graph = graph(&[
            file("a/app.php", own, vec![member_call(caller, "$this", "m", 3)]),
            file("b/app.php", elsewhere, Vec::new()),
            file(
                "c/other.php",
                vec![member("Other", "m", SymbolKind::Method, 1)],
                Vec::new(),
            ),
        ]);
        callees(&graph, "A\\App::f")
    };

    assert_eq!(
        calling(Some(&[]), vec![qualified("B", "m", 1)]),
        Vec::<String>::new(),
        "a class that declares no base has no member of another scope's class of its name"
    );
    assert_eq!(
        calling(Some(&["BaseApp"]), vec![qualified("B", "m", 1)]),
        ["b/app.php:1@80"],
        "a base written as an alias can be the class of the caller's owner's name in another scope"
    );
    assert_eq!(
        calling(
            Some(&["BaseApp"]),
            vec![class(&[]), qualified("B", "m", 12)]
        ),
        ["b/app.php:12@80"],
        "the caller's own class declares the base, not a copy of it elsewhere"
    );
    assert_eq!(
        calling(None, vec![qualified("B", "m", 1)]),
        ["b/app.php:1@80"],
        "a caller's class the graph does not hold may declare a base"
    );
    assert_eq!(
        calling(
            Some(&[]),
            vec![qualified("B", "m", 1), qualified("A", "m", 5)]
        ),
        ["b/app.php:5@80"]
    );
}

#[test]
fn a_bare_receiver_naming_no_class_of_the_graph_binds_through_includes_alone_where_it_names_a_class()
 {
    let callees_of = |bare_receivers: BareReceivers, callee: &str, receiver: &str| {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::Unreached;
        conventions.bare_receivers = bare_receivers;
        let mut test = including(
            "a/test.php",
            &["b/model.php"],
            vec![member_call(0, receiver, callee, 2)],
        );
        test.symbols
            .push(member("Dumper", "dump", SymbolKind::Method, 7));
        let graph = graph_of(
            conventions,
            &[
                test,
                file(
                    "b/model.php",
                    vec![
                        Symbol::new("Model", SymbolKind::Class, 1, 9),
                        member("Model", "boot", SymbolKind::Method, 2),
                    ],
                    Vec::new(),
                ),
                file(
                    "c/container.php",
                    vec![
                        member("Container", "mock", SymbolKind::Method, 1),
                        member("Base", "query", SymbolKind::Method, 3),
                    ],
                    Vec::new(),
                ),
            ],
        );
        callees(&graph, "run")
    };
    let classes = BareReceivers::Classes(&["parent", "self", "static"]);

    assert_eq!(
        callees_of(classes, "mock", "Mockery"),
        Vec::<String>::new(),
        "a class outside the graph has no member that the one callable of the name stands for"
    );
    assert_eq!(
        callees_of(classes, "dump", "VarDumper"),
        Vec::<String>::new(),
        "a class outside the graph has no member in the caller's file"
    );
    assert_eq!(
        callees_of(classes, "boot", "Eloquent"),
        ["b/model.php:2@70"],
        "a class the file imports under another name binds through the import"
    );
    assert_eq!(
        callees_of(classes, "query", "Model"),
        ["c/container.php:3@60"],
        "a class the graph holds can inherit the one callable of the name"
    );
    assert_eq!(
        callees_of(classes, "mock", "Self"),
        ["c/container.php:1@60"],
        "a scope keyword names the caller's class, in any case"
    );
    assert_eq!(
        callees_of(classes, "mock", "$mockery"),
        ["c/container.php:1@60"],
        "a value is no bare name"
    );
    assert_eq!(
        callees_of(BareReceivers::Any, "mock", "Mockery"),
        ["c/container.php:1@60"],
        "a bare name can hold a value where the language writes values bare"
    );
    assert_eq!(
        callees_of(BareReceivers::Any, "dump", "VarDumper"),
        ["a/test.php:7@80"]
    );
}

#[test]
fn a_runtime_method_s_call_binds_an_included_member_its_receiver_spells_or_its_caller_s_owner_reaches()
 {
    let set_value = || member("Settings", "setValue", SymbolKind::Method, 2);
    let calling = |included_runtime_members, caller: Symbol, receiver: &str| {
        let mut conventions = Conventions::default();
        conventions.runtime_methods = &["setValue"];
        conventions.included_runtime_members = included_runtime_members;
        let name = caller.qualified_name.clone();
        let mut panel = Symbol::new("Panel", SymbolKind::Class, 10, 20);
        panel.parents = vec!["Settings".to_owned()];
        let mut options = file(
            "ui/options.nut",
            vec![caller, panel],
            vec![member_call(0, receiver, "setValue", 3)],
        );
        options.includes = vec![IncludeRef {
            target: "ui/settings.nut".to_owned(),
        }];
        let graph = graph_of(
            conventions,
            &[
                options,
                file(
                    "ui/settings.nut",
                    vec![
                        Symbol::new("Settings", SymbolKind::Class, 1, 5),
                        set_value(),
                    ],
                    Vec::new(),
                ),
            ],
        );
        callees(&graph, &name)
    };
    let narrowed = IncludedRuntimeMembers::SpelledOrOwn;

    assert_eq!(
        calling(narrowed, function("onSelect", 1), "obj"),
        Vec::<String>::new(),
        "a value's name reaches no member of a class an included file declares"
    );
    assert_eq!(
        calling(IncludedRuntimeMembers::Any, function("onSelect", 1), "obj"),
        ["ui/settings.nut:2@70"]
    );
    assert_eq!(
        calling(narrowed, function("onSelect", 1), "settings"),
        ["ui/settings.nut:2@70"],
        "a receiver that spells the class reaches its member"
    );
    assert_eq!(
        calling(
            narrowed,
            member("Panel", "onSelect", SymbolKind::Method, 11),
            "this"
        ),
        ["ui/settings.nut:2@70"],
        "so does a member of a class that derives from it"
    );
}

#[test]
fn a_language_that_lists_its_own_type_receivers_binds_the_caller_s_owner_only_on_one() {
    let drain = |owner: &str| member(owner, "drain", SymbolKind::Method, 1);
    let calling = |own_type_receivers, receiver: &str| {
        let mut conventions = Conventions::default();
        conventions.own_type_receivers = own_type_receivers;
        let graph = graph_of(
            conventions,
            &[
                file(
                    "a/x.py",
                    vec![member("Pool", "run", SymbolKind::Method, 1)],
                    vec![member_call(0, receiver, "drain", 2)],
                ),
                file("b/y.py", vec![drain("Pool")], Vec::new()),
                file("c/z.py", vec![drain("Queue")], Vec::new()),
            ],
        );
        callees(&graph, "Pool::run")
    };
    let python = Some(&["cls", "other", "self"][..]);

    assert_eq!(calling(python, "self"), ["b/y.py:1@80"]);
    assert_eq!(calling(python, "cls"), ["b/y.py:1@80"]);
    assert_eq!(calling(python, "other"), ["b/y.py:1@80"]);
    assert_eq!(
        calling(python, "sock"),
        Vec::<String>::new(),
        "a value's name the language does not list has no type of the caller's"
    );
    assert_eq!(
        calling(python, "Queue"),
        ["c/z.py:1@85"],
        "a receiver that names an owner still binds that owner's member"
    );
    assert_eq!(
        calling(None, "sock"),
        ["b/y.py:1@80"],
        "a language that lists none binds the caller's owner on any name"
    );
}

#[test]
fn a_member_call_of_a_runtime_method_binds_the_one_callable_whose_owner_its_receiver_spells() {
    let mut conventions = Conventions::default();
    conventions.runtime_methods = &["push"];
    let push = |owner: &str, line: u32| member(owner, "push", SymbolKind::Method, line);
    let calling = |receiver: &str, others: Vec<Extraction>| {
        let mut files = vec![file(
            "a/x.js",
            vec![function("run", 1)],
            vec![member_call(0, receiver, "push", 2)],
        )];
        files.extend(others);
        callees(&graph_of(conventions, &files), "run")
    };
    let stack = || vec![file("b/y.js", vec![push("Stack", 1)], Vec::new())];

    assert_eq!(calling("stack", stack()), ["b/y.js:1@60"]);
    assert_eq!(calling("this.stack", stack()), ["b/y.js:1@60"]);
    assert_eq!(
        calling("items", stack()),
        Vec::<String>::new(),
        "a receiver that spells no owner binds nothing"
    );
    assert_eq!(
        calling("st", vec![file("b/y.js", vec![push("St", 1)], Vec::new())]),
        ["b/y.js:1@60"],
        "a short receiver spells its owner too"
    );

    let mut queue_too = stack();
    queue_too.push(file("c/z.js", vec![push("Queue", 1)], Vec::new()));
    assert_eq!(
        calling("stack", queue_too),
        Vec::<String>::new(),
        "beside another callable of the name, the spelling alone binds nothing"
    );
}

#[test]
fn a_member_call_binds_past_a_namesake_of_a_table_it_does_not_reach() {
    let get = |owner: &str, line: u32| member(owner, "get", SymbolKind::Method, line);
    let calling = |untyped_owners, receiver: &str, included: Vec<Symbol>| {
        let mut conventions = Conventions::default();
        conventions.untyped_owners = untyped_owners;
        let graph = graph_of(
            conventions,
            &[
                including(
                    "app/main.nut",
                    &["lib/events.nut"],
                    vec![member_call(0, receiver, "get", 2)],
                ),
                file("lib/events.nut", included, Vec::new()),
            ],
        );
        callees(&graph, "run")
    };
    let class_and_table = || {
        vec![
            Symbol::new("Events", SymbolKind::Class, 1, 9),
            get("Events", 2),
            get("leaderboards", 12),
        ]
    };

    let tables = UntypedOwners::Tables;

    assert_eq!(
        calling(tables, "events", class_and_table()),
        ["lib/events.nut:2@70"]
    );
    assert_eq!(
        calling(UntypedOwners::Types, "events", class_and_table()),
        Vec::<String>::new(),
        "where an owner no type declares can be a type outside the graph, the two owners tie"
    );
    assert_eq!(
        calling(
            tables,
            "events",
            vec![get("leaderboards", 2), get("tournaments", 12)]
        ),
        Vec::<String>::new(),
        "two tables the receiver does not spell tie"
    );
    assert_eq!(
        calling(tables, "lb", vec![get("leaderboards", 2)]),
        ["lib/events.nut:2@70"],
        "a table's member alone binds a call on a name that renames the table"
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
            including("d.cpp", &["base.h", "derived.h"], Vec::new()),
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
        .references("parse", SiteQuery::new(usize::MAX))
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
    let (from, to) = &path.candidates;
    assert_eq!(candidate_files(from), [("b.go", 0)]);
    assert_eq!(
        candidate_files(to),
        [("b.go", 0), ("c.go", 0)],
        "uncalled copies follow in path order"
    );
}

fn candidate_files<'g>(candidates: &[Candidate<'g>]) -> Vec<(&'g str, usize)> {
    candidates
        .iter()
        .map(|candidate| (candidate.symbol.file, candidate.count))
        .collect()
}

fn using(name: &str, qualifier: Option<&str>) -> UseRef {
    UseRef {
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
    caller.uses = vec![using("Config", Some("lib"))];
    let mut same_file = package(
        "pkg/lib/config.go",
        "lib",
        vec![
            exported(Symbol::new("Config", SymbolKind::Struct, 1, 2)),
            function("load", 5),
        ],
    );
    same_file.uses = vec![UseRef {
        site_of: SymbolId(1),
        ..using("Config", None)
    }];
    let mut same_package = package("pkg/lib/other.go", "lib", vec![function("other", 1)]);
    same_package.uses = vec![using("Config", None)];

    let graph = graph_of(go(), &[caller, same_file, same_package]);

    let references = graph
        .references("Config", SiteQuery::new(usize::MAX))
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
    defining.uses = vec![using("Config", None), using("Config", None)];

    let graph = graph(&[defining]);

    let limited = graph
        .references("Config", SiteQuery::new(1))
        .expect("Config is defined");
    let counted = graph
        .references("Config", SiteQuery::new(0))
        .expect("Config is defined");
    let floored = graph
        .references("Config", SiteQuery::new(usize::MAX).with_min_confidence(90))
        .expect("Config is defined");
    assert_eq!((limited.total, limited.sites.len()), (2, 1));
    assert_eq!(
        (limited.counts.calls, limited.counts.reads),
        (1, 1),
        "the counts count the sites past the limit"
    );
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
    used.uses = vec![
        UseRef {
            site_of: SymbolId(1),
            ..using("parse", None)
        },
        UseRef {
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
        .references("parse", SiteQuery::new(usize::MAX))
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
    constant.uses = [6, 7, 8]
        .into_iter()
        .map(|line| UseRef {
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
        .references("limit", SiteQuery::new(usize::MAX))
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
    caller.uses = vec![using("Limit", Some("lib"))];
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
        .references("Limit", SiteQuery::new(usize::MAX))
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
        using_file.uses = vec![UseRef {
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
            .references("Limit", SiteQuery::new(usize::MAX))
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
        .references("limit", SiteQuery::new(usize::MAX))
        .map(|references| references.symbol.kind);
    assert_eq!(field, Some(SymbolKind::Field));
    assert!(
        graph
            .references("Red", SiteQuery::new(usize::MAX))
            .is_none()
    );
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

fn with_callables(
    mut extraction: Extraction,
    role: CallableRole,
    keys: &[(u32, &str)],
) -> Extraction {
    extraction.callables = keys
        .iter()
        .map(|&(symbol, key)| Callable {
            symbol: SymbolId(symbol),
            key: key.to_owned(),
            role,
        })
        .collect();
    extraction
}

/// `net/uri.hpp` declares the class and includes each of `included` at its end, and each of those and of `elsewhere`
/// defines the members; `main.cpp` includes the class header and calls `parse`.
fn inline_header_tree(included: &[&str], elsewhere: &[&str]) -> Vec<Extraction> {
    let mut class_header = file(
        "net/uri.hpp",
        vec![
            Symbol::new("uri", SymbolKind::Class, 3, 8),
            member("uri", "parse", SymbolKind::Method, 5),
            member("uri", "reset", SymbolKind::Method, 6),
        ],
        Vec::new(),
    );
    class_header.includes = included
        .iter()
        .map(|&target| IncludeRef {
            target: target.to_owned(),
        })
        .collect();
    let keys = [
        (1, "net::uri::parse(const char *)"),
        (2, "net::uri::reset()"),
    ];
    let mut tree = vec![with_callables(
        class_header,
        CallableRole::Declaration,
        &keys,
    )];
    for &path in included.iter().chain(elsewhere) {
        let definitions = file(
            path,
            vec![
                member("uri", "reset", SymbolKind::Method, 3),
                member("uri", "parse", SymbolKind::Method, 4),
            ],
            vec![call(1, "reset", 4), member_call(1, "uri", "reset", 4)],
        );
        let keys = [
            (1, "net::uri::parse(const char *)"),
            (0, "net::uri::reset()"),
        ];
        tree.push(with_callables(definitions, CallableRole::Definition, &keys));
    }
    let mut caller = including(
        "main.cpp",
        &["net/uri.hpp"],
        vec![member_call(0, "uri", "parse", 2)],
    );
    caller.symbols = vec![function("connect", 2)];
    tree.push(caller);
    tree
}

#[test]
fn a_declaration_links_to_the_definition_in_a_header_its_own_file_includes() {
    let graph = graph_of(
        cpp(),
        &inline_header_tree(&["net/details/uri.hpp"], &["copy/net/details/uri.hpp"]),
    );

    let parse = graph
        .callees("uri::parse", Limits::new(1))
        .expect("uri::parse is defined");
    assert_eq!(
        (parse.root.file, parse.root.line),
        ("net/details/uri.hpp", 4)
    );
    assert_eq!(reached(&parse), ["net/details/uri.hpp:3@85"]);
    assert_eq!(callees(&graph, "connect"), ["net/details/uri.hpp:4@85"]);
}

#[test]
fn a_declaration_links_to_the_definition_in_a_header_its_own_file_reaches_through_another() {
    let mut tree = inline_header_tree(&["net/uri_impl.hpp"], &["net/details/uri.hpp"]);
    tree.push(including(
        "net/uri_impl.hpp",
        &["net/details/uri.hpp"],
        Vec::new(),
    ));
    let graph = graph_of(cpp(), &tree);

    assert_eq!(callees(&graph, "connect"), ["net/details/uri.hpp:4@85"]);
}

#[test]
fn a_definition_in_a_header_no_file_of_the_class_includes_links_to_nothing() {
    let graph = graph_of(
        cpp(),
        &inline_header_tree(&[], &["copy/net/details/uri.hpp"]),
    );

    assert_eq!(callees(&graph, "connect"), ["net/uri.hpp:5@85"]);
}

#[test]
fn a_declaration_whose_file_includes_two_definitions_links_to_neither() {
    let graph = graph_of(
        cpp(),
        &inline_header_tree(
            &["net/details/uri_posix.hpp", "net/details/uri_win.hpp"],
            &[],
        ),
    );

    assert_eq!(callees(&graph, "connect"), ["net/uri.hpp:5@85"]);
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

fn copies_of_parse() -> Graph {
    graph(&[
        file(
            "app/a.go",
            vec![function("parse", 1), function("once", 5)],
            vec![call(1, "parse", 6)],
        ),
        file(
            "vendor/b.go",
            vec![
                function("parse", 1),
                function("first", 5),
                function("second", 9),
                function("third", 13),
            ],
            vec![
                call(1, "parse", 6),
                call(2, "parse", 10),
                call(3, "parse", 14),
            ],
        ),
        file("vendor/c.go", vec![function("parse", 1)], Vec::new()),
    ])
}

#[test]
fn passed_over_lists_the_other_callables_the_most_called_first() {
    let graph = copies_of_parse();

    let walk = graph
        .callers("parse", Limits::new(1))
        .expect("parse is defined");
    let references = graph
        .references("parse", SiteQuery::new(usize::MAX))
        .expect("parse is defined");

    assert_eq!((walk.root.file, walk.passed_over), ("vendor/b.go", 2));
    assert_eq!(
        candidate_files(&walk.candidates),
        [("app/a.go", 1), ("vendor/c.go", 0)]
    );
    assert_eq!(
        (references.symbol.file, references.passed_over),
        ("vendor/b.go", 2)
    );
    assert_eq!(
        candidate_files(&references.candidates),
        [("app/a.go", 1), ("vendor/c.go", 0)],
        "references rank by sites"
    );
}

#[test]
fn passed_over_lists_at_most_five_and_counts_the_rest() {
    let copies: Vec<Extraction> = (0..7_u32)
        .map(|callers| {
            let mut symbols = vec![function("parse", 1)];
            let mut calls = Vec::new();
            for caller in 1..=callers {
                let line = caller * 4;
                symbols.push(function(&format!("caller{caller}"), line));
                calls.push(call(caller, "parse", line + 1));
            }
            file(&format!("p{callers}.go"), symbols, calls)
        })
        .collect();
    let graph = graph(&copies);

    let walk = graph
        .callers("parse", Limits::new(1))
        .expect("parse is defined");

    assert_eq!((walk.root.file, walk.passed_over), ("p6.go", 6));
    assert_eq!(
        candidate_files(&walk.candidates),
        [
            ("p5.go", 5),
            ("p4.go", 4),
            ("p3.go", 3),
            ("p2.go", 2),
            ("p1.go", 1)
        ],
        "the least called copy, p0.go, is counted and not listed"
    );
}

#[test]
fn a_path_prefix_selects_a_copy_the_most_called_one_hides() {
    let graph = copies_of_parse();

    let walk = graph
        .callers(Selector::new("parse", Some("app/")), Limits::new(1))
        .expect("parse is defined under app/");
    let references = graph
        .references(
            Selector::new("parse", Some("vendor/c.go")),
            SiteQuery::new(usize::MAX),
        )
        .expect("parse is defined in vendor/c.go");

    assert_eq!((walk.root.file, walk.passed_over), ("app/a.go", 0));
    assert_eq!(walk.candidates, []);
    assert_eq!(reached(&walk), ["app/a.go:5@80"]);
    assert_eq!(
        (
            references.symbol.file,
            references.total,
            references.passed_over
        ),
        ("vendor/c.go", 0, 0)
    );
    assert!(
        graph
            .callers(Selector::new("parse", Some("lib/")), Limits::new(1))
            .is_none(),
        "no definition of the name under the prefix"
    );
}

#[test]
fn a_path_prefix_narrows_an_owner_qualified_name_several_symbols_end_with_to_one() {
    let close = |qualified: &str| {
        let mut symbol = Symbol::new("Close", SymbolKind::Method, 1, 2);
        symbol.qualified_name = qualified.to_owned();
        symbol.owner = Some("Store".to_owned());
        symbol
    };
    let graph = graph(&[
        file("a/store.go", vec![close("a.Store.Close")], Vec::new()),
        file("b/store.go", vec![close("b.Store.Close")], Vec::new()),
    ]);

    let narrowed = graph
        .callers(Selector::new("Store.Close", Some("b/")), Limits::new(1))
        .expect("one Store.Close is under b/");

    assert!(graph.callers("Store.Close", Limits::new(1)).is_none());
    assert_eq!(narrowed.root.qualified_name, "b.Store.Close");
}

fn overloads_of_f() -> Graph {
    graph_of(
        cpp(),
        &[file(
            "o.cpp",
            vec![
                function("a", 1),
                function("b", 2),
                member("ns", "f", SymbolKind::Function, 4),
                member("ns", "f", SymbolKind::Function, 5),
            ],
            vec![call(2, "a", 4), call(3, "b", 5)],
        )],
    )
}

#[test]
fn a_line_selects_one_of_several_symbols_of_a_name_in_one_file() {
    let graph = overloads_of_f();
    let on = |line| Selector::new("ns::f", Some("o.cpp")).with_line(line);

    let by_file = graph
        .callees(on(None), Limits::new(1))
        .expect("ns::f is defined in o.cpp");
    let by_line = graph
        .callees(on(Some(5)), Limits::new(1))
        .expect("an ns::f starts on line 5");
    let callers = graph
        .callers(on(Some(5)), Limits::new(1))
        .expect("an ns::f starts on line 5");
    let references = graph
        .references(on(Some(5)), SiteQuery::new(usize::MAX))
        .expect("an ns::f starts on line 5");
    let path = graph
        .call_path(on(Some(5)), Selector::new("b", None), Limits::new(1))
        .expect("both ends select");
    let bare = graph
        .callees(
            Selector::new("f", Some("o.cpp")).with_line(Some(5)),
            Limits::new(1),
        )
        .expect("an f starts on line 5");

    assert_eq!((bare.root.line, bare.passed_over), (5, 0));
    assert_eq!(
        (by_file.root.line, by_file.passed_over, reached(&by_file)),
        (4, 1, vec!["o.cpp:1@80".to_owned()])
    );
    assert_eq!(
        (by_line.root.line, by_line.passed_over, reached(&by_line)),
        (5, 0, vec!["o.cpp:2@80".to_owned()])
    );
    assert_eq!((callers.root.line, callers.passed_over), (5, 0));
    assert_eq!((references.symbol.line, references.passed_over), (5, 0));
    assert!(path.found);
    assert_eq!((path.from.line, path.passed_over), (5, (0, 0)));
    assert!(
        graph.callees(on(Some(3)), Limits::new(1)).is_none(),
        "no ns::f starts on line 3"
    );
}

#[test]
fn a_name_selects_as_callers_where_its_callers_answer() {
    let close = |qualified: &str| {
        let mut symbol = Symbol::new("Close", SymbolKind::Method, 1, 2);
        symbol.qualified_name = qualified.to_owned();
        symbol.owner = Some("Store".to_owned());
        symbol
    };
    let graph = graph(&[
        file(
            "a/store.go",
            vec![close("a.Store.Close"), function("parse", 5)],
            Vec::new(),
        ),
        file(
            "b/store.go",
            vec![close("b.Store.Close"), function("parse", 5)],
            Vec::new(),
        ),
    ]);
    let written = [
        Selector::new("parse", None),
        Selector::new("parse", Some("b/")),
        Selector::new("parse", Some("c/")),
        Selector::new("Store.Close", None),
        Selector::new("Store.Close", Some("b/")),
        Selector::new("missing", None),
    ];

    let selects: Vec<bool> = written
        .iter()
        .map(|&name| graph.selects_as_callers(name))
        .collect();
    let walks: Vec<bool> = written
        .iter()
        .map(|&name| graph.callers(name, Limits::new(1)).is_some())
        .collect();

    assert_eq!(selects, [true, true, false, false, true, false]);
    assert_eq!(selects, walks);
}

#[test]
fn a_call_path_takes_a_path_prefix_at_each_end() {
    let graph = copies_of_parse();

    let into_app = graph
        .call_path(
            Selector::new("once", Some("app/")),
            Selector::new("parse", Some("app/")),
            Limits::new(2),
        )
        .expect("both are defined under app/");
    let into_vendor = graph
        .call_path(
            "first",
            Selector::new("parse", Some("vendor/b.go")),
            Limits::new(2),
        )
        .expect("both are defined");

    assert!(into_app.found);
    assert_eq!(
        (into_app.from.file, into_app.to.file),
        ("app/a.go", "app/a.go")
    );
    assert_eq!(into_app.passed_over, (0, 0));
    assert!(into_vendor.found);
    assert_eq!(into_vendor.to.file, "vendor/b.go");
    assert!(
        graph
            .call_path(
                "once",
                Selector::new("parse", Some("vendor/c.go")),
                Limits::new(2)
            )
            .is_some_and(|path| !path.found),
        "once calls the copy under app/, not the one in vendor/c.go"
    );
}

fn file_scope<'g>(walk: &Walk<'g>) -> Vec<(&'g str, u8)> {
    walk.file_scope
        .iter()
        .map(|caller| (caller.file, caller.confidence))
        .collect()
}

#[test]
fn callers_list_the_files_whose_top_level_names_a_function_by_how_each_sees_it() {
    let mut script = file(
        "bin/run.sh",
        vec![function("setup", 1)],
        vec![
            call(FILE_SCOPE.0, "setup", 9),
            call(FILE_SCOPE.0, "shared", 10),
        ],
    );
    script.includes.push(IncludeRef {
        target: "lib/common.sh".to_owned(),
    });
    let graph = graph(&[
        script,
        file("lib/common.sh", vec![function("shared", 1)], Vec::new()),
        file("lib/other.sh", vec![function("setup", 1)], Vec::new()),
    ]);
    let callers = |name: Selector<'_>, limits: Limits| {
        graph
            .callers(name, limits)
            .expect("the name selects a function")
    };

    let own = callers(Selector::new("setup", Some("bin/")), Limits::new(3));
    let included = callers(Selector::new("shared", None), Limits::new(3));
    let floored = callers(
        Selector::new("shared", None),
        Limits::new(3).with_min_confidence(75),
    );
    let unseen = callers(Selector::new("setup", Some("lib/")), Limits::new(3));

    assert_eq!(
        file_scope(&own),
        [("bin/run.sh", 80)],
        "its own file's top level"
    );
    assert!(
        own.reached.is_empty(),
        "a file is no symbol the walk reaches"
    );
    assert_eq!(
        file_scope(&included),
        [("bin/run.sh", 70)],
        "through an include"
    );
    assert_eq!(
        (file_scope(&floored), floored.below_confidence),
        (Vec::new(), 1),
        "the floor drops an include's file-scope call and counts it"
    );
    assert!(
        unseen.file_scope.is_empty(),
        "a file whose top level names a function it does not see calls another of the name"
    );
}

#[test]
fn callers_list_the_files_whose_top_level_names_a_function_in_path_order() {
    let script = |path: &str| {
        let mut script = file(path, Vec::new(), vec![call(FILE_SCOPE.0, "shared", 3)]);
        script.includes.push(IncludeRef {
            target: "lib/common.sh".to_owned(),
        });
        script
    };
    let graph = graph(&[
        script("bin/zip.sh"),
        file("lib/common.sh", vec![function("shared", 1)], Vec::new()),
        script("bin/all.sh"),
    ]);

    let walk = graph
        .callers("shared", Limits::new(3))
        .expect("shared is defined");

    assert_eq!(
        file_scope(&walk),
        [("bin/all.sh", 70), ("bin/zip.sh", 70)],
        "the files come in path order, not in the order the graph took them"
    );
}

#[test]
fn callers_list_a_package_file_whose_top_level_names_a_function_at_the_package_s_confidence() {
    let graph = graph_of(
        go(),
        &[
            file("pkg/load.go", vec![function("Load", 3)], Vec::new()),
            file(
                "pkg/vars.go",
                Vec::new(),
                vec![call(FILE_SCOPE.0, "Load", 3)],
            ),
            file(
                "cmd/vars.go",
                Vec::new(),
                vec![call(FILE_SCOPE.0, "Load", 3)],
            ),
        ],
    );

    let walk = graph
        .callers("Load", Limits::new(3))
        .expect("Load is defined");

    assert_eq!(
        file_scope(&walk),
        [("pkg/vars.go", 75)],
        "a file of the package sees Load, and one of another package does not"
    );
}

#[test]
fn callers_list_a_file_whose_top_level_passes_a_function_as_a_value() {
    let mut script = file("bin/run.sh", Vec::new(), Vec::new());
    script.uses = vec![UseRef {
        site_of: FILE_SCOPE,
        line: 4,
        ..using("shared", None)
    }];
    script.includes.push(IncludeRef {
        target: "lib/common.sh".to_owned(),
    });
    let graph = graph(&[
        script,
        file("lib/common.sh", vec![function("shared", 1)], Vec::new()),
    ]);

    let walk = graph
        .callers("shared", Limits::new(3))
        .expect("shared is defined");

    assert_eq!(
        file_scope(&walk),
        [("bin/run.sh", 70)],
        "a top level that uses a function without calling it names it all the same"
    );
}

#[test]
fn callers_of_a_field_list_no_file_whose_top_level_names_it() {
    let mut config = file(
        "app/config.ts",
        vec![
            Symbol::new("Config", SymbolKind::Class, 1, 3),
            member("Config", "limit", SymbolKind::Field, 2),
        ],
        vec![call(FILE_SCOPE.0, "limit", 5)],
    );
    config.uses = vec![UseRef {
        site_of: FILE_SCOPE,
        line: 6,
        ..using("limit", None)
    }];
    let graph = graph(&[config]);

    let walk = graph
        .callers("Config::limit", Limits::new(3))
        .expect("the field is defined");

    assert!(
        walk.file_scope.is_empty(),
        "a field is read, not called, so the top level that names it calls nothing: {:?}",
        walk.file_scope
    );
}

fn ambiguous_callers(graph: &Graph, name: &str, depth: u32) -> (Vec<String>, usize) {
    let walk = graph
        .callers(name, Limits::new(depth))
        .expect("the name selects a symbol");
    (reached(&walk), walk.ambiguous)
}

#[test]
fn callers_of_a_member_count_the_calls_through_a_receiver_its_name_s_other_members_may_answer() {
    let graph = graph(&[
        file(
            "app/run.rs",
            vec![function("run", 1), function("stop", 10)],
            vec![
                member_call(0, "token", "cancel", 2),
                member_call(0, "", "is_cancelled", 3),
                member_call(1, "", "cancel", 11),
            ],
        ),
        file(
            "walk/cancel.rs",
            vec![
                member("CancelToken", "cancel", SymbolKind::Method, 1),
                member("CancelToken", "is_cancelled", SymbolKind::Method, 5),
            ],
            Vec::new(),
        ),
        file(
            "ffi/handle.rs",
            vec![member("Handle", "cancel", SymbolKind::Method, 1)],
            Vec::new(),
        ),
    ]);

    assert_eq!(
        ambiguous_callers(&graph, "CancelToken::cancel", 1),
        (Vec::new(), 2)
    );
    assert_eq!(
        ambiguous_callers(&graph, "Handle::cancel", 1),
        (Vec::new(), 2),
        "each member the calls may mean counts them"
    );
    assert_eq!(
        ambiguous_callers(&graph, "CancelToken::cancel", 3),
        (Vec::new(), 2),
        "the count is the root's at every depth"
    );
    assert_eq!(
        ambiguous_callers(&graph, "CancelToken::is_cancelled", 1),
        (vec!["app/run.rs:1@60".to_owned()], 0),
        "the one member of a name binds its calls"
    );
    let callees = graph
        .callees("run", Limits::new(1))
        .expect("the function is defined");
    assert_eq!(
        (callees.ambiguous, callees.unresolved),
        (0, 1),
        "a walk over callees counts the call it could not bind as unresolved"
    );
    let references = graph
        .references("CancelToken::cancel", SiteQuery::new(10))
        .expect("the method is defined");
    assert_eq!(
        (references.total, references.ambiguous),
        (0, 2),
        "its references count the calls its callers count"
    );
}

#[test]
fn callers_of_a_free_function_count_its_name_s_ambiguous_calls_a_free_function_can_answer() {
    let formatters = || {
        vec![
            file("a/fmt.js", vec![function("format", 1)], Vec::new()),
            file("b/fmt.js", vec![function("format", 1)], Vec::new()),
            file(
                "c/x.js",
                vec![function("render", 1)],
                vec![call(0, "format", 2), member_call(0, "utils", "format", 3)],
            ),
        ]
    };
    let mut with_members = formatters();
    with_members.push(file(
        "d/fmt.js",
        vec![
            member("Formatter", "format", SymbolKind::Method, 1),
            member("Printer", "format", SymbolKind::Method, 5),
        ],
        Vec::new(),
    ));

    assert_eq!(
        ambiguous_callers(&graph(&formatters()), "format", 1),
        (Vec::new(), 2),
        "no member of the name answers a call through a receiver, so the free functions may"
    );
    let free = graph(&formatters());
    let references = free
        .references("format", SiteQuery::new(10))
        .expect("the function is defined");
    assert_eq!(
        (references.total, references.ambiguous),
        (0, 2),
        "the references of a free function count the calls its callers count"
    );
    assert_eq!(
        ambiguous_callers(&graph(&with_members), "format", 1),
        (Vec::new(), 1),
        "a call through a receiver means one of the members"
    );
    assert_eq!(
        ambiguous_callers(&graph(&with_members), "Formatter::format", 1),
        (Vec::new(), 1),
        "a bare call means one of the free functions"
    );
    let mut with_a_local = formatters();
    let mut holder = file(
        "e/wrap.js",
        vec![
            Symbol::new("wrap", SymbolKind::Function, 1, 10),
            function("format", 3),
        ],
        Vec::new(),
    );
    holder.local_symbols = vec![SymbolId(1)];
    with_a_local.push(holder);
    let local = graph(&with_a_local)
        .callers(Selector::new("format", Some("e/")), Limits::new(1))
        .expect("the local is defined")
        .ambiguous;
    assert_eq!(
        local, 0,
        "a call outside its body cannot mean a local function"
    );
}

fn recursive() -> Graph {
    graph(&[file(
        "a.go",
        vec![function("f", 1), function("g", 10), function("h", 20)],
        vec![
            call(0, "f", 2),
            call(1, "f", 11),
            call(2, "g", 21),
            call(2, "h", 22),
        ],
    )])
}

fn self_call(walk: &Walk<'_>) -> Option<(u32, u8)> {
    walk.self_call.map(|call| (call.line, call.confidence))
}

#[test]
fn callers_callees_and_references_agree_on_a_function_that_calls_itself() {
    let graph = recursive();

    let upward = graph.callers("f", Limits::new(1)).expect("f is defined");
    let downward = graph.callees("f", Limits::new(1)).expect("f is defined");
    let references = graph
        .references("f", SiteQuery::new(usize::MAX))
        .expect("f is defined");

    assert_eq!(reached(&upward), ["a.go:10@80"]);
    assert_eq!(self_call(&upward), Some((2, 80)));
    assert_eq!(reached(&downward), Vec::<String>::new());
    assert_eq!(self_call(&downward), Some((2, 80)));
    assert_eq!(
        references
            .sites
            .iter()
            .map(|site| (site.site_of.name, site.line))
            .collect::<Vec<_>>(),
        [("f", 2), ("g", 11)]
    );
}

#[test]
fn a_walk_from_a_function_that_calls_itself_expands_it_once() {
    let graph = recursive();

    let walk = graph.callers("f", Limits::new(5)).expect("f is defined");

    assert_eq!(reached(&walk), ["a.go:10@80", "a.go:20@80"]);
    assert_eq!(self_call(&walk), Some((2, 80)));
    assert_eq!((walk.unexpanded, walk.below_confidence), (0, 0));
}

#[test]
fn a_symbol_further_out_that_calls_itself_is_listed_once_and_is_no_self_call_of_the_root() {
    let graph = recursive();

    let walk = graph.callers("g", Limits::new(3)).expect("g is defined");

    assert_eq!(reached(&walk), ["a.go:20@80"]);
    assert_eq!(self_call(&walk), None, "h calls itself, g does not");
}

#[test]
fn the_confidence_floor_counts_a_self_call_it_drops() {
    let graph = recursive();

    let upward = graph
        .callers("f", Limits::new(1).with_min_confidence(85))
        .expect("f is defined");
    let downward = graph
        .callees("f", Limits::new(1).with_min_confidence(85))
        .expect("f is defined");

    assert_eq!(
        (self_call(&upward), upward.below_confidence),
        (None, 2),
        "the caller g and the self-call both bind below the floor"
    );
    assert_eq!((self_call(&downward), downward.below_confidence), (None, 1));
}

fn class_members() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::First;
    conventions.inherited_members = InheritedMembers::Admitted;
    conventions.owner_chain_separator = Some("::");
    conventions.class_member_separator = Some(".");
    conventions.own_type_receivers = Some(&["other"]);
    conventions
}

fn joined(owner: &str, separator: &str, name: &str, lines: [u32; 2]) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Method, lines[0], lines[1]);
    symbol.qualified_name = format!("{owner}{separator}{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

/// A file whose class `Formula` declares `build` joined with each of `separators`, in that order, and whose methods
/// call it on `self` in an instance method, on `self` in a class method, on a constant, a value and an expression, and
/// use it on a constant.
fn formula(separators: &[&str]) -> Extraction {
    let mut symbols = vec![Symbol::new("Formula", SymbolKind::Class, 1, 11)];
    for (line, separator) in (2..).zip(separators) {
        symbols.push(joined("Formula", separator, "build", [line, line]));
    }
    let first_caller = u32::try_from(symbols.len()).expect("a few symbols");
    symbols.extend([
        joined("Formula", "#", "install", [8, 8]),
        joined("Formula", ".", "make", [9, 9]),
        Symbol::new("Installer", SymbolKind::Class, 12, 20),
        joined("Installer", "#", "run", [13, 13]),
        joined("Installer", "#", "rerun", [14, 14]),
        joined("Installer", "#", "retry", [15, 15]),
    ]);
    let calls = [
        (0, "Formula"),
        (1, "Formula."),
        (3, "."),
        (4, "formula"),
        (5, ""),
    ]
    .into_iter()
    .map(|(caller, receiver)| {
        let caller = first_caller + caller;
        member_call(caller, receiver, "build", symbols[caller as usize].line)
    })
    .collect();
    let mut extraction = file("formula.rb", symbols, calls);
    extraction.uses = vec![UseRef {
        site_of: SymbolId(first_caller + 3),
        name: "build".to_owned(),
        receiver: Receiver::Path("Formula.".to_owned()),
        access: Access::Read,
        line: 13,
    }];
    extraction
}

fn sites(graph: &Graph, name: &str) -> Vec<(String, bool, u8)> {
    graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol")
        .sites
        .iter()
        .map(|site| {
            (
                site.site_of.qualified_name.to_owned(),
                site.call,
                site.confidence,
            )
        })
        .collect()
}

#[test]
fn a_receiver_on_a_class_reaches_its_own_method_first_and_any_other_receiver_the_instance_method() {
    for separators in [["#", "."], [".", "#"]] {
        let graph = graph_of(class_members(), &[formula(&separators)]);

        assert_eq!(
            (
                sites(&graph, "Formula.build"),
                sites(&graph, "Formula#build")
            ),
            (
                vec![
                    ("Formula.make".to_owned(), true, 85),
                    ("Installer#run".to_owned(), true, 80),
                    ("Installer#run".to_owned(), false, 85),
                ],
                vec![
                    ("Formula#install".to_owned(), true, 85),
                    ("Installer#rerun".to_owned(), true, 80),
                    ("Installer#retry".to_owned(), true, 80),
                ]
            ),
            "{separators:?}"
        );
    }
}

#[test]
fn an_owner_that_holds_one_kind_offers_it_to_every_receiver() {
    for (separator, name) in [("#", "Formula#build"), (".", "Formula.build")] {
        let graph = graph_of(class_members(), &[formula(&[separator])]);

        assert_eq!(
            sites(&graph, name),
            [
                ("Formula#install".to_owned(), true, 85),
                ("Formula.make".to_owned(), true, 85),
                ("Installer#run".to_owned(), true, 80),
                ("Installer#run".to_owned(), false, 85),
                ("Installer#rerun".to_owned(), true, 80),
                ("Installer#retry".to_owned(), true, 80),
            ],
            "{name}"
        );
    }
}

#[test]
fn a_use_on_a_class_takes_its_own_method_where_includes_are_transitive_too() {
    let mut conventions = class_members();
    conventions.include_reach = IncludeReach::Transitive;
    for separators in [["#", "."], [".", "#"]] {
        let graph = graph_of(conventions, &[formula(&separators)]);

        assert_eq!(
            sites(&graph, "Formula.build")
                .into_iter()
                .filter(|(_, call, _)| !call)
                .collect::<Vec<_>>(),
            [("Installer#run".to_owned(), false, 85)],
            "{separators:?}"
        );
    }
}

#[test]
fn a_class_reopened_in_another_file_offers_the_kind_its_receiver_reaches_before_a_nearer_one() {
    let reopened = |path: &str, separator: &str, caller: (&str, &str)| {
        file(
            path,
            vec![
                Symbol::new("Formula", SymbolKind::Class, 1, 9),
                joined("Formula", separator, "build", [2, 2]),
                joined("Formula", caller.0, "run", [4, 4]),
            ],
            vec![member_call(2, caller.1, "build", 4)],
        )
    };
    let graph = graph_of(
        class_members(),
        &[
            reopened("a/formula.rb", "#", (".", "Formula.")),
            reopened("b/formula/class.rb", ".", ("#", "Formula")),
        ],
    );

    assert_eq!(
        (
            sites(&graph, "Formula.build"),
            sites(&graph, "Formula#build")
        ),
        (
            vec![("Formula.run".to_owned(), true, 85)],
            vec![("Formula#run".to_owned(), true, 85)]
        )
    );
}

fn script_module(path: &str, module: &str, names: &[&str], calls: Vec<CallRef>) -> Extraction {
    let mut symbols = vec![exported(Symbol::new(module, SymbolKind::Namespace, 1, 1))];
    symbols.extend(names.iter().zip(2..).map(|(&name, line)| {
        let mut symbol = exported(function(name, line * 10));
        symbol.qualified_name = format!("{module}.{name}");
        symbol
    }));
    let mut extraction = file(path, symbols, calls);
    extraction.package = Some(module.to_owned());
    extraction
}

#[test]
fn a_call_qualified_by_a_required_module_binds_that_module_s_function_over_its_file_s_namesake() {
    let checks = script_module("game/checks.das", "checks", &["has_volume"], Vec::new());
    let binds = crate::including(
        script_module(
            "game/client/binds.das",
            "binds",
            &["has_volume", "own"],
            vec![
                member_call(1, "checks", "has_volume", 21),
                member_call(2, "binds", "has_volume", 31),
            ],
        ),
        &["game/checks.das"],
    );
    let graph = graph_of(Conventions::default(), &[checks, binds]);

    assert_eq!(
        (
            callees(&graph, "binds.has_volume"),
            callees(&graph, "binds.own")
        ),
        (
            vec!["game/checks.das:20@95".to_owned()],
            vec!["game/client/binds.das:20@80".to_owned()]
        ),
        "the module a qualifier names is the file that declares it, so the file's own module stays its own"
    );
}
