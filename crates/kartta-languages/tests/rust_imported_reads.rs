//! What a Rust function read as a value through a `use` reaches: a name a `use` or a glob imports from another module
//! file binds that module's function, as a bare call of the name does, so the function leaves the dead list, while a
//! namesake no `use` names stays on it.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Selector, SiteQuery};
use tempfile::TempDir;

const RENDER: &str = "mod calls;
mod other;

fn flat(signature: &str) -> String {
    signature.to_owned()
}
";

const CALLS: &str = "use super::flat;

pub fn sig(signature: Option<&str>) -> Option<String> {
    signature.map(flat)
}
";

const ORDER: &str = "pub fn by_name(a: &u8, b: &u8) -> std::cmp::Ordering {
    a.cmp(b)
}
";

const SORTING: &str = "use crate::order::*;

pub fn sort(items: &mut [u8]) {
    items.sort_by(by_name);
}

pub fn sort_by_path(items: &mut [u8]) {
    items.sort_by(crate::order::by_name);
}
";

const EXTRACT: &str = "mod walk;

pub fn extract(source: &[u8]) -> usize {
    apply(source, walk::raw_walk)
}

fn apply(source: &[u8], walk: fn(&[u8]) -> usize) -> usize {
    walk(source)
}
";

const WALK: &str = "pub(crate) fn raw_walk(source: &[u8]) -> usize {
    source.len()
}
";

const OTHER: &str = "fn flat(signature: &str) -> String {
    signature.to_owned()
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"reads\"\nversion = \"0.1.0\"\n",
        ),
        (
            "src/lib.rs",
            "pub mod extract;\npub mod order;\npub mod render;\npub mod sorting;\n",
        ),
        ("src/render.rs", RENDER),
        ("src/render/calls.rs", CALLS),
        ("src/order.rs", ORDER),
        ("src/sorting.rs", SORTING),
        ("src/extract.rs", EXTRACT),
        ("src/extract/walk.rs", WALK),
        ("src/render/other.rs", OTHER),
    ];
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["rust"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// Each site of the definition of `name` in `path`: its holder, its line and whether it is a call.
fn sites(engine: &Engine, name: &str, path: &str) -> Vec<(String, u32, bool)> {
    let (_, graph) = engine.graph_of("src/lib.rs").expect("Rust claims the file");
    let references = graph
        .references(Selector::new(name, Some(path)), SiteQuery::new(usize::MAX))
        .expect("the name is defined");
    references
        .sites
        .iter()
        .map(|site| (site.site_of.qualified_name.to_owned(), site.line, site.call))
        .collect()
}

fn dead(engine: &Engine) -> Vec<(String, String)> {
    let (_, graph) = engine.graph_of("src/lib.rs").expect("Rust claims the file");
    graph
        .dead_code(&[], None, 10)
        .found
        .iter()
        .map(|dead| {
            (
                dead.definition.file.to_owned(),
                dead.definition.qualified_name.to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_function_a_use_imports_and_a_body_reads_as_a_value_is_a_use_of_it() {
    let (_tree, engine) = build();

    assert_eq!(
        sites(&engine, "flat", "src/render.rs"),
        [("sig".to_owned(), 4, false)]
    );
    assert_eq!(sites(&engine, "flat", "src/render/other.rs"), []);
}

#[test]
fn a_function_a_glob_imports_and_a_body_reads_as_a_value_is_a_use_of_it() {
    let (_tree, engine) = build();

    assert_eq!(
        sites(&engine, "by_name", "src/order.rs")[0],
        ("sort".to_owned(), 4, false)
    );
}

#[test]
fn a_function_a_path_through_a_module_names_as_a_value_is_a_use_of_it() {
    let (_tree, engine) = build();

    assert_eq!(
        sites(&engine, "by_name", "src/order.rs"),
        [
            ("sort".to_owned(), 4, false),
            ("sort_by_path".to_owned(), 8, false)
        ]
    );
    assert_eq!(
        sites(&engine, "raw_walk", "src/extract/walk.rs"),
        [("extract".to_owned(), 4, false)]
    );
}

#[test]
fn only_a_namesake_no_use_imports_reads_as_unused() {
    let (_tree, engine) = build();

    assert_eq!(
        dead(&engine),
        [
            ("src/extract.rs".to_owned(), "extract".to_owned()),
            ("src/render/calls.rs".to_owned(), "sig".to_owned()),
            ("src/render/other.rs".to_owned(), "flat".to_owned()),
            ("src/sorting.rs".to_owned(), "sort".to_owned()),
            ("src/sorting.rs".to_owned(), "sort_by_path".to_owned()),
        ]
    );
}
