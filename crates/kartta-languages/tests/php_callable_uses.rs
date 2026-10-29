//! What a PHP callable spelled as data binds: a string in a builtin's callable argument binds the function its file
//! declares, an array on `$this` binds the method of the class around it, and a string that spells a namespace other
//! than the file's names nothing.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
use tempfile::TempDir;

const GLOBAL: &str = r"<?php
function helper($x) { return $x; }

function run($items) {
    return array_map('helper', $items);
}

class View {
    public function show() {
        return call_user_func([$this, 'render']);
    }

    public function render() {}
}

class Other {
    public function render() {}
}
";

const NAMESPACED: &str = r"<?php
namespace App;

function helper($x) { return $x; }

function run($items) {
    array_map('helper', $items);
    return array_map('App\helper', $items);
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("global.php"), GLOBAL).expect("the file is written");
    fs::write(tree.path().join("app.php"), NAMESPACED).expect("the file is written");
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// A site's holder, line, whether it is a call, and confidence.
type Site<'e> = (&'e str, u32, bool, u8);

/// The file of the symbol `name` selects, and its sites.
fn sites<'e>(engine: &'e Engine, name: &str) -> (&'e str, Vec<Site<'e>>) {
    let (_, graph) = engine.graph_of("global.php").expect("PHP claims the file");
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name is defined");
    let sites = references
        .sites
        .iter()
        .map(|site| {
            (
                site.site_of.qualified_name,
                site.line,
                site.call,
                site.confidence,
            )
        })
        .collect();
    (references.symbol.file, sites)
}

#[test]
fn a_callable_string_binds_the_function_its_file_declares() {
    let (_tree, engine) = build();

    assert_eq!(
        sites(&engine, "helper"),
        ("global.php", vec![("run", 5, false, 80)])
    );
}

#[test]
fn a_callable_array_on_this_binds_the_method_of_the_class_around_it() {
    let (_tree, engine) = build();

    assert_eq!(
        (
            sites(&engine, "View::render"),
            sites(&engine, "Other::render")
        ),
        (
            ("global.php", vec![("View::show", 10, false, 85)]),
            ("global.php", Vec::new())
        )
    );
}

#[test]
fn a_callable_string_binds_only_where_it_spells_the_file_s_namespace() {
    let (_tree, engine) = build();

    assert_eq!(
        sites(&engine, "App\\helper"),
        ("app.php", vec![("App\\run", 8, false, 80)]),
        "the bare string names the global function, which a file of App does not declare"
    );
}
