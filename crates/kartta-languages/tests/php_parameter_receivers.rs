//! What a PHP call on a parameter binds: a member of the class its type declares, past a method of the name in a nearer
//! file, and no namesake of a class outside the graph.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const COMPOSER: (&str, &str) = (
    "composer.json",
    r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
);

const TWO_CLASSES: &str = r"<?php
namespace App;
use Vendor\Logger;
class Container {
    public function instance($k) {}
}
class Other {
    public function instance($k) {}
}
";

/// A namesake of a class outside the tree, which holds the method the outside class's call names.
const NAMESAKE: (&str, &str) = (
    "src/Support/Logger.php",
    r"<?php
namespace App\Support;

class Logger {
    public function info($m) {}
}
",
);

/// The qualified name of each callee of `App\Kernel::typed`, whose parameters and body the test writes.
fn callees_of_typed(parameters: &str, body: &str) -> Vec<String> {
    let source = format!(
        r"{TWO_CLASSES}
class Kernel {{
    public function typed({parameters}) {{
        {body}
    }}
    public function own() {{}}
}}
"
    );
    let files = [("src/Kernel.php", source.as_str()), COMPOSER, NAMESAKE];
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
    graph
        .callees("App\\Kernel::typed", Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| reached.symbol.qualified_name.to_owned())
        .collect()
}

#[test]
fn a_call_on_a_parameter_binds_the_member_of_the_class_its_type_declares() {
    assert_eq!(
        callees_of_typed("Container $c", "$c->instance(1);"),
        ["App\\Container::instance"],
    );
}

#[test]
fn a_nullable_parameter_and_a_promoted_one_bind_alike() {
    for parameter in ["?Container $c", "Container|null $c", "private Container $c"] {
        assert_eq!(
            callees_of_typed(parameter, "$c->instance(1);"),
            ["App\\Container::instance"],
            "{parameter}",
        );
    }
}

#[test]
fn a_parameter_of_the_caller_s_own_type_binds_the_caller_s_member() {
    assert_eq!(
        callees_of_typed("self $c", "$c->own();"),
        ["App\\Kernel::own"],
    );
}

#[test]
fn a_type_outside_the_graph_binds_no_namesake_of_the_tree() {
    for (parameter, body) in [
        ("\\Vendor\\Logger $l", "$l->info('m');"),
        ("Logger $l", "$l->info('m');"),
        ("\\Closure $l", "$l->instance(1);"),
        ("\\Vendor\\Logger $l", "$l::info('m');"),
    ] {
        assert_eq!(
            callees_of_typed(parameter, body),
            Vec::<String>::new(),
            "{parameter}",
        );
    }
}

#[test]
fn a_test_of_an_assignment_binds_as_a_test_of_the_variable_does() {
    for (body, control) in [
        (
            "if (($c = make()) instanceof Container) { $c->instance(1); }",
            "$c = make(); if ($c instanceof Container) { $c->instance(1); }",
        ),
        (
            "if (!($c = make()) instanceof Container) { return; } $c->instance(1);",
            "$c = make(); if (!$c instanceof Container) { return; } $c->instance(1);",
        ),
    ] {
        assert_eq!(callees_of_typed("", control), ["App\\Container::instance"]);
        assert_eq!(
            callees_of_typed("", body),
            ["App\\Container::instance"],
            "{body}"
        );
    }
}

#[test]
fn a_write_to_the_parameter_leaves_the_call_to_the_tiers_that_read_no_class() {
    assert_eq!(
        callees_of_typed("Container $c", "$c = make(); $c->instance(1);"),
        Vec::<String>::new(),
    );
}
