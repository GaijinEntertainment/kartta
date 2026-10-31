//! What a bare Ruby call binds where a `let` or a `subject` defines its name on the example group around the call: the
//! method of the innermost group that holds the call, never a definition of the name in another file, and another file
//! reaches the method by no name of its own.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Selector};
use tempfile::TempDir;

const LET_SPEC: &str = "let_spec.rb";
const OTHER_SPEC: &str = "other_spec.rb";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (name, source) in files {
        fs::write(tree.path().join(name), source).expect("the file is written");
    }
    let languages = kartta_languages::select(["ruby"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// The symbols that call the `name` that starts on `line` of `path`, or at any line for `None`, and the files whose top
/// level calls it.
fn callers<'e>(
    engine: &'e Engine,
    path: &str,
    name: &str,
    line: Option<u32>,
) -> (Vec<&'e str>, Vec<&'e str>) {
    let (_, graph) = engine.graph_of(path).expect("Ruby claims the file");
    let selector = Selector::new(name, Some(path)).with_line(line);
    let walk = graph
        .callers(selector, Limits::new(1))
        .expect("the file defines the name");
    let reached = walk.reached.iter().map(|reached| reached.symbol.name);
    let file_scope = walk.file_scope.iter().map(|file| file.file);
    (reached.collect(), file_scope.collect())
}

#[test]
fn a_bare_call_of_a_let_binds_the_let_and_not_another_file_s_def() {
    let (_tree, engine) = build(&[
        (
            LET_SPEC,
            "RSpec.describe \"generate\" do\n  let(:current) do\n    build_formula\n  end\n\n  \
             def with_history\n    current\n  end\nend\n",
        ),
        (
            OTHER_SPEC,
            "RSpec.describe \"upload\" do\n  def current\n    :other\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callers(&engine, OTHER_SPEC, "current", None),
        (Vec::new(), Vec::new())
    );
    assert_eq!(
        callers(&engine, LET_SPEC, "current", None),
        (vec!["with_history"], Vec::new())
    );
}

#[test]
fn a_call_binds_the_method_of_the_innermost_group_that_holds_it_wherever_it_is_defined() {
    let source = "RSpec.describe \"a\" do\n\
                  \x20 let(:item) { 1 }\n\
                  \n\
                  \x20 def outer_helper\n\
                  \x20   item\n\
                  \x20 end\n\
                  \n\
                  \x20 context \"inner\" do\n\
                  \x20   def inner_helper\n\
                  \x20     item\n\
                  \x20   end\n\
                  \n\
                  \x20   let(:item) { 2 }\n\
                  \x20 end\n\
                  \n\
                  \x20 context \"sibling\" do\n\
                  \x20   def sibling_helper\n\
                  \x20     item\n\
                  \x20   end\n\
                  \x20 end\n\
                  end\n";
    let (_tree, engine) = build(&[(LET_SPEC, source)]);

    assert_eq!(
        (
            callers(&engine, LET_SPEC, "item", Some(2)).0,
            callers(&engine, LET_SPEC, "item", Some(13)).0,
        ),
        (vec!["outer_helper", "sibling_helper"], vec!["inner_helper"])
    );
}

#[test]
fn a_later_method_of_one_group_replaces_the_earlier() {
    let source = "RSpec.describe \"a\" do\n\
                  \x20 let(:item) { 1 }\n\
                  \x20 let(:item) { 2 }\n\
                  \n\
                  \x20 def helper\n\
                  \x20   item\n\
                  \x20 end\n\
                  end\n";
    let (_tree, engine) = build(&[(LET_SPEC, source)]);

    assert_eq!(
        (
            callers(&engine, LET_SPEC, "item", Some(2)).0,
            callers(&engine, LET_SPEC, "item", Some(3)).0,
        ),
        (Vec::new(), vec!["helper"])
    );
}

#[test]
fn a_group_reaches_no_method_of_a_group_beside_it() {
    let source = "RSpec.describe \"a\" do\n\
                  \x20 context \"first\" do\n\
                  \x20   let(:item) { 1 }\n\
                  \x20 end\n\
                  \n\
                  \x20 context \"second\" do\n\
                  \x20   def helper\n\
                  \x20     item\n\
                  \x20   end\n\
                  \x20 end\n\
                  end\n";
    let (_tree, engine) = build(&[(LET_SPEC, source)]);

    assert_eq!(
        callers(&engine, LET_SPEC, "item", None).0,
        Vec::<&str>::new()
    );
}

#[test]
fn a_call_that_passes_an_argument_or_a_block_is_no_call_of_a_group_method() {
    let source = "RSpec.describe \"a\" do\n\
                  \x20 let(:version) { \"1.0\" }\n\
                  \x20 let(:livecheck) { 1 }\n\
                  \n\
                  \x20 def with_argument\n\
                  \x20   version \"2.0\"\n\
                  \x20 end\n\
                  \n\
                  \x20 def with_block\n\
                  \x20   livecheck do\n\
                  \x20   end\n\
                  \x20 end\n\
                  \n\
                  \x20 def with_parentheses\n\
                  \x20   version()\n\
                  \x20 end\n\
                  end\n";
    let (_tree, engine) = build(&[(LET_SPEC, source)]);

    assert_eq!(
        (
            callers(&engine, LET_SPEC, "version", None).0,
            callers(&engine, LET_SPEC, "livecheck", None).0,
        ),
        (vec!["with_parentheses"], Vec::new())
    );
}

#[test]
fn another_file_and_a_call_on_a_value_reach_no_group_method() {
    let (_tree, engine) = build(&[
        (
            LET_SPEC,
            "RSpec.describe \"a\" do\n  let(:widget) { \"x\" }\n\n  \
             def helper(value)\n    value.widget\n  end\nend\n",
        ),
        (
            OTHER_SPEC,
            "RSpec.describe \"b\" do\n  def helper\n    widget\n  end\n\n  \
             def on_a_value(value)\n    value.widget\n  end\nend\n",
        ),
    ]);

    assert_eq!(
        callers(&engine, LET_SPEC, "widget", None).0,
        Vec::<&str>::new()
    );
}

#[test]
fn a_let_s_block_is_its_body_and_a_call_at_file_scope_keeps_its_name() {
    let source = "RSpec.describe \"a\" do\n\
                  \x20 let(:base) { 1 }\n\
                  \x20 subject(:total) { base }\n\
                  \n\
                  \x20 it \"adds\" do\n\
                  \x20   total\n\
                  \x20 end\n\
                  end\n";
    let (_tree, engine) = build(&[(LET_SPEC, source)]);

    assert_eq!(
        (
            callers(&engine, LET_SPEC, "base", None),
            callers(&engine, LET_SPEC, "total", None),
        ),
        ((vec!["total"], Vec::new()), (Vec::new(), vec![LET_SPEC]))
    );
}
