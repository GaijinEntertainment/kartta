//! What a PHP call binds where parentheses stand around the value it reads: the member of the class inside them, past a
//! namesake of the same file.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const SOURCE: &str = r"<?php
namespace App;
class C {
    public function m() {}
}
class D {
    public function m() {}
}
class Uses {
    private $q;
    private $r;
    public function write() {
        $this->q = ($this->r = new C);
    }
    public function viaParen() { $this->q->m(); }
    public function viaHeldParen() {
        $w = (new C);
        $w->m();
    }
    public function viaNew() {
        (new C)->m();
    }
    public function viaVariable() {
        $w = new C;
        ($w)->m();
    }
    public function viaProperty() { ($this->q)->m(); }
}
";

/// The qualified name of each callee of `App\Uses::<method>`.
fn callees(method: &str) -> Vec<String> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let path = tree.path().join("src/A.php");
    fs::create_dir_all(path.parent().expect("a file has a directory"))
        .expect("the directory is made");
    fs::write(&path, SOURCE).expect("the file is written");
    fs::write(
        tree.path().join("composer.json"),
        r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
    )
    .expect("the file is written");
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of("src/A.php")
        .expect("PHP claims the file");
    let mut reached: Vec<String> = graph
        .callees(format!("App\\Uses::{method}").as_str(), Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| reached.symbol.qualified_name.to_owned())
        .collect();
    reached.sort();
    reached
}

#[test]
fn a_property_written_through_a_parenthesized_assignment_holds_the_class() {
    assert_eq!(callees("viaParen"), ["App\\C::m"]);
}

#[test]
fn a_variable_a_parenthesized_new_assigned_holds_the_class() {
    assert_eq!(callees("viaHeldParen"), ["App\\C", "App\\C::m"]);
}

#[test]
fn a_call_on_a_parenthesized_new_binds_the_class_it_creates() {
    assert_eq!(callees("viaNew"), ["App\\C", "App\\C::m"]);
}

#[test]
fn a_call_on_a_parenthesized_variable_or_property_reads_what_it_holds() {
    assert_eq!(callees("viaVariable"), ["App\\C", "App\\C::m"]);
    assert_eq!(callees("viaProperty"), ["App\\C::m"]);
}
