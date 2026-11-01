//! What a TypeScript test registration's name reaches: a description or a hook's name is a label no code spells, so
//! a call or a use of that name inside the registration binds the definition its file imports, and the registration
//! keeps owning the call.

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SiteQuery};

const RECORD: &str = "export function labelOf(record: string): string {
  return record;
}
";

const RECORD_TEST: &str = "import { beforeEach, describe, expect, test } from \"vitest\";
import { labelOf } from \"./labelled.ts\";

describe(\"labelOf\", () => {
  beforeEach(() => {});
  test(\"gives the label of a record\", () => {
    expect(labelOf(\"a\")).toBe(\"a\");
    expect([\"b\"].map(labelOf)).toStrictEqual([\"b\"]);
  });
  labelOf(\"c\");
});
";

#[test]
fn a_call_of_a_registration_s_name_inside_it_binds_the_imported_definition() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("labelled.ts"), RECORD).expect("the file is written");
    fs::write(tree.path().join("labelled.test.ts"), RECORD_TEST).expect("the file is written");
    let languages = kartta_languages::select(["typescript"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of("labelled.ts")
        .expect("TypeScript claims the file");

    let callers = graph
        .callers("labelOf", Limits::new(1))
        .expect("the name is defined");
    let reached: Vec<(&str, u32)> = (callers.reached.iter())
        .map(|reached| (reached.symbol.name, reached.line))
        .collect();
    assert_eq!((callers.root.file, callers.root.line), ("labelled.ts", 1));
    assert_eq!(
        reached,
        [("labelOf", 10), ("gives the label of a record", 7)]
    );
    assert_eq!(callers.self_call, None);

    let references = graph
        .references("labelOf", SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    let sites: Vec<(&str, u32)> = (references.sites.iter())
        .map(|site| (site.site_of.name, site.line))
        .collect();
    assert_eq!(
        (references.symbol.file, references.symbol.line),
        ("labelled.ts", 1)
    );
    assert_eq!(
        sites,
        [
            ("gives the label of a record", 7),
            ("gives the label of a record", 8),
            ("labelOf", 10)
        ]
    );

    let hook = graph
        .callers("beforeEach", Limits::new(1))
        .expect("the hook is defined");
    assert_eq!(hook.reached, [], "no call binds a hook by its name");
}
