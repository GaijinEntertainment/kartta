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
const SHARED: &str = "spec/support/value.rb";
const JOB_SPEC: &str = "spec/job_spec.rb";
const CONFIG: &str = "spec/spec_helper.rb";
const WITH_VALUE: &str =
    "RSpec.shared_context \"with value\" do\n  let(:shared_value) { 3 }\nend\n";
const WITH_VALUE_FILTER: &str = "RSpec.configure do |config|\n\
                                 \x20 config.include_context \"with value\", :with_value\n\
                                 end\n";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (name, source) in files {
        let path = tree.path().join(name);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
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

#[test]
fn a_group_that_includes_a_shared_context_of_another_file_reads_its_let() {
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 let(:own_value) { 2 }\n\
               \x20 let(:derived) { shared_value }\n\
               \n\
               \x20 it \"reads\" do\n\
               \x20   own_value\n\
               \x20   shared_value\n\
               \x20 end\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, WITH_VALUE), (JOB_SPEC, job)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (vec!["derived"], vec![JOB_SPEC])
    );
}

#[test]
fn a_read_of_the_name_outside_a_group_that_includes_the_context_binds_nothing_of_it() {
    let job = "RSpec.describe \"job\" do\n\
               \x20 context \"with it\" do\n\
               \x20   include_context \"with value\"\n\
               \x20 end\n\
               \n\
               \x20 context \"without it\" do\n\
               \x20   let(:derived) { shared_value }\n\
               \n\
               \x20   it \"reads\" do\n\
               \x20     shared_value\n\
               \x20   end\n\
               \x20 end\n\
               end\n";
    let other = "RSpec.describe \"other\" do\n\
                 \x20 let(:derived) { shared_value }\n\
                 \n\
                 \x20 it \"reads\" do\n\
                 \x20   shared_value\n\
                 \x20 end\n\
                 end\n";
    let (_tree, engine) = build(&[(SHARED, WITH_VALUE), (JOB_SPEC, job), (OTHER_SPEC, other)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn a_group_s_own_let_stands_before_the_context_it_includes() {
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 let(:shared_value) { 4 }\n\
               \x20 let(:derived) { shared_value }\n\
               \n\
               \x20 it \"reads\" do\n\
               \x20   shared_value\n\
               \x20 end\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, WITH_VALUE), (JOB_SPEC, job)]);

    assert_eq!(
        (
            callers(&engine, SHARED, "shared_value", None),
            callers(&engine, JOB_SPEC, "shared_value", None),
        ),
        ((Vec::new(), Vec::new()), (vec!["derived"], vec![JOB_SPEC]))
    );
}

#[test]
fn every_form_that_defines_a_shared_group_and_include_examples_reach_its_let() {
    let shared = "shared_examples \"with examples\" do\n\
                  \x20 let(:from_examples) { 1 }\n\
                  end\n\
                  \n\
                  shared_examples_for \"with examples for\" do\n\
                  \x20 let(:from_examples_for) { 2 }\n\
                  end\n\
                  \n\
                  RSpec.describe \"holder\" do\n\
                  \x20 RSpec.shared_context :inside_a_group do\n\
                  \x20   let(:from_a_symbol) { 3 }\n\
                  \x20 end\n\
                  end\n";
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_examples \"with examples\"\n\
               \x20 include_examples 'with examples for'\n\
               \x20 include_context :inside_a_group\n\
               \n\
               \x20 let(:first) { from_examples }\n\
               \x20 let(:second) { from_examples_for }\n\
               \x20 let(:third) { from_a_symbol }\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, shared), (JOB_SPEC, job)]);

    assert_eq!(
        (
            callers(&engine, SHARED, "from_examples", None).0,
            callers(&engine, SHARED, "from_examples_for", None).0,
            callers(&engine, SHARED, "from_a_symbol", None).0,
        ),
        (vec!["first"], vec!["second"], vec!["third"])
    );
}

#[test]
fn a_string_and_a_symbol_of_one_spelling_are_two_names() {
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_context :with_value\n\
               \n\
               \x20 let(:derived) { shared_value }\n\
               end\n";
    let shared = "RSpec.shared_context \"with_value\" do\n  let(:shared_value) { 3 }\nend\n";
    let (_tree, engine) = build(&[(SHARED, shared), (JOB_SPEC, job)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn a_shared_group_a_group_of_another_file_defines_is_not_reached() {
    let scoped = "RSpec.describe \"holder\" do\n\
                  \x20 shared_context \"with value\" do\n\
                  \x20   let(:shared_value) { 3 }\n\
                  \x20 end\n\
                  end\n";
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 let(:derived) { shared_value }\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, scoped), (JOB_SPEC, job)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn it_behaves_like_reaches_the_group_s_let_from_its_own_block_alone() {
    let job = "RSpec.describe \"job\" do\n\
               \x20 it_behaves_like \"with value\" do\n\
               \x20   let(:inside) { shared_value }\n\
               \x20 end\n\
               \n\
               \x20 it_should_behave_like \"with value\"\n\
               \n\
               \x20 let(:outside) { shared_value }\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, WITH_VALUE), (JOB_SPEC, job)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (vec!["inside"], Vec::new())
    );
}

#[test]
fn a_let_nested_in_a_shared_group_s_own_group_stays_there() {
    let shared = "RSpec.shared_context \"with value\" do\n\
                  \x20 context \"inner\" do\n\
                  \x20   let(:shared_value) { 3 }\n\
                  \x20 end\n\
                  end\n";
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 let(:derived) { shared_value }\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, shared), (JOB_SPEC, job)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn of_two_files_that_define_one_shared_group_the_nearest_binds() {
    let near = "RSpec.shared_context \"with value\" do\n  let(:shared_value) { 1 }\nend\n";
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 let(:derived) { shared_value }\n\
               end\n";
    let far_path = "spec/aaa/value.rb";
    let (_tree, engine) = build(&[
        (far_path, WITH_VALUE),
        ("spec/near.rb", near),
        (JOB_SPEC, job),
    ]);

    assert_eq!(
        (
            callers(&engine, far_path, "shared_value", None).0,
            callers(&engine, "spec/near.rb", "shared_value", None).0,
        ),
        (Vec::new(), vec!["derived"])
    );
}

#[test]
fn of_two_contexts_a_group_includes_the_later_binds_and_a_nearer_group_stands_first() {
    let shared = "RSpec.shared_context \"first\" do\n  let(:item) { 1 }\nend\n\n\
                  RSpec.shared_context \"second\" do\n  let(:item) { 2 }\nend\n\n\
                  RSpec.shared_context \"third\" do\n  let(:item) { 3 }\nend\n";
    let job = "RSpec.describe \"job\" do\n\
               \x20 context \"inner\" do\n\
               \x20   include_context \"third\"\n\
               \n\
               \x20   let(:inner) { item }\n\
               \x20 end\n\
               \n\
               \x20 include_context \"first\"\n\
               \x20 include_context \"second\"\n\
               \n\
               \x20 let(:outer) { item }\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, shared), (JOB_SPEC, job)]);

    assert_eq!(
        (
            callers(&engine, SHARED, "item", Some(2)).0,
            callers(&engine, SHARED, "item", Some(6)).0,
            callers(&engine, SHARED, "item", Some(10)).0,
        ),
        (Vec::new(), vec!["outer"], vec!["inner"])
    );
}

#[test]
fn a_file_that_requires_the_context_lists_a_read_at_file_scope_once() {
    let job = "require_relative \"support/value\"\n\n\
               RSpec.describe \"job\" do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 it \"reads\" do\n\
               \x20   shared_value\n\
               \x20 end\n\
               end\n";
    let (_tree, engine) = build(&[(SHARED, WITH_VALUE), (JOB_SPEC, job)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None).1,
        vec![JOB_SPEC]
    );
}

#[test]
fn a_read_at_file_scope_of_another_file_reaches_no_let_outside_a_shared_group() {
    let other = "RSpec.describe \"other\" do\n\
                 \x20 it \"reads\" do\n\
                 \x20   item\n\
                 \x20 end\n\
                 end\n";
    let (_tree, engine) = build(&[
        (
            LET_SPEC,
            "RSpec.describe \"a\" do\n  let(:item) { 1 }\nend\n",
        ),
        (OTHER_SPEC, other),
    ]);

    assert_eq!(
        callers(&engine, LET_SPEC, "item", None).1,
        Vec::<&str>::new()
    );
}

#[test]
fn a_let_of_another_file_binds_at_the_receiver_tier_and_of_the_same_file_at_its_tier() {
    let job = format!(
        "{WITH_VALUE}\nRSpec.shared_context \"near\" do\n  let(:near_value) {{ 1 }}\nend\n\n\
         RSpec.describe \"job\" do\n\
         \x20 include_context \"near\"\n\
         \n\
         \x20 let(:derived) {{ near_value }}\n\
         end\n"
    );
    let other = "RSpec.describe \"other\" do\n\
                 \x20 include_context \"near\"\n\
                 \n\
                 \x20 let(:derived) { near_value }\n\
                 end\n";
    let (_tree, engine) = build(&[(JOB_SPEC, &job), (OTHER_SPEC, other)]);

    let (_, graph) = engine.graph_of(JOB_SPEC).expect("Ruby claims the file");
    let selector = Selector::new("near_value", Some(JOB_SPEC));
    let walk = graph
        .callers(selector, Limits::new(1))
        .expect("the file defines the name");
    let mut tiers: Vec<(&str, u8)> = (walk.reached.iter())
        .map(|reached| (reached.symbol.file, reached.confidence))
        .collect();
    tiers.sort_unstable();
    assert_eq!(tiers, [(OTHER_SPEC, 85), (JOB_SPEC, 80)]);
}

#[test]
fn a_bare_call_no_included_group_answers_binds_as_any_bare_call() {
    let helper = "def helper_method\n  1\nend\n";
    let job = "RSpec.describe \"job\" do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 let(:derived) { helper_method }\n\
               end\n";
    let (_tree, engine) = build(&[
        (SHARED, WITH_VALUE),
        ("spec/helper.rb", helper),
        (JOB_SPEC, job),
    ]);

    assert_eq!(
        callers(&engine, "spec/helper.rb", "helper_method", None).0,
        vec!["derived"]
    );
}

#[test]
fn a_group_a_filter_reaches_reads_the_let_of_a_context_config_includes() {
    let job = "RSpec.describe \"job\", :with_value do\n\
               \x20 let(:derived) { shared_value }\n\
               \n\
               \x20 context \"nested\" do\n\
               \x20   let(:nested) { shared_value }\n\
               \x20 end\n\
               \n\
               \x20 it \"reads\" do\n\
               \x20   shared_value\n\
               \x20 end\n\
               end\n";
    let (_tree, engine) = build(&[
        (SHARED, WITH_VALUE),
        (CONFIG, WITH_VALUE_FILTER),
        (JOB_SPEC, job),
    ]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (vec!["derived", "nested"], vec![JOB_SPEC])
    );
}

#[test]
fn a_filter_reaches_a_group_or_an_example_that_passes_it_and_no_group_beside_them() {
    let job = "RSpec.describe \"job\" do\n\
               \x20 let(:outside) { shared_value }\n\
               \n\
               \x20 context \"filtered\", :other, :with_value do\n\
               \x20   let(:inside) { shared_value }\n\
               \x20 end\n\
               \n\
               \x20 context \"plain\", :other do\n\
               \x20   let(:beside) { shared_value }\n\
               \x20 end\n\
               \n\
               \x20 context :with_value do\n\
               \x20   let(:described) { shared_value }\n\
               \x20 end\n\
               \n\
               \x20 it \"reads\", :with_value do\n\
               \x20   def helper\n\
               \x20     shared_value\n\
               \x20   end\n\
               \x20 end\n\
               end\n";
    let (_tree, engine) = build(&[
        (SHARED, WITH_VALUE),
        (CONFIG, WITH_VALUE_FILTER),
        (JOB_SPEC, job),
    ]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None).0,
        vec!["inside", "helper"]
    );
}

#[test]
fn any_filter_of_the_config_reaches_and_a_filter_of_another_context_does_not() {
    let config = "RSpec.configure do |config|\n\
                  \x20 config.include_context \"with value\", :first, :second\n\
                  \x20 config.include_context \"elsewhere\", :third\n\
                  end\n";
    let job = "RSpec.describe \"job\", :second do\n\
               \x20 let(:derived) { shared_value }\n\
               end\n";
    let other = "RSpec.describe \"other\", :third do\n\
                 \x20 let(:beside) { shared_value }\n\
                 end\n";
    let (_tree, engine) = build(&[
        (SHARED, WITH_VALUE),
        (CONFIG, config),
        (JOB_SPEC, job),
        (OTHER_SPEC, other),
    ]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None).0,
        vec!["derived"]
    );
}

#[test]
fn a_group_s_own_let_stands_before_a_context_a_filter_includes() {
    let job = "RSpec.describe \"job\", :with_value do\n\
               \x20 let(:derived) { shared_value }\n\
               \x20 let(:shared_value) { 4 }\n\
               \n\
               \x20 it \"reads\" do\n\
               \x20   shared_value\n\
               \x20 end\n\
               end\n";
    let (_tree, engine) = build(&[
        (SHARED, WITH_VALUE),
        (CONFIG, WITH_VALUE_FILTER),
        (JOB_SPEC, job),
    ]);

    assert_eq!(
        (
            callers(&engine, SHARED, "shared_value", None),
            callers(&engine, JOB_SPEC, "shared_value", None),
        ),
        ((Vec::new(), Vec::new()), (vec!["derived"], vec![JOB_SPEC]))
    );
}

#[test]
fn a_describe_on_another_receiver_and_an_include_of_a_group_pass_no_filter() {
    let job = "Other.describe \"job\", :with_value do\n\
               \x20 let(:on_a_value) { shared_value }\n\
               end\n\
               \n\
               RSpec.describe \"plain\" do\n\
               \x20 include_context \"elsewhere\", :with_value\n\
               \n\
               \x20 let(:included) { shared_value }\n\
               end\n";
    let (_tree, engine) = build(&[
        (SHARED, WITH_VALUE),
        (CONFIG, WITH_VALUE_FILTER),
        (JOB_SPEC, job),
    ]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn a_file_scope_read_through_two_filters_or_a_filter_and_an_include_lists_its_file_once() {
    let job = "RSpec.describe \"job\", :with_value, :also do\n\
               \x20 include_context \"with value\"\n\
               \n\
               \x20 it \"reads\" do\n\
               \x20   shared_value\n\
               \x20 end\n\
               end\n";
    let config = "RSpec.configure do |config|\n\
                  \x20 config.include_context \"with value\", :with_value, :also\n\
                  end\n";
    let (_tree, engine) = build(&[(SHARED, WITH_VALUE), (CONFIG, config), (JOB_SPEC, job)]);

    assert_eq!(
        callers(&engine, SHARED, "shared_value", None).1,
        vec![JOB_SPEC]
    );
}

#[test]
fn a_let_a_filter_reaches_binds_at_the_receiver_tier() {
    let job = "RSpec.describe \"job\", :with_value do\n\
               \x20 let(:derived) { shared_value }\n\
               end\n";
    let (_tree, engine) = build(&[
        (SHARED, WITH_VALUE),
        (CONFIG, WITH_VALUE_FILTER),
        (JOB_SPEC, job),
    ]);

    let (_, graph) = engine.graph_of(SHARED).expect("Ruby claims the file");
    let selector = Selector::new("shared_value", Some(SHARED));
    let walk = graph
        .callers(selector, Limits::new(1))
        .expect("the file defines the name");
    let tiers: Vec<u8> = (walk.reached.iter())
        .map(|reached| reached.confidence)
        .collect();
    assert_eq!(tiers, [85]);
}
