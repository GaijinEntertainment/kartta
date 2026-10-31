//! What a PHP anonymous class's members bind: a call on `$this` inside the class binds the class's own member, a call
//! on a variable the creating callable assigned the class to binds that member as well, past a namesake method of the
//! class around it, and a class that lacks the member binds no anonymous class of another file.
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

const INHERITS: &str = r"<?php
namespace App;

class Trim {
    public function handle() {}
}

class TrimTest {
    public function testTrim() {
        $middleware = new class extends Trim {};
        $middleware->handle();
    }
}
";

const NAMESAKE: &str = r"<?php
namespace App;

class QueueTest {
    public function testQueue() {
        $middleware = new class {
            public function handle() {}
        };
    }
}
";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (name, source) in files {
        fs::write(tree.path().join(name), source).expect("the file is written");
    }
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees<'e>(engine: &'e Engine, file: &str, name: &str) -> Vec<&'e str> {
    let (_, graph) = engine.graph_of(file).expect("PHP claims the file");
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
    let (_tree, engine) = build(&[("Suite.php", SOURCE)]);

    assert_eq!(
        callees(&engine, "Suite.php", "App\\{testRun\u{b7}job}::handle"),
        ["App\\{testRun\u{b7}job}::step"]
    );
}

#[test]
fn a_call_on_a_variable_holding_an_anonymous_class_binds_the_class_s_member() {
    let (_tree, engine) = build(&[("Suite.php", SOURCE)]);

    assert_eq!(
        callees(&engine, "Suite.php", "App\\Suite::testRun"),
        ["App\\{testRun\u{b7}job}", "App\\{testRun\u{b7}job}::handle"]
    );
}

#[test]
fn an_inherited_call_binds_the_base_past_a_namesake_variable_s_class_in_another_file() {
    let (_tree, engine) = build(&[("TrimTest.php", INHERITS), ("QueueTest.php", NAMESAKE)]);

    assert_eq!(
        callees(&engine, "TrimTest.php", "App\\TrimTest::testTrim"),
        ["App\\Trim::handle", "App\\{testTrim\u{b7}middleware}"]
    );
}
