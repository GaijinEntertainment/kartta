//! What an engine built from a tree on disk binds: a selection of part of the TypeScript family leaves the files of the
//! other languages out of the graph, no Rust call binds a module, and a Rust call into another crate of the workspace
//! binds what that crate exports.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SymbolKind};
use tempfile::TempDir;

fn tree_of(files: &[(&str, &str)]) -> TempDir {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    tree
}

fn tree() -> TempDir {
    tree_of(&[
        (
            "app.ts",
            "import { Button } from \"./Button\";\n\nexport function render() {\n  return Button();\n}\n",
        ),
        (
            "Button.tsx",
            "export function Button() {\n  return <button />;\n}\n",
        ),
    ])
}

fn build(tree: &TempDir, names: &[&str]) -> Engine {
    let languages = kartta_languages::select(names).expect("every name is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    outcome.engine
}

type Callee<'e> = (&'e str, SymbolKind, &'e str);

fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> (Vec<Callee<'e>>, usize) {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    let reached = (callees.reached.iter())
        .map(|reached| {
            let symbol = reached.symbol;
            (symbol.name, symbol.kind, symbol.file)
        })
        .collect();
    (reached, callees.unresolved)
}

#[test]
fn typescript_without_tsx_indexes_a_tsx_file_and_binds_no_call_into_it() {
    let tree = tree();
    let engine = build(&tree, &["typescript"]);

    assert!(engine.index().size("Button.tsx").is_some());
    assert!(engine.graph_of("Button.tsx").is_none());
    assert_eq!(callees(&engine, "app.ts", "render"), (vec![], 1));
}

#[test]
fn typescript_with_tsx_binds_the_call_into_the_tsx_file() {
    let tree = tree();
    let engine = build(&tree, &["typescript", "tsx"]);

    assert_eq!(
        callees(&engine, "app.ts", "render"),
        (vec![("Button", SymbolKind::Function, "Button.tsx")], 0)
    );
}

#[test]
fn a_rust_call_named_like_a_module_binds_no_call_to_the_module() {
    let tree = tree_of(&[
        ("lib.rs", "mod walk;\n"),
        (
            "walk.rs",
            "pub fn raw_walk(tree: &Tree) {\n    tree.walk();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(callees(&engine, "walk.rs", "raw_walk"), (vec![], 1));
}

#[test]
fn a_rust_call_through_another_workspace_crate_binds_the_function_that_crate_exports() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other-crate\"\n",
        ),
        ("crates/other/src/lib.rs", "pub mod m;\n"),
        ("crates/other/src/m.rs", "pub fn f() {}\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/main.rs",
            "mod helpers;\n\nuse other_crate::m::f;\n\nfn through_use() {\n    f();\n}\n\n\
             fn through_path() {\n    other_crate::m::f();\n}\n\nfn main() {}\n",
        ),
        ("crates/app/src/helpers.rs", "pub fn f() {}\n"),
    ]);
    let engine = build(&tree, &["rust"]);
    let exported = (
        vec![("f", SymbolKind::Function, "crates/other/src/m.rs")],
        0,
    );

    assert_eq!(
        callees(&engine, "crates/app/src/main.rs", "through_use"),
        exported
    );
    assert_eq!(
        callees(&engine, "crates/app/src/main.rs", "through_path"),
        exported
    );
}

#[test]
fn a_rust_call_binds_the_function_its_file_declares_beside_a_module_of_its_name() {
    let tree = tree_of(&[(
        "lib.rs",
        "mod text;\n\nfn text() {}\n\nfn draw() {\n    text();\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "lib.rs", "draw"),
        (vec![("text", SymbolKind::Function, "lib.rs")], 0)
    );
}
