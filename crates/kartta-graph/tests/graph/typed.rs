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
fn a_receiver_that_names_a_type_binds_no_method_that_takes_fewer_than_the_call_passes() {
    let passing = |arguments| kartta_lang::CallRef {
        arguments,
        ..member_call("Server", "Close", 2)
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
    UseRef {
        site_of: SymbolId(site_of),
        ..accessing(name, path, Access::Read, line)
    }
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
    let qualified = |qualifier: &str, line| UseRef {
        site_of: SymbolId(0),
        name: "Timeout".to_owned(),
        receiver: Receiver::Qualifier(qualifier.to_owned()),
        access: Access::Read,
        line,
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
    let qualified = |access, line| UseRef {
        site_of: SymbolId(0),
        name: "Config".to_owned(),
        receiver: Receiver::Qualifier("cfg".to_owned()),
        access,
        line,
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
