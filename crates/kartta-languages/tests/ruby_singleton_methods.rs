//! What a Ruby call or use binds where one class declares an instance method and a class method of one name,
//! `Formula#build` and `Formula.build`: a constant receiver, `self.class` in an instance method, and `self` or no
//! receiver in a class method, name the class, so they reach its class method, while `self` or no receiver in an
//! instance method, a bare `new` in a class method and a value reach the instance method, in either declaration order;
//! a class that declares one kind offers it to every receiver.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
use tempfile::TempDir;

const INSTANCE: &str = "  def build\n    :instance\n  end\n";
const SINGLETON: &str = "  def self.build\n    :singleton\n  end\n";
const CALLERS: &str = "  def install\n    build\n  end\n\n  def reinstall\n    self.build()\n  end\n\n  \
                       def rebuild\n    self.class.build\n  end\n\n  \
                       def rebuild_again\n    self.class().build\n  end\n\n  \
                       def self.make\n    build()\n  end\n\n  \
                       def self.remake\n    self.build()\n  end\n\n  \
                       def self.fresh\n    new.build\n  end\nend\n\n\
                       class Installer\n  def run(formula)\n    Formula.build\n    Formula.method(:build)\n  end\n\n  \
                       def rerun(formula)\n    formula.build\n  end\nend\n";

fn build(kinds: &[&str]) -> (TempDir, Engine) {
    let source = format!("class Formula\n{}\n{CALLERS}", kinds.join("\n"));
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("formula.rb"), source).expect("the file is written");
    let languages = kartta_languages::select(["ruby"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// The symbols that call or use `name`, each with whether its site is a call and the site's confidence.
fn sites<'e>(engine: &'e Engine, name: &str) -> Vec<(&'e str, bool, u8)> {
    let (_, graph) = engine.graph_of("formula.rb").expect("Ruby claims the file");
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name is defined");
    references
        .sites
        .iter()
        .map(|site| (site.site_of.qualified_name, site.call, site.confidence))
        .collect()
}

#[test]
fn each_site_reaches_the_method_of_the_kind_its_receiver_names() {
    for kinds in [[INSTANCE, SINGLETON], [SINGLETON, INSTANCE]] {
        let (_tree, engine) = build(&kinds);

        assert_eq!(
            (
                sites(&engine, "Formula.build"),
                sites(&engine, "Formula#build")
            ),
            (
                vec![
                    ("Formula#rebuild", true, 80),
                    ("Formula#rebuild_again", true, 80),
                    ("Formula.make", true, 80),
                    ("Formula.remake", true, 85),
                    ("Installer#run", true, 85),
                    ("Installer#run", false, 85),
                ],
                vec![
                    ("Formula#install", true, 80),
                    ("Formula#reinstall", true, 85),
                    ("Formula.fresh", true, 85),
                    ("Installer#rerun", true, 80),
                ]
            ),
            "instance first: {}",
            kinds[0] == INSTANCE
        );
    }
}

#[test]
fn a_class_that_declares_one_kind_offers_it_to_every_receiver_but_a_bare_call_of_the_other_kind() {
    let (_tree, engine) = build(&[INSTANCE]);
    assert_eq!(
        sites(&engine, "Formula#build"),
        [
            ("Formula#install", true, 80),
            ("Formula#reinstall", true, 85),
            ("Formula#rebuild", true, 80),
            ("Formula#rebuild_again", true, 80),
            ("Formula.remake", true, 85),
            ("Formula.fresh", true, 85),
            ("Installer#run", true, 85),
            ("Installer#run", false, 85),
            ("Installer#rerun", true, 80),
        ]
    );

    let (_tree, engine) = build(&[SINGLETON]);
    assert_eq!(
        sites(&engine, "Formula.build"),
        [
            ("Formula#reinstall", true, 85),
            ("Formula#rebuild", true, 80),
            ("Formula#rebuild_again", true, 80),
            ("Formula.make", true, 80),
            ("Formula.remake", true, 85),
            ("Formula.fresh", true, 85),
            ("Installer#run", true, 85),
            ("Installer#run", false, 85),
            ("Installer#rerun", true, 80),
        ]
    );
}
