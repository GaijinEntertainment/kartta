//! What the arguments of a daslang call bind: never a function that takes fewer, and among overloads one that takes
//! them.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Walk};

fn tree(files: &[(&str, &str)]) -> Engine {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["daslang"]).expect("the language is known");

    Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds")
        .engine
}

fn walk_of<'t>(tree: &'t Engine, path: &str, name: &str) -> Walk<'t> {
    tree.graph_of(path)
        .expect("a language claims the file")
        .1
        .callees(name, Limits::new(1))
        .expect("the name is defined")
}

fn reached(walk: &Walk<'_>) -> Vec<String> {
    walk.reached
        .iter()
        .map(|reached| format!("{}:{}", reached.symbol.qualified_name, reached.symbol.line))
        .collect()
}

fn callees(files: &[(&str, &str)], path: &str, name: &str) -> Vec<String> {
    reached(&walk_of(&tree(files), path, name))
}

#[test]
fn a_call_that_passes_more_than_the_one_function_of_its_name_takes_binds_nothing() {
    let source = "options gen2
def make_visitor(someClass; blk) {
    make_visitor(someClass, 1, blk)
}
def other(a) {
    make_visitor(a, 1, 2)
}
";
    let tree = tree(&[("a.das", source)]);

    let wrapper = walk_of(&tree, "a.das", "make_visitor");
    assert_eq!(reached(&wrapper), Vec::<String>::new());
    assert_eq!(wrapper.unresolved, 1);
    assert_eq!(wrapper.self_call, None);

    let other = walk_of(&tree, "a.das", "other");
    assert_eq!(reached(&other), Vec::<String>::new());
    assert_eq!(other.unresolved, 1);
}

#[test]
fn a_call_binds_the_overload_of_its_file_that_takes_its_arguments() {
    let source = "options gen2
def write(path) {
    print(path)
}
def write(path, text) {
    print(text)
}
def run() {
    write(1, 2)
}
";
    assert_eq!(callees(&[("a.das", source)], "a.das", "run"), ["write:5"]);
}

#[test]
fn a_parameter_with_a_default_takes_an_argument_and_needs_none() {
    let source = "options gen2
def open(path; mode = 1; var flags : int = 2) {
    print(path)
}
def full() {
    open(1, 2, 3)
}
def short() {
    open(1)
}
";
    let tree = tree(&[("a.das", source)]);

    assert_eq!(reached(&walk_of(&tree, "a.das", "full")), ["open:2"]);
    assert_eq!(reached(&walk_of(&tree, "a.das", "short")), ["open:2"]);
}

#[test]
fn a_call_that_passes_fewer_than_a_definition_requires_binds_nothing() {
    let builtin = "options gen2
module builtin shared public

def float3(a, b, c) {
    return a;
}
";
    let shapes = "options gen2
require daslib/builtin

def too_few() {
    let v = float3(1);
}

def fits() {
    let v = float3(1, 2, 3);
}

def too_many() {
    let v = float3(1, 2, 3, 4);
}
";
    let tree = tree(&[("daslib/builtin.das", builtin), ("shapes.das", shapes)]);

    let too_few = walk_of(&tree, "shapes.das", "too_few");
    assert_eq!(reached(&too_few), Vec::<String>::new());
    assert_eq!(too_few.unresolved, 1);
    assert_eq!(
        reached(&walk_of(&tree, "shapes.das", "fits")),
        ["builtin.float3:4"]
    );
    assert_eq!(
        reached(&walk_of(&tree, "shapes.das", "too_many")),
        Vec::<String>::new()
    );
}

#[test]
fn a_parameter_with_a_default_is_not_required() {
    let source = "options gen2
def open(path; mode = 1; var flags : int = 2) {
    print(path)
}
def clamped(a, b : int = 2) {
    print(a)
}
def with_required() {
    open(1)
}
def default_covers_each_name_of_its_entry() {
    clamped()
}
";
    let tree = tree(&[("a.das", source)]);

    assert_eq!(
        reached(&walk_of(&tree, "a.das", "with_required")),
        ["open:2"]
    );
    assert_eq!(
        reached(&walk_of(
            &tree,
            "a.das",
            "default_covers_each_name_of_its_entry"
        )),
        ["clamped:5"]
    );
}

#[test]
fn each_name_of_a_parameter_list_takes_an_argument() {
    let source = "options gen2
def blend(a, b, c : int; d) {
    print(a)
}
def fits() {
    blend(1, 2, 3, 4)
}
def over() {
    blend(1, 2, 3, 4, 5)
}
";
    let tree = tree(&[("a.das", source)]);

    assert_eq!(reached(&walk_of(&tree, "a.das", "fits")), ["blend:2"]);
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "over")),
        Vec::<String>::new()
    );
}

#[test]
fn a_call_passes_what_a_pipe_gives_it() {
    let source = "options gen2
def two(a, b) {
    print(a)
}
def fits_block() {
    two(1) <| $() {
        print(2)
    }
}
def fits_trailing() {
    two(1) {
        print(2)
    }
}
def fits_value() {
    1 |> two(2)
}
def over_block() {
    two(1, 2) <| $() {
        print(3)
    }
}
def over_trailing() {
    two(1, 2) {
        print(3)
    }
}
def over_value() {
    1 |> two(2, 3)
}
def over_chain() {
    1 |> two(2, 3) |> two(4)
}
";
    let tree = tree(&[("a.das", source)]);

    for fits in ["fits_block", "fits_trailing", "fits_value"] {
        assert_eq!(reached(&walk_of(&tree, "a.das", fits)), ["two:2"], "{fits}");
    }
    for over in ["over_block", "over_trailing", "over_value"] {
        assert_eq!(
            reached(&walk_of(&tree, "a.das", over)),
            Vec::<String>::new(),
            "{over}"
        );
    }
    let chain = walk_of(&tree, "a.das", "over_chain");
    assert_eq!(reached(&chain), ["two:2"]);
    assert_eq!(chain.unresolved, 1);
}

#[test]
fn a_call_passes_what_each_pipe_at_its_head_gives_it() {
    let source = "options gen2
def three(a, b, c) {
    print(a)
}
def two(a, b) {
    print(a)
}
def value_and_block() {
    1 |> three(2) <| $() {
        print(3)
    }
}
def value_and_trailing_block() {
    1 |> three(2) {
        print(3)
    }
}
def value_and_too_few() {
    1 |> three() <| $() {
        print(3)
    }
}
def block_of_the_last_call() {
    two(1) <| two(2) <| 3
}
";
    let tree = tree(&[("a.das", source)]);
    let bound = |name| reached(&walk_of(&tree, "a.das", name));

    assert_eq!(bound("value_and_block"), ["three:2"]);
    assert_eq!(bound("value_and_trailing_block"), ["three:2"]);
    assert_eq!(bound("value_and_too_few"), Vec::<String>::new());
    let last_call = walk_of(&tree, "a.das", "block_of_the_last_call");
    assert_eq!(reached(&last_call), ["two:5"]);
    assert_eq!(last_call.unresolved, 0);
}

#[test]
fn a_named_argument_is_an_argument() {
    let source = "options gen2
def two(a, b) {
    print(a)
}
def fits() {
    two(1, b = 2)
}
def over() {
    two(1, 2, c = 3)
}
";
    let tree = tree(&[("a.das", source)]);

    assert_eq!(reached(&walk_of(&tree, "a.das", "fits")), ["two:2"]);
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "over")),
        Vec::<String>::new()
    );
}

#[test]
fn the_object_of_a_member_call_fills_the_self_a_def_static_writes() {
    let source = "options gen2
struct Bits {
    def static erase(var self : Bits; at : int) {
        print(at)
    }
}
def fits(b) {
    b.erase(1)
}
def short(b) {
    b.erase()
}
def too_few_as_free(b, c) {
    two(b)
}
def two(a, b) {
    print(a)
}
def short_member(b) {
    b.two()
}
";
    let tree = tree(&[("a.das", source)]);
    let bound = |name| reached(&walk_of(&tree, "a.das", name));

    assert_eq!(bound("fits"), ["Bits.erase:3"]);
    assert_eq!(bound("short"), Vec::<String>::new());
    assert_eq!(bound("too_few_as_free"), Vec::<String>::new());
    assert_eq!(bound("short_member"), Vec::<String>::new());
}

#[test]
fn a_member_call_passes_its_object_to_a_free_function_and_to_a_method_s_implicit_parameter() {
    let source = "options gen2
class Widget {
    def resize(width) {
        print(width)
    }
    def over() {
        resize(1, 2)
    }
}
def scale(widget, by) {
    print(by)
}
def free_fits(w) {
    w.scale(2)
}
def free_over(w) {
    w.scale(2, 3)
}
def method_fits(w) {
    w.resize(1)
}
def method_over(w) {
    w.resize(1, 2)
}
";
    let tree = tree(&[("a.das", source)]);
    let bound = |name| reached(&walk_of(&tree, "a.das", name));

    assert_eq!(bound("free_fits"), ["scale:10"]);
    assert_eq!(bound("free_over"), Vec::<String>::new());
    assert_eq!(bound("method_fits"), ["Widget.resize:3"]);
    assert_eq!(bound("method_over"), Vec::<String>::new());
    assert_eq!(bound("Widget.over"), Vec::<String>::new());
}

#[test]
fn a_block_applied_to_a_member_passes_the_object_and_the_block() {
    let source = "options gen2
def run_with(widget, blk) {
    print(1)
}
def short(widget) {
    print(1)
}
def piped(w) {
    w.run_with() <| $() {
        print(2)
    }
}
def applied(w) {
    w.run_with {
        print(2)
    }
}
def over(w) {
    w.short {
        print(2)
    }
}
";
    let tree = tree(&[("a.das", source)]);

    assert_eq!(reached(&walk_of(&tree, "a.das", "piped")), ["run_with:2"]);
    assert_eq!(reached(&walk_of(&tree, "a.das", "applied")), ["run_with:2"]);
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "over")),
        Vec::<String>::new()
    );
}

#[test]
fn a_class_method_macro_gives_its_def_the_self_a_pipe_passes() {
    let source = "options gen2
class Table {
    [class_method]
    def static grow() {
        print(1)
    }
    [explicit_const_class_method]
    def static find(key) {
        print(key)
    }
    def run() {
        self |> grow()
        self |> find(1)
        self |> find(1, 2)
    }
}
";
    assert_eq!(
        callees(&[("a.das", source)], "a.das", "Table.run"),
        ["Table.grow:3", "Table.find:7"]
    );
}

#[test]
fn a_member_call_short_of_a_class_method_s_written_parameter_binds_nothing() {
    let source = "options gen2
class Table {
    [class_method]
    def static find(key) {
        print(key)
    }
}
def given(var t : Table) {
    t.find(1)
}
def short(var t : Table) {
    t.find()
}
";
    let tree = tree(&[("a.das", source)]);

    assert_eq!(reached(&walk_of(&tree, "a.das", "given")), ["Table.find:3"]);
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "short")),
        Vec::<String>::new()
    );
}

#[test]
fn a_call_through_a_module_passes_no_object() {
    let source = "options gen2
require lib
def run() {
    lib::two(1, 2)
}
def over() {
    lib::two(1, 2, 3)
}
";
    let lib = "options gen2\nmodule lib\ndef two(a, b) {\n    print(a)\n}\n";
    let tree = tree(&[("a.das", source), ("lib.das", lib)]);

    assert_eq!(reached(&walk_of(&tree, "a.das", "run")), ["lib.two:3"]);
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "over")),
        Vec::<String>::new()
    );
}

#[test]
fn a_call_of_a_native_overload_binds_no_function_a_required_file_declares() {
    let source = "options gen2\nrequire lib\ndef run() {\n    stat(1, 2)\n}\n";
    let lib = "options gen2\nmodule lib\ndef stat(path) {\n    print(path)\n}\n";

    assert_eq!(
        callees(&[("a.das", source), ("lib.das", lib)], "a.das", "run"),
        Vec::<String>::new()
    );
}

#[test]
fn a_call_binds_by_name_alone_no_definition_its_arguments_leave_alone_among_those_of_its_name() {
    let source = "options gen2\ndef run() {\n    cvt(1, 2)\n}\n";
    let two = "options gen2\nmodule two\ndef cvt(a, b) {\n    print(a)\n}\n";
    let one = "options gen2\nmodule one\ndef cvt(a) {\n    print(a)\n}\n";

    assert_eq!(
        callees(
            &[
                ("x/p/a.das", source),
                ("y/q/two.das", two),
                ("z/r/one.das", one)
            ],
            "x/p/a.das",
            "run"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn a_block_query_binds_no_script_function_of_its_name() {
    let source = "options gen2
require uriparser_boost

def run() {
    query() <| $ [es] (eid : int) {
        print(\"{eid}\");
    }
}

def plain() {
    let text = query(\"a=b\");
}

def two() {
    query(\"a=b\", 1);
}
";
    let library = "options gen2
module uriparser_boost shared public

def query(text : string) {
    return text;
}
";
    let tree = tree(&[("a.das", source), ("uriparser_boost.das", library)]);

    let query = walk_of(&tree, "a.das", "run");
    assert_eq!(reached(&query), Vec::<String>::new());
    assert_eq!(query.unresolved, 2);
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "plain")),
        ["uriparser_boost.query:4"]
    );
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "two")),
        Vec::<String>::new()
    );
}

#[test]
fn a_query_binds_nothing_however_it_passes_its_block() {
    let source = "options gen2
require uriparser_boost

def trailing() {
    query() $(eid : int) {
        print(\"{eid}\");
    }
}

def written() {
    query($ [es] (eid : int) {
        print(\"{eid}\");
    })
}

def through_a_module() {
    ecs::query() <| $ [es] (eid : int) {
        print(\"{eid}\");
    }
}
";
    let library = "options gen2
module uriparser_boost shared public

def query(text : string) {
    return text;
}
";
    let tree = tree(&[("a.das", source), ("uriparser_boost.das", library)]);

    for form in ["trailing", "written", "through_a_module"] {
        let query = walk_of(&tree, "a.das", form);
        assert_eq!(reached(&query), Vec::<String>::new(), "{form}");
        assert_eq!(query.unresolved, 2, "{form}");
    }
}

#[test]
fn a_block_binds_a_def_of_another_name_annotated_or_not() {
    let source = "options gen2
require lib

def piped() {
    each_item() <| $ [es] (eid : int) {
        print(\"{eid}\");
    }
}

def trailing() {
    each_item() $(eid : int) {
        print(\"{eid}\");
    }
}
";
    let library = "options gen2
module lib shared public

def each_item(blk : block<(eid : int) : void>) {
    invoke(blk, 1);
}
";
    let tree = tree(&[("a.das", source), ("lib.das", library)]);

    for form in ["piped", "trailing"] {
        assert_eq!(
            reached(&walk_of(&tree, "a.das", form)),
            ["lib.each_item:4"],
            "{form}"
        );
    }
}

#[test]
fn the_arguments_decide_where_the_name_has_enough_candidates_to_share_an_index() {
    let lib = |at: usize| {
        format!("options gen2\nmodule lib{at}\ndef stat(path) {{\n    print(path)\n}}\n")
    };
    let files: Vec<(String, String)> = (0..64)
        .map(|at| (format!("lib{at}.das"), lib(at)))
        .collect();
    let mut sources: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    let source =
        "options gen2\nrequire lib0\ndef one() {\n    stat(1)\n}\ndef two() {\n    stat(1, 2)\n}\n";
    sources.push(("a.das", source));
    let tree = tree(&sources);

    assert_eq!(reached(&walk_of(&tree, "a.das", "one")), ["lib0.stat:3"]);
    assert_eq!(
        reached(&walk_of(&tree, "a.das", "two")),
        Vec::<String>::new()
    );
}
