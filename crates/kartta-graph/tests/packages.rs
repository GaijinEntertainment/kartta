//! The package rules of a language whose package is a directory: which package a call sees.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the query helpers \
              of an integration test crate are neither; a name that selects nothing is the failure"
)]

use kartta_graph::{Graph, Limits};
use kartta_lang::{
    CallRef, Conventions, Extraction, ImportBinding, MemberCalls, OwnMembers, Symbol, SymbolFlags,
    SymbolId, SymbolKind,
};

fn go() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.directory_packages = true;
    conventions.member_calls = MemberCalls::Members;
    conventions
}

fn function(package: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Function, line, line + 1);
    symbol.qualified_name = format!("{package}.{name}");
    symbol.exported = true;
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
    extraction
}

fn test_file(mut extraction: Extraction) -> Extraction {
    for symbol in &mut extraction.symbols {
        symbol.flags = SymbolFlags::empty().with_test(true);
    }
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
