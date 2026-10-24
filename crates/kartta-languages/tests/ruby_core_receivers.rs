//! What a Ruby call on a constant or on a literal binds: a method of the class the constant names, and no namesake of
//! another class where the tree holds no class of that name or the receiver is a literal, whose value is of a class of
//! Ruby's runtime.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const OPTION_PARSER: &str = "class OptionParser\n  def new(x)\n    x\n  end\n\n  def freeze\n    self\n  end\n\n  \
                             def self.parse\n    1\n  end\nend\n";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        fs::write(tree.path().join(path), source).expect("the file is written");
    }
    let languages = kartta_languages::select(["ruby"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// The qualified names of the symbols `name` calls, and the count of its calls that bind nothing.
fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> (Vec<&'e str>, usize) {
    let (_, graph) = engine.graph_of(path).expect("Ruby claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    let names = (callees.reached.iter())
        .map(|reached| reached.symbol.qualified_name)
        .collect();
    (names, callees.unresolved)
}

#[test]
fn a_call_on_a_core_class_or_a_literal_binds_no_namesake_of_another_class() {
    let (_tree, engine) = build(&[
        ("opt.rb", OPTION_PARSER),
        (
            "run.rb",
            "class Runner\n  def go\n    h = Hash.new(0)\n    s = \"a\".freeze\n    h\n  end\nend\n",
        ),
    ]);

    assert_eq!(callees(&engine, "run.rb", "Runner#go"), (vec![], 2));
}

#[test]
fn a_call_on_a_constant_naming_a_class_of_the_tree_binds_its_method() {
    let (_tree, engine) = build(&[
        ("opt.rb", OPTION_PARSER),
        (
            "run.rb",
            "class Runner\n  def go\n    OptionParser.parse\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "run.rb", "Runner#go"),
        (vec!["OptionParser.parse"], 0)
    );
}
