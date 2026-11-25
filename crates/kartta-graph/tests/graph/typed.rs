//! Member calls whose receiver spells a type path.

use kartta_graph::{Graph, Limits, Miss, SiteCounts, SiteQuery};
use kartta_lang::{
    Access, BuiltApart, Conventions, Extraction, Implements, MemberCalls, OwnMembers, Receiver,
    Receivers, SameMethod, Symbol, SymbolFlags, SymbolId, SymbolKind, Twins, UseRef,
};

use crate::go_code::{callees_of, file, function, importing};
use crate::{graph_of, member_call};

fn go() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.directory_packages = true;
    conventions.member_calls = MemberCalls::Members;
    conventions.receivers = Receivers::TypePaths;
    conventions
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

fn graph(files: &[Extraction]) -> Graph {
    graph_of(go(), files)
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
fn a_receiver_that_names_a_type_binds_no_method_that_takes_fewer_than_the_call_passes() {
    let passing = |arguments| {
        let mut call = member_call("Server", "Close", 2);
        call.arguments = arguments;
        call
    };
    let mut library = server_package();
    library.symbols[1].most_arguments = 0;
    let fits = file(
        "srv/fits.go",
        "srv",
        vec![function("srv", "Fits", 1)],
        vec![passing(1)],
    );
    let over = file(
        "srv/over.go",
        "srv",
        vec![function("srv", "Over", 1)],
        vec![passing(2)],
    );

    let graph = graph(&[library, fits, over]);

    assert_eq!(
        callees_of(&graph, "srv.Fits"),
        [("srv/server.go:srv.Server.Close".to_owned(), 85)]
    );
    assert_eq!(callees_of(&graph, "srv.Over"), Vec::new());
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
                    &["()", "Holder", "error"],
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
            member_call("Factory~().server", "Close", 5),
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

fn embedded_beside_an_import() -> Extraction {
    importing(
        file(
            "app/types.go",
            "main",
            vec![
                embedding(
                    typed("main", "Outer", SymbolKind::Struct, 1),
                    &["http.Client", "srv.Server"],
                ),
                embedding(
                    typed("main", "Plain", SymbolKind::Struct, 3),
                    &["srv.Server"],
                ),
                embedding(typed("main", "Mid", SymbolKind::Struct, 5), &["srv.Server"]),
                embedding(typed("main", "Upper", SymbolKind::Struct, 7), &["Mid"]),
                embedding(
                    typed("main", "SameDepth", SymbolKind::Struct, 9),
                    &["http.Client", "Mid"],
                ),
                embedding(
                    typed("main", "ImportNearer", SymbolKind::Struct, 11),
                    &["http.Client", "Upper"],
                ),
            ],
            Vec::new(),
        ),
        &[("srv", Some("srv")), ("http", None)],
    )
}

#[test]
fn an_embedded_field_beside_an_import_binds_at_any_depth() {
    let caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        vec![
            member_call("Outer.Server", "Close", 2),
            member_call("Plain.Server", "Close", 3),
            member_call("SameDepth.Server", "Close", 4),
            member_call("ImportNearer.Server", "Close", 5),
        ],
    );

    let graph = graph(&[server_package(), embedded_beside_an_import(), caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("srv/server.go:srv.Server.Close".to_owned(), 85)]
    );
    assert_eq!(misses_of(&graph), Vec::new(), "every path binds");
}

#[test]
fn a_shallower_embedded_field_outranks_a_deeper_promoted_member_of_its_name() {
    let declarations = importing(
        file(
            "app/types.go",
            "main",
            vec![
                typed("main", "Mid", SymbolKind::Struct, 1),
                member("main.Mid", "Server", SymbolKind::Method, 2),
                embedding(typed("main", "Wrap", SymbolKind::Struct, 3), &["Mid"]),
                embedding(
                    typed("main", "Outer", SymbolKind::Struct, 5),
                    &["Wrap", "srv.Server"],
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
        vec![member_call("Outer.Server", "Close", 2)],
    );

    let graph = graph(&[server_package(), declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("srv/server.go:srv.Server.Close".to_owned(), 85)]
    );
}

#[test]
fn a_name_no_embedded_field_gives_stays_outside_the_graph_beside_an_import() {
    let caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        vec![
            member_call("Outer.Transport", "RoundTrip", 2),
            member_call("Outer.Client", "Do", 3),
        ],
    );

    let graph = graph(&[server_package(), embedded_beside_an_import(), caller]);

    assert_eq!(callees_of(&graph, "main.Run"), Vec::new());
    assert_eq!(
        misses_of(&graph),
        [
            ("RoundTrip", Miss::ExternalImport),
            ("Do", Miss::ExternalImport)
        ],
        "the import may give what no held type names"
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
fn a_file_s_own_declaration_stands_beside_a_type_a_sibling_file_names_later() {
    let other_build = file(
        "dec/z.go",
        "dec",
        vec![
            typed("dec", "Zed", SymbolKind::Struct, 1),
            member("dec.Zed", "Set", SymbolKind::Method, 2),
            yielding(typed("dec", "held", SymbolKind::Variable, 3), &["Zed"]),
        ],
        Vec::new(),
    );
    let caller = file(
        "dec/a.go",
        "dec",
        vec![
            function("dec", "run", 1),
            typed("dec", "Ace", SymbolKind::Struct, 3),
            member("dec.Ace", "Set", SymbolKind::Method, 4),
            yielding(typed("dec", "held", SymbolKind::Variable, 5), &["Ace"]),
        ],
        vec![member_call("held", "Set", 2)],
    );

    let graph = graph(&[other_build, caller]);

    assert_eq!(
        callees_of(&graph, "dec.run"),
        [("dec/a.go:dec.Ace.Set".to_owned(), 85)]
    );
}

fn declaring_new_over(path: &str, value_type: &str) -> Extraction {
    importing(
        file(
            path,
            "dec",
            vec![
                typed("dec", "Dec", SymbolKind::Struct, 1),
                member("dec.Dec", "Set", SymbolKind::Method, 2),
                yielding(function("dec", "New", 3), &[value_type]),
            ],
            Vec::new(),
        ),
        &[("http", None)],
    )
}

fn reading_new_of(path: &str, package: &str, spelled: &str, imports: &[&str]) -> Extraction {
    let imports: Vec<(&str, Option<&str>)> = imports
        .iter()
        .map(|&import| (import, Some(import)))
        .collect();
    importing(
        file(
            path,
            package,
            vec![function(package, "run", 1)],
            vec![member_call(spelled, "Set", 2)],
        ),
        &imports,
    )
}

#[test]
fn a_name_two_files_declare_reads_what_either_reaches_whichever_comes_first_in_path_order() {
    for (first, second) in [("dec/a.go", "dec/b.go"), ("dec/b.go", "dec/a.go")] {
        let graph = graph(&[
            declaring_new_over(first, "http.Client"),
            declaring_new_over(second, "Dec"),
            reading_new_of("m.go", "m", "dec.New()", &["dec"]),
        ]);

        assert_eq!(
            callees_of(&graph, "m.run"),
            [("dec/a.go:dec.Dec.Set".to_owned(), 85)],
            "with the import's declaration in {first}"
        );
    }
}

fn variable_file(path: &str, name: &str, read: &str) -> Extraction {
    let declared = typed("x", name, SymbolKind::Variable, 1);
    file(path, "x", vec![yielding(declared, &[read])], Vec::new())
}

fn doubled_chain(steps: usize, calls: Vec<kartta_lang::CallRef>) -> Graph {
    let mut files = vec![
        file(
            "x/t.go",
            "x",
            vec![
                typed("x", "T", SymbolKind::Struct, 1),
                member("x.T", "Set", SymbolKind::Method, 2),
                member("x.T", "Get", SymbolKind::Method, 3),
                embedding(typed("x", "Over", SymbolKind::Struct, 4), &["T"]),
                typed("x", "U", SymbolKind::Struct, 5),
                member("x.U", "Get", SymbolKind::Method, 6),
                yielding(typed("x", "over", SymbolKind::Variable, 7), &["Over"]),
                embedding(
                    typed("x", "Own", SymbolKind::Struct, 8),
                    &["own.a", "own.b", "own.c"],
                ),
                yielding(typed("x", "own", SymbolKind::Variable, 9), &["Own"]),
            ],
            Vec::new(),
        ),
        file("x/run.go", "x", vec![function("x", "run", 1)], calls),
    ];
    for step in 0..steps {
        let name = format!("v{step}");
        let read = format!("v{}", step + 1);
        for copy in 0..2 {
            files.push(variable_file(&format!("x/{name}_{copy}.go"), &name, &read));
        }
    }
    files.push(variable_file("x/v0_t.go", "v0", "T"));
    graph(&files)
}

#[test]
fn a_read_whose_declarations_multiply_past_the_bound_binds_nothing() {
    let read = || vec![member_call("v0", "Set", 2)];
    assert_eq!(
        callees_of(&doubled_chain(4, read()), "x.run"),
        [("x/t.go:x.T.Set".to_owned(), 85)],
        "a read inside the bound reaches the last declaration"
    );

    let past = doubled_chain(11, read());

    assert_eq!(callees_of(&past, "x.run"), []);
    assert_eq!(misses_of(&past), [("Set", Miss::PastBound)]);
}

#[test]
fn a_chain_of_declarations_past_the_depth_counts_as_a_read_the_graph_stopped() {
    let chain = |steps: usize| {
        let mut files: Vec<Extraction> = (0..steps)
            .map(|step| {
                let read = if step + 1 == steps {
                    "T".to_owned()
                } else {
                    format!("v{}", step + 1)
                };
                variable_file(&format!("x/v{step}.go"), &format!("v{step}"), &read)
            })
            .collect();
        files.push(file(
            "x/t.go",
            "x",
            vec![
                typed("x", "T", SymbolKind::Struct, 1),
                member("x.T", "Set", SymbolKind::Method, 2),
            ],
            Vec::new(),
        ));
        let calls = vec![member_call("v0", "Set", 2)];
        files.push(file("x/run.go", "x", vec![function("x", "run", 1)], calls));
        graph(&files)
    };
    assert_eq!(misses_of(&chain(4)), []);

    let past = chain(20);

    let resolution = past.resolution(0);
    assert_eq!(resolution.sites, 1, "a site past the bound is a site");
    assert_eq!(resolution.misses.past_bound, 1);
    assert_eq!(resolution.misses.not_admitted, 0);
    assert_eq!(
        resolution.misses.through_a_receiver, 0,
        "the receiver has no type at all, so it is no value of a type the graph lacks"
    );
}

#[test]
fn a_member_walk_after_a_read_past_the_bound_has_a_bound_of_its_own() {
    let graph = doubled_chain(
        11,
        vec![
            member_call("over", "Get", 2),
            member_call("v0", "Set", 3),
            member_call("over", "Get", 4),
        ],
    );

    assert_eq!(
        callees_of(&graph, "x.run"),
        [("x/t.go:x.T.Get".to_owned(), 85)]
    );
    assert_eq!(misses_of(&graph), [("Set", Miss::PastBound)]);
}

#[test]
fn a_read_after_a_member_walk_past_the_bound_has_a_bound_of_its_own() {
    let graph = doubled_chain(
        1,
        vec![member_call("own", "Zap", 2), member_call("over", "Get", 3)],
    );

    assert_eq!(
        callees_of(&graph, "x.run"),
        [("x/t.go:x.T.Get".to_owned(), 85)]
    );
    assert_eq!(misses_of(&graph), [("Zap", Miss::Undefined)]);
}

#[test]
fn a_third_file_of_the_package_reads_what_either_declaration_reaches() {
    for (first, second) in [("dec/a.go", "dec/b.go"), ("dec/b.go", "dec/a.go")] {
        let graph = graph(&[
            declaring_new_over(first, "http.Client"),
            declaring_new_over(second, "Dec"),
            reading_new_of("dec/c.go", "dec", "New()", &[]),
        ]);

        assert_eq!(
            callees_of(&graph, "dec.run"),
            [("dec/a.go:dec.Dec.Set".to_owned(), 85)],
            "with the import's declaration in {first}"
        );
    }
}

#[test]
fn a_declaration_that_reaches_an_import_outranks_one_that_reaches_nothing_declared() {
    for (first, second) in [("dec/a.go", "dec/b.go"), ("dec/b.go", "dec/a.go")] {
        let graph = graph(&[
            declaring_new_over(first, "http.Client"),
            declaring_new_over(second, "int"),
            reading_new_of("m.go", "m", "dec.New()", &["dec"]),
        ]);

        assert_eq!(
            misses_of(&graph),
            [("Set", Miss::ExternalImport)],
            "with the import's declaration in {first}"
        );
    }
}

#[test]
fn a_declaration_whose_path_stops_outranks_one_that_reaches_nothing_declared() {
    for (first, second) in [("dec/a.go", "dec/b.go"), ("dec/b.go", "dec/a.go")] {
        let graph = graph(&[
            declaring_new_over(first, "Dec.Set()"),
            declaring_new_over(second, "int"),
            reading_new_of("m.go", "m", "dec.New()", &["dec"]),
        ]);

        assert!(
            !misses_of(&graph).contains(&("Set", Miss::Undefined)),
            "with the path that stops in {first}: {:?}",
            misses_of(&graph)
        );
    }
}

#[test]
fn an_alias_of_an_import_reads_as_the_import_beside_a_held_type() {
    let alias = |path: &str, target: &str| {
        importing(
            file(
                path,
                "dec",
                vec![
                    yielding(typed("dec", "Dec", SymbolKind::TypeAlias, 1), &[target]),
                    yielding(function("dec", "New", 2), &["Dec"]),
                ],
                Vec::new(),
            ),
            &[("http", None), ("held", Some("held"))],
        )
    };
    let held = file(
        "held/held.go",
        "held",
        vec![
            typed("held", "Dec", SymbolKind::Struct, 1),
            member("held.Dec", "Set", SymbolKind::Method, 2),
        ],
        Vec::new(),
    );
    for (first, second) in [("dec/a.go", "dec/b.go"), ("dec/b.go", "dec/a.go")] {
        let graph = graph(&[
            alias(first, "held.Dec"),
            alias(second, "http.Client"),
            held.clone(),
            reading_new_of("m.go", "m", "dec.New()", &["dec"]),
        ]);

        assert_eq!(
            callees_of(&graph, "m.run"),
            [("held/held.go:held.Dec.Set".to_owned(), 85)],
            "with the held alias in {first}"
        );
    }
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

fn defined_over(name: &str, underlying: &str, line: u32) -> Symbol {
    yielding(
        typed("main", name, SymbolKind::DefinedType, line),
        &[underlying],
    )
}

fn caller_of(calls: Vec<kartta_lang::CallRef>) -> Extraction {
    file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        calls,
    )
}

#[test]
fn a_type_defined_over_an_interface_has_its_methods_through_defined_types_and_aliases() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            typed("main", "Logger", SymbolKind::Interface, 1),
            member("main.Logger", "Info", SymbolKind::Method, 2),
            defined_over("Direct", "~Logger", 4),
            yielding(
                typed("main", "Named", SymbolKind::TypeAlias, 5),
                &["Direct"],
            ),
            defined_over("Chained", "~Named", 6),
            embedding(typed("main", "Holder", SymbolKind::Struct, 7), &["Chained"]),
        ],
        Vec::new(),
    );
    let caller = caller_of(vec![
        member_call("Direct", "Info", 2),
        member_call("Chained", "Info", 3),
        member_call("Holder", "Info", 4),
    ]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("app/types.go:main.Logger.Info".to_owned(), 85)],
        "every path reaches the interface"
    );
    assert_eq!(misses_of(&graph), Vec::new());
}

#[test]
fn a_type_defined_over_a_type_that_is_no_interface_has_none_of_its_methods() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            typed("main", "Plain", SymbolKind::Struct, 1),
            member("main.Plain", "Close", SymbolKind::Method, 2),
            defined_over("Direct", "~Plain", 4),
            defined_over("Chained", "~Direct", 5),
            defined_over("Outside", "~missing", 6),
            yielding(typed("main", "Builtin", SymbolKind::TypeAlias, 7), &["int"]),
        ],
        Vec::new(),
    );
    let caller = caller_of(vec![
        member_call("Direct", "Close", 2),
        member_call("Chained", "Close", 3),
        member_call("Outside", "Close", 4),
        member_call("Builtin", "Close", 5),
    ]);

    let graph = graph(&[declarations, caller]);

    assert!(
        callees_of(&graph, "main.Run").is_empty(),
        "a defined type keeps its underlying type's fields and none of its methods"
    );
    assert_eq!(
        misses_of(&graph),
        [
            ("Close", Miss::NotAdmitted),
            ("Close", Miss::NotAdmitted),
            ("Close", Miss::NotAdmitted),
            ("Close", Miss::NotAdmitted),
        ]
    );
}

#[test]
fn a_read_through_a_type_over_an_import_stays_outside_the_graph() {
    let declarations = importing(
        file(
            "app/types.go",
            "main",
            vec![
                defined_over("Elements", "~lib.List", 1),
                defined_over("Calls", "~lib.Func", 2),
                yielding(
                    typed("main", "Aliased", SymbolKind::TypeAlias, 3),
                    &["lib.List"],
                ),
            ],
            Vec::new(),
        ),
        &[("lib", None)],
    );
    let caller = caller_of(vec![
        member_call("Elements[]", "Close", 2),
        member_call("Calls~()", "Close", 3),
        member_call("Aliased[]", "Close", 4),
    ]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        misses_of(&graph),
        [
            ("Close", Miss::ExternalImport),
            ("Close", Miss::ExternalImport),
            ("Close", Miss::ExternalImport),
        ]
    );
}

#[test]
fn a_read_through_a_cycle_of_defined_types_stops_past_its_bound() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            defined_over("Loop", "~Back", 1),
            defined_over("Back", "~Loop", 2),
        ],
        Vec::new(),
    );
    let caller = caller_of(vec![
        member_call("Loop[]", "Close", 2),
        member_call("Loop~()", "Close", 3),
    ]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        misses_of(&graph),
        [("Close", Miss::PastBound), ("Close", Miss::PastBound)]
    );
}

#[test]
fn a_call_through_a_package_converts_at_a_type_and_calls_at_a_value() {
    let declarations = file(
        "lib/lib.go",
        "lib",
        vec![
            typed("lib", "Result", SymbolKind::Struct, 1),
            member("lib.Result", "Close", SymbolKind::Method, 2),
            yielding(
                typed("lib", "Func", SymbolKind::DefinedType, 3),
                &["()", "Result"],
            ),
            member("lib.Func", "Close", SymbolKind::Method, 4),
            yielding(typed("lib", "Held", SymbolKind::Variable, 5), &["Func"]),
        ],
        Vec::new(),
    );
    let caller = importing(
        caller_of(vec![
            member_call("lib.Func()", "Close", 2),
            member_call("lib.Held()", "Close", 3),
        ]),
        &[("lib", Some("lib"))],
    );

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [
            ("lib/lib.go:lib.Result.Close".to_owned(), 85),
            ("lib/lib.go:lib.Func.Close".to_owned(), 85),
        ]
    );
}

#[test]
fn a_type_over_an_imported_type_takes_its_members_from_outside_the_graph() {
    let declarations = importing(
        file(
            "app/types.go",
            "main",
            vec![
                defined_over("Direct", "~http.Handler", 1),
                defined_over("Chained", "~Direct", 2),
                yielding(
                    typed("main", "Aliased", SymbolKind::TypeAlias, 3),
                    &["http.Handler"],
                ),
                defined_over("OverAlias", "~Aliased", 4),
                typed("main", "Namesake", SymbolKind::Interface, 5),
                member("main.Namesake", "Transport", SymbolKind::Method, 6),
                member("main.Namesake", "RoundTrip", SymbolKind::Method, 7),
            ],
            Vec::new(),
        ),
        &[("http", None)],
    );
    let caller = caller_of(vec![
        member_call("Direct", "ServeHTTP", 2),
        member_call("Chained", "ServeHTTP", 3),
        member_call("Aliased", "ServeHTTP", 4),
        member_call("OverAlias", "ServeHTTP", 5),
        member_call("Direct.Transport", "RoundTrip", 6),
    ]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        Vec::new(),
        "a member of the imported type is no namesake the graph holds"
    );
    assert_eq!(
        misses_of(&graph),
        [
            ("ServeHTTP", Miss::ExternalImport),
            ("ServeHTTP", Miss::ExternalImport),
            ("ServeHTTP", Miss::ExternalImport),
            ("ServeHTTP", Miss::ExternalImport),
            ("RoundTrip", Miss::ExternalImport),
        ]
    );
}

#[test]
fn types_defined_over_one_another_end_the_walk_with_no_method() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            defined_over("Left", "~Right", 1),
            defined_over("Right", "~Left", 2),
        ],
        Vec::new(),
    );
    let caller = caller_of(vec![member_call("Left", "Close", 2)]);

    let graph = graph(&[declarations, caller]);

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

fn accessing(name: &str, path: &str, access: Access, line: u32) -> UseRef {
    let mut site = UseRef::new(
        SymbolId(0),
        name.to_owned(),
        Receiver::Path(path.to_owned()),
        line,
    );
    site.access = access;
    site
}

fn config_package() -> Extraction {
    file(
        "cfg/config.go",
        "cfg",
        vec![
            typed("cfg", "Config", SymbolKind::Struct, 1),
            member("cfg.Config", "Timeout", SymbolKind::Field, 2),
            member("cfg.Config", "Reset", SymbolKind::Method, 4),
        ],
        Vec::new(),
    )
}

fn sites_of(graph: &Graph, name: &str) -> Vec<(String, u32, u8)> {
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    references
        .sites
        .iter()
        .map(|site| {
            (
                site.site_of.qualified_name.to_owned(),
                site.line,
                site.confidence,
            )
        })
        .collect()
}

fn struct_with_a_promoted_method() -> Vec<Symbol> {
    vec![
        typed("main", "Inner", SymbolKind::Struct, 1),
        member("main.Inner", "Deep", SymbolKind::Field, 2),
        member("main.Inner", "Promoted", SymbolKind::Method, 3),
        member("main.Inner", "Shadowed", SymbolKind::Method, 4),
        embedding(typed("main", "S", SymbolKind::Struct, 5), &["Inner"]),
        member("main.S", "X", SymbolKind::Field, 6),
        member("main.S", "Own", SymbolKind::Method, 7),
        member("main.S", "Shadowed", SymbolKind::Method, 8),
    ]
}

fn over_the_struct(line: u32) -> Vec<Symbol> {
    vec![
        defined_over("T", "~S", line),
        member("main.T", "Mine", SymbolKind::Method, line + 1),
        defined_over("Chained", "~T", line + 2),
        yielding(
            typed("main", "Named", SymbolKind::TypeAlias, line + 3),
            &["S"],
        ),
        defined_over("OverAlias", "~Named", line + 4),
    ]
}

#[test]
fn a_type_defined_over_a_struct_keeps_what_its_embedded_fields_promote() {
    let mut symbols = struct_with_a_promoted_method();
    symbols.extend(over_the_struct(10));
    let declarations = file("app/types.go", "main", symbols, Vec::new());
    let caller = caller_of(vec![
        member_call("T", "Promoted", 2),
        member_call("Chained", "Promoted", 3),
        member_call("OverAlias", "Promoted", 4),
        member_call("T.Inner", "Promoted", 5),
    ]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("app/types.go:main.Inner.Promoted".to_owned(), 85)],
        "over a struct, over a defined type and over an alias, and through the embedded field's own name"
    );
    assert_eq!(misses_of(&graph), Vec::new());
}

#[test]
fn a_type_defined_over_a_struct_keeps_its_fields_at_every_depth() {
    let mut symbols = struct_with_a_promoted_method();
    symbols.extend(over_the_struct(10));
    let declarations = file("app/types.go", "main", symbols, Vec::new());
    let mut caller = caller_of(Vec::new());
    caller.uses = vec![
        accessing("X", "T", Access::Read, 2),
        accessing("X", "Chained", Access::Read, 3),
        accessing("X", "OverAlias", Access::Read, 4),
        accessing("Deep", "T", Access::Read, 5),
        accessing("Deep", "T.Inner", Access::Read, 6),
    ];

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        sites_of(&graph, "main.S.X"),
        [
            ("main.Run".to_owned(), 2, 85),
            ("main.Run".to_owned(), 3, 85),
            ("main.Run".to_owned(), 4, 85)
        ]
    );
    assert_eq!(
        sites_of(&graph, "main.Inner.Deep"),
        [
            ("main.Run".to_owned(), 5, 85),
            ("main.Run".to_owned(), 6, 85)
        ]
    );
}

#[test]
fn a_type_defined_over_a_struct_takes_a_promoted_method_past_the_struct_s_own() {
    let mut symbols = struct_with_a_promoted_method();
    symbols.extend(over_the_struct(10));
    let declarations = file("app/types.go", "main", symbols, Vec::new());
    let caller = caller_of(vec![
        member_call("T", "Shadowed", 2),
        member_call("Named", "Shadowed", 3),
    ]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [
            ("app/types.go:main.Inner.Shadowed".to_owned(), 85),
            ("app/types.go:main.S.Shadowed".to_owned(), 85),
        ],
        "T drops S.Shadowed and reaches the embedded one, where an alias of S keeps it"
    );
    assert_eq!(misses_of(&graph), Vec::new());
}

#[test]
fn a_selector_past_a_type_over_a_struct_goes_on_through_the_promoted_method_s_result() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            typed("main", "Inner", SymbolKind::Struct, 1),
            yielding(
                member("main.Inner", "Open", SymbolKind::Method, 2),
                &["Right"],
            ),
            embedding(typed("main", "S", SymbolKind::Struct, 3), &["Inner"]),
            yielding(member("main.S", "Open", SymbolKind::Method, 4), &["Wrong"]),
            defined_over("T", "~S", 5),
            typed("main", "Right", SymbolKind::Struct, 6),
            member("main.Right", "Close", SymbolKind::Method, 7),
            typed("main", "Wrong", SymbolKind::Struct, 8),
            member("main.Wrong", "Close", SymbolKind::Method, 9),
        ],
        Vec::new(),
    );
    let caller = caller_of(vec![member_call("T.Open()", "Close", 2)]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("app/types.go:main.Right.Close".to_owned(), 85)],
        "the read of Open passes S.Open as the walk of a last selector does"
    );
}

#[test]
fn a_type_defined_over_a_struct_has_only_its_own_methods_at_its_own_depth() {
    let mut symbols = struct_with_a_promoted_method();
    symbols.extend(over_the_struct(10));
    let declarations = file("app/types.go", "main", symbols, Vec::new());
    let caller = caller_of(vec![
        member_call("T", "Own", 2),
        member_call("Chained", "Mine", 3),
        member_call("T", "Mine", 4),
    ]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("app/types.go:main.T.Mine".to_owned(), 85)],
        "the struct's method and the method of the defined type in between stay off"
    );
    assert_eq!(
        misses_of(&graph),
        [("Own", Miss::NotAdmitted), ("Mine", Miss::NotAdmitted)]
    );
}

#[test]
fn a_struct_under_a_defined_type_sits_at_that_type_s_depth() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            typed("main", "Near", SymbolKind::Struct, 1),
            member("main.Near", "Y", SymbolKind::Field, 2),
            embedding(typed("main", "Far", SymbolKind::Struct, 3), &["Near"]),
            typed("main", "Under", SymbolKind::Struct, 4),
            member("main.Under", "Y", SymbolKind::Field, 5),
            defined_over("Sib", "~Under", 6),
            embedding(
                typed("main", "Order", SymbolKind::Struct, 7),
                &["Far", "Sib"],
            ),
        ],
        Vec::new(),
    );
    let mut caller = caller_of(Vec::new());
    caller.uses = vec![accessing("Y", "Order", Access::Read, 2)];

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        sites_of(&graph, "main.Under.Y"),
        [("main.Run".to_owned(), 2, 85)],
        "Sib holds Y at depth 1, and Far only at depth 2"
    );
}

#[test]
fn a_method_promoted_through_a_struct_that_embeds_itself_past_a_defined_type_binds() {
    let declarations = file(
        "app/types.go",
        "main",
        vec![
            embedding(typed("main", "S", SymbolKind::Struct, 1), &["S"]),
            member("main.S", "M", SymbolKind::Method, 2),
            defined_over("T", "~S", 3),
        ],
        Vec::new(),
    );
    let caller = caller_of(vec![member_call("T", "M", 2)]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(
        callees_of(&graph, "main.Run"),
        [("app/types.go:main.S.M".to_owned(), 85)],
        "the embedded S keeps the methods the S under T drops"
    );
}

#[test]
fn a_member_a_type_over_a_struct_cannot_give_reaches_the_import_the_struct_embeds() {
    let declarations = importing(
        file(
            "app/types.go",
            "main",
            vec![
                embedding(typed("main", "S", SymbolKind::Struct, 1), &["http.Client"]),
                defined_over("T", "~S", 2),
            ],
            Vec::new(),
        ),
        &[("http", None)],
    );
    let caller = caller_of(vec![member_call("T", "Do", 2)]);

    let graph = graph(&[declarations, caller]);

    assert_eq!(misses_of(&graph), [("Do", Miss::ExternalImport)]);
}

#[test]
fn a_member_on_a_value_binds_the_field_or_method_its_type_path_reaches() {
    let mut caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        Vec::new(),
    );
    caller.uses = vec![
        accessing("Timeout", "cfg.Config", Access::Write, 2),
        accessing("Timeout", "cfg.Config", Access::Read, 3),
        accessing("Reset", "cfg.Config", Access::Read, 4),
    ];
    let caller = importing(caller, &[("cfg", Some("cfg"))]);

    let graph = graph(&[config_package(), caller]);

    assert_eq!(
        sites_of(&graph, "Config.Timeout"),
        [
            ("main.Run".to_owned(), 2, 85),
            ("main.Run".to_owned(), 3, 85)
        ],
        "a field is a referent, and each line of a body that touches it is a site"
    );
    assert_eq!(
        sites_of(&graph, "cfg.Config.Reset"),
        [("main.Run".to_owned(), 4, 85)],
        "a method taken as a value is a use of it"
    );
}

#[test]
fn a_use_through_a_cycle_binds_no_field_its_file_declares() {
    let mut cycle = file(
        "x/x.go",
        "x",
        vec![
            function("x", "run", 1),
            yielding(typed("x", "a", SymbolKind::Variable, 2), &["b"]),
            yielding(typed("x", "b", SymbolKind::Variable, 3), &["a"]),
            typed("x", "Holder", SymbolKind::Struct, 4),
            member("x.Holder", "Timeout", SymbolKind::Field, 5),
        ],
        Vec::new(),
    );
    cycle.uses = vec![accessing("Timeout", "a", Access::Read, 1)];

    assert_eq!(sites_of(&graph(&[cycle]), "Holder.Timeout"), []);
}

const fn spanning(mut symbol: Symbol, end_line: u32) -> Symbol {
    symbol.end_line = end_line;
    symbol
}

fn anonymous_field(qualified_host: &str, name: &str, line: u32) -> Symbol {
    let (_, host) = qualified_host
        .rsplit_once('.')
        .expect("a host is qualified by its scope");
    let mut symbol = member(qualified_host, name, SymbolKind::Field, line);
    symbol.owner = Some(host.to_owned());
    symbol
}

fn used_in(site_of: u32, name: &str, path: &str, line: u32) -> UseRef {
    let mut site = accessing(name, path, Access::Read, line);
    site.site_of = SymbolId(site_of);
    site
}

#[test]
fn a_field_whose_struct_is_anonymous_owns_its_fields_apart_from_a_namesake_type_s() {
    let mut server = file(
        "srv/server.go",
        "srv",
        vec![
            spanning(typed("srv", "Server", SymbolKind::Struct, 1), 6),
            yielding(
                spanning(member("srv.Server", "Limits", SymbolKind::Field, 2), 4),
                &["{1}"],
            ),
            anonymous_field("srv.Server.Limits", "MaxConns", 3),
            spanning(typed("srv", "Limits", SymbolKind::Struct, 7), 9),
            member("srv.Limits", "MaxConns", SymbolKind::Field, 8),
            spanning(function("srv", "Run", 11), 14),
        ],
        Vec::new(),
    );
    server.uses = vec![
        used_in(5, "MaxConns", "Server.Limits", 12),
        used_in(5, "MaxConns", "Limits", 13),
    ];

    let graph = graph(&[server]);

    assert_eq!(
        sites_of(&graph, "srv.Server.Limits.MaxConns"),
        [("srv.Run".to_owned(), 12, 85)],
        "the field's value names its own index, and its struct's fields are owned by its name"
    );
    assert_eq!(
        sites_of(&graph, "srv.Limits.MaxConns"),
        [("srv.Run".to_owned(), 13, 85)],
        "a type's member is qualified by the type, so the field's struct is a namesake's"
    );
}

#[test]
fn a_body_s_values_of_one_name_own_the_fields_their_lines_hold_and_the_types_their_structs_embed() {
    let mut caller = file(
        "app/run_test.go",
        "app",
        vec![
            spanning(function("app", "TestRun", 1), 20),
            yielding(
                spanning(typed("app", "tests", SymbolKind::Variable, 2), 4),
                &["[]{1}"],
            ),
            anonymous_field("app.TestRun.tests", "in", 3),
            yielding(
                spanning(typed("app", "tests", SymbolKind::Variable, 8), 10),
                &["[]{3}"],
            ),
            anonymous_field("app.TestRun.tests", "want", 9),
            embedding(
                yielding(
                    spanning(typed("app", "aux", SymbolKind::Variable, 12), 14),
                    &["{5}"],
                ),
                &["Alias"],
            ),
            anonymous_field("app.TestRun.aux", "extra", 13),
            spanning(typed("app", "Alias", SymbolKind::Struct, 22), 24),
            member("app.Alias", "Name", SymbolKind::Field, 23),
        ],
        Vec::new(),
    );
    caller.local_symbols = (1..=6).map(SymbolId).collect();
    caller.uses = vec![
        used_in(0, "want", "{1}", 6),
        used_in(0, "want", "{3}", 11),
        used_in(0, "Name", "{5}", 15),
        used_in(0, "extra", "{5}", 16),
    ];

    let graph = graph(&[caller]);

    assert_eq!(
        sites_of(&graph, "tests.want"),
        [("app.TestRun".to_owned(), 11, 85)],
        "the first value's struct holds no field of the name"
    );
    assert_eq!(
        sites_of(&graph, "Alias.Name"),
        [("app.TestRun".to_owned(), 15, 85)],
        "the value holds its struct's embedded types"
    );
    assert_eq!(
        sites_of(&graph, "aux.extra"),
        [("app.TestRun".to_owned(), 16, 85)]
    );
}

#[test]
fn a_member_whose_type_the_path_cannot_reach_binds_only_the_one_field_of_its_file() {
    let mut caller = file(
        "cfg/load.go",
        "cfg",
        vec![
            function("cfg", "Load", 1),
            typed("cfg", "Local", SymbolKind::Struct, 5),
            member("cfg.Local", "Limit", SymbolKind::Field, 6),
            function("cfg", "Other", 8),
            typed("cfg", "entry", SymbolKind::Struct, 9),
            member("cfg.entry", "consumer", SymbolKind::Field, 10),
        ],
        Vec::new(),
    );
    caller.local_symbols = vec![SymbolId(4), SymbolId(5)];
    caller.uses = vec![
        accessing("Limit", "", Access::Read, 2),
        accessing("Timeout", "", Access::Read, 3),
        accessing("consumer", "", Access::Read, 4),
    ];

    let graph = graph(&[config_package(), caller]);

    assert_eq!(
        sites_of(&graph, "Local.Limit"),
        [("cfg.Load".to_owned(), 2, 80)]
    );
    assert_eq!(
        sites_of(&graph, "Config.Timeout"),
        [],
        "the one field of the name in the package is not taken: a namesake is as likely"
    );
    assert_eq!(
        sites_of(&graph, "entry.consumer"),
        [],
        "a field of a type another body declares is out of the use's reach"
    );
}

#[test]
fn a_member_of_a_type_outside_the_graph_or_of_one_without_it_binds_nothing() {
    let mut caller = file(
        "app/main.go",
        "main",
        vec![
            function("main", "Run", 1),
            typed("main", "Plain", SymbolKind::Struct, 5),
        ],
        Vec::new(),
    );
    caller.uses = vec![
        accessing("Timeout", "http.Client", Access::Read, 2),
        accessing("Timeout", "Plain", Access::Read, 3),
    ];
    let caller = importing(caller, &[("http", None)]);

    let graph = graph(&[config_package(), caller]);

    assert_eq!(sites_of(&graph, "Config.Timeout"), []);
}

#[test]
fn a_qualifier_no_import_binds_reads_a_member_of_the_package_value_it_names() {
    let declaring = file(
        "cfg/vars.go",
        "cfg",
        vec![yielding(
            typed("cfg", "current", SymbolKind::Variable, 1),
            &["Config"],
        )],
        Vec::new(),
    );
    let mut reader = file(
        "cfg/read.go",
        "cfg",
        vec![function("cfg", "Read", 1)],
        Vec::new(),
    );
    let qualified = |qualifier: &str, line| {
        UseRef::new(
            SymbolId(0),
            "Timeout".to_owned(),
            Receiver::Qualifier(qualifier.to_owned()),
            line,
        )
    };
    reader.uses = vec![
        qualified("current", 2),
        qualified("http", 3),
        qualified("absent", 4),
    ];
    let reader = importing(reader, &[("http", None)]);

    let graph = graph(&[config_package(), declaring, reader]);

    assert_eq!(
        sites_of(&graph, "Config.Timeout"),
        [("cfg.Read".to_owned(), 2, 85)],
        "a package's value a sibling file declares is read through its type; an import's qualifier and a name \
         nothing declares bind no field"
    );
}

#[test]
fn a_field_answers_each_access_counted_the_ones_asked_for_and_the_uses_it_could_not_tell() {
    let mut caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        Vec::new(),
    );
    caller.uses = vec![
        accessing("Timeout", "cfg.Config", Access::Write, 2),
        accessing("Timeout", "cfg.Config", Access::Read, 2),
        accessing("Timeout", "cfg.Config", Access::Update, 3),
        accessing("Timeout", "cfg.Config", Access::Read, 4),
        accessing("Timeout", "", Access::Read, 5),
        accessing("Timeout", "http.Client", Access::Read, 6),
    ];
    let caller = importing(caller, &[("cfg", Some("cfg")), ("http", None)]);
    let graph = graph(&[config_package(), caller]);
    let answer = |query| {
        graph
            .references("Config.Timeout", query)
            .expect("the field is selected")
    };

    let every = answer(SiteQuery::new(usize::MAX));
    let writers = answer(SiteQuery::new(usize::MAX).with_access(Access::Write));
    let readers = answer(SiteQuery::new(1).with_access(Access::Read));

    let accesses = |sites: &[kartta_graph::Reference<'_>]| -> Vec<(u32, Access)> {
        sites.iter().map(|site| (site.line, site.access)).collect()
    };
    assert_eq!(
        accesses(&every.sites),
        [
            (2, Access::Read),
            (2, Access::Write),
            (3, Access::Update),
            (4, Access::Read)
        ],
        "a read and a write on one line are two sites"
    );
    let counts = |counts: SiteCounts| (counts.calls, counts.reads, counts.writes, counts.updates);
    assert_eq!(counts(every.counts), (0, 2, 1, 1));
    assert_eq!(
        every.ambiguous, 1,
        "a member on a value of no known type may be this field; one of a type outside the graph is not"
    );
    assert_eq!(
        accesses(&writers.sites),
        [(2, Access::Write), (3, Access::Update)]
    );
    assert_eq!(
        (writers.total, counts(writers.counts)),
        (2, (0, 2, 1, 1)),
        "the counts count every site whatever access the query keeps"
    );
    assert_eq!(
        (accesses(&readers.sites), readers.total),
        (vec![(2, Access::Read)], 3),
        "an update reads too, and the limit cuts what is listed"
    );
}

#[test]
fn a_type_counts_its_uses_as_a_type_apart_from_its_reads_and_a_read_query_keeps_both() {
    let mut caller = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        Vec::new(),
    );
    let qualified = |access, line| {
        let mut site = UseRef::new(
            SymbolId(0),
            "Config".to_owned(),
            Receiver::Qualifier("cfg".to_owned()),
            line,
        );
        site.access = access;
        site
    };
    caller.uses = vec![
        qualified(Access::Type, 2),
        qualified(Access::Read, 2),
        qualified(Access::Type, 3),
    ];
    let caller = importing(caller, &[("cfg", Some("cfg"))]);
    let graph = graph(&[config_package(), caller]);
    let answer = |query| {
        graph
            .references("cfg.Config", query)
            .expect("the type is selected")
    };
    let accesses = |query| -> Vec<(u32, Access)> {
        answer(query)
            .sites
            .iter()
            .map(|site| (site.line, site.access))
            .collect()
    };

    let counts = answer(SiteQuery::new(usize::MAX)).counts;

    assert_eq!(
        (counts.reads, counts.types),
        (1, 2),
        "a use as a type and a read on one line are two sites"
    );
    assert_eq!(
        accesses(SiteQuery::new(usize::MAX).with_access(Access::Read)),
        [(2, Access::Read), (2, Access::Type), (3, Access::Type)]
    );
    assert_eq!(
        accesses(SiteQuery::new(usize::MAX).with_access(Access::Type)),
        [(2, Access::Type), (3, Access::Type)]
    );
}

fn return_paths() -> Graph {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.owner_chain_separator = Some("::");
    conventions.receivers = Receivers::ReturnPaths;
    let calls = vec![
        member_call("Root::open()?::inner()", "drain", 2),
        member_call("Root::open()::inner()|.held", "drain", 3),
        member_call("Root::open()?::missing()", "tick", 4),
    ];
    graph_of(
        conventions,
        &[
            file("a/a.rs", "a", vec![function("a", "caller", 1)], calls),
            file(
                "b/b.rs",
                "b",
                vec![
                    yielding(member("b.Root", "open", SymbolKind::Method, 1), &["?Mid"]),
                    yielding(member("b.Mid", "inner", SymbolKind::Method, 3), &["Held"]),
                    member("b.Held", "drain", SymbolKind::Method, 5),
                    member("b.Other", "drain", SymbolKind::Method, 7),
                    member("b.Other", "tick", SymbolKind::Method, 9),
                    member("b.Held", "tick", SymbolKind::Method, 11),
                ],
                Vec::new(),
            ),
        ],
    )
}

#[test]
fn a_return_path_binds_the_method_of_the_type_its_last_call_returns_in_another_file() {
    assert_eq!(
        callees_of(&return_paths(), "caller"),
        [("b/b.rs:b.Held.drain".to_owned(), 85)]
    );
}

#[test]
fn a_return_path_that_reaches_no_type_leaves_the_value_after_it_or_the_empty_receiver() {
    let graph = return_paths();
    let unbound = graph.resolution(usize::MAX).unbound;

    assert_eq!(
        unbound
            .iter()
            .map(|unbound| (unbound.line, unbound.receiver))
            .collect::<Vec<_>>(),
        [(3, Some(".held")), (4, Some(""))]
    );
}

fn method_of(qualified_owner: &str, name: &str, line: u32) -> Symbol {
    let mut method = member(qualified_owner, name, SymbolKind::Method, line);
    method.signature = Some("func()".to_owned());
    method
}

fn interface_file(path: &str) -> Extraction {
    file(
        path,
        "p",
        vec![
            typed("p", "Doer", SymbolKind::Interface, 3),
            method_of("p.Doer", "Do", 4),
        ],
        Vec::new(),
    )
}

fn through_of(graph: &Graph, name: &str) -> Vec<(String, Option<String>)> {
    let walk = graph
        .callers(name, Limits::new(1))
        .expect("the method is held");
    walk.reached
        .iter()
        .map(|reached| {
            (
                reached.symbol.qualified_name.to_owned(),
                reached.through.map(|parent| parent.file.to_owned()),
            )
        })
        .collect()
}

#[test]
fn a_caller_through_an_interface_two_files_declare_names_the_first_in_path_order_after_files_move()
{
    let mut conventions = go();
    conventions.implements = Implements::MethodSets(SameMethod(|own, parent| own == parent));
    conventions.twins = Twins::Package(BuiltApart(|_, _| false));
    let lone = |path: &str, name: &str| file(path, "p", vec![function("p", name, 3)], Vec::new());
    let implementation = file(
        "p/impl.go",
        "p",
        vec![
            typed("p", "thing", SymbolKind::Struct, 3),
            method_of("p.thing", "Do", 5),
        ],
        Vec::new(),
    );
    let user = file(
        "p/use.go",
        "p",
        vec![function("p", "Use", 3)],
        vec![member_call("Doer", "Do", 4)],
    );
    let mut moved = graph_of(
        conventions,
        &[
            lone("p/a.go", "A"),
            interface_file("p/iface.go"),
            implementation.clone(),
            user.clone(),
            lone("p/z.go", "Z"),
        ],
    );
    moved.remove("p/a.go");
    moved.insert(&interface_file("p/iface.k1.go"));
    let fresh = graph_of(
        conventions,
        &[
            interface_file("p/iface.go"),
            interface_file("p/iface.k1.go"),
            implementation,
            user,
            lone("p/z.go", "Z"),
        ],
    );

    let first = [("p.Use".to_owned(), Some("p/iface.go".to_owned()))];
    assert_eq!(through_of(&fresh, "p.thing.Do"), first);
    assert_eq!(
        through_of(&moved, "p.thing.Do"),
        first,
        "the copy took the slot the removed file freed, before the first copy's"
    );
}
