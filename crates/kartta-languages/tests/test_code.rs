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

fn build(language: &str, files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select([language]).expect("the language is known");
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
    let (_tree, engine) = build(
        "python",
        &[
            ("app/main.py", "def run():\n    helper()\n"),
            ("tests/helpers.py", "def helper():\n    pass\n"),
            ("tests/test_main.py", "def test_run():\n    helper()\n"),
        ],
    );

    assert_eq!(callees(&engine, "app/main.py", "run"), (vec![], 1));
    assert_eq!(
        callees(&engine, "tests/test_main.py", "test_run"),
        (vec!["tests/helpers.py"], 0)
    );
}

#[test]
fn a_name_test_code_shares_binds_no_test_definition_nearest_a_call_from_outside_test_code() {
    let (_tree, engine) = build(
        "python",
        &[
            ("app/main.py", "def run():\n    helper()\n"),
            (
                "app/test_main.py",
                "def helper():\n    pass\n\n\ndef test_run():\n    helper()\n",
            ),
            ("lib/util.py", "def helper():\n    pass\n"),
            ("lib/test_util.py", "def test_helper():\n    helper()\n"),
        ],
    );

    assert_eq!(callees(&engine, "app/main.py", "run"), (vec![], 1));
    assert_eq!(
        callees(&engine, "app/test_main.py", "test_run"),
        (vec!["app/test_main.py"], 0)
    );
    assert_eq!(
        callees(&engine, "lib/test_util.py", "test_helper"),
        (vec!["lib/util.py"], 0)
    );
}

#[test]
fn a_stub_of_the_quirrel_compiler_s_test_data_binds_no_call_from_outside_test_code() {
    let (_tree, engine) = build(
        "quirrel",
        &[
            ("ui/menu.nut", "function show() {\n  Computed(1)\n}\n"),
            (
                "quirrel/testData/static_analyzer/w259.nut",
                "function Computed(_p) {}\n\nfunction check() {\n  Computed(1)\n}\n",
            ),
        ],
    );

    assert_eq!(callees(&engine, "ui/menu.nut", "show"), (vec![], 1));
    assert_eq!(
        callees(
            &engine,
            "quirrel/testData/static_analyzer/w259.nut",
            "check"
        ),
        (vec!["quirrel/testData/static_analyzer/w259.nut"], 0)
    );
}
