//! What a Ruby method named by a symbol binds: in a class body the class makes the use of its own method, with a
//! constant receiver the use names that class's method, in a method of a class it names a method of the type of `self`,
//! in a top-level method it binds the file's top-level method, and at file scope it binds no site yet keeps the method
//! it names out of the dead list.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
use tempfile::TempDir;

const SOURCE: &str = "def setup
end

def teardown
end

def main
  send(:setup)
end

send(:teardown)

class Runner
  send(:prepare)

  def prepare
  end

  def self.start
  end
end

class Launcher
  def go
    Runner.method(:start)
  end
end
";

const IN_METHODS: &str = "class Job
  def setup; end

  def run
    send(:setup)
    self.send(:setup)
    method(:setup)
  end

  def self.prepare; end
  def self.setup; end

  def self.boot
    send(:prepare)
    method(:setup)
  end
end

class Nightly < Job
  def go
    send(:setup)
  end
end
";

fn build() -> (TempDir, Engine) {
    build_from(SOURCE)
}

fn build_from(source: &str) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("tasks.rb"), source).expect("the file is written");
    let languages = kartta_languages::select(["ruby"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// Each site's holder, line, whether it is a call, and confidence.
fn sites<'e>(engine: &'e Engine, name: &str) -> Vec<(&'e str, u32, bool, u8)> {
    let (_, graph) = engine.graph_of("tasks.rb").expect("Ruby claims the file");
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name is defined");
    references
        .sites
        .iter()
        .map(|site| {
            (
                site.site_of.qualified_name,
                site.line,
                site.call,
                site.confidence,
            )
        })
        .collect()
}

#[test]
fn a_symbol_in_a_class_body_binds_the_class_s_own_method() {
    let (_tree, engine) = build();

    assert_eq!(
        sites(&engine, "Runner#prepare"),
        [("Runner", 14, false, 85)]
    );
}

#[test]
fn a_symbol_on_a_constant_receiver_binds_that_class_s_method() {
    let (_tree, engine) = build();

    assert_eq!(
        sites(&engine, "Runner.start"),
        [("Launcher#go", 25, false, 85)]
    );
}

#[test]
fn a_symbol_in_a_top_level_method_binds_the_file_s_top_level_method() {
    let (_tree, engine) = build();

    assert_eq!(sites(&engine, "setup"), [("main", 8, false, 80)]);
}

#[test]
fn a_symbol_in_an_instance_method_binds_the_method_of_the_object_s_own_type() {
    let (_tree, engine) = build_from(IN_METHODS);

    assert_eq!(
        sites(&engine, "Job#setup"),
        [
            ("Job#run", 5, false, 85),
            ("Job#run", 6, false, 85),
            ("Job#run", 7, false, 85),
            ("Nightly#go", 21, false, 80),
        ]
    );
}

#[test]
fn a_symbol_in_a_class_method_binds_the_class_s_method_first() {
    let (_tree, engine) = build_from(IN_METHODS);

    assert_eq!(sites(&engine, "Job.prepare"), [("Job.boot", 14, false, 85)]);
    assert_eq!(sites(&engine, "Job.setup"), [("Job.boot", 15, false, 85)]);
}

#[test]
fn a_method_a_symbol_names_inside_a_method_is_not_dead() {
    let (_tree, engine) = build_from(IN_METHODS);
    let (_, graph) = engine.graph_of("tasks.rb").expect("Ruby claims the file");

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(
        dead.found
            .iter()
            .map(|dead| dead.definition.qualified_name)
            .collect::<Vec<_>>(),
        ["Job#run", "Job.boot", "Nightly#go"],
        "only the methods nothing names are dead"
    );
}

#[test]
fn a_symbol_at_file_scope_binds_no_site_and_keeps_its_method_alive() {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of("tasks.rb").expect("Ruby claims the file");

    let dead = graph.dead_code(&[], None, 10);

    assert_eq!(sites(&engine, "teardown"), []);
    assert_eq!(
        dead.found
            .iter()
            .map(|dead| dead.definition.qualified_name)
            .collect::<Vec<_>>(),
        ["main", "Launcher#go"],
        "only the methods nothing names are dead"
    );
}
