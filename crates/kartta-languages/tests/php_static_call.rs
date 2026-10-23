//! What a PHP static call binds by its name alone: a call on a class the graph does not hold, `Mockery::mock()`,
//! binds no method of another class, a call on a class the graph holds still binds the one method of the name, and a
//! call on an alias binds the class its `use` maps to.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const CONTAINER: &str = r"<?php
namespace App\Testing;

trait InteractsWithContainer {
    protected function mock($abstract) {}
}

class Model {
    public static function query() {}
}

class User extends Model {}

class Kernel {
    public function boot() {}
}
";

const ELOQUENT: &str = r"<?php
namespace App\Database;

class Model {
    public static function boot() {}
}
";

const TEST: &str = r"<?php
namespace Tests;

use Mockery;
use Mockery as m;
use App\Testing\User;
use App\Database\Model as Eloquent;

class UserTest {
    public function testMock() {
        Mockery::mock(Connection::class);
        m::mock(Connection::class);
    }

    public function testQuery() {
        User::query();
    }

    public function testBoot() {
        Eloquent::boot();
    }
}
";

const COMPOSER: &str = r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#;

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::create_dir_all(tree.path().join("src/Database")).expect("the directory is made");
    fs::create_dir_all(tree.path().join("tests")).expect("the directory is made");
    fs::write(tree.path().join("composer.json"), COMPOSER).expect("the file is written");
    fs::write(tree.path().join("src/Container.php"), CONTAINER).expect("the file is written");
    fs::write(tree.path().join("src/Database/Model.php"), ELOQUENT).expect("the file is written");
    fs::write(tree.path().join("tests/UserTest.php"), TEST).expect("the file is written");
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees<'e>(engine: &'e Engine, name: &str) -> Vec<&'e str> {
    let (_, graph) = engine
        .graph_of("tests/UserTest.php")
        .expect("PHP claims the file");
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
fn a_static_call_on_a_class_the_graph_does_not_hold_binds_no_method_of_another_class() {
    let (_tree, engine) = build();

    assert_eq!(
        callees(&engine, "Tests\\UserTest::testMock"),
        Vec::<&str>::new()
    );
}

#[test]
fn a_static_call_on_a_class_the_graph_holds_binds_the_one_method_of_its_name() {
    let (_tree, engine) = build();

    assert_eq!(
        callees(&engine, "Tests\\UserTest::testQuery"),
        ["App\\Testing\\Model::query"]
    );
}

#[test]
fn a_static_call_on_an_alias_binds_the_class_its_use_maps_to() {
    let (_tree, engine) = build();

    assert_eq!(
        callees(&engine, "Tests\\UserTest::testBoot"),
        ["App\\Database\\Model::boot"]
    );
}
