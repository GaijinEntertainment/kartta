//! What a Ruby call on `self.class` binds in an instance method of a class: a class method of that class or of a class
//! above it, or a method of a module, ahead of a namesake of another class, and never an instance method of a class.
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
    let languages = kartta_languages::select(["ruby"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// The qualified names of the symbols `name` calls, each with its confidence.
fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> Vec<(&'e str, u8)> {
    let (_, graph) = engine.graph_of(path).expect("Ruby claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .map(|reached| (reached.symbol.qualified_name, reached.confidence))
        .collect()
}

#[test]
fn self_class_binds_the_class_method_of_the_owner_and_no_namesake_of_another_class() {
    let (_tree, engine) = build(&[(
        "formula.rb",
        "class Formula\n  def self.build\n  end\n\n  def rebuild\n    self.class.build\n  end\nend\n\n\
         class Cask\n  def self.build\n  end\nend\n",
    )]);

    assert_eq!(
        callees(&engine, "formula.rb", "Formula#rebuild"),
        [("Formula.build", 85)]
    );
}

#[test]
fn self_class_binds_a_class_method_the_owner_inherits_and_no_namesake_of_another_class() {
    let (_tree, engine) = build(&[
        ("a_cask.rb", "class Cask\n  def self.build\n  end\nend\n"),
        ("base.rb", "class Base\n  def self.build\n  end\nend\n"),
        (
            "formula.rb",
            "class Formula < Base\n  def rebuild\n    self.class.build\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "formula.rb", "Formula#rebuild"),
        [("Base.build", 85)]
    );
}

#[test]
fn self_class_binds_a_class_method_above_an_instance_method_of_the_name() {
    let (_tree, engine) = build(&[
        ("base.rb", "class Base\n  def self.build\n  end\nend\n"),
        (
            "formula.rb",
            "class Formula < Base\n  def build\n    self.class.build\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "formula.rb", "Formula#build"),
        [("Base.build", 85)]
    );
}

#[test]
fn self_class_binds_no_instance_method_of_the_owner() {
    let (_tree, engine) = build(&[(
        "keg.rb",
        "class Keg\n  def name\n  end\n\n  def inspect\n    self.class.name\n  end\nend\n",
    )]);

    assert_eq!(callees(&engine, "keg.rb", "Keg#inspect"), []);
}

#[test]
fn self_class_binds_no_class_whose_chain_ends_in_the_owner_s_name() {
    let (_tree, engine) = build(&[
        (
            "gem.rb",
            "module Gem\n  class Version\n    def self.create\n    end\n  end\nend\n",
        ),
        (
            "version.rb",
            "class Version\n  def bump\n    self.class.create\n  end\nend\n",
        ),
    ]);

    assert_eq!(callees(&engine, "version.rb", "Version#bump"), []);
}

#[test]
fn self_class_keeps_a_method_of_a_module_the_owner_may_extend() {
    let (_tree, engine) = build(&[
        (
            "props.rb",
            "module Props\n  module ClassMethods\n    def decorator\n    end\n  end\nend\n",
        ),
        (
            "struct.rb",
            "class Struct2\n  def serialize\n    self.class.decorator\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "struct.rb", "Struct2#serialize"),
        [("Props::ClassMethods#decorator", 60)]
    );
}

#[test]
fn self_class_binds_no_class_method_of_an_unrelated_class() {
    let (_tree, engine) = build(&[
        ("team.rb", "class Team\n  def self.create\n  end\nend\n"),
        (
            "cop.rb",
            "class Cop\n  def ready\n    self.class.create\n  end\nend\n",
        ),
    ]);

    assert_eq!(callees(&engine, "cop.rb", "Cop#ready"), []);
}
