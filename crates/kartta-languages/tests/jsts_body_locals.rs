//! What a TypeScript variable or constant a function body declares binds: it is local to that body, so a read or a
//! call of the module's value of its name elsewhere in the file binds the module's value, and a call of its name in
//! the body binds the local.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SiteQuery, SymbolKind};
use tempfile::TempDir;

const SOURCE: &str = "export function first() {
  const limit = 1;
  return limit;
}

const limit = 2;

export function second() {
  return limit;
}

export function third() {
  const run = make();
  return run();
}

export function run() {
  return 0;
}

export function fourth() {
  return run();
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("a.ts"), SOURCE).expect("the file is written");
    let languages = kartta_languages::select(["typescript"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees(engine: &Engine, name: &str) -> Vec<(SymbolKind, u32)> {
    let (_, graph) = engine.graph_of("a.ts").expect("TypeScript claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .map(|reached| (reached.symbol.kind, reached.symbol.line))
        .collect()
}

#[test]
fn a_read_of_the_module_s_value_binds_it_and_not_a_body_s_local_of_its_name() {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of("a.ts").expect("TypeScript claims the file");

    let references = graph
        .references("limit", SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    assert_eq!(references.symbol.line, 6);
    let lines: Vec<u32> = references.sites.iter().map(|site| site.line).collect();
    assert_eq!(lines, [9]);
}

#[test]
fn a_call_binds_the_body_s_local_of_its_name_and_elsewhere_the_module_s_function() {
    let (_tree, engine) = build();

    assert_eq!(callees(&engine, "third"), [(SymbolKind::Constant, 13)]);
    assert_eq!(callees(&engine, "fourth"), [(SymbolKind::Function, 17)]);
}
