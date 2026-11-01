//! What a Quirrel call on a class the graph holds binds of the classes above it: `this` starts at the class around the
//! caller and a bare receiver at the class it names, and a member that class does not hold binds where the nearest
//! class above it holds one, since a class copies its base's members and then applies its own body over them.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

/// The qualified name and the confidence of each callee of `caller`, in a tree of `files`, `caller` in the first.
fn callees(files: &[(&str, &str)], caller: &str) -> Vec<(String, u8)> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        fs::write(tree.path().join(path), source).expect("the file is written");
    }
    let languages = kartta_languages::select(["quirrel"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of(files[0].0)
        .expect("Quirrel claims the file");
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

const CHAIN: &str = "class Root {
  function close() {}
  function reset() {}
}

class Middle(Root) {
  function reset() {}
}

class Leaf(Middle) {
  function run() {
    this.close()
    this.reset()
  }
  function own() {}
  function callsOwn() {
    this.own()
  }
  function deferred() {
    later(function() {
      this.close()
    })
  }
}

class Other {
  function close() {}
  function reset() {}
}

function outside() {
  Leaf.close()
}
";

fn pairs(names: &[&str], confidence: u8) -> Vec<(String, u8)> {
    names
        .iter()
        .map(|&name| (name.to_owned(), confidence))
        .collect()
}

#[test]
fn this_binds_the_member_the_nearest_class_above_the_caller_s_holds() {
    assert_eq!(
        callees(&[("chain.nut", CHAIN)], "Leaf.run"),
        pairs(&["Root.close", "Middle.reset"], 85),
    );
}

#[test]
fn a_member_of_the_class_itself_keeps_the_tier_of_the_caller_s_owner() {
    assert_eq!(
        callees(&[("chain.nut", CHAIN)], "Leaf.callsOwn"),
        pairs(&["Leaf.own"], 80),
    );
}

#[test]
fn a_function_value_in_a_member_reads_this_as_the_member_s_class() {
    assert_eq!(
        callees(&[("chain.nut", CHAIN)], "Leaf.deferred"),
        pairs(&["Root.close"], 85),
    );
}

#[test]
fn a_call_on_a_named_class_binds_the_member_its_nearest_parent_holds() {
    assert_eq!(
        callees(&[("chain.nut", CHAIN)], "outside"),
        pairs(&["Root.close"], 85),
    );
}

#[test]
fn a_class_above_the_caller_s_binds_across_a_require() {
    assert_eq!(
        callees(
            &[
                (
                    "leaf.nut",
                    "let { Handler } = require(\"handler.nut\")\n\
                     class Leaf(Handler) {\n  function run() {\n    this.goBack()\n  }\n}\n",
                ),
                (
                    "handler.nut",
                    "class Handler {\n  function goBack() {}\n}\nreturn { Handler }\n",
                ),
                ("other.nut", "class Other {\n  function goBack() {}\n}\n"),
            ],
            "Leaf.run",
        ),
        pairs(&["Handler.goBack"], 85),
    );
}

#[test]
fn a_value_s_name_keeps_the_caller_s_owner() {
    assert_eq!(
        callees(
            &[
                (
                    "pool.nut",
                    "class Pool {\n  function run(other) {\n    other.drain()\n  }\n}\n",
                ),
                ("model.nut", "class Pool {\n  function drain() {}\n}\n"),
            ],
            "Pool.run",
        ),
        pairs(&["Pool.drain"], 80),
    );
}

const FIELD_OVER_A_METHOD: &str = "class Base {
  function hide() {}
  function other() {}
}
class Child (Base) {
  hide = null
  constructor(fn) {
    this.hide = fn
  }
  function run() {
    this.hide()
    this.other()
  }
}
";

#[test]
fn a_field_of_the_class_hides_its_parent_s_method() {
    assert_eq!(
        callees(&[("field.nut", FIELD_OVER_A_METHOD)], "Child.run"),
        pairs(&["Base.other"], 85),
    );
}

#[test]
fn a_static_field_of_the_class_hides_its_parent_s_method() {
    assert_eq!(
        callees(
            &[(
                "field.nut",
                "class Base {\n  function hide() {}\n}\n\
                 class Child(Base) {\n  static hide = 1\n  function run() {\n    this.hide()\n  }\n}\n",
            )],
            "Child.run",
        ),
        [],
    );
}

#[test]
fn a_field_of_a_class_between_hides_the_method_above_it() {
    assert_eq!(
        callees(
            &[(
                "field.nut",
                "class Root {\n  function hide() {}\n}\nclass Middle(Root) {\n  hide = null\n}\n\
                 class Leaf(Middle) {\n  function run() {\n    this.hide()\n  }\n}\n",
            )],
            "Leaf.run",
        ),
        [],
    );
}

#[test]
fn a_base_call_stops_at_the_field_of_the_base() {
    assert_eq!(
        callees(
            &[(
                "field.nut",
                "class Root {\n  function hide() {}\n}\nclass Middle(Root) {\n  hide = null\n}\n\
                 class Leaf(Middle) {\n  function run() {\n    base.hide()\n  }\n}\n",
            )],
            "Leaf.run",
        ),
        [],
    );
}

#[test]
fn a_write_outside_the_class_body_leaves_the_method_in_its_slot() {
    assert_eq!(
        callees(
            &[(
                "field.nut",
                "class Base {\n  function hide() {}\n}\n\
                 class Child(Base) {\n  function run() {\n    this.hide = 1\n    this.hide()\n  }\n}\n",
            )],
            "Child.run",
        ),
        pairs(&["Base.hide"], 85),
    );
}

#[test]
fn a_function_over_a_parent_s_field_answers_for_its_class_and_those_below() {
    let source = "class Base {\n  hide = null\n}\n\
                  class Child(Base) {\n  function hide() {}\n  function run() {\n    this.hide()\n  }\n}\n\
                  class Leaf(Child) {\n  function run() {\n    this.hide()\n  }\n}\n";
    assert_eq!(
        callees(&[("field.nut", source)], "Child.run"),
        pairs(&["Child.hide"], 80),
    );
    assert_eq!(
        callees(&[("field.nut", source)], "Leaf.run"),
        pairs(&["Child.hide"], 85),
    );
}

#[test]
fn this_in_a_table_s_member_binds_the_table_s_sibling() {
    assert_eq!(
        callees(
            &[(
                "table.nut",
                "let t = {\n  function a() {}\n  function b() {\n    this.a()\n  }\n}\nreturn t\n",
            )],
            "t.b",
        ),
        pairs(&["t.a"], 80),
    );
}
