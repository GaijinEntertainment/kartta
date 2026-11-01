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
fn a_call_with_a_capital_name_binds_the_method_and_no_constant_of_the_name() {
    let (_tree, engine) = build(&[
        (
            "gem.rb",
            "module Gem\n  URI = compute\n\n  def URI(uri)\n    uri\n  end\nend\n",
        ),
        (
            "run.rb",
            "class Runner\n  def go\n    Gem::URI(1)\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "run.rb", "Runner#go"),
        (vec!["Gem#URI"], 0)
    );
}

#[test]
fn a_block_that_eval_runs_on_a_constant_binds_the_members_of_that_class() {
    let (_tree, engine) = build(&[(
        "shape.rb",
        "class Other\n  def getopts\n  end\n\n  def self.getopts\n  end\nend\n\nclass Shape\n  def getopts\n  end\n\n  \
         def self.getopts\n  end\n\n  def self.parse\n    Other.class_eval { new.getopts }\n  end\n\n  \
         def self.look\n    Other.instance_eval { getopts }\n  end\n\n  def self.keep\n    Other.tap { getopts }\n  \
         end\nend\n",
    )]);

    assert_eq!(
        callees(&engine, "shape.rb", "Shape.parse").0,
        ["Other#getopts"]
    );
    assert_eq!(
        callees(&engine, "shape.rb", "Shape.look").0,
        ["Other.getopts"]
    );
    assert_eq!(
        callees(&engine, "shape.rb", "Shape.keep").0,
        ["Shape.getopts"]
    );
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
