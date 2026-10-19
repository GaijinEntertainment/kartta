//! Member calls whose receiver spells a type path.

use kartta_graph::{Graph, Limits, Miss, SiteCounts, SiteQuery};
use kartta_lang::{
    Access, Conventions, Extraction, MemberCalls, OwnMembers, Receiver, Receivers, Symbol,
    SymbolFlags, SymbolId, SymbolKind, UseRef,
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

fn accessing(name: &str, path: &str, access: Access, line: u32) -> UseRef {
    UseRef {
        site_of: SymbolId(0),
        name: name.to_owned(),
        receiver: Receiver::Path(path.to_owned()),
        access,
        line,
    }
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
