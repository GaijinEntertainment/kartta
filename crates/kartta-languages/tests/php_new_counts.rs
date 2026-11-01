//! How a PHP `new` counts among the calls that bound nothing: a class outside the graph is one such site, whose two
//! calls leave the class and its constructor unread as one, and a class of the graph counts each call that bound none.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const OUTSIDE: &str = r#"<?php // second/Out.php
namespace App;
function outside() { new \Exception("x"); new \Outside\Thing(); }
"#;

const HELD: &str = r"<?php
namespace App;

use Vendor\Gone as Alias;

class Made {
    public function __construct() {}
}

class Plain {}

class Child extends Base {
    public function __construct() { Base::__construct(); }
}

function made() { new Made(); }
function plain() { new Plain(); }
function aliased() { new Alias(); }
function called() { Base::__construct(); }
function fromTheRoot() { \Vendor\Gone::__construct(); }
function afterTheClass() { new \Vendor\Gone(); \Vendor\Gone::__construct(); }
function afterBareName() { Gone::Gone(); Gone::__construct(); }
function afterTheName() { \Vendor\Gone::Gone(); \Vendor\Gone::__construct(); }
function afterTheAlias() { Alias::Gone(); Alias::__construct(); }
";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        fs::write(tree.path().join(path), source).expect("the file is written");
    }
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// How many callees `name` reaches and how many of its calls bind nothing.
fn counted(engine: &Engine, path: &str, name: &str) -> (usize, usize) {
    let (_, graph) = engine.graph_of(path).expect("PHP claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.len(), callees.unresolved)
}

#[test]
fn a_new_of_an_outside_class_is_one_unresolved_call() {
    let (_tree, engine) = build(&[("Out.php", OUTSIDE)]);
    assert_eq!(counted(&engine, "Out.php", "App\\outside"), (0, 2));
}

#[test]
fn coverage_counts_a_new_of_an_outside_class_as_one_site_it_lists_by_the_class() {
    let (_tree, engine) = build(&[("Out.php", OUTSIDE)]);
    let (_, graph) = engine.graph_of("Out.php").expect("PHP claims the file");
    let resolution = graph.resolution(usize::MAX);
    assert_eq!(resolution.sites, 2);
    let listed: Vec<_> = (resolution.unbound.iter())
        .map(|site| (site.callee, site.receiver))
        .collect();
    assert_eq!(
        listed,
        [
            ("Exception", Some("\\Exception")),
            ("Thing", Some("\\Outside\\Thing"))
        ]
    );
    assert_eq!(resolution.unlisted, 0);
}

#[test]
fn an_alias_of_an_outside_class_is_one_unresolved_call() {
    let (_tree, engine) = build(&[("Held.php", HELD)]);
    assert_eq!(counted(&engine, "Held.php", "App\\aliased"), (0, 1));
}

#[test]
fn a_new_of_a_class_of_the_graph_counts_as_before() {
    let (_tree, engine) = build(&[("Held.php", HELD)]);
    assert_eq!(counted(&engine, "Held.php", "App\\made"), (2, 0));
    assert_eq!(counted(&engine, "Held.php", "App\\plain"), (1, 1));
}

#[test]
fn a_constructor_called_by_hand_stays_a_call() {
    let (_tree, engine) = build(&[("Held.php", HELD)]);
    assert_eq!(counted(&engine, "Held.php", "App\\called"), (0, 1));
    assert_eq!(counted(&engine, "Held.php", "App\\fromTheRoot"), (0, 1));
    assert_eq!(counted(&engine, "Held.php", "App\\afterTheClass"), (0, 2));
    assert_eq!(counted(&engine, "Held.php", "App\\afterBareName"), (0, 2));
    assert_eq!(counted(&engine, "Held.php", "App\\afterTheName"), (0, 2));
    assert_eq!(counted(&engine, "Held.php", "App\\afterTheAlias"), (0, 2));
}

#[test]
fn coverage_counts_a_constructor_called_by_hand_as_a_site() {
    let (_tree, engine) = build(&[(
        "Ex.php",
        r"<?php
function raise() { \Exception::__construct(); }
",
    )]);
    let (_, graph) = engine.graph_of("Ex.php").expect("PHP claims the file");
    assert_eq!(graph.resolution(0).sites, 1);
}
