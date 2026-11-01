//! What a TypeScript class expression's body makes outside its members: the calls and the reads of a field's
//! initializer are made by the name that binds the class, as a declared class makes those of its own body.

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SiteQuery};

const SOURCE: &str = "function make() { return 1; }
export class Named {
  field = make();
}
export const Box = class {
  field = make();
  size = limit;
};
const limit = 2;
";

#[test]
fn a_call_and_a_read_in_a_class_expression_s_body_are_made_by_the_name_that_binds_it() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("a.ts"), SOURCE).expect("the file is written");
    let languages = kartta_languages::select(["typescript"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of("a.ts")
        .expect("TypeScript claims the file");

    let callers = graph
        .callers("make", Limits::new(1))
        .expect("the name is defined");
    let reached: Vec<(&str, u32)> = (callers.reached.iter())
        .map(|reached| (reached.symbol.name, reached.line))
        .collect();
    let file_scope: Vec<&str> = callers.file_scope.iter().map(|scope| scope.file).collect();
    assert_eq!(reached, [("Named", 3), ("Box", 6)]);
    assert_eq!(file_scope, Vec::<&str>::new());

    let references = graph
        .references("limit", SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    let sites: Vec<(&str, u32)> = (references.sites.iter())
        .map(|site| (site.site_of.name, site.line))
        .collect();
    assert_eq!(sites, [("Box", 7)]);
}

const BOUND_WITHOUT_A_DECLARATOR: &str = "function make() { return 1; }
const ns: any = {};
ns.Box = class {
  value = make();
  size = limit;
};
const table = {
  K: class {
    value = make();
  },
};
const limit = 2;
";

#[test]
fn a_class_expression_an_assignment_or_a_property_binds_makes_the_calls_and_reads_of_its_body() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("a.ts"), BOUND_WITHOUT_A_DECLARATOR).expect("the file is written");
    let languages = kartta_languages::select(["typescript"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of("a.ts")
        .expect("TypeScript claims the file");

    let callers = graph
        .callers("make", Limits::new(1))
        .expect("the name is defined");
    let reached: Vec<(&str, u32)> = (callers.reached.iter())
        .map(|reached| (reached.symbol.name, reached.line))
        .collect();
    let file_scope: Vec<&str> = callers.file_scope.iter().map(|scope| scope.file).collect();
    assert_eq!(reached, [("Box", 4), ("K", 9)]);
    assert_eq!(file_scope, Vec::<&str>::new());

    let references = graph
        .references("limit", SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    let sites: Vec<(&str, u32)> = (references.sites.iter())
        .map(|site| (site.site_of.name, site.line))
        .collect();
    assert_eq!(sites, [("Box", 5)]);
}
