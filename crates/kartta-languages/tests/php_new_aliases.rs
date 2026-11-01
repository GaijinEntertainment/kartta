//! What a PHP construction and a `::class` callable written through a `use` alias bind: the class the alias imports,
//! never a class of the alias's name, and nothing where the class lies outside the tree.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SiteQuery};
use tempfile::TempDir;

const FILES: [(&str, &str); 6] = [
    (
        "src/T/Make.php",
        r"<?php
namespace T;

use A\B as C;
use A\Child as Heir;
use Vendor\Thing as B;
use A\Plain as Bare;

class Make {
    public function own() { return new C(); }
    public function inherited() { return new Heir; }
    public function outside() { return new B(); }
    public function noConstructor() { return new Bare(); }
    public function callable() { return [C::class, 'run']; }
    public function outsideCallable() { return [B::class, 'run']; }
}

class Near {
    public function __construct() {}
}
",
    ),
    (
        "src/A/Plain.php",
        r"<?php
namespace A;

class Plain {}
",
    ),
    (
        "src/A/B.php",
        r"<?php
namespace A;

class B {
    public function __construct() {}
    public function run() {}
}
",
    ),
    (
        "src/A/Child.php",
        r"<?php
namespace A;

class Child extends B {}
",
    ),
    (
        "src/T/Other.php",
        r"<?php
namespace T;

class C {
    public function __construct() {}
    public function run() {}
}

class B {
    public function __construct() {}
    public function run() {}
}
",
    ),
    ("composer.json", r#"{"autoload": {"psr-4": {"": "src/"}}}"#),
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

/// The qualified name and the confidence of each callee of `caller`.
fn callees(caller: &str) -> Vec<(String, u8)> {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of(FILES[0].0).expect("PHP claims the file");
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

/// The holder of each site of the symbol `name` selects.
fn holders(name: &str) -> Vec<String> {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of(FILES[0].0).expect("PHP claims the file");
    graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name is defined")
        .sites
        .iter()
        .map(|site| site.site_of.qualified_name.to_owned())
        .collect()
}

#[test]
fn a_new_of_an_alias_binds_the_class_it_imports_and_the_constructor() {
    assert_eq!(
        callees("T\\Make::own"),
        [
            ("A\\B".to_owned(), 85),
            ("A\\B::__construct".to_owned(), 85)
        ]
    );
}

#[test]
fn a_new_of_an_alias_binds_the_constructor_the_class_inherits() {
    assert_eq!(
        callees("T\\Make::inherited"),
        [
            ("A\\B::__construct".to_owned(), 85),
            ("A\\Child".to_owned(), 85)
        ]
    );
}

#[test]
fn a_new_of_an_alias_of_an_outside_class_binds_no_class_of_the_alias_s_name() {
    assert_eq!(callees("T\\Make::outside"), []);
}

#[test]
fn a_new_of_a_class_that_has_no_constructor_binds_no_other_class_s_constructor() {
    assert_eq!(
        callees("T\\Make::noConstructor"),
        [("A\\Plain".to_owned(), 85)]
    );
}

#[test]
fn a_callable_on_an_alias_reads_the_class_it_imports() {
    assert_eq!(holders("A\\B::run"), ["T\\Make::callable"]);
    assert_eq!(holders("T\\C::run"), Vec::<String>::new());
}

#[test]
fn a_callable_on_an_alias_of_an_outside_class_reads_no_class_of_the_alias_s_name() {
    assert_eq!(holders("T\\B::run"), Vec::<String>::new());
}
