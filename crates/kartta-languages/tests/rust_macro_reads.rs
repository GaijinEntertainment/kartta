//! What a name a Rust macro's arguments read reaches: a constant read only inside `assert_eq!` lists that read among
//! its references, as a read outside the macro does.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
use tempfile::TempDir;

const SOURCE: &str = "const LIMIT: u8 = 3;

fn check(x: u8) {
    assert_eq!(x, LIMIT);
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("lib.rs"), SOURCE).expect("the file is written");
    let languages = kartta_languages::select(["rust"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

#[test]
fn a_constant_read_only_in_a_macro_s_arguments_lists_that_read_among_its_references() {
    let (_tree, engine) = build();
    let (_, graph) = engine.graph_of("lib.rs").expect("Rust claims the file");
    let references = graph
        .references("LIMIT", SiteQuery::new(usize::MAX))
        .expect("the name is defined");
    let sites: Vec<(&str, u32, bool)> = references
        .sites
        .iter()
        .map(|site| (site.site_of.qualified_name, site.line, site.call))
        .collect();

    assert_eq!(sites, [("check", 4, false)]);
}
