//! What a PHP `new` binds, whatever spelling names the class: the class PHP compiles the name to and the constructor
//! its table holds, and no class that shares only the name's last part.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SiteQuery};
use tempfile::TempDir;

const FILES: [(&str, &str); 5] = [
    (
        "src/Builder.php",
        r"<?php
namespace App;

use Exception;
use Vendor\Gone as Outside;
use Vendor\Near;
use App\Sub as S;
use App\Sub\Thing as T;
use App\Sub\Plain as P;
use App\Sub\Plain;
use App\Sub\Child;
use App\Sub\Odd;
use App\Sub\Thing;

class Builder {
    public function plain() { new Exception; }
    public function outsideAlias() { new Outside; }
    public function outsideUse() { new Near(); }
    public function fromTheRootOutside() { new \Vendor\Thing; }
    public function qualified() { new S\Thing; }
    public function aliased() { new T(); }
    public function imported() { new Thing; }
    public function importedInAnotherCase() { new THING; }
    public function fromTheRoot() { new \App\Sub\Thing; }
    public function inherited() { new Child; }
    public function plainAlias() { new P; }
    public function plainUse() { new Plain(); }
    public function plainQualified() { new S\Plain; }
    public function plainFromTheRoot() { new \App\Sub\Plain; }
    public function ownNamespace() { new Local; }
    public function namespaceKeyword() { new namespace\Local; }
    public function ownName() { new Odd; }
    public function staticOwnName() { S\Odd::Odd(); }
    public function held() { $made = new S\Plain; $made->run(); }
}

class Local {}
",
    ),
    (
        "src/Sibling.php",
        r"<?php
namespace App;

class Thing {
    public function __construct() {}
}

class Plain {
    public function run() {}
}

class Near {
    public function __construct() {}
}
",
    ),
    (
        "src/Renderer/Exception.php",
        r"<?php
namespace App\Renderer;

class Exception {
    public function __construct() {}
}
",
    ),
    (
        "src/Sub/Thing.php",
        r"<?php
namespace App\Sub;

class Thing {
    public function __construct() {}
}

class Child extends Thing {}

class Plain {
    public function run() {}
}

class Odd {
    public function Odd() {}
}
",
    ),
    (
        "composer.json",
        r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
    ),
];

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in FILES {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// The sorted qualified names of each callee of a method of `App\Builder`, with the confidence of each.
fn callees_in(engine: &Engine, method: &str) -> Vec<(String, u8)> {
    let (_, graph) = engine.graph_of(FILES[0].0).expect("PHP claims the file");
    let mut callees: Vec<_> = graph
        .callees(format!("App\\Builder::{method}").as_str(), Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect();
    callees.sort();
    callees
}

fn callees(method: &str) -> Vec<(String, u8)> {
    callees_in(&build().1, method)
}

fn bound(names: &[&str]) -> Vec<(String, u8)> {
    names.iter().map(|&name| (name.to_owned(), 85)).collect()
}

#[test]
fn a_new_of_an_outside_class_binds_no_class_of_its_last_name() {
    let (_tree, engine) = build();
    for method in ["plain", "outsideAlias", "outsideUse", "fromTheRootOutside"] {
        assert_eq!(callees_in(&engine, method), [], "{method}");
    }
}

#[test]
fn every_spelling_of_a_class_binds_the_class_and_its_constructor() {
    let (_tree, engine) = build();
    for method in [
        "qualified",
        "aliased",
        "imported",
        "importedInAnotherCase",
        "fromTheRoot",
    ] {
        assert_eq!(
            callees_in(&engine, method),
            bound(&["App\\Sub\\Thing", "App\\Sub\\Thing::__construct"]),
            "{method}",
        );
    }
}

#[test]
fn every_spelling_of_a_class_with_no_constructor_binds_the_class() {
    let (_tree, engine) = build();
    for method in [
        "plainAlias",
        "plainUse",
        "plainQualified",
        "plainFromTheRoot",
    ] {
        assert_eq!(
            callees_in(&engine, method),
            bound(&["App\\Sub\\Plain"]),
            "{method}"
        );
    }
}

#[test]
fn a_name_no_use_binds_is_a_class_of_the_namespace() {
    let (_tree, engine) = build();
    for method in ["ownNamespace", "namespaceKeyword"] {
        assert_eq!(
            callees_in(&engine, method),
            bound(&["App\\Local"]),
            "{method}"
        );
    }
}

#[test]
fn a_new_binds_the_constructor_the_class_inherits() {
    assert_eq!(
        callees("inherited"),
        bound(&["App\\Sub\\Child", "App\\Sub\\Thing::__construct"])
    );
}

#[test]
fn a_new_binds_the_class_past_a_method_of_the_class_s_name() {
    assert_eq!(callees("ownName"), bound(&["App\\Sub\\Odd"]));
}

#[test]
fn a_static_call_of_the_class_s_name_binds_the_method() {
    assert_eq!(callees("staticOwnName"), bound(&["App\\Sub\\Odd::Odd"]));
}

#[test]
fn a_variable_a_new_assigned_holds_the_class_the_new_binds() {
    assert_eq!(
        callees("held"),
        bound(&["App\\Sub\\Plain", "App\\Sub\\Plain::run"])
    );
}

#[test]
fn a_class_lists_every_spelling_that_constructs_it() {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of(FILES[0].0).expect("PHP claims the file");
    let holders = |name: &str| -> Vec<String> {
        graph
            .references(name, SiteQuery::new(usize::MAX))
            .expect("the name is defined")
            .sites
            .iter()
            .map(|site| site.site_of.name.to_owned())
            .collect()
    };
    let spellings = [
        "qualified",
        "aliased",
        "imported",
        "importedInAnotherCase",
        "fromTheRoot",
    ];
    assert_eq!(holders("App\\Sub\\Thing"), spellings);
    assert_eq!(
        holders("App\\Sub\\Thing::__construct"),
        [&spellings[..], &["inherited"]].concat()
    );
}
