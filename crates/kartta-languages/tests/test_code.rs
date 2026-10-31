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

const TEST_ASSEMBLER: (&str, &str) = (
    "lib/test_assembler.h",
    "namespace assembler {\nclass Section {\npublic:\n  int Size() { return 0; }\n};\n}\n",
);
const SYNTH_HEADER: (&str, &str) = (
    "lib/synth.h",
    "#include \"lib/test_assembler.h\"\nnamespace synth {\n\
     class Section : public assembler::Section {};\n\
     class Dump : public Section {\npublic:\n  int Finish();\n};\n}\n",
);
const SYNTH_SOURCE: (&str, &str) = (
    "lib/synth.cc",
    "#include \"lib/synth.h\"\nnamespace synth {\nint Dump::Finish() { return Size(); }\n}\n",
);

fn is_test_code(engine: &Engine, path: &str) -> bool {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let outline = graph.outline(path).expect("the graph holds the file");
    let is_test = outline[0].flags.is_test();
    assert!(
        outline
            .iter()
            .all(|symbol| symbol.flags.is_test() == is_test),
        "a file's definitions share its test flag: {path}"
    );
    is_test
}

#[test]
fn a_cpp_helper_only_tests_include_is_test_code_and_binds_the_test_definition_it_pastes_in() {
    let (_tree, engine) = build(
        "cpp",
        &[
            TEST_ASSEMBLER,
            SYNTH_HEADER,
            SYNTH_SOURCE,
            ("lib/dump_unittest.cc", "#include \"lib/synth.h\"\n"),
        ],
    );

    assert!(is_test_code(&engine, "lib/synth.h"));
    assert!(is_test_code(&engine, "lib/synth.cc"));
    assert_eq!(
        callees(&engine, "lib/synth.cc", "Dump::Finish"),
        (vec!["lib/test_assembler.h"], 0)
    );
}

#[test]
fn a_cpp_header_product_code_includes_stays_product_code_with_the_source_beside_it() {
    let (_tree, engine) = build(
        "cpp",
        &[
            TEST_ASSEMBLER,
            SYNTH_HEADER,
            SYNTH_SOURCE,
            ("lib/dump_unittest.cc", "#include \"lib/synth.h\"\n"),
            ("app/main.cc", "#include \"lib/synth.h\"\n"),
        ],
    );

    assert!(!is_test_code(&engine, "lib/synth.h"));
    assert!(!is_test_code(&engine, "lib/synth.cc"));
    assert_eq!(
        callees(&engine, "lib/synth.cc", "Dump::Finish"),
        (vec![], 1)
    );
}

#[test]
fn a_cpp_source_of_a_helper_header_stays_product_code_when_a_file_includes_it() {
    let (_tree, engine) = build(
        "cpp",
        &[
            TEST_ASSEMBLER,
            SYNTH_HEADER,
            SYNTH_SOURCE,
            ("lib/dump_unittest.cc", "#include \"lib/synth.h\"\n"),
            ("app/all.cc", "#include \"lib/synth.cc\"\n"),
        ],
    );

    assert!(!is_test_code(&engine, "lib/synth.cc"));
    assert!(!is_test_code(&engine, "lib/synth.h"));
}

#[test]
fn a_cpp_header_only_tests_include_stays_product_code_when_it_includes_no_test_file() {
    let (_tree, engine) = build(
        "cpp",
        &[
            ("lib/scanner.h", "void scan();\n"),
            (
                "lib/scanner.cc",
                "#include \"lib/scanner.h\"\nvoid scan() {}\n",
            ),
            (
                "lib/scanner_unittest.cc",
                "#include \"lib/scanner.h\"\nvoid check() { scan(); }\n",
            ),
        ],
    );

    assert!(!is_test_code(&engine, "lib/scanner.h"));
    assert!(!is_test_code(&engine, "lib/scanner.cc"));
}
