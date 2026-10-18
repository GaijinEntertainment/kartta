//! What a selection of part of the TypeScript family builds: the files of the languages left out stay out of the graph.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

fn tree() -> TempDir {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("app.ts"),
        "import { Button } from \"./Button\";\n\nexport function render() {\n  return Button();\n}\n",
    )
    .expect("app.ts is written");
    fs::write(
        tree.path().join("Button.tsx"),
        "export function Button() {\n  return <button />;\n}\n",
    )
    .expect("Button.tsx is written");
    tree
}

fn build(tree: &TempDir, names: &[&str]) -> Engine {
    let languages = kartta_languages::select(names).expect("every name is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    outcome.engine
}

fn callees_of_render(engine: &Engine) -> (Vec<(&str, &str)>, usize) {
    let graph = engine.graph_of("app.ts").expect("typescript claims app.ts");
    let callees = graph
        .callees("render", Limits::new(1))
        .expect("render is defined");
    let reached = (callees.reached.iter())
        .map(|reached| (reached.symbol.name, reached.symbol.file))
        .collect();
    (reached, callees.unresolved)
}

#[test]
fn typescript_without_tsx_indexes_a_tsx_file_and_binds_no_call_into_it() {
    let tree = tree();
    let engine = build(&tree, &["typescript"]);

    assert!(engine.index().size("Button.tsx").is_some());
    assert!(engine.graph_of("Button.tsx").is_none());
    assert_eq!(callees_of_render(&engine), (vec![], 1));
}

#[test]
fn typescript_with_tsx_binds_the_call_into_the_tsx_file() {
    let tree = tree();
    let engine = build(&tree, &["typescript", "tsx"]);

    assert_eq!(
        callees_of_render(&engine),
        (vec![("Button", "Button.tsx")], 0)
    );
}
