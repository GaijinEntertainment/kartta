//! What a Bash handler named as a word binds: a `trap` inside a function binds the function it names from that
//! function, one at file scope binds no site yet keeps its handler out of the dead list, and a quoted handler that
//! holds more than one word is a command line and names nothing.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
use tempfile::TempDir;

const SOURCE: &str = r#"cleanup() {
  :
}

finish() {
  :
}

stop() {
  :
}

main() {
  trap cleanup EXIT
}

trap finish EXIT
trap 'stop now' HUP
main "$@"
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

/// Each site's holder, line, whether it is a call, and confidence.
fn sites<'e>(engine: &'e Engine, name: &str) -> Vec<(&'e str, u32, bool, u8)> {
    let (_, graph) = engine.graph_of("tool.sh").expect("Bash claims the file");
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

fn dead(engine: &Engine) -> Vec<&str> {
    let (_, graph) = engine.graph_of("tool.sh").expect("Bash claims the file");
    graph
        .dead_code(&[], None, 10)
        .found
        .iter()
        .map(|dead| dead.definition.qualified_name)
        .collect()
}

#[test]
fn a_trap_inside_a_function_binds_its_handler_from_that_function() {
    let (_tree, engine) = build();

    assert_eq!(sites(&engine, "cleanup"), [("main", 14, false, 80)]);
}

#[test]
fn a_trap_at_file_scope_binds_no_site_and_keeps_its_handler_alive() {
    let (_tree, engine) = build();

    assert_eq!(sites(&engine, "finish"), []);
    assert!(!dead(&engine).contains(&"finish"));
}

#[test]
fn a_quoted_handler_of_more_than_one_word_names_nothing() {
    let (_tree, engine) = build();

    assert_eq!(sites(&engine, "stop"), []);
    assert_eq!(dead(&engine), ["stop"]);
}
