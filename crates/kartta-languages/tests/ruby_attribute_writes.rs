//! What a Ruby attribute write binds: the setter of the name it spells, and the reader of that name only where the
//! write reads the attribute first.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const CONFIG: &str =
    "class Other\n  def self.env\n    1\n  end\n\n  def self.env=(value)\n    value\n  end\nend\n";

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

fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> Vec<&'e str> {
    let (_, graph) = engine.graph_of(path).expect("Ruby claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .map(|reached| reached.symbol.qualified_name)
        .collect()
}

#[test]
fn a_write_on_a_class_binds_its_class_setter_and_the_reader_gains_no_caller() {
    let (_tree, engine) = build(&[
        ("other.rb", CONFIG),
        (
            "run.rb",
            "class Runner\n  def go\n    Other.env = 1\n  end\nend\n",
        ),
    ]);

    assert_eq!(callees(&engine, "run.rb", "Runner#go"), ["Other.env="]);
}

#[test]
fn an_operator_write_binds_the_reader_and_the_setter() {
    let (_tree, engine) = build(&[
        ("other.rb", CONFIG),
        (
            "run.rb",
            "class Runner\n  def add\n    Other.env += 1\n  end\n\n  def fill\n    Other.env ||= 1\n  end\nend\n",
        ),
    ]);

    for caller in ["Runner#add", "Runner#fill"] {
        let mut bound = callees(&engine, "run.rb", caller);
        bound.sort_unstable();
        assert_eq!(bound, ["Other.env", "Other.env="]);
    }
}

#[test]
fn a_write_on_a_value_binds_the_setter_only_as_a_guess() {
    let (_tree, engine) = build(&[
        ("other.rb", CONFIG.replace("self.", "").as_str()),
        (
            "run.rb",
            "class Runner\n  def go(ui)\n    ui.env = 1\n  end\nend\n",
        ),
    ]);

    let (_, graph) = engine.graph_of("run.rb").expect("Ruby claims the file");
    let callees = graph
        .callees("Runner#go", Limits::new(1))
        .expect("the name is defined");
    let bound: Vec<_> = (callees.reached.iter())
        .map(|reached| (reached.symbol.qualified_name, reached.confidence))
        .collect();
    assert_eq!(bound, [("Other#env=", 15)]);
}

#[test]
fn a_write_on_self_binds_the_setter_an_accessor_macro_defines() {
    let (_tree, engine) = build(&[(
        "shape.rb",
        "class Shape\n  attr_accessor :size\n\n  def grow\n    self.size = 2\n  end\nend\n",
    )]);

    assert_eq!(callees(&engine, "shape.rb", "Shape#grow"), ["Shape#size="]);
}
