//! The base a JavaScript class's `extends` names reaches every reader of a class's parents, as the same class's does in
//! TypeScript, whose grammar wraps the base in a clause of its own.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        fs::write(tree.path().join(path), source).expect("the file is written");
    }
    let languages =
        kartta_languages::select(["javascript", "typescript"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

const CONTROLLER: [(&str, &str); 2] = [
    (
        "target.js",
        "export class Target { dispatchEvent(event) { return event; } }\n",
    ),
    (
        "controller.js",
        "import { Target } from './target.js';
export class Controller extends Target {
  pause() { return this.dispatchEvent(1); }
}
",
    ),
];

#[test]
fn an_own_receiver_reaches_the_member_of_the_class_a_javascript_class_extends() {
    let (_tree, engine) = build(&CONTROLLER);
    let (_, graph) = engine
        .graph_of("target.js")
        .expect("JavaScript claims the file");

    let callees = graph
        .callees("Controller.pause", Limits::new(1))
        .expect("the name is defined");

    let reached: Vec<(&str, u8)> = (callees.reached.iter())
        .map(|reached| (reached.symbol.qualified_name, reached.confidence))
        .collect();
    assert_eq!(reached, [("Target.dispatchEvent", 70)]);
}

#[test]
fn find_implementations_lists_a_javascript_class_that_extends_the_base() {
    let (_tree, engine) = build(&CONTROLLER);
    let (_, graph) = engine
        .graph_of("target.js")
        .expect("JavaScript claims the file");

    let implementations = graph
        .implementations("Target", 1, 10)
        .expect("the name is defined");

    let names: Vec<&str> = (implementations.found.iter())
        .map(|implementor| implementor.symbol.qualified_name)
        .collect();
    assert_eq!(names, ["Controller"]);
}
