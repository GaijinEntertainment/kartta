//! The definitions no call and no use reaches: what the query judges, what keeps a definition alive, and what it counts
//! as left out.

use kartta_graph::{DeadCode, Graph};
use kartta_lang::{
    Access, CallRef, Callable, CallableRole, Conventions, DataUses, Exports, Extraction,
    FILE_SCOPE, ImportBinding, IncludeRef, OwnMembers, Receiver, Receivers, Symbol, SymbolFlags,
    SymbolId, SymbolKind, TypeUses, UntypedMembers, UseRef,
};

use crate::{call, function, graph, graph_of, member_call};

fn member(owner: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Method, line, line + 1);
    symbol.qualified_name = format!("{owner}::{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

fn with_parent(mut symbol: Symbol, parent: &str) -> Symbol {
    symbol.parents.push(parent.to_owned());
    symbol
}

const fn flagged(mut symbol: Symbol, flags: SymbolFlags) -> Symbol {
    symbol.flags = flags;
    symbol
}

fn file(path: &str, symbols: Vec<Symbol>, calls: Vec<CallRef>) -> Extraction {
    let mut extraction = crate::file(path, symbols);
    extraction.calls = calls;
    extraction
}

fn field(owner: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Field, line, line);
    symbol.qualified_name = format!("{owner}.{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

fn use_of(name: &str, receiver: Receiver, access: Access, line: u32) -> UseRef {
    let mut site = UseRef::new(SymbolId(0), name.to_owned(), receiver, line);
    site.access = access;
    site
}

fn go() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.directory_packages = true;
    conventions.receivers = Receivers::TypePaths;
    conventions.type_uses = TypeUses::Every;
    conventions.data_uses = DataUses::Every;
    conventions
}

const DATA: [SymbolKind; 3] = [
    SymbolKind::Field,
    SymbolKind::Variable,
    SymbolKind::Constant,
];

fn names<'g>(dead: &DeadCode<'g>) -> Vec<&'g str> {
    dead.found
        .iter()
        .map(|found| found.definition.name)
        .collect()
}

#[test]
fn a_function_no_call_reaches_is_dead_and_a_called_one_is_live() {
    let graph = graph(&[file(
        "src/lib.c",
        vec![
            function("main_loop", 1),
            function("helper", 5),
            function("orphan", 9),
        ],
        vec![call(0, "helper", 2)],
    )]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["main_loop", "orphan"]);
    assert_eq!((dead.total, dead.live), (2, 1));
    assert!(dead.found.iter().all(|found| !found.spelled));
}

#[test]
fn the_dead_a_language_exports_are_counted_past_the_limit_and_not_where_it_records_no_export() {
    let exported = |mut symbol: Symbol| {
        symbol.exported = true;
        symbol
    };
    let files = [file(
        "src/lib.c",
        vec![
            exported(function("api", 1)),
            function("internal", 5),
            exported(function("tail", 9)),
            exported(function("used", 13)),
        ],
        vec![call(1, "used", 6)],
    )];
    let mut unrecorded = Conventions::default();
    unrecorded.exports = Exports::Unrecorded;

    let (recording, unrecording) = (graph(&files), graph_of(unrecorded, &files));
    let dead = recording.dead_code(&[], None, 1);
    let unmarked = unrecording.dead_code(&[], None, 10);

    assert_eq!(
        (names(&dead), dead.total, dead.live, dead.exported),
        (vec!["api"], 3, 1, Some(2))
    );
    assert!(dead.found[0].definition.exported);
    assert_eq!((unmarked.total, unmarked.exported), (3, None));
}

#[test]
fn recursion_alone_keeps_nothing_alive() {
    let graph = graph(&[file(
        "src/walk.c",
        vec![function("walk", 1)],
        vec![call(0, "walk", 2)],
    )]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["walk"]);
    assert!(dead.found[0].spelled);
}

#[test]
fn a_call_at_file_scope_reaches_its_file_and_the_files_it_includes() {
    let mut script = file(
        "bin/run.sh",
        vec![function("setup", 1)],
        vec![
            call(FILE_SCOPE.0, "setup", 9),
            call(FILE_SCOPE.0, "shared", 10),
        ],
    );
    script
        .includes
        .push(IncludeRef::new("lib/common.sh".to_owned()));
    let graph = graph(&[
        script,
        file("lib/common.sh", vec![function("shared", 1)], Vec::new()),
        file("lib/other.sh", vec![function("setup", 1)], Vec::new()),
    ]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(dead.live, 2);
    assert_eq!(names(&dead), ["setup"]);
    assert_eq!(dead.found[0].definition.file, "lib/other.sh");
    assert!(dead.found[0].spelled);
}

#[test]
fn removing_the_file_that_calls_a_function_leaves_it_dead() {
    let mut graph = graph(&[
        file("src/a.c", vec![function("used", 1)], Vec::new()),
        file(
            "src/b.c",
            vec![function("caller", 1)],
            vec![call(0, "used", 2), call(FILE_SCOPE.0, "caller", 5)],
        ),
    ]);
    assert_eq!(graph.dead_code(&[], None, 10).total, 0);

    graph.remove("src/b.c");

    assert_eq!(names(&graph.dead_code(&[], None, 10)), ["used"]);
}

#[test]
fn a_call_the_cascade_cannot_bind_leaves_every_definition_of_its_name_uncertain() {
    let graph = graph(&[
        file("a/one.c", vec![function("helper", 1)], Vec::new()),
        file("b/two.c", vec![function("helper", 1)], Vec::new()),
        file(
            "c/main.c",
            vec![function("run", 1)],
            vec![call(0, "helper", 2), call(FILE_SCOPE.0, "run", 5)],
        ),
    ]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!((dead.total, dead.live), (0, 1));
    assert_eq!(dead.excluded.uncertain, 2);
}

#[test]
fn a_call_bound_to_a_fallback_leaves_every_namesake_uncertain() {
    let library = file(
        "lib/fog.dshl",
        vec![
            flagged(
                Symbol::new("USE_FOG", SymbolKind::Macro, 1, 2),
                SymbolFlags::empty().with_fallback(true),
            ),
            Symbol::new("fog", SymbolKind::Class, 4, 6),
        ],
        vec![call(1, "USE_FOG", 5), call(FILE_SCOPE.0, "fog", 7)],
    );
    let graph = graph(&[
        library,
        file(
            "game/fog.dshl",
            vec![Symbol::new("USE_FOG", SymbolKind::Macro, 1, 2)],
            Vec::new(),
        ),
        file(
            "game/haze.dshl",
            vec![Symbol::new("USE_HAZE", SymbolKind::Macro, 1, 2)],
            Vec::new(),
        ),
    ]);

    let dead = graph.dead_code(&[SymbolKind::Macro], None, 10);

    assert_eq!(names(&dead), ["USE_HAZE"]);
    assert_eq!((dead.live, dead.excluded.uncertain), (1, 1));
}

#[test]
fn a_receiver_call_that_binds_nothing_leaves_members_and_what_its_file_sees_uncertain() {
    let mut caller = file("app/main.c", vec![function("run", 1)], Vec::new());
    caller.calls.push(CallRef::new(
        SymbolId(0),
        "close".to_owned(),
        Some(String::new()),
        2,
    ));
    caller
        .includes
        .extend(["lib/fd.h", "lib/sock.h"].map(|target| IncludeRef::new(target.to_owned())));
    let graph = graph(&[
        caller,
        file("lib/fd.h", vec![function("close", 1)], Vec::new()),
        file("lib/sock.h", vec![function("close", 1)], Vec::new()),
        file(
            "src/file.cpp",
            vec![
                Symbol::new("File", SymbolKind::Class, 1, 3),
                member("File", "close", 2),
                Symbol::new("Pipe", SymbolKind::Class, 4, 6),
                member("Pipe", "close", 5),
            ],
            Vec::new(),
        ),
        file("src/io.c", vec![function("close", 1)], Vec::new()),
    ]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["run", "close"]);
    assert_eq!(dead.found[1].definition.file, "src/io.c");
    assert_eq!(dead.excluded.uncertain, 4);
}

#[test]
fn a_receiver_call_that_binds_nothing_leaves_a_free_definition_no_member_could_stand_for_uncertain()
{
    let mut loader = file("tools/load.py", vec![function("load", 1)], Vec::new());
    loader.calls.push(CallRef::new(
        SymbolId(0),
        "get_mapping".to_owned(),
        Some("module".to_owned()),
        3,
    ));
    let graph = graph(&[
        loader,
        file(
            "layouts/console.py",
            vec![function("get_mapping", 1)],
            Vec::new(),
        ),
        file(
            "layouts/store.py",
            vec![function("get_mapping", 1)],
            Vec::new(),
        ),
    ]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["load"]);
    assert_eq!(dead.excluded.uncertain, 2);
}

#[test]
fn a_call_through_a_value_bound_to_its_own_caller_leaves_the_members_of_its_name_uncertain() {
    let mut wrapper = file(
        "src/group.cpp",
        vec![
            Symbol::new("Group", SymbolKind::Class, 1, 9),
            member("Group", "setLimit", 2),
        ],
        Vec::new(),
    );
    wrapper.calls.push(CallRef::new(
        SymbolId(1),
        "setLimit".to_owned(),
        Some(String::new()),
        3,
    ));
    let graph = graph(&[
        wrapper,
        file(
            "src/group_impl.cpp",
            vec![
                Symbol::new("GroupImpl", SymbolKind::Class, 1, 9),
                member("GroupImpl", "setLimit", 2),
            ],
            Vec::new(),
        ),
    ]);

    let dead = graph.dead_code(&[SymbolKind::Method], None, 10);

    assert_eq!(dead.total, 0);
    assert_eq!(dead.excluded.uncertain, 2);
}

#[test]
fn test_generated_entry_and_vendored_code_is_counted_and_not_judged() {
    let graph = graph(&[file(
        "src/main.c",
        vec![
            flagged(function("check", 1), SymbolFlags::empty().with_test(true)),
            flagged(
                function("made", 3),
                SymbolFlags::empty().with_generated(true),
            ),
            flagged(function("main", 5), SymbolFlags::empty().with_entry(true)),
            flagged(
                function("third", 7),
                SymbolFlags::empty().with_vendored(true),
            ),
            function("orphan", 9),
        ],
        Vec::new(),
    )]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["orphan"]);
    let excluded = dead.excluded;
    assert_eq!(
        (
            excluded.test,
            excluded.generated,
            excluded.entry,
            excluded.vendored
        ),
        (1, 1, 1, 1)
    );
}

#[test]
fn a_file_only_test_modules_include_is_counted_as_test_code() {
    let mut lib = file("src/lib.rs", vec![function("unused", 1)], Vec::new());
    lib.includes
        .push(IncludeRef::new("src/tests.rs".to_owned()));
    lib.test_includes.push("src/tests.rs".to_owned());
    let graph = graph(&[
        lib,
        file("src/tests.rs", vec![function("fixture", 1)], Vec::new()),
    ]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["unused"]);
    assert_eq!(dead.excluded.test, 1);
}

#[test]
fn fields_values_and_types_are_unobservable_where_uses_name_no_type() {
    let graph = graph(&[file(
        "src/model.py",
        vec![
            Symbol::new("Model", SymbolKind::Class, 1, 9),
            member("Model", "name", 2),
            Symbol::new("LIMIT", SymbolKind::Constant, 10, 10),
            Symbol::new("count", SymbolKind::Field, 11, 11),
        ],
        Vec::new(),
    )]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["name"]);
    assert_eq!(dead.excluded.unobservable, 3);
}

#[test]
fn fields_and_values_are_judged_by_their_reads_and_writes_where_the_language_records_each_one() {
    let mut user = file("pkg/run.go", vec![function("Run", 1)], Vec::new());
    user.uses.extend([
        use_of("Port", Receiver::Path("Config".to_owned()), Access::Read, 2),
        use_of("limit", Receiver::Bare, Access::Write, 3),
    ]);
    let config = file(
        "pkg/config.go",
        vec![
            Symbol::new("Config", SymbolKind::Struct, 1, 4),
            field("Config", "Port", 2),
            field("Config", "Host", 3),
            Symbol::new("limit", SymbolKind::Variable, 5, 5),
            Symbol::new("Retries", SymbolKind::Constant, 6, 6),
        ],
        Vec::new(),
    );
    let graph = graph_of(go(), &[user, config]);

    let dead = graph.dead_code(&DATA, None, 10);

    assert_eq!(names(&dead), ["Host", "Retries"]);
    assert_eq!(dead.live, 2);
    assert_eq!(dead.excluded.unobservable, 0);
}

#[test]
fn a_use_that_binds_nothing_leaves_each_field_of_its_name_and_each_value_its_file_sees_uncertain() {
    let mut user = file("pkg/run.go", vec![function("Run", 1)], Vec::new());
    user.uses.extend([
        use_of("Port", Receiver::Path(String::new()), Access::Read, 2),
        use_of(
            "limit",
            Receiver::Qualifier("ext".to_owned()),
            Access::Read,
            3,
        ),
    ]);
    let graph = graph_of(
        go(),
        &[
            user,
            file(
                "pkg/config.go",
                vec![
                    Symbol::new("Config", SymbolKind::Struct, 1, 3),
                    field("Config", "Port", 2),
                    Symbol::new("limit", SymbolKind::Variable, 4, 4),
                ],
                Vec::new(),
            ),
            file(
                "other/model.go",
                vec![
                    Symbol::new("Model", SymbolKind::Struct, 1, 3),
                    field("Model", "Port", 2),
                    Symbol::new("limit", SymbolKind::Variable, 4, 4),
                ],
                Vec::new(),
            ),
        ],
    );

    let dead = graph.dead_code(&DATA, None, 10);

    assert_eq!(names(&dead), ["limit"]);
    assert_eq!(dead.found[0].definition.file, "other/model.go");
    assert_eq!(dead.excluded.uncertain, 3);
}

#[test]
fn an_ambiguous_use_leaves_each_member_of_its_name_uncertain() {
    let mut user = file("pkg/run.go", vec![function("Run", 1)], Vec::new());
    user.uses.push(use_of(
        "Handle",
        Receiver::Path(String::new()),
        Access::Read,
        2,
    ));
    let handlers = file(
        "pkg/handlers.go",
        vec![
            Symbol::new("Get", SymbolKind::Struct, 1, 3),
            member("Get", "Handle", 2),
            Symbol::new("Put", SymbolKind::Struct, 4, 6),
            member("Put", "Handle", 5),
            function("Handle", 7),
        ],
        Vec::new(),
    );
    let graph = graph_of(go(), &[user, handlers]);

    let dead = graph.dead_code(&[SymbolKind::Method, SymbolKind::Function], None, 10);

    assert_eq!(names(&dead), ["Handle", "Run"]);
    assert_eq!(dead.found[0].definition.qualified_name, "Handle");
    assert_eq!(dead.excluded.uncertain, 2);
}

#[test]
fn a_use_ambiguous_among_fields_alone_leaves_a_method_of_its_name_dead() {
    let mut user = file("src/main.cpp", vec![function("run", 1)], Vec::new());
    user.uses.push(use_of(
        "count",
        Receiver::Path(String::new()),
        Access::Read,
        2,
    ));
    user.includes
        .extend(["src/box.h", "src/bag.h"].map(|target| IncludeRef::new(target.to_owned())));
    let holder = |path, owner| {
        file(
            path,
            vec![
                Symbol::new(owner, SymbolKind::Struct, 1, 3),
                field(owner, "count", 2),
            ],
            Vec::new(),
        )
    };
    let counter = file(
        "src/counter.cpp",
        vec![
            Symbol::new("Counter", SymbolKind::Class, 1, 3),
            member("Counter", "count", 2),
        ],
        Vec::new(),
    );
    let mut conventions = Conventions::default();
    conventions.untyped_members = UntypedMembers::IncludedField;
    let graph = graph_of(
        conventions,
        &[
            user,
            holder("src/box.h", "Box"),
            holder("src/bag.h", "Bag"),
            counter,
        ],
    );

    let dead = graph.dead_code(&[SymbolKind::Method], None, 10);

    assert_eq!(names(&dead), ["count"]);
    assert_eq!(dead.excluded.uncertain, 0);
}

#[test]
fn a_value_a_call_goes_through_is_kept_by_the_read_of_it_and_not_by_the_call_s_receiver() {
    let mut view = file("pkg/view.go", vec![function("View", 1)], Vec::new());
    view.calls.push(member_call("style", "Render", 2));
    view.uses
        .push(use_of("style", Receiver::Bare, Access::Read, 2));
    view.calls.push(member_call("palette", "Render", 3));
    let values = |path, names: &[&str]| {
        let symbols = names
            .iter()
            .map(|&name| Symbol::new(name, SymbolKind::Variable, 1, 1))
            .collect();
        file(path, symbols, Vec::new())
    };
    let graph = graph_of(
        go(),
        &[
            view,
            values("pkg/styles.go", &["style", "palette"]),
            values("other/styles.go", &["style"]),
        ],
    );

    let dead = graph.dead_code(&DATA, None, 10);

    let found: Vec<(&str, &str)> = dead
        .found
        .iter()
        .map(|found| (found.definition.file, found.definition.name))
        .collect();
    assert_eq!(
        found,
        [("other/styles.go", "style"), ("pkg/styles.go", "palette")],
        "a call's receiver alone reads no value; Go records the read of a value it calls through"
    );
    assert_eq!((dead.live, dead.excluded.uncertain), (1, 0));
}

#[test]
fn a_field_reflection_reads_is_counted_as_reflected_where_no_use_reaches_it() {
    let mut user = file("pkg/run.go", vec![function("Run", 1)], Vec::new());
    user.uses.push(use_of(
        "Name",
        Receiver::Path("Seed".to_owned()),
        Access::Read,
        2,
    ));
    let reflected = |symbol| flagged(symbol, SymbolFlags::empty().with_reflected(true));
    let seed = file(
        "pkg/seed.go",
        vec![
            Symbol::new("Seed", SymbolKind::Struct, 1, 5),
            reflected(field("Seed", "Name", 2)),
            reflected(field("Seed", "Count", 3)),
            field("Seed", "Spare", 4),
        ],
        Vec::new(),
    );
    let graph = graph_of(go(), &[user, seed]);

    let dead = graph.dead_code(&DATA, None, 10);

    assert_eq!(names(&dead), ["Spare"]);
    assert_eq!((dead.live, dead.excluded.reflected), (1, 1));
}

#[test]
fn a_method_taken_only_as_a_value_is_live() {
    let mut user = file("pkg/user.go", vec![function("Run", 1)], Vec::new());
    user.uses.push(UseRef::new(
        SymbolId(0),
        "handle".to_owned(),
        Receiver::Path("Server".to_owned()),
        2,
    ));
    let conventions = go();
    let method = |name, line| {
        let mut symbol = member("Server", name, line);
        symbol.qualified_name = format!("Server.{name}");
        symbol
    };
    let server = file(
        "pkg/server.go",
        vec![
            Symbol::new("Server", SymbolKind::Struct, 1, 3),
            method("handle", 4),
            method("unused", 6),
        ],
        Vec::new(),
    );
    let graph = graph_of(conventions, &[user, server]);

    let dead = graph.dead_code(&[SymbolKind::Method], None, 10);

    assert_eq!(names(&dead), ["unused"]);
}

#[test]
fn a_type_is_judged_where_uses_name_every_type() {
    let mut user = file("pkg/user.go", vec![function("Handle", 1)], Vec::new());
    user.uses.push(UseRef::new(
        SymbolId(0),
        "Request".to_owned(),
        Receiver::Bare,
        2,
    ));
    let graph = graph_of(
        go(),
        &[
            user,
            file(
                "pkg/types.go",
                vec![
                    Symbol::new("Request", SymbolKind::Struct, 1, 4),
                    Symbol::new("Unused", SymbolKind::Struct, 5, 8),
                    Symbol::new("Base", SymbolKind::Interface, 9, 12),
                    with_parent(Symbol::new("Impl", SymbolKind::Struct, 13, 16), "Base"),
                ],
                vec![call(0, "Handle", 3)],
            ),
        ],
    );

    let dead = graph.dead_code(&[SymbolKind::Struct, SymbolKind::Interface], None, 10);

    assert_eq!(names(&dead), ["Unused", "Impl"]);
    assert_eq!(dead.live, 2);
    assert_eq!(dead.excluded.kind, 1);
}

#[test]
fn a_definition_local_to_a_body_is_unobservable_as_no_use_binds_a_local() {
    let mut body = file(
        "pkg/run.go",
        vec![
            function("Run", 1),
            Symbol::new("row", SymbolKind::Struct, 2, 2),
            function("each", 3),
        ],
        vec![call(FILE_SCOPE.0, "Run", 9)],
    );
    body.local_symbols.extend([SymbolId(1), SymbolId(2)]);
    let graph = graph_of(go(), &[body]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!((dead.total, dead.live), (0, 1));
    assert_eq!(dead.excluded.unobservable, 2);
}

#[test]
fn a_use_at_file_scope_reaches_the_types_its_package_defines() {
    let mut assertion = file("pkg/assert.go", Vec::new(), Vec::new());
    assertion.uses.push(UseRef::new(
        FILE_SCOPE,
        "Checked".to_owned(),
        Receiver::Bare,
        3,
    ));
    let graph = graph_of(
        go(),
        &[
            assertion,
            file(
                "pkg/types.go",
                vec![Symbol::new("Checked", SymbolKind::Struct, 1, 4)],
                Vec::new(),
            ),
            file(
                "other/types.go",
                vec![Symbol::new("Checked", SymbolKind::Struct, 1, 4)],
                Vec::new(),
            ),
        ],
    );

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(dead.live, 1);
    assert_eq!(names(&dead), ["Checked"]);
    assert_eq!(dead.found[0].definition.file, "other/types.go");
    assert!(dead.found[0].spelled);
}

#[test]
fn a_type_only_its_own_members_name_is_dead() {
    let mut types = file(
        "pkg/list.go",
        vec![
            Symbol::new("Lonely", SymbolKind::Struct, 1, 3),
            member("Lonely", "Walk", 5),
            Symbol::new("Used", SymbolKind::Struct, 9, 9),
            function("Make", 11),
        ],
        vec![call(FILE_SCOPE.0, "Make", 20)],
    );
    for (site_of, name, line) in [(1, "Lonely", 5), (1, "Lonely", 6), (3, "Used", 12)] {
        types.uses.push(UseRef::new(
            SymbolId(site_of),
            name.to_owned(),
            Receiver::Bare,
            line,
        ));
    }
    let graph = graph_of(go(), &[types]);

    let dead = graph.dead_code(&[SymbolKind::Struct], None, 10);

    assert_eq!(names(&dead), ["Lonely"]);
    assert_eq!(dead.live, 1);
}

#[test]
fn a_method_a_call_through_another_type_may_reach_is_counted_as_dispatched() {
    let graph = graph(&[
        file(
            "src/shapes.ts",
            vec![
                Symbol::new("Shape", SymbolKind::Interface, 1, 3),
                member("Shape", "area", 2),
                with_parent(Symbol::new("Square", SymbolKind::Class, 4, 9), "Shape"),
                member("Square", "area", 5),
                member("Square", "describe", 7),
            ],
            Vec::new(),
        ),
        file(
            "src/widget.ts",
            vec![
                with_parent(Symbol::new("Widget", SymbolKind::Class, 1, 6), "Component"),
                member("Widget", "render", 2),
                Symbol::new("Plain", SymbolKind::Class, 7, 9),
                member("Plain", "render", 8),
            ],
            Vec::new(),
        ),
    ]);

    let dead = graph.dead_code(&[SymbolKind::Method], None, 10);

    assert_eq!(names(&dead), ["area", "describe", "render"]);
    assert_eq!(dead.found[0].definition.owner, Some("Shape"));
    assert_eq!(dead.found[2].definition.owner, Some("Plain"));
    assert_eq!(dead.excluded.dispatched, 2);
}

#[test]
fn a_method_defined_apart_from_its_owner_reads_the_parents_of_its_owners_name() {
    let graph = graph(&[
        file(
            "include/widget.h",
            vec![with_parent(
                Symbol::new("Widget", SymbolKind::Class, 1, 6),
                "Component",
            )],
            Vec::new(),
        ),
        file(
            "src/widget.cpp",
            vec![member("Widget", "render", 2)],
            Vec::new(),
        ),
    ]);

    let dead = graph.dead_code(&[SymbolKind::Method], None, 10);

    assert_eq!(dead.total, 0);
    assert_eq!(dead.excluded.dispatched, 1);
}

#[test]
fn a_declaration_and_the_definition_it_links_to_are_judged_once() {
    let mut header = file("src/api.h", vec![function("open", 1)], Vec::new());
    header.callables.push(Callable::new(
        SymbolId(0),
        "open()".to_owned(),
        CallableRole::Declaration,
    ));
    let mut source = file("src/api.c", vec![function("open", 3)], Vec::new());
    source.callables.push(Callable::new(
        SymbolId(0),
        "open()".to_owned(),
        CallableRole::Definition,
    ));
    source
        .includes
        .push(IncludeRef::new("src/api.h".to_owned()));
    let graph = graph(&[header, source]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["open"]);
    assert_eq!(dead.found[0].definition.file, "src/api.c");
}

#[test]
fn a_call_reaches_every_definition_the_tier_that_bound_it_held_equal() {
    let draw = |line| {
        let mut symbol = function("draw", line);
        symbol.qualified_name = "gfx::draw".to_owned();
        symbol
    };
    let graph = graph(&[
        file(
            "app/main.c",
            vec![function("run", 1)],
            vec![call(0, "gfx::draw", 2)],
        ),
        file(
            "lib/a.c",
            vec![
                function("helper", 1),
                function("helper", 3),
                function("user", 5),
            ],
            vec![call(2, "helper", 6)],
        ),
        file("lib/b.c", vec![function("helper", 1)], Vec::new()),
        file("real/render.c", vec![draw(1)], Vec::new()),
        file("stub/render.c", vec![draw(1)], Vec::new()),
    ]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["run", "user", "helper"]);
    assert_eq!(dead.found[2].definition.file, "lib/b.c");
    assert_eq!(dead.live, 4);
}

#[test]
fn every_definition_of_a_declaration_too_many_implement_to_link_is_reached_through_it() {
    let callable = |at: u32, role| Callable::new(SymbolId(at), "open()".to_owned(), role);
    let including = |mut extraction: Extraction| {
        extraction
            .includes
            .push(IncludeRef::new("include/api.h".to_owned()));
        extraction
    };
    let mut header = file("include/api.h", vec![function("open", 1)], Vec::new());
    header
        .callables
        .push(callable(0, CallableRole::Declaration));
    let mut real = including(file("src/real.c", vec![function("open", 1)], Vec::new()));
    real.callables.push(callable(0, CallableRole::Definition));
    let mut stub = including(file("src/stub.c", vec![function("open", 1)], Vec::new()));
    stub.callables.push(callable(0, CallableRole::Definition));
    let user = including(file(
        "app/main.c",
        vec![function("run", 1)],
        vec![call(0, "open", 2)],
    ));
    let graph = graph(&[header, real, stub, user]);

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(names(&dead), ["run"]);
    assert_eq!(dead.live, 3);
}

#[test]
fn the_limit_bounds_the_answer_and_a_path_prefix_the_files_judged() {
    let graph = graph(&[
        file(
            "app/a.c",
            vec![function("one", 1), function("two", 3)],
            Vec::new(),
        ),
        file("lib/b.c", vec![function("three", 1)], Vec::new()),
        file(
            "lib/c.c",
            vec![
                function("used", 1),
                flagged(function("check", 3), SymbolFlags::empty().with_test(true)),
            ],
            vec![call(1, "used", 4)],
        ),
    ]);

    let limited = graph.dead_code(&[], None, 1);
    let counted = graph.dead_code(&[], None, 0);
    let in_app = graph.dead_code(&[], Some("app/"), 10);

    assert_eq!((names(&limited), limited.total), (vec!["one"], 3));
    assert_eq!(
        (
            counted.found.len(),
            counted.total,
            counted.live,
            counted.excluded.test
        ),
        (0, 3, 1, 1)
    );
    assert_eq!(names(&in_app), ["one", "two"]);
}

fn binding_of_f(alias: &str) -> Extraction {
    let mut extraction = file("src/user.rs", Vec::new(), Vec::new());
    extraction.bindings = vec![ImportBinding::new(
        Some(alias.to_owned()),
        Some("src/m.rs".to_owned()),
        "f".to_owned(),
    )];
    extraction
}

fn f_is_spelled(graph: &Graph) -> bool {
    graph.dead_code(&[], None, 10).found[0].spelled
}

#[test]
fn a_rename_of_an_item_spells_its_name_until_the_file_that_writes_it_goes() {
    let module = file("src/m.rs", vec![function("f", 1)], Vec::new());
    let mut graph = graph(&[module, binding_of_f("g")]);

    assert!(f_is_spelled(&graph));

    graph.remove("src/user.rs");

    assert!(!f_is_spelled(&graph));
}

#[test]
fn a_glob_or_a_directory_package_s_alias_renames_no_item() {
    let module = || file("src/m.rs", vec![function("f", 1)], Vec::new());
    let mut packages = Conventions::default();
    packages.directory_packages = true;

    assert!(!f_is_spelled(&graph(&[module(), binding_of_f(".")])));
    assert!(!f_is_spelled(&graph(&[module(), binding_of_f("_")])));
    assert!(!f_is_spelled(&graph_of(
        packages,
        &[module(), binding_of_f("g")]
    )));
}
