//! What a PHP anonymous class's members bind: a call on `$this` inside the class binds the class's own member, and a
//! call on a variable the creating callable assigned the class to binds that member as well, past a namesake method
//! of the class around it.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const SOURCE: &str = r"<?php
namespace App;

class Suite {
    public function testRun() {
        $job = new class {
            public function handle() { $this->step(); }
            public function step() {}
        };
        $job->handle();
    }

    public function handle() {}
    public function step() {}
}
";

fn build(source: &str) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("Suite.php"), source).expect("the file is written");
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees<'e>(engine: &'e Engine, name: &str) -> Vec<&'e str> {
    let (_, graph) = engine.graph_of("Suite.php").expect("PHP claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    callees
        .reached
        .iter()
        .map(|reached| reached.symbol.qualified_name)
        .collect()
}

#[test]
fn a_call_on_this_inside_an_anonymous_class_binds_the_class_s_own_member() {
    let (_tree, engine) = build(SOURCE);

    assert_eq!(
        callees(&engine, "App\\{Suite\u{b7}testRun\u{b7}job}::handle"),
        ["App\\{Suite\u{b7}testRun\u{b7}job}::step"]
    );
}

#[test]
fn a_call_on_a_variable_holding_an_anonymous_class_binds_the_class_s_member() {
    let (_tree, engine) = build(SOURCE);

    assert_eq!(
        callees(&engine, "App\\Suite::testRun"),
        [
            "App\\{Suite\u{b7}testRun\u{b7}job}",
            "App\\{Suite\u{b7}testRun\u{b7}job}::handle"
        ]
    );
}
