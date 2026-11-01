//! What a Quirrel call on a field of `this` binds: a slot the walk reads as holding one class calls that class's
//! member, and any other slot binds the caller's own member only as a guess.
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

fn one(name: &str, confidence: u8) -> Vec<(String, u8)> {
    vec![(name.to_owned(), confidence)]
}

const VIEW: &str = "class Record {
  function getOwnerId() {
    return 1
  }
}

class View {
  record = null

  constructor() {
    this.record = Record()
  }

  function getOwnerId() {
    return 2
  }

  function run() {
    return this.record.getOwnerId()
  }

  function own() {
    return this.getOwnerId()
  }
}
";

#[test]
fn a_field_a_constructor_sets_to_a_class_binds_that_class_s_member() {
    assert_eq!(
        callees(&[("a.nut", VIEW)], "View.run"),
        one("Record.getOwnerId", 85),
    );
}

#[test]
fn a_call_on_this_keeps_the_caller_s_own_member() {
    assert_eq!(
        callees(&[("a.nut", VIEW)], "View.own"),
        one("View.getOwnerId", 80),
    );
}

/// A `View` over classes `A` and `B`, each and `View` holding `get`, with `body` in its class body.
fn view_with(body: &str) -> String {
    format!(
        "class A {{\n  function get() {{}}\n}}\nclass B(A) {{\n  function get() {{}}\n}}\n\
         class View {{\n  slot = null\n  function get() {{}}\n  \
         function run() {{\n    this.slot.get()\n  }}\n{body}\n}}\n"
    )
}

#[test]
fn a_default_in_the_class_body_names_the_class() {
    let source = view_with("").replace("slot = null", "slot = A()");
    assert_eq!(callees(&[("a.nut", &source)], "View.run"), one("A.get", 85));
}

#[test]
fn a_write_in_a_later_member_names_the_class() {
    let source = view_with("  function set() {\n    this.slot = A()\n  }");
    assert_eq!(callees(&[("a.nut", &source)], "View.run"), one("A.get", 85));
}

#[test]
fn a_new_slot_write_on_an_instance_names_no_class() {
    let source = view_with("  function set() {\n    this.slot <- A()\n  }");
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("View.get", 15)
    );
}

#[test]
fn writes_of_null_beside_a_class_leave_the_class() {
    let source = view_with(
        "  function set() {\n    this.slot = A()\n  }\n  function clear() {\n    this.slot = null\n  }",
    );
    assert_eq!(callees(&[("a.nut", &source)], "View.run"), one("A.get", 85));
}

#[test]
fn a_field_set_from_two_classes_keeps_the_caller_s_own_member() {
    let source = view_with("  function set() {\n    this.slot = A()\n    this.slot = B()\n  }");
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("View.get", 15),
    );
}

#[test]
fn a_field_set_from_a_parameter_keeps_the_caller_s_own_member() {
    let source = view_with("  constructor(slot) {\n    this.slot = slot\n  }");
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("View.get", 15),
    );
}

#[test]
fn a_field_binds_the_file_s_one_definition_of_the_name_only_as_a_guess() {
    let source = "class View {\n  slot = null\n  constructor(slot) {\n    this.slot = slot\n  }\n  \
                  function get() {}\n  function run() {\n    this.slot.get()\n  }\n}\n";
    assert_eq!(
        callees(&[("a.nut", source)], "View.run"),
        one("View.get", 15),
    );
}

#[test]
fn a_chain_off_a_field_keeps_the_caller_s_own_member() {
    let source = "class View {\n  slot = null\n  function get() {}\n  \
                  function run() {\n    this.slot.part.get()\n  }\n}\n";
    assert_eq!(
        callees(&[("a.nut", source)], "View.run"),
        one("View.get", 80),
    );
}

#[test]
fn a_field_binds_another_class_of_the_file_at_its_tier() {
    let source = "class A {\n  function get() {}\n}\n\
                  class View {\n  slot = null\n  function run() {\n    this.slot.get()\n  }\n}\n";
    assert_eq!(callees(&[("a.nut", source)], "View.run"), one("A.get", 80));
}

#[test]
fn a_parameter_named_as_a_class_builds_no_class() {
    let source = view_with("  constructor(A) {\n    this.slot = A()\n  }");
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("View.get", 15),
    );
}

#[test]
fn a_write_in_another_class_says_nothing_of_this_one_s_field() {
    let source = format!(
        "{}class Other {{\n  slot = null\n  constructor() {{\n    this.slot = A()\n  }}\n}}\n",
        view_with("")
    );
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("View.get", 15),
    );
}

#[test]
fn a_write_on_another_value_says_nothing_of_the_field() {
    let source = view_with("  function set(other) {\n    other.slot = A()\n  }");
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("View.get", 15),
    );
}

#[test]
fn a_class_the_file_binds_no_name_for_is_a_namesake() {
    let holder = "class View {\n  slot = null\n  constructor() {\n    this.slot = Native()\n  }\n  \
                  function get() {}\n  function run() {\n    this.slot.get()\n  }\n}\n";
    assert_eq!(
        callees(
            &[
                ("view.nut", holder),
                ("mock.nut", "class Native {\n  function get() {}\n}\n"),
            ],
            "View.run",
        ),
        one("View.get", 15),
    );
}

fn factory_holder(factory: &str) -> String {
    format!(
        "{factory}\nclass View {{\n  slot = null\n  constructor() {{\n    this.slot = Part()\n  }}\n  \
         function get() {{}}\n  function run() {{\n    this.slot.get()\n  }}\n}}\n"
    )
}

#[test]
fn a_function_the_file_binds_builds_no_class() {
    for factory in [
        "local function Part() {\n  return {}\n}",
        "let Part = function() {\n  return {}\n}",
        "let Part = @() {}",
    ] {
        assert_eq!(
            callees(
                &[
                    ("view.nut", &factory_holder(factory)),
                    ("part.nut", "class Part {\n  function get() {}\n}\n"),
                ],
                "View.run",
            ),
            one("View.get", 15),
            "{factory}",
        );
    }
}

#[test]
fn a_local_class_of_the_file_binds() {
    let source = factory_holder("local class Part {\n  function get() {}\n}");
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("Part.get", 85)
    );
}

#[test]
fn a_class_a_require_names_binds_across_files() {
    let holder = "let { Part } = require(\"part.nut\")\nclass View {\n  slot = null\n  constructor() {\n    \
                  this.slot = Part()\n  }\n  function get() {}\n  function run() {\n    this.slot.get()\n  }\n}\n";
    assert_eq!(
        callees(
            &[
                ("view.nut", holder),
                (
                    "part.nut",
                    "class Part {\n  function get() {}\n}\nreturn { Part }\n"
                ),
            ],
            "View.run",
        ),
        one("Part.get", 85),
    );
}

#[test]
fn a_member_the_class_inherits_binds_where_its_parent_holds_it() {
    let source = "class Root {\n  function close() {}\n}\nclass Part(Root) {}\n\
                  class View {\n  slot = Part()\n  function close() {}\n  \
                  function run() {\n    this.slot.close()\n  }\n}\n";
    assert_eq!(
        callees(&[("a.nut", source)], "View.run"),
        one("Root.close", 85),
    );
}

#[test]
fn a_field_of_this_read_with_a_null_safe_access_binds_as_a_guess() {
    let source = "class Model {\n  function getView() {}\n}\n\
                  class Holder {\n  slot = null\n  function getView() {}\n  \
                  function a() {\n    this.slot.getView()\n  }\n  \
                  function b() {\n    this?.slot.getView()\n  }\n  \
                  function c() {\n    this?.slot?.getView()\n  }\n}\n";
    for caller in ["Holder.a", "Holder.b", "Holder.c"] {
        assert_eq!(
            callees(&[("a.nut", source)], caller),
            one("Holder.getView", 15),
            "{caller}",
        );
    }
}

#[test]
fn a_field_read_with_a_null_safe_access_names_the_class_its_writes_build() {
    let source = VIEW.replace("this.record.getOwnerId", "this?.record.getOwnerId");
    assert_eq!(
        callees(&[("a.nut", &source)], "View.run"),
        one("Record.getOwnerId", 85),
    );
}

#[test]
fn a_new_slot_write_on_a_table_names_the_class() {
    let source = "class Part {\n  function get() {}\n}\n\
                  let t = {\n  function get() {}\n  function set() {\n    this.slot <- Part()\n  }\n  \
                  function run() {\n    this.slot.get()\n  }\n}\n\
                  return t\n";
    assert_eq!(callees(&[("a.nut", source)], "t.run"), one("Part.get", 85));
}

#[test]
fn a_table_s_slot_names_the_class_its_member_reads() {
    let source = "class Part {\n  function get() {}\n}\n\
                  let t = {\n  slot = Part()\n  function get() {}\n  function run() {\n    this.slot.get()\n  }\n}\n\
                  return t\n";
    assert_eq!(callees(&[("a.nut", source)], "t.run"), one("Part.get", 85));
}
