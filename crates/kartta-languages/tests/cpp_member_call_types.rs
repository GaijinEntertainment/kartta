//! What a C++ call through `.` or `->` binds where the graph holds a type of the name it spells: it names a member, so
//! it binds no class, no alias and no constructor, and such a definition does not keep it from a member of the name,
//! though another included file that offers one leaves the member a guess; a call with no receiver that names a type
//! keeps that binding.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SymbolKind, confidence};
use tempfile::TempDir;

const CALL_THROUGH_A_FIELD: &str = "struct visitor
{
\tvoid operator()(int i) {}
};

struct Args
{
\tbool (*visitor)(int heap, int area);
};

bool run(Args *args, int heap)
{
\tif (!args->visitor(heap, 2))
\t\treturn false;
\treturn true;
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

fn callees(engine: &Engine, path: &str, name: &str) -> Vec<(String, SymbolKind)> {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .map(|reached| (reached.symbol.name.to_owned(), reached.symbol.kind))
        .collect()
}

fn confidence_of(engine: &Engine, path: &str, from: &str, to: &str) -> u8 {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(from, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .find(|reached| reached.symbol.name == to && reached.symbol.kind == SymbolKind::Method)
        .expect("the call binds a member of the name")
        .confidence
}

#[test]
fn a_call_through_a_field_binds_no_struct_of_its_name_and_counts_as_bound_to_nothing() {
    let (_tree, engine) = build(&[("run.cpp", CALL_THROUGH_A_FIELD)]);
    let (_, graph) = engine
        .graph_of("run.cpp")
        .expect("a language claims the file");

    assert_eq!(callees(&engine, "run.cpp", "run"), []);

    let resolution = graph.resolution(0);
    assert_eq!((resolution.sites, resolution.bound), (1, vec![]));
}

#[test]
fn a_call_with_no_receiver_keeps_the_type_it_names() {
    let (_tree, engine) = build(&[(
        "run.cpp",
        "struct visitor\n{\n\tint at;\n};\n\nvisitor make()\n{\n\treturn visitor();\n}\n",
    )]);

    assert_eq!(
        callees(&engine, "run.cpp", "make"),
        [("visitor".to_owned(), SymbolKind::Struct)]
    );
}

#[test]
fn a_call_on_a_value_binds_no_alias_of_its_name() {
    let (_tree, engine) = build(&[(
        "run.cpp",
        "typedef int string;\n\nstruct Path;\n\nvoid run(Path &p)\n{\n\tp.string();\n}\n",
    )]);

    assert_eq!(callees(&engine, "run.cpp", "run"), []);
}

#[test]
fn a_call_on_a_value_binds_no_constructor_of_its_name() {
    let (_tree, engine) = build(&[
        (
            "set.cpp",
            "#include \"set.h\"\n\nset::set(int size)\n{\n\tstored = size;\n}\n",
        ),
        (
            "set.h",
            "struct set\n{\n\tset(int size);\n\tint stored;\n};\n",
        ),
        (
            "run.cpp",
            "#include \"set.h\"\n\nauto make();\n\nvoid run()\n{\n\tauto bits = make();\n\tbits.set(1);\n}\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "run.cpp", "run"),
        [("make".to_owned(), SymbolKind::Function)]
    );
}

const CALL_OF_A_HASH: &str = "#include \"hash.h\"\n#include \"hasher.h\"\n\nauto make();\n\nint run()\n{\n\t\
                              auto hasher = make();\n\treturn hasher.hash();\n}\n";

#[test]
fn a_type_another_include_offers_leaves_the_call_a_guess_at_the_one_member() {
    let (_tree, engine) = build(&[
        (
            "hash.h",
            "namespace eastl\n{\nstruct hash\n{\n\tint at;\n};\n}\n",
        ),
        ("hasher.h", "struct Hasher\n{\n\tint hash();\n};\n"),
        ("run.cpp", CALL_OF_A_HASH),
    ]);

    assert_eq!(
        callees(&engine, "run.cpp", "run"),
        [
            ("hash".to_owned(), SymbolKind::Method),
            ("make".to_owned(), SymbolKind::Function),
        ]
    );
    assert_eq!(
        confidence_of(&engine, "run.cpp", "run", "hash"),
        confidence::FIRST_PARTY
    );
}

#[test]
fn a_type_only_a_further_include_offers_leaves_the_member_of_one_as_far_at_its_tier() {
    let (_tree, engine) = build(&[
        (
            "hash.h",
            "namespace eastl\n{\nstruct hash\n{\n\tint at;\n};\n}\n",
        ),
        ("hasher.h", "struct Hasher\n{\n\tint hash();\n};\n"),
        ("through.h", "#include \"hash.h\"\n#include \"hasher.h\"\n"),
        (
            "run.cpp",
            "#include \"through.h\"\n\nauto make();\n\nint run()\n{\n\tauto hasher = make();\n\t\
             return hasher.hash();\n}\n",
        ),
    ]);

    assert_eq!(
        confidence_of(&engine, "run.cpp", "run", "hash"),
        confidence::INCLUDE
    );
}

#[test]
fn a_name_of_many_types_leaves_the_call_the_one_member() {
    let types = "namespace n\n{\nstruct hash\n{\n\tint at;\n};\n}\n".repeat(64);
    let (_tree, engine) = build(&[
        ("hash.h", &types),
        ("hasher.h", "struct Hasher\n{\n\tint hash();\n};\n"),
        ("through.h", "#include \"hasher.h\"\n"),
        (
            "run.cpp",
            "#include \"through.h\"\n\nauto make();\n\nint run()\n{\n\tauto hasher = make();\n\t\
             return hasher.hash();\n}\n",
        ),
    ]);

    assert_eq!(
        confidence_of(&engine, "run.cpp", "run", "hash"),
        confidence::UNIQUE
    );
}

#[test]
fn a_type_in_the_member_s_own_file_leaves_the_one_member_at_its_tier() {
    let (_tree, engine) = build(&[
        ("hash.h", "#pragma once\n"),
        (
            "hasher.h",
            "struct hash\n{\n\tint at;\n};\n\nstruct Hasher\n{\n\tint hash();\n};\n",
        ),
        ("run.cpp", CALL_OF_A_HASH),
    ]);

    assert_eq!(
        confidence_of(&engine, "run.cpp", "run", "hash"),
        confidence::INCLUDE
    );
}

#[test]
fn a_constructor_an_include_offers_leaves_the_call_a_guess_at_the_one_member() {
    let (_tree, engine) = build(&[
        ("hash.h", "struct hash\n{\n\thash();\n};\n"),
        ("hasher.h", "struct Hasher\n{\n\tint hash();\n};\n"),
        ("run.cpp", CALL_OF_A_HASH),
    ]);

    assert_eq!(
        confidence_of(&engine, "run.cpp", "run", "hash"),
        confidence::FIRST_PARTY
    );
}

#[test]
fn a_type_of_the_name_in_the_caller_s_file_leaves_the_member_of_the_file_at_its_tier() {
    let (_tree, engine) = build(&[(
        "run.cpp",
        "namespace eastl\n{\nstruct hash\n{\n\tint at;\n};\n}\n\nstruct Hasher\n{\n\tint hash();\n};\n\n\
         auto make();\n\nint run()\n{\n\tauto hasher = make();\n\treturn hasher.hash();\n}\n",
    )]);

    assert_eq!(
        confidence_of(&engine, "run.cpp", "run", "hash"),
        confidence::SAME_FILE
    );
}
