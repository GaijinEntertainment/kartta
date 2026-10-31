//! What a Bash command binds: a function or an alias of its name, and never a variable, which a command name cannot
//! name.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SiteQuery, SymbolKind};
use tempfile::TempDir;

const SOURCE: &str = r#"basename="s,^.*/,,"
shared=1

shared() {
  :
}

alias listing='ls -l'
listing=1

run() {
  name=`basename "$1"`
  shared
  listing
}

basename "$0"
run "$@"
"#;

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("tool.sh"), SOURCE).expect("the file is written");
    let languages = kartta_languages::select(["bash"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// The kind and line of the definition the name selects, and each call of it by holder and line.
fn calls<'e>(engine: &'e Engine, name: &str) -> (SymbolKind, u32, Vec<(&'e str, u32)>) {
    let (_, graph) = engine.graph_of("tool.sh").expect("Bash claims the file");
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name is defined");
    let calls = references
        .sites
        .iter()
        .filter(|site| site.call)
        .map(|site| (site.site_of.qualified_name, site.line))
        .collect();
    (references.symbol.kind, references.symbol.line, calls)
}

fn file_scope_callers(engine: &Engine, name: &str) -> usize {
    let (_, graph) = engine.graph_of("tool.sh").expect("Bash claims the file");
    graph
        .callers(name, Limits::new(1))
        .expect("the name selects a symbol")
        .file_scope
        .len()
}

#[test]
fn a_command_binds_no_variable_of_its_name() {
    let (_tree, engine) = build();

    assert_eq!(
        calls(&engine, "basename"),
        (SymbolKind::Variable, 1, vec![])
    );
}

#[test]
fn a_command_at_file_scope_names_no_variable_of_its_name() {
    let (_tree, engine) = build();

    assert_eq!(file_scope_callers(&engine, "basename"), 0);
}

#[test]
fn a_command_binds_the_function_beside_a_variable_of_its_name() {
    let (_tree, engine) = build();

    assert_eq!(
        calls(&engine, "shared"),
        (SymbolKind::Function, 4, vec![("run", 13)])
    );
}

#[test]
fn a_command_binds_the_alias_beside_a_variable_of_its_name() {
    let (_tree, engine) = build();

    assert_eq!(
        calls(&engine, "listing"),
        (SymbolKind::Function, 8, vec![("run", 14)])
    );
}
