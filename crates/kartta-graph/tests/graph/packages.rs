//! The package rules of a language whose package is a directory: which package a call sees.

use kartta_graph::Graph;
use kartta_lang::{
    BuiltApart, CallRef, Conventions, Extraction, MemberCalls, OwnMembers, SymbolFlags, Twins,
};

use crate::go_code::{callees_of, file, function, importing};
use crate::{call, graph_of, member_call};

fn go() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.directory_packages = true;
    conventions.member_calls = MemberCalls::Members;
    conventions
}

fn bare_call(callee: &str, line: u32) -> CallRef {
    call(0, callee, line)
}

fn test_file(mut extraction: Extraction) -> Extraction {
    for symbol in &mut extraction.symbols {
        symbol.flags = SymbolFlags::empty().with_test(true);
    }
    extraction
}

fn graph(files: &[Extraction]) -> Graph {
    graph_of(go(), files)
}

#[test]
fn a_build_constraint_goes_with_its_file_when_the_file_is_replaced_or_removed() {
    let declaring = |path: &str, constraint: Option<&str>| {
        let mut extraction = file(path, "lib", vec![function("lib", "New", 1)], Vec::new());
        extraction.build_constraint = constraint.map(str::to_owned);
        extraction
    };
    let mut caller = file(
        "lib/run.go",
        "lib",
        vec![function("lib", "run", 1)],
        vec![bare_call("New", 1)],
    );
    caller.build_constraint = Some("here".to_owned());
    let mut conventions = go();
    conventions.twins = Twins::Package(BuiltApart(|one, other| one == "apart" || other == "apart"));
    let mut graph = graph_of(
        conventions,
        &[caller, declaring("lib/new.go", Some("apart"))],
    );
    assert_eq!(callees_of(&graph, "lib.run").len(), 0);

    graph.insert(&declaring("lib/new.go", None));
    assert_eq!(callees_of(&graph, "lib.run").len(), 1);

    graph.insert(&declaring("lib/new.go", Some("apart")));
    assert_eq!(callees_of(&graph, "lib.run").len(), 0);

    graph.remove("lib/new.go");
    graph.insert(&declaring("lib/other.go", None));
    assert_eq!(callees_of(&graph, "lib.run").len(), 1);
}

#[test]
fn a_bare_call_binds_in_the_package_its_file_declares_and_never_through_an_import() {
    let library = file(
        "lib/lib.go",
        "lib",
        vec![function("lib", "helper", 1), function("lib", "Shared", 2)],
        Vec::new(),
    );
    let helpers = test_file(file(
        "lib/helpers_test.go",
        "lib_test",
        vec![function("lib_test", "helper", 1)],
        Vec::new(),
    ));
    let tests = test_file(importing(
        file(
            "lib/lib_test.go",
            "lib_test",
            vec![function("lib_test", "TestRun", 1)],
            vec![bare_call("helper", 2), bare_call("Shared", 3)],
        ),
        &[("lib", Some("lib"))],
    ));
    let mut through_import = file(
        "app/main.go",
        "main",
        vec![function("main", "Run", 1)],
        vec![bare_call("Shared", 2)],
    );
    through_import.includes = vec![kartta_lang::IncludeRef::new("lib/lib.go".to_owned())];

    let graph = graph(&[library, helpers, tests, through_import]);

    assert_eq!(
        callees_of(&graph, "lib_test.TestRun"),
        [("lib/helpers_test.go:lib_test.helper".to_owned(), 75)]
    );
    assert!(
        callees_of(&graph, "main.Run").is_empty(),
        "a bare call cannot name an imported package's function"
    );
}

#[test]
fn an_external_test_package_sees_what_the_tested_package_s_test_files_export() {
    let library = file(
        "lib/lib.go",
        "lib",
        vec![function("lib", "Public", 1)],
        Vec::new(),
    );
    let exports = test_file(file(
        "lib/export_test.go",
        "lib",
        vec![function("lib", "ExportedForTest", 1)],
        Vec::new(),
    ));
    let tests = test_file(importing(
        file(
            "lib/lib_test.go",
            "lib_test",
            vec![function("lib_test", "TestRun", 1)],
            vec![member_call("lib", "ExportedForTest", 2)],
        ),
        &[("lib", Some("lib"))],
    ));
    let elsewhere = importing(
        file(
            "app/main.go",
            "main",
            vec![function("main", "Run", 1)],
            vec![member_call("lib", "ExportedForTest", 2)],
        ),
        &[("lib", Some("lib"))],
    );

    let graph = graph(&[library, exports, tests, elsewhere]);

    assert_eq!(
        callees_of(&graph, "lib_test.TestRun"),
        [("lib/export_test.go:lib.ExportedForTest".to_owned(), 95)]
    );
    assert!(
        callees_of(&graph, "main.Run").is_empty(),
        "another package's import sees no test file"
    );
}
