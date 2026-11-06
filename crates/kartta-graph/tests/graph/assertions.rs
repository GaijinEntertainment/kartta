//! Compile-time implements assertions, `var _ io.Reader = (*File)(nil)` in Go: the type they name implements the
//! interface they name, both names read from the asserting file.

use kartta_graph::{Graph, Implementations};
use kartta_lang::{
    Conventions, Extraction, ImplementsAssertion, ImportBinding, Symbol, SymbolKind,
};

use crate::{file, graph_of};

fn ty(name: &str, qualified: &str, kind: SymbolKind) -> Symbol {
    let mut symbol = Symbol::new(name, kind, 1, 5);
    qualified.clone_into(&mut symbol.qualified_name);
    symbol
}

fn asserting(path: &str, assertions: &[(&str, &str)], imports: &[(&str, &str)]) -> Extraction {
    let mut extraction = file(path, Vec::new());
    extraction.assertions = assertions
        .iter()
        .map(|&(type_name, interface)| {
            ImplementsAssertion::new(type_name.to_owned(), interface.to_owned())
        })
        .collect();
    extraction.bindings = imports
        .iter()
        .map(|&(name, dir)| {
            ImportBinding::new(Some(name.to_owned()), Some(dir.to_owned()), name.to_owned())
        })
        .collect();
    extraction
}

fn graph(files: &[Extraction]) -> Graph {
    let mut conventions = Conventions::default();
    conventions.directory_packages = true;
    graph_of(conventions, files)
}

fn found(implementations: &Implementations<'_>) -> Vec<String> {
    implementations
        .found
        .iter()
        .map(|implementor| {
            format!(
                "{}:{}",
                implementor.symbol.file, implementor.symbol.qualified_name
            )
        })
        .collect()
}

fn package() -> Vec<Extraction> {
    vec![
        file(
            "io/reader.go",
            vec![ty("Reader", "io.Reader", SymbolKind::Interface)],
        ),
        file(
            "app/closer.go",
            vec![ty("Closer", "app.Closer", SymbolKind::Interface)],
        ),
        file(
            "app/file.go",
            vec![ty("File", "app.File", SymbolKind::Struct)],
        ),
        file(
            "lib/handle.go",
            vec![ty("Handle", "lib.Handle", SymbolKind::TypeAlias)],
        ),
    ]
}

#[test]
fn an_assertion_makes_the_type_it_names_an_implementor_of_the_interface_it_names() {
    let mut files = package();
    files.push(asserting(
        "app/assert.go",
        &[
            ("File", "io.Reader"),
            ("File", "Closer"),
            ("lib.Handle", "Closer"),
        ],
        &[("io", "io"), ("lib", "lib")],
    ));
    let graph = graph(&files);

    let reader = graph.implementations("io.Reader", 1, 10).expect("a type");
    let closer = graph.implementations("app.Closer", 1, 10).expect("a type");

    assert_eq!(found(&reader), ["app/file.go:app.File"]);
    assert_eq!(
        found(&closer),
        ["app/file.go:app.File", "lib/handle.go:lib.Handle"]
    );
}

#[test]
fn an_assertion_whose_names_reach_another_type_names_none_of_these() {
    let mut files = package();
    files.push(file(
        "other/closer.go",
        vec![ty("Closer", "other.Closer", SymbolKind::Interface)],
    ));
    files.push(asserting(
        "other/assert.go",
        &[("File", "Closer"), ("app.Missing", "Closer")],
        &[("app", "app")],
    ));
    let graph = graph(&files);

    let own = graph.implementations("app.Closer", 1, 10).expect("a type");
    let other = graph
        .implementations("other.Closer", 1, 10)
        .expect("a type");

    assert_eq!(found(&own), [] as [&str; 0]);
    assert_eq!(found(&other), [] as [&str; 0]);
}

#[test]
fn an_assertion_of_an_interface_outside_the_graph_lists_the_type_it_names() {
    let mut files = package();
    files.retain(|extraction| extraction.file != "io/reader.go");
    files.push(asserting(
        "app/assert.go",
        &[("File", "io.Reader"), ("File", "Closer")],
        &[("io", "io")],
    ));
    let graph = graph(&files);

    let reader = graph.implementations_outside("io.Reader", 10);
    let closer = graph.implementations_outside("Closer", 10);

    assert_eq!(graph.implementations("io.Reader", 1, 10), None);
    assert_eq!(
        (reader.found.iter())
            .map(|definition| definition.qualified_name)
            .collect::<Vec<_>>(),
        ["app.File"]
    );
    assert_eq!(closer.total, 0);
}

#[test]
fn a_type_name_two_types_of_the_package_share_names_neither() {
    let mut files = package();
    files.push(file(
        "app/file_test.go",
        vec![ty("File", "app_test.File", SymbolKind::Struct)],
    ));
    files.push(asserting("app/assert.go", &[("File", "Closer")], &[]));
    let graph = graph(&files);

    let closer = graph.implementations("app.Closer", 1, 10).expect("a type");

    assert_eq!(found(&closer), [] as [&str; 0]);
}

#[test]
fn a_type_name_the_asserting_file_declares_names_that_type_beside_a_namesake() {
    let mut files = package();
    let mut own = asserting("app/file_test.go", &[("File", "Closer")], &[]);
    own.symbols = vec![ty("File", "app_test.File", SymbolKind::Struct)];
    files.push(own);
    let graph = graph(&files);

    let closer = graph.implementations("app.Closer", 1, 10).expect("a type");

    assert_eq!(found(&closer), ["app/file_test.go:app_test.File"]);
}

#[test]
fn a_qualified_type_name_names_the_imported_type_beside_the_file_s_namesake() {
    let mut files = package();
    let mut own = asserting("app/own.go", &[("lib.Handle", "Closer")], &[("lib", "lib")]);
    own.symbols = vec![ty("Handle", "app.Handle", SymbolKind::Struct)];
    files.push(own);
    let graph = graph(&files);

    let closer = graph.implementations("app.Closer", 1, 10).expect("a type");

    assert_eq!(found(&closer), ["lib/handle.go:lib.Handle"]);
}

#[test]
fn a_replaced_asserting_file_stops_asserting() {
    let mut files = package();
    files.push(asserting("app/assert.go", &[("File", "Closer")], &[]));
    let mut graph = graph(&files);

    graph.insert(&asserting("app/assert.go", &[], &[]));
    let closer = graph.implementations("app.Closer", 1, 10).expect("a type");

    assert_eq!(found(&closer), [] as [&str; 0]);
}
