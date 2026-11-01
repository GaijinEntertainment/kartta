//! What a PHP call on a variable a `new` assigned binds: a member of the class the file's `new` names, past a method of
//! the name in a nearer file.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const CONTAINER: (&str, &str) = (
    "src/Container.php",
    r"<?php
namespace App;

class Container extends BaseContainer {
    public function instance($k) {}
}
",
);

const BASE_CONTAINER: (&str, &str) = (
    "src/BaseContainer.php",
    r"<?php
namespace App;

class BaseContainer {
    public function inherited() {}
}
",
);

/// A nearer file's method of the name, which the name tier binds when the receiver's class does not decide the call.
const NEAR_STUB: (&str, &str) = (
    "tests/Support/FacadeTest.php",
    r"<?php
namespace Tests\Support;

class ApplicationStub {
    public function instance($k) {}
    public function inherited() {}
    public function get() {}
}
",
);

/// The qualified name of each callee of `caller`, the first file's.
fn callees(files: &[(&str, &str)], caller: &str) -> Vec<String> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
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
        .graph_of(files[0].0)
        .expect("PHP claims the file");
    let mut reached: Vec<String> = graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| reached.symbol.qualified_name.to_owned())
        .collect();
    reached.sort();
    reached
}

fn callees_of_set_up(body: &str) -> Vec<String> {
    let source = format!(
        r"<?php
namespace Tests\Auth;

use App\Container;
use App\Container as Held;
use Vendor\Container as Outside;
use Vendor\Client;

class AuthTest {{
    public function setUp() {{
        {body}
    }}
}}

class Local extends Container {{}}
"
    );
    let files = [
        ("tests/Auth/AuthTest.php", source.as_str()),
        CONTAINER,
        BASE_CONTAINER,
        NEAR_STUB,
    ];
    callees(&files, "Tests\\Auth\\AuthTest::setUp")
}

#[test]
fn a_call_on_a_variable_a_new_assigned_binds_the_class_past_a_nearer_namesake() {
    assert_eq!(
        callees_of_set_up("$container = new Container; $container->instance('a');"),
        ["App\\Container", "App\\Container::instance"],
    );
}

#[test]
fn a_call_on_a_variable_a_new_assigned_binds_the_class_a_rooted_name_spells() {
    assert_eq!(
        callees_of_set_up("$container = new \\App\\Container; $container->instance('a');"),
        ["App\\Container", "App\\Container::instance"],
    );
}

#[test]
fn a_call_on_a_variable_a_new_of_an_alias_assigned_binds_the_class_the_alias_names() {
    assert_eq!(
        callees_of_set_up("$container = new Held; $container->instance('a');"),
        ["App\\Container::instance"],
    );
}

#[test]
fn a_call_a_class_inherits_binds_the_nearest_type_above_it() {
    assert_eq!(
        callees_of_set_up("$container = new Container; $container->inherited();"),
        ["App\\BaseContainer::inherited", "App\\Container"],
    );
}

#[test]
fn a_call_on_a_class_of_the_file_s_namespace_binds_its_member() {
    assert_eq!(
        callees_of_set_up("$local = new Local; $local->instance('a');"),
        ["App\\Container::instance", "Tests\\Auth\\Local"],
    );
}

#[test]
fn a_call_on_a_class_outside_the_tree_binds_no_namesake_of_the_tree() {
    assert_eq!(
        callees_of_set_up("$client = new Client; $client->get();"),
        Vec::<String>::new(),
    );
    assert_eq!(
        callees_of_set_up("$container = new Outside; $container->instance('a');"),
        Vec::<String>::new(),
    );
}

#[test]
fn a_method_no_type_above_the_class_holds_stays_with_the_tiers_that_read_no_class() {
    assert_eq!(
        callees_of_set_up("$container = new Container; $container->get();"),
        ["App\\Container", "Tests\\Support\\ApplicationStub::get"],
    );
}

#[test]
fn another_assignment_leaves_the_call_to_the_tiers_that_read_no_class() {
    assert_eq!(
        callees_of_set_up(
            "$container = new Container; $container = make(); $container->instance('a');"
        ),
        [
            "App\\Container",
            "Tests\\Support\\ApplicationStub::instance"
        ],
    );
}
