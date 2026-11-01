//! What a static PHP call on a qualified class name binds: the class PHP compiles the name to, its own member or the
//! one above it, and no class that shares only the name's last part.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const FILES: [(&str, &str); 4] = [
    (
        "src/Caller.php",
        r"<?php
namespace App;

use Lib\Tools;
use Lib\Tools as Kit;

class Thing {
    public static function f() {}
}

class Caller {
    public function relativeToTheNamespace() { Sub\Thing::f(); }
    public function fromTheRoot() { \Vendor\Thing::f(); }
    public function ownFromTheRoot() { \App\Thing::f(); }
    public function ownThroughTheNamespaceKeyword() { namespace\Thing::f(); }
    public function outsideThroughTheNamespaceKeyword() { namespace\Gone\Thing::f(); }
    public function heldRelativeToTheNamespace() { Deep\Held::own(); }
    public function inheritedRelativeToTheNamespace() { Deep\Held::inherited(); }
    public function inheritedFromTheRoot() { \App\Deep\Held::inherited(); }
    public function throughAnImport() { Tools\Saw::cut(); }
    public function throughAnImportInAnotherCase() { tools\Saw::cut(); }
    public function throughAnAlias() { Kit\Saw::cut(); }
    public function outsideThroughAnImport() { Tools\Thing::f(); }
}
",
    ),
    (
        "src/Deep/Held.php",
        r"<?php
namespace App\Deep;

class Base {
    public static function inherited() {}
}

class Held extends Base {
    public static function own() {}
}
",
    ),
    (
        "src/Lib/Tools/Saw.php",
        r"<?php
namespace Lib\Tools;

class Saw {
    public static function cut() {}
}
",
    ),
    (
        "composer.json",
        r#"{"autoload": {"psr-4": {"App\\": "src/", "Lib\\": "src/Lib/"}}}"#,
    ),
];

/// The qualified name and the confidence of each callee of `App\Caller::<method>`.
fn callees(method: &str) -> Vec<(String, u8)> {
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
    let (_, graph) = outcome
        .engine
        .graph_of(FILES[0].0)
        .expect("PHP claims the file");
    graph
        .callees(format!("App\\Caller::{method}").as_str(), Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

fn bound(qualified: &str) -> [(String, u8); 1] {
    [(qualified.to_owned(), 85)]
}

#[test]
fn a_name_relative_to_the_namespace_binds_no_class_of_its_last_part() {
    assert_eq!(callees("relativeToTheNamespace"), []);
}

#[test]
fn a_name_from_the_root_outside_the_graph_binds_no_class_of_its_last_part() {
    assert_eq!(callees("fromTheRoot"), []);
}

#[test]
fn a_name_from_the_root_binds_the_class_it_spells() {
    assert_eq!(callees("ownFromTheRoot"), bound("App\\Thing::f"));
}

#[test]
fn the_namespace_keyword_names_a_class_of_the_file_s_namespace() {
    assert_eq!(
        callees("ownThroughTheNamespaceKeyword"),
        bound("App\\Thing::f")
    );
    assert_eq!(callees("outsideThroughTheNamespaceKeyword"), []);
}

#[test]
fn a_name_relative_to_the_namespace_binds_the_member_of_the_class_it_compiles_to() {
    assert_eq!(
        callees("heldRelativeToTheNamespace"),
        bound("App\\Deep\\Held::own")
    );
}

#[test]
fn a_qualified_name_binds_a_parent_s_member_by_the_walk() {
    for caller in ["inheritedRelativeToTheNamespace", "inheritedFromTheRoot"] {
        assert_eq!(
            callees(caller),
            bound("App\\Deep\\Base::inherited"),
            "{caller}"
        );
    }
}

#[test]
fn the_first_part_of_a_qualified_name_reads_through_the_file_s_imports() {
    for caller in [
        "throughAnImport",
        "throughAnImportInAnotherCase",
        "throughAnAlias",
    ] {
        assert_eq!(callees(caller), bound("Lib\\Tools\\Saw::cut"), "{caller}");
    }
    assert_eq!(callees("outsideThroughAnImport"), []);
}
