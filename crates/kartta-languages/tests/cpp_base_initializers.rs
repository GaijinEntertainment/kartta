//! What a name in a C++ constructor's initializer list binds: the constructor of the base it names, and nothing where
//! it names a member.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const BASE_AND_DERIVED: &str = "class B
{
public:
\tB(int x);
};

B::B(int x) {}

class D : public B
{
public:
\tD(int x) : B(x) {}
};

void make()
{
\tB b(1);
}
";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        fs::write(tree.path().join(path), source).expect("the file is written");
    }
    let languages = kartta_languages::select(&["cpp"]).expect("every name is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees(engine: &Engine, path: &str, name: &str) -> Vec<String> {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .map(|reached| reached.symbol.qualified_name.to_owned())
        .collect()
}

fn callers(engine: &Engine, path: &str, name: &str) -> Vec<String> {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callers = graph
        .callers(name, Limits::new(1))
        .expect("the name is defined");
    (callers.reached.iter())
        .map(|reached| reached.symbol.qualified_name.to_owned())
        .collect()
}

#[test]
fn a_base_initializer_binds_the_base_constructor() {
    let (_tree, engine) = build(&[("a.cpp", BASE_AND_DERIVED)]);

    assert_eq!(callees(&engine, "a.cpp", "D::D"), ["B::B"]);
    assert_eq!(callers(&engine, "a.cpp", "B::B"), ["D::D"]);
}

#[test]
fn a_base_initializer_binds_a_constructor_in_an_included_file() {
    let (_tree, engine) = build(&[
        ("base.h", "class B\n{\npublic:\n\tB(int x);\n};\n"),
        (
            "derived.h",
            "#include \"base.h\"\n\nclass D : public B\n{\npublic:\n\tD(int x) : B(x) {}\n};\n",
        ),
    ]);

    assert_eq!(callees(&engine, "derived.h", "D::D"), ["B::B"]);
}

#[test]
fn a_base_initializer_written_with_a_scope_or_arguments_binds_the_base_constructor() {
    let (_tree, engine) = build(&[(
        "a.cpp",
        "namespace n\n{\nstruct S\n{\n\tS(int x) {}\n};\n}\n\ntemplate <class T> struct G\n{\n\tG(int x) {}\n};\n\n\
         struct D : n::S, G<int>\n{\n\tD(int x) : n::S(x), G<int>(x) {}\n};\n",
    )]);

    let mut bound = callees(&engine, "a.cpp", "D::D");
    bound.sort();
    assert_eq!(bound, ["G::G", "S::S"]);
}

#[test]
fn a_scoped_base_of_the_caller_s_name_binds_its_constructor_and_not_the_caller() {
    let (_tree, engine) = build(&[(
        "a.cpp",
        "namespace a\n{\nstruct B\n{\n\tB(int x);\n};\nB::B(int x) {}\n}\n\n\
         struct B : a::B\n{\n\tB(int x) : a::B(x) {}\n};\n",
    )]);
    let (_, graph) = engine
        .graph_of("a.cpp")
        .expect("a language claims the file");
    let callers = graph
        .callers("B::B", Limits::new(1))
        .expect("the name is defined");

    let lines: Vec<u32> = (callers.reached.iter())
        .map(|reached| reached.symbol.line)
        .collect();
    assert_eq!(lines, [12]);
    assert!(callers.self_call.is_none());
}

#[test]
fn a_scoped_template_base_of_the_caller_s_name_binds_its_constructor_and_not_the_caller() {
    let (_tree, engine) = build(&[(
        "a.cpp",
        "namespace a\n{\ntemplate <class T> struct B\n{\n\tB(int x) {}\n};\n}\n\n\
         struct B : a::B<int>\n{\n\tB(int x) : a::B<int>(x) {}\n};\n",
    )]);
    let (_, graph) = engine
        .graph_of("a.cpp")
        .expect("a language claims the file");
    let callers = graph
        .callers("B::B", Limits::new(1))
        .expect("the name is defined");

    let lines: Vec<u32> = (callers.reached.iter())
        .map(|reached| reached.symbol.line)
        .collect();
    assert_eq!(lines, [11]);
    assert!(callers.self_call.is_none());
}

#[test]
fn a_base_with_no_constructor_binds_its_class() {
    let (_tree, engine) = build(&[(
        "a.cpp",
        "struct B\n{\n\tint at;\n};\n\nstruct D : B\n{\n\tD() : B() {}\n};\n",
    )]);

    assert_eq!(callees(&engine, "a.cpp", "D::D"), ["B"]);
}

#[test]
fn a_member_initializer_binds_no_constructor_of_its_name() {
    let (_tree, engine) = build(&[(
        "a.cpp",
        "struct m\n{\n\tm(int x) {}\n};\n\nstruct B\n{\n\tB(int x) {}\n};\n\n\
         struct D : B\n{\n\tD(int x) : B(x), m(x) {}\n\tint m;\n};\n",
    )]);

    assert_eq!(callees(&engine, "a.cpp", "D::D"), ["B::B"]);
}
