//! What a Ruby call on a constant binds where a scoped write, `A::B = Foo.new`, defines that constant.
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

/// The symbols `Runner#go` in `run.rb` calls, each with the confidence of its call.
fn tiers(engine: &Engine) -> Vec<(&str, u8)> {
    let (_, graph) = engine.graph_of("run.rb").expect("Ruby claims the file");
    let callees = graph
        .callees("Runner#go", Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .map(|reached| (reached.symbol.qualified_name, reached.confidence))
        .collect()
}

#[test]
fn a_call_on_a_constant_a_scoped_write_defines_binds_a_method_of_the_class_it_holds() {
    let (_tree, engine) = build(&[
        ("foo.rb", "class Foo\n  def bar\n  end\nend\n"),
        ("set.rb", "module A\nend\nA::B = Foo.new\n"),
        (
            "run.rb",
            "class Runner\n  def go\n    A::B.bar\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "run.rb", "Runner#go"),
        (vec!["Foo#bar"], 0)
    );
}

#[test]
fn a_call_on_a_constant_a_scoped_write_defines_binds_no_namesake_of_another_class() {
    let (_tree, engine) = build(&[
        ("foo.rb", "class Foo\n  def bar\n  end\nend\n"),
        ("other.rb", "class Other\n  def bar\n  end\nend\n"),
        ("set.rb", "module A\nend\nA::B = Foo.new\n"),
        (
            "run.rb",
            "class Runner\n  def go\n    A::B.bar\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "run.rb", "Runner#go"),
        (vec!["Foo#bar"], 0)
    );
}

#[test]
fn a_constant_that_a_scoped_write_makes_an_alias_binds_the_class_method_of_the_class() {
    let (_tree, engine) = build(&[
        (
            "mac.rb",
            "module OS\n  module Mac\n    ::MacOS = OS::Mac\n\n    def self.version\n    end\n\n    def version\n    \
             end\n  end\nend\n",
        ),
        (
            "other.rb",
            "class Formula\n  def self.version\n  end\nend\n",
        ),
        (
            "run.rb",
            "class Runner\n  def go\n    MacOS.version\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "run.rb", "Runner#go"),
        (vec!["OS::Mac.version"], 0)
    );
}

#[test]
fn a_class_of_the_spelling_answers_ahead_of_a_scoped_write_that_aliases_another() {
    let (_tree, engine) = build(&[
        (
            "gem.rb",
            "module Gem\nend\n\nmodule Gem::URI\n  def self.parse\n  end\nend\n",
        ),
        (
            "fallback.rb",
            "module Bundler::URI\n  def self.parse\n  end\nend\n\nGem::URI = Bundler::URI\n",
        ),
        (
            "run.rb",
            "class Runner\n  def go\n    Gem::URI.parse\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "run.rb", "Runner#go"),
        (vec!["Gem::URI.parse"], 0)
    );
}

#[test]
fn a_constant_holding_a_class_outside_the_graph_binds_nothing() {
    let (_tree, engine) = build(&[
        ("other.rb", "class Other\n  def fetch\n  end\nend\n"),
        (
            "set.rb",
            "module A\nend\nA::B = Hash.new\nA::C = DEBUGGER__::Session\n",
        ),
        (
            "run.rb",
            "class Runner\n  def go\n    A::B.fetch\n    A::C.fetch\n  end\nend\n",
        ),
    ]);

    assert_eq!(callees(&engine, "run.rb", "Runner#go"), (vec![], 2));
}

#[test]
fn a_call_on_a_scoped_constant_of_no_named_class_binds_only_as_a_guess() {
    let (_tree, engine) = build(&[
        ("other.rb", "class Other\n  def fetch\n  end\nend\n"),
        ("set.rb", "module A\nend\nA::B = compute\n"),
        (
            "run.rb",
            "class Runner\n  def go\n    A::B.fetch\n  end\nend\n",
        ),
    ]);

    assert_eq!(tiers(&engine), [("Other#fetch", 15)]);
}

#[test]
fn a_call_on_a_scoped_constant_two_writes_give_two_classes_binds_only_as_a_guess() {
    let (_tree, engine) = build(&[
        ("foo.rb", "class Foo\n  def fetch\n  end\nend\n"),
        ("bar.rb", "class Bar\nend\n"),
        (
            "set.rb",
            "module A\nend\nif x\n  A::B = Foo.new\nelse\n  A::B = Bar.new\nend\n",
        ),
        (
            "run.rb",
            "class Runner\n  def go\n    A::B.fetch\n  end\nend\n",
        ),
    ]);

    assert_eq!(tiers(&engine), [("Foo#fetch", 15)]);
}

#[test]
fn a_scoped_constant_answers_only_a_receiver_that_spells_it() {
    let (_tree, engine) = build(&[
        ("other.rb", "class Other\n  def dup\n  end\nend\n"),
        ("set.rb", "module Irb\nend\nIrb::BINDING = binding\n"),
        (
            "run.rb",
            "class Runner\n  def go\n    BINDING.dup\n  end\nend\n",
        ),
    ]);

    assert_eq!(callees(&engine, "run.rb", "Runner#go"), (vec![], 1));
}
