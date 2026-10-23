//! What a Quirrel member call binds of a method a module table declares: the table is one value, which a call reaches
//! through the table's name or from the table's own members, so a call on another value binds the namesake method of
//! the class it holds.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const MANAGER: &str = "let _scores = {
  function getMainRequest(event) {
    return event
  }
  function refresh(event) {
    return this.getMainRequest(event)
  }
}

let Feed = class {
  function getMainRequest(event) {
    return _scores.getMainRequest(event)
  }
}

let feed = Feed()

return {
  feed
  _scores
}
";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["quirrel"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> Vec<&'e str> {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    graph
        .callees(name, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| reached.symbol.qualified_name)
        .collect()
}

#[test]
fn a_member_call_on_a_value_binds_the_class_method_past_a_module_table_s_namesake() {
    let (_tree, engine) = build(&[
        ("feed/feedManager.nut", MANAGER),
        (
            "feed/feedView.nut",
            "from \"feedManager.nut\" import feed\n\
             function fetchData(event) {\n  return feed.getMainRequest(event)\n}\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "feed/feedView.nut", "fetchData"),
        ["Feed.getMainRequest"]
    );
}

#[test]
fn a_member_call_reaches_a_module_table_s_method_through_the_table_s_name_or_from_its_members() {
    let (_tree, engine) = build(&[
        ("feed/feedManager.nut", MANAGER),
        (
            "feed/feedView.nut",
            "from \"feedManager.nut\" import _scores\n\
             function fetchData(event) {\n  return _scores.getMainRequest(event)\n}\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "feed/feedView.nut", "fetchData"),
        ["_scores.getMainRequest"],
        "an importer writes the table's name"
    );
    assert_eq!(
        callees(&engine, "feed/feedManager.nut", "Feed.getMainRequest"),
        ["_scores.getMainRequest"],
        "so does the table's own file"
    );
    assert_eq!(
        callees(&engine, "feed/feedManager.nut", "_scores.refresh"),
        ["_scores.getMainRequest"],
        "and a member of the table reaches its sibling through `this`"
    );
}
