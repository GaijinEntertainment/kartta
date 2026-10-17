//! Member calls whose receiver spells a type path.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the query helpers \
              of an integration test crate are neither; a name that selects nothing is the failure"
)]

use kartta_graph::{Graph, Limits, Miss};
use kartta_lang::{
    CallRef, Conventions, Extraction, ImportBinding, MemberCalls, OwnMembers, Receivers, Symbol,
    SymbolFlags, SymbolId, SymbolKind,
};

fn go() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.directory_packages = true;
    conventions.member_calls = MemberCalls::Members;
    conventions.receivers = Receivers::TypePaths;
    conventions
}

fn function(package: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Function, line, line + 1);
    symbol.qualified_name = format!("{package}.{name}");
    symbol.exported = true;
    symbol
}

fn typed(package: &str, name: &str, kind: SymbolKind, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, kind, line, line + 1);
    symbol.qualified_name = format!("{package}.{name}");
    symbol.exported = true;
    symbol
}

fn member(qualified_owner: &str, name: &str, kind: SymbolKind, line: u32) -> Symbol {
    let (_, owner) = qualified_owner
        .split_once('.')
        .expect("an owner is qualified by its package");
    let mut symbol = Symbol::new(name, kind, line, line + 1);
    symbol.qualified_name = format!("{qualified_owner}.{name}");
    symbol.owner = Some(owner.to_owned());
    symbol.exported = true;
    symbol
}

fn yielding(mut symbol: Symbol, value_types: &[&str]) -> Symbol {
    symbol.value_types = value_types.iter().map(|&path| path.to_owned()).collect();
    symbol
}

fn embedding(mut symbol: Symbol, parents: &[&str]) -> Symbol {
    symbol.parents = parents.iter().map(|&parent| parent.to_owned()).collect();
    symbol
}

fn member_call(receiver: &str, callee: &str, line: u32) -> CallRef {
    CallRef {
        caller_id: SymbolId(0),
        callee: callee.to_owned(),
        receiver: Some(receiver.to_owned()),
        line,
    }
}

fn file(path: &str, package: &str, symbols: Vec<Symbol>, calls: Vec<CallRef>) -> Extraction {
    let mut extraction = Extraction::default();
    path.clone_into(&mut extraction.file);
    extraction.package = Some(package.to_owned());
    extraction.symbols = symbols;
    extraction.calls = calls;
    extraction
}

fn importing(mut extraction: Extraction, imports: &[(&str, Option<&str>)]) -> Extraction {
    extraction.bindings = imports
        .iter()
        .map(|&(name, target_dir)| ImportBinding {
            alias: None,
            target_dir: target_dir.map(str::to_owned),
            last_segment: name.to_owned(),
        })
        .collect();
    extraction.includes = Vec::new();
    extraction
}

fn graph(files: &[Extraction]) -> Graph {
    let mut graph = Graph::new(go());
    for extraction in files {
        graph.insert(extraction);
    }
    graph
}

fn callees_of(graph: &Graph, name: &str) -> Vec<(String, u8)> {
    let walk = graph
        .callees(name, Limits::new(1))
        .expect("the caller is held");
    walk.reached
        .iter()
        .map(|reached| {
            (
                format!("{}:{}", reached.symbol.file, reached.symbol.qualified_name),
                reached.confidence,
            )
        })
        .collect()
}

fn misses_of(graph: &Graph) -> Vec<(&str, Miss)> {
    graph
        .resolution(usize::MAX)
        .unbound
        .iter()
        .map(|unbound| (unbound.callee, unbound.miss))
        .collect()
}

fn server_package() -> Extraction {
    file(
        "srv/server.go",
        "srv",
        vec![
            typed("srv", "Server", SymbolKind::Struct, 1),
            member("srv.Server", "Close", SymbolKind::Method, 3),
            yielding(function("srv", "New", 5), &["Server"]),
        ],
        Vec::new(),
    )
}

fn namesake_package() -> Extraction {
    file(
        "other/server.go",
        "other",
        vec![
            typed("other", "Server", SymbolKind::Struct, 1),
            member("other.Server", "Close", SymbolKind::Method, 3),
        ],
        Vec::new(),
    )
}

#[test]
fn a_receiver_that_names_a_type_binds_its_method_in_the_type_s_own_package() {
    let caller = file(
        "srv/run.go",
        "srv",
        vec![function("srv", "Run", 1)],
        vec![member_call("Server", "Close", 2)],
    );

    let graph = graph(&[namesake_package(), server_package(), caller]);

    assert_eq!(
        callees_of(&graph, "srv.Run"),
        [("srv/server.go:srv.Server.Close".to_owned(), 85)]
    );
}

#[test]
fn a_receiver_path_through_an_import_binds_a_method_of_the_imported_package_s_type() {
    let caller = importing(
        file(
            "app/main.go",
            "main",
            vec![function("main", "Run", 1)],
            vec![
                member_call("srv.Server", "Close", 2),
                member_call("srv.New()", "Close", 3),
            ],
        ),
        &[("srv", Some("srv"))],
    );

    let graph = graph(&[namesake_package(), server_package(), caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("srv/server.go:srv.Server.Close".to_owned(), 85)],
        "a function's result names the type its call yields"
    );
    assert_eq!(misses_of(&graph), Vec::new(), "both paths bind");
}

#[test]
fn a_method_of_the_type_s_name_in_the_external_test_package_beside_it_is_a_namesake() {
    let mut tests = file(
        "lib/a_test.go",
        "lib_test",
        vec![
            typed("lib_test", "T", SymbolKind::Struct, 1),
            member("lib_test.T", "Close", SymbolKind::Method, 3),
        ],
        Vec::new(),
    );
    for symbol in &mut tests.symbols {
        symbol.flags = SymbolFlags::empty().with_test(true);
    }
    let library = file(
        "lib/t.go",
        "lib",
        vec![
            typed("lib", "T", SymbolKind::Struct, 1),
            member("lib.T", "Close", SymbolKind::Method, 3),
        ],
        Vec::new(),
    );
    let caller = importing(
        file(
            "app/main.go",
            "main",
            vec![function("main", "Run", 1)],
            vec![member_call("lib.T", "Close", 2)],
        ),
        &[("lib", Some("lib"))],
    );

    let graph = graph(&[tests, library, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("lib/t.go:lib.T.Close".to_owned(), 85)]
    );
}

#[test]
fn a_type_outside_the_graph_binds_no_namesake_method() {
    let caller = importing(
        file(
            "app/main.go",
            "main",
            vec![function("main", "Run", 1)],
            vec![
                member_call("http.Client", "Close", 2),
                member_call("error", "Close", 3),
            ],
        ),
        &[("http", None)],
    );

    let graph = graph(&[namesake_package(), server_package(), caller]);

    assert_eq!(callees_of(&graph, "main.Run"), Vec::new());
    assert_eq!(
        misses_of(&graph),
        [("Close", Miss::ExternalImport), ("Close", Miss::Undefined)]
    );
}

#[test]
fn fields_results_and_elements_carry_the_path_to_the_receiver_s_type() {
    let declarations = importing(
        file(
            "app/types.go",
            "main",
            vec![
                typed("main", "Holder", SymbolKind::Struct, 1),
                yielding(
                    member("main.Holder", "server", SymbolKind::Field, 2),
                    &["srv.Server"],
                ),
                yielding(function("main", "Open", 4), &["", "Holder"]),
                yielding(function("main", "All", 6), &["[]Holder"]),
                yielding(
                    typed("main", "Factory", SymbolKind::DefinedType, 8),
                    &["Holder", "error"],
                ),
                yielding(
                    typed("main", "Holders", SymbolKind::TypeAlias, 9),
                    &["[]Holder"],
                ),
            ],
            Vec::new(),
        ),
        &[("srv", Some("srv"))],
    );
    let caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        vec![
            member_call("Holder.server", "Close", 2),
            member_call("Open()#1.server", "Close", 3),
            member_call("All()[].server", "Close", 4),
            member_call("Factory().server", "Close", 5),
            member_call("Holders[].server", "Close", 6),
        ],
    );

    let graph = graph(&[namesake_package(), server_package(), declarations, caller]);

    let walk = graph
        .callees("main.Run", Limits::new(1))
        .expect("the caller is held");
    assert_eq!(walk.reached.len(), 1, "every path reaches one method");
    assert_eq!(misses_of(&graph), Vec::new(), "every path binds");
}

#[test]
fn a_method_an_embedded_type_promotes_binds_and_one_outside_the_graph_binds_nothing() {
    let declarations = importing(
        file(
            "app/types.go",
            "main",
            vec![
                embedding(
                    typed("main", "Outer", SymbolKind::Struct, 1),
                    &["srv.Server"],
                ),
                embedding(
                    typed("main", "Locked", SymbolKind::Struct, 3),
                    &["sync.Mutex"],
                ),
            ],
            Vec::new(),
        ),
        &[("srv", Some("srv")), ("sync", None)],
    );
    let caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        vec![
            member_call("Outer", "Close", 2),
            member_call("Locked", "Close", 3),
            member_call("Outer.Server", "Close", 4),
        ],
    );

    let graph = graph(&[namesake_package(), server_package(), declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("srv/server.go:srv.Server.Close".to_owned(), 85)]
    );
    assert_eq!(
        misses_of(&graph),
        [("Close", Miss::ExternalImport)],
        "a mutex may give the method"
    );
}

#[test]
fn a_name_two_files_of_a_package_declare_is_read_from_the_caller_s_own_file() {
    let other_build = importing(
        file(
            "unix/mmap_nomremap.go",
            "unix",
            vec![
                embedding(
                    typed("unix", "mmapper", SymbolKind::Struct, 1),
                    &["sync.Mutex"],
                ),
                yielding(
                    typed("unix", "mapper", SymbolKind::Variable, 3),
                    &["mmapper"],
                ),
            ],
            Vec::new(),
        ),
        &[("sync", None)],
    );
    let caller = file(
        "unix/mremap.go",
        "unix",
        vec![
            function("unix", "Remap", 1),
            typed("unix", "mremapMmapper", SymbolKind::Struct, 3),
            yielding(
                typed("unix", "mapper", SymbolKind::Variable, 5),
                &["mremapMmapper"],
            ),
            member("unix.mremapMmapper", "Mremap", SymbolKind::Method, 7),
        ],
        vec![member_call("mapper", "Mremap", 2)],
    );

    let graph = graph(&[other_build, caller]);

    assert_eq!(
        callees_of(&graph, "unix.Remap"),
        [("unix/mremap.go:unix.mremapMmapper.Mremap".to_owned(), 85)]
    );
}

#[test]
fn a_type_with_no_method_of_the_name_binds_none() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            typed("main", "Plain", SymbolKind::Struct, 1),
            member("main.Plain", "Close", SymbolKind::Field, 2),
            member("main.Plain", "Other", SymbolKind::Method, 3),
        ],
        Vec::new(),
    );
    let caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        vec![member_call("Plain", "Close", 2)],
    );

    let graph = graph(&[server_package(), declarations, caller]);

    assert!(
        callees_of(&graph, "main.Run").is_empty(),
        "a field of a function type holds no method"
    );
    assert_eq!(misses_of(&graph), [("Close", Miss::NotAdmitted)]);
}

#[test]
fn a_path_the_graph_cannot_follow_leaves_the_call_to_the_tiers_that_read_no_type() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            typed("main", "Plain", SymbolKind::Variable, 1),
            typed("main", "Local", SymbolKind::Struct, 2),
            member("main.Local", "Close", SymbolKind::Method, 3),
        ],
        Vec::new(),
    );
    let caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        vec![member_call("Plain.unknown", "Close", 2)],
    );

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("app/types.go:main.Local.Close".to_owned(), 75)],
        "a variable whose type is not recorded reads like a value of no known type"
    );
}

#[test]
fn a_type_local_to_a_body_gives_its_fields() {
    let mut caller = file(
        "app/main.go",
        "main",
        vec![
            function("main", "Run", 1),
            typed("main", "stage", SymbolKind::Struct, 2),
            yielding(
                member("main.stage", "server", SymbolKind::Field, 2),
                &["srv.Server"],
            ),
        ],
        vec![member_call("stage.server", "Close", 3)],
    );
    caller.local_symbols = vec![SymbolId(1), SymbolId(2)];
    let caller = importing(caller, &[("srv", Some("srv"))]);

    let graph = graph(&[namesake_package(), server_package(), caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("srv/server.go:srv.Server.Close".to_owned(), 85)]
    );
}
