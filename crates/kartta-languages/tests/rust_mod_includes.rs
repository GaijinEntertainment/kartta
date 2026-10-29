//! What a Rust method call on a value whose type the graph does not follow reaches through a file its caller declares
//! with `mod`: an exported member of the module's file, and a member out of the caller's reach only where the receiver
//! spells its type, while a namesake it turns away leaves the call unbound.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Selector, confidence};
use tempfile::TempDir;

const PREPARE: &str = "mod child;
mod other;

fn build<T>(make: impl FnOnce(&mut T)) {}

pub fn load() {
    build(|pool| {
        pool.prec(1);
    });
}

pub fn render() {
    build(|step| step.draw());
    build(|shape| shape.draw());
}

pub fn drain() {
    build(|pending| pending.push_stack());
}

pub fn inspect() {
    build(|task| task.header());
}

pub fn count() {
    build(|items| items.len());
}
";

const CHILD: &str = "use crate::rules::Draw;

pub struct Step;

pub struct Pool;

impl Pool {
    fn header(&self) {}
}

pub struct Queue;

impl Queue {
    pub fn push_stack(&mut self) {}

    pub fn len(&self) -> usize {
        0
    }
}

impl Draw for Step {
    fn draw(&self) {}
}

#[cfg(test)]
mod tests {
    use super::Step;

    impl Step {
        fn prec(self) -> Self {
            self
        }
    }
}
";

const OTHER: &str = "pub struct Raw;

impl Raw {
    pub fn header(&self) {}
}
";

const RULES: &str = "pub struct RulePool;

impl RulePool {
    pub fn prec(&mut self, _: u8) {}
}

pub trait Draw {
    fn draw(&self);
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"generate\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub mod prepare;\npub mod rules;\n"),
        ("src/prepare.rs", PREPARE),
        ("src/prepare/child.rs", CHILD),
        ("src/prepare/other.rs", OTHER),
        ("src/rules.rs", RULES),
    ];
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["rust"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callers(engine: &Engine, name: &str, path: &str) -> Vec<(String, u32, u8)> {
    let (_, graph) = engine
        .graph_of("src/prepare.rs")
        .expect("Rust claims the file");
    let walk = graph
        .callers(Selector::new(name, Some(path)), Limits::new(1))
        .expect("the name is defined");
    (walk.reached.iter())
        .map(|reached| {
            (
                reached.symbol.qualified_name.to_owned(),
                reached.line,
                reached.confidence,
            )
        })
        .collect()
}

#[test]
fn a_method_call_on_a_value_of_another_type_binds_no_member_of_a_mod_file_s_test_module() {
    let (_tree, engine) = build();

    assert_eq!(callers(&engine, "Step::prec", "src/prepare/child.rs"), []);
}

#[test]
fn a_method_call_binds_a_trait_impl_s_member_of_a_mod_file_only_where_the_receiver_spells_its_type()
{
    let (_tree, engine) = build();

    assert_eq!(
        callers(&engine, "Step::draw", "src/prepare/child.rs"),
        [("render".to_owned(), 13, confidence::INCLUDE)]
    );
}

#[test]
fn a_method_call_on_a_value_binds_an_exported_member_of_a_mod_file() {
    let (_tree, engine) = build();

    assert_eq!(
        callers(&engine, "Queue::push_stack", "src/prepare/child.rs"),
        [("drain".to_owned(), 18, confidence::INCLUDE)]
    );
}

#[test]
fn a_private_namesake_in_one_mod_file_leaves_a_method_call_unbound() {
    let (_tree, engine) = build();

    assert_eq!(callers(&engine, "Raw::header", "src/prepare/other.rs"), []);
    assert_eq!(callers(&engine, "Pool::header", "src/prepare/child.rs"), []);
}

#[test]
fn a_runtime_method_s_call_on_a_value_binds_no_exported_member_of_a_mod_file_its_receiver_does_not_spell()
 {
    let (_tree, engine) = build();

    assert_eq!(callers(&engine, "Queue::len", "src/prepare/child.rs"), []);
}
