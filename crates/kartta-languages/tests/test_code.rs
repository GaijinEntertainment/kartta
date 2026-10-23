//! What a call from outside test code binds of a name test code defines: never the test definition, so the call binds
//! a definition outside test code or none, and counts as unresolved, while a call from test code still binds it.
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
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["python"]).expect("python is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> (Vec<&'e str>, usize) {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    let files = (callees.reached.iter())
        .map(|reached| reached.symbol.file)
        .collect();
    (files, callees.unresolved)
}

#[test]
fn a_name_only_test_code_defines_binds_no_call_from_outside_test_code() {
    let (_tree, engine) = build(&[
        ("app/main.py", "def run():\n    helper()\n"),
        ("tests/helpers.py", "def helper():\n    pass\n"),
        ("tests/test_main.py", "def test_run():\n    helper()\n"),
    ]);

    assert_eq!(callees(&engine, "app/main.py", "run"), (vec![], 1));
    assert_eq!(
        callees(&engine, "tests/test_main.py", "test_run"),
        (vec!["tests/helpers.py"], 0)
    );
}
