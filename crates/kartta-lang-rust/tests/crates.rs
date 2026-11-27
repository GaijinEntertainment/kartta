//! The paths into other crates a Rust file names through its `use` declarations and path calls, and the module files
//! the resolver binds them to through the packages of the workspace.

use kartta_lang::{Extraction, ImportBinding, Language, MemoryRepository};
use kartta_lang_rust::Rust;

const USE_FORM: u32 = 2;

fn bound(source: &str) -> Vec<(String, String, u32)> {
    Rust::new()
        .extract(source.as_bytes())
        .directives
        .into_iter()
        .filter(|directive| directive.form == USE_FORM)
        .map(|directive| (directive.names[0].clone(), directive.spec, directive.line))
        .collect()
}

fn path(name: &str, path: &str, line: u32) -> (String, String, u32) {
    (name.to_owned(), path.to_owned(), line)
}

const GLOB_FORM: u32 = 3;

fn globs(source: &str) -> Vec<(String, u32)> {
    Rust::new()
        .extract(source.as_bytes())
        .directives
        .into_iter()
        .filter(|directive| directive.form == GLOB_FORM)
        .map(|directive| (directive.spec, directive.line))
        .collect()
}

#[test]
fn a_glob_outside_every_block_keeps_the_module_path_it_ends_at() {
    let got = globs(
        "use super::*;\nuse crate::m::*;\nuse other::m::{self, sub::*};\nuse ::rooted::*;\nuse std::collections::*;\n\
         use crate::m::Kind::*;\n\nmod tests {\n    use other::t::*;\n}\n\nfn run() {\n    use other::n::*;\n}\n",
    );

    assert_eq!(
        got,
        [
            ("super".to_owned(), 1),
            ("crate::m".to_owned(), 2),
            ("other::m::sub".to_owned(), 3),
            ("rooted".to_owned(), 4),
        ]
    );
}

#[test]
fn a_use_binds_each_name_it_imports_from_another_crate_to_the_path_in_source_order() {
    let got = bound(
        "use kartta_lang::tree::{self, line as at, text};\nuse kartta_lang::path::*;\nuse std::mem::take;\n\
         use crate::local::thing;\nuse other::Item as _;\nuse ::rooted::item; use first::zeta; use second::alpha;\n\n\
         fn run() { text(); }\n",
    );

    assert_eq!(
        got,
        [
            path("tree", "kartta_lang::tree", 1),
            path("at", "kartta_lang::tree::line", 1),
            path("text", "kartta_lang::tree::text", 1),
            path("thing", "crate::local::thing", 4),
            path("item", "rooted::item", 6),
            path("zeta", "first::zeta", 6),
            path("alpha", "second::alpha", 6),
        ]
    );
}

#[test]
fn a_path_call_binds_its_receiver_to_the_path_through_another_crate() {
    let got = bound(
        "use kartta_lang::tree;\n\nfn run() {\n    kartta_walk::walk();\n    other::m::f();\n    tree::sub::g();\n\
         \x20   self::h();\n    crate::n::i();\n    Vec::new();\n    std::fs::read();\n}\n",
    );

    assert_eq!(
        got,
        [
            path("tree", "kartta_lang::tree", 1),
            path("kartta_walk", "kartta_walk", 4),
            path("m", "other::m", 5),
            path("sub", "kartta_lang::tree::sub", 6),
            path("h", "self::h", 7),
            path("n", "crate::n", 8),
        ]
    );
}

#[test]
fn a_path_call_in_a_macro_s_arguments_binds_its_receiver_to_the_path_as_one_outside_does() {
    let got = bound(
        "use kartta_lang::tree;\n\nfn run() {\n    assert!(other::m::f());\n    assert_eq!(tree::sub::g(), \
         super::h());\n    debug_assert!(first::mac!(crate::n::i()));\n}\n",
    );

    assert_eq!(
        got,
        [
            path("tree", "kartta_lang::tree", 1),
            path("m", "other::m", 4),
            path("sub", "kartta_lang::tree::sub", 5),
            path("h", "super::h", 5),
            path("mac", "first::mac", 6),
            path("n", "crate::n", 6),
        ]
    );
}

#[test]
fn a_macro_invocation_through_another_crate_binds_the_macro_name_to_its_path() {
    let got = bound(
        "use other_crate as oc;\n\nfn run() {\n    first::mac!();\n    oc::sub::deep!();\n    std::println!();\n\
         \x20   crate::local!();\n    $crate::inner!();\n}\n",
    );

    assert_eq!(
        got,
        [
            path("oc", "other_crate", 1),
            path("mac", "first::mac", 4),
            path("deep", "other_crate::sub::deep", 5),
            path("local", "crate::local", 7),
        ]
    );
}

#[test]
fn a_name_only_a_function_body_or_an_inline_module_declares_keeps_its_path() {
    let got = bound(
        "use dep as client;\nuse other_crate::helper;\n\nmod inner {\n    pub fn helper() {}\n}\n\nfn run() {\n\
         \x20   fn client() {}\n    client::m::run();\n    helper();\n}\n",
    );

    assert_eq!(
        got,
        [
            path("client", "dep", 1),
            path("helper", "other_crate::helper", 2),
            path("m", "dep::m", 10),
        ]
    );
}

#[test]
fn a_use_inside_a_function_binds_a_name_only_for_the_calls_of_its_block() {
    let scoped = bound("fn a() {\n    use other_crate::m;\n    m::f();\n}\n");
    let called_outside = bound(
        "fn a() {\n    use other_crate::m::{self, f};\n    m::f();\n    f();\n}\n\n\
         fn b() {\n    m::f();\n    f();\n}\n",
    );
    let after_an_inner_block = bound(
        "fn a() {\n    {\n        use other_crate::m;\n        m::f();\n    }\n    m::f();\n}\n",
    );
    let in_an_inline_module = bound(
        "fn a() {\n    use other_crate::m;\n    m::f();\n    mod inner {\n        fn g() { m::f(); }\n    }\n}\n",
    );
    let outside_through_it = bound(
        "fn a() {\n    use other_crate::m;\n    m::f();\n}\n\nfn b() {\n    m::sub::g();\n}\n",
    );
    let also_at_file_level = bound(
        "use other_crate::m;\n\nfn a() {\n    use other_crate::m;\n    m::f();\n}\n\n\
         fn b() {\n    m::f();\n}\n",
    );

    assert_eq!(scoped, [path("m", "other_crate::m", 2)]);
    assert_eq!(called_outside, []);
    assert_eq!(after_an_inner_block, []);
    assert_eq!(in_an_inline_module, []);
    assert_eq!(
        outside_through_it,
        [path("m", "other_crate::m", 2), path("sub", "m::sub", 7)]
    );
    assert_eq!(also_at_file_level, [path("m", "other_crate::m", 1)]);
}

#[test]
fn a_use_inside_an_inline_module_binds_a_name_only_for_the_calls_of_that_module() {
    let scoped = bound("mod tests {\n    use other_crate::m;\n\n    fn t() { m::f(); }\n}\n");
    let called_outside = bound(
        "mod tests {\n    use other_crate::m::{self, f};\n\n    fn t() { m::f(); f(); }\n}\n\n\
         fn outside() {\n    m::f();\n    f();\n}\n",
    );
    let in_a_nested_module = bound(
        "mod tests {\n    use other_crate::m;\n\n    fn t() { m::f(); }\n\n    mod inner {\n\
         \x20       fn g() { m::f(); }\n    }\n}\n",
    );
    let also_at_file_level = bound(
        "use other_crate::m;\n\nmod tests {\n    use other_crate::m;\n\n    fn t() { m::f(); }\n}\n\n\
         fn outside() {\n    m::f();\n}\n",
    );

    assert_eq!(scoped, [path("m", "other_crate::m", 2)]);
    assert_eq!(called_outside, []);
    assert_eq!(in_a_nested_module, []);
    assert_eq!(also_at_file_level, [path("m", "other_crate::m", 1)]);
}

#[test]
fn a_use_inside_a_block_binds_a_macro_name_only_for_the_invocations_of_its_block() {
    let in_a_function = bound("fn a() { use other_crate::mac; mac!(); }\n");
    let invoked_outside_a_function =
        bound("fn a() { use other_crate::mac; mac!(); }\nfn outside() { mac!(); }\n");
    let in_a_module =
        bound("mod tests {\n    use other_crate::mac;\n\n    fn t() { mac!(); }\n}\n");
    let invoked_outside_a_module = bound(
        "mod tests {\n    use other_crate::mac;\n\n    fn t() { mac!(); }\n}\n\nfn outside() { mac!(); }\n",
    );
    let invoked_as_an_item_outside_a_module =
        bound("mod tests {\n    use other_crate::mac;\n\n    fn t() { mac!(); }\n}\n\nmac! {}\n");

    assert_eq!(in_a_function, [path("mac", "other_crate::mac", 1)]);
    assert_eq!(invoked_outside_a_function, []);
    assert_eq!(in_a_module, [path("mac", "other_crate::mac", 2)]);
    assert_eq!(invoked_outside_a_module, []);
    assert_eq!(invoked_as_an_item_outside_a_module, []);
}

#[test]
fn a_path_through_self_or_super_is_kept_only_outside_every_inline_module() {
    let got = bound(
        "use super::shared;\n\nfn run() { super::super::up(); self::sub::g(); }\n\nmod tests {\n\
         \x20   use super::helper;\n\n    fn t() { super::super::up(); helper::h(); }\n}\n",
    );

    assert_eq!(
        got,
        [
            path("shared", "super::shared", 1),
            path("up", "super::super::up", 3),
            path("sub", "self::sub", 3),
        ]
    );
}

#[test]
fn a_path_through_crate_or_super_to_a_name_the_file_declares_binds_its_module_keyword() {
    let source = "pub fn new() {\n    super::new();\n    crate::new();\n    self::new();\n    super::other();\n}\n";
    let receivers: Vec<Option<String>> = Rust::new()
        .extract(source.as_bytes())
        .calls
        .into_iter()
        .map(|call| call.receiver)
        .collect();

    assert_eq!(
        bound(source),
        [
            path("super", "super", 2),
            path("crate", "crate", 3),
            path("other", "super::other", 5),
        ]
    );
    assert_eq!(
        receivers,
        [
            Some("super".to_owned()),
            Some("crate".to_owned()),
            None,
            None
        ]
    );
}

#[test]
fn a_path_that_ends_at_a_capitalised_item_is_kept_only_for_a_bare_call_of_its_name() {
    let got = bound(
        "use other::m::Wrapper;\nuse other::m::Kind;\nuse crate::m::Pair as P;\nuse super::Unit;\n\n\
         fn run() {\n    Wrapper(1);\n    Kind::new();\n    P(1, 2);\n    other::m::Handle::new();\n}\n",
    );

    let called_bare_outside_its_block = bound(
        "fn a() {\n    use other::m::Wrapper;\n    Wrapper::new();\n}\n\nfn b() {\n    Wrapper(1);\n}\n",
    );

    assert_eq!(
        got,
        [
            path("Wrapper", "other::m::Wrapper", 1),
            path("P", "crate::m::Pair", 3),
        ]
    );
    assert_eq!(called_bare_outside_its_block, []);
}

#[test]
fn a_value_read_through_a_path_keeps_the_path_of_its_last_segment_and_none_of_a_capitalised_one() {
    let got = bound(
        "mod walk;

fn run() {
    apply(walk::raw_walk);
    apply(crate::sort::by_name);
    Kind(1);
             let a = other::Kind::A;
}
",
    );

    assert_eq!(
        got,
        [
            path("walk", "self::walk", 4),
            path("sort", "crate::sort", 5)
        ]
    );
}

#[test]
fn a_value_read_through_a_path_a_path_call_binds_otherwise_keeps_no_path_and_no_use() {
    let source = "fn run() {\n    crate::opaques::f();\n    apply(super::opaques::g);\n}\n";

    assert_eq!(bound(source), [path("opaques", "crate::opaques", 2)]);
    assert_eq!(Rust::new().extract(source.as_bytes()).uses, []);
}

#[test]
fn a_kept_path_includes_no_file_its_name_spells() {
    let rust = Rust::new();
    let repository = MemoryRepository::new([("src/lib.rs", ""), ("src/walk", "")]);

    let extraction = rust.resolver(&repository).resolve(
        rust.extract(b"use other::walk;\n"),
        "src/lib.rs",
        &repository,
    );

    assert_eq!(extraction.includes, []);
}

#[test]
fn a_name_the_file_declares_or_binds_to_two_paths_binds_no_path() {
    let got = bound(
        "use a::text;\nuse b::path;\nuse d::keep;\n\nfn text() {}\n\nmod util {\n    pub fn f() {}\n}\n\n\
         fn run() {\n    util::f();\n    x::util::g();\n    c::path::join();\n    y::m::f();\n    z::m::f();\n\
         \x20   keep();\n}\n",
    );

    assert_eq!(got, [path("keep", "d::keep", 3)]);
}

#[test]
fn a_use_whose_first_segment_is_a_module_the_file_declares_starts_at_the_file_s_own_module() {
    let got = bound(
        "use m::helper;\nuse m::m;\nuse self::n::n;\nuse other_crate::shared;\n\nmod m;\nmod n;\n\n\
         mod inner {\n    mod deep;\n    use deep::thing;\n    use m::other;\n}\n\nfn run() {\n    use m::local;\n}\n",
    );
    let globs = globs("pub use m::*;\nuse inline::*;\nuse other::*;\n\nmod m;\nmod inline {}\n");

    assert_eq!(
        got,
        [
            path("helper", "self::m::helper", 1),
            path("m", "self::m::m", 2),
            path("n", "self::n::n", 3),
            path("shared", "other_crate::shared", 4),
            path("other", "m::other", 12),
            path("local", "self::m::local", 16),
        ]
    );
    assert_eq!(
        globs,
        [
            ("self::m".to_owned(), 1),
            ("self::inline".to_owned(), 2),
            ("other".to_owned(), 3),
        ]
    );
}

#[test]
fn a_path_call_through_a_module_file_the_file_declares_starts_at_the_file_s_own_module() {
    let got = bound(
        "mod alpha;\nmod beta;\n#[path = \"g.rs\"]\nmod gamma;\nmod inline {}\n\nfn run() {\n    alpha::run();\n\
         \x20   beta::deep::run();\n    gamma::run();\n    inline::run();\n}\n\nmod tests {\n    fn t() {\n\
         \x20       alpha::run();\n    }\n}\n",
    );
    let through_the_crate = bound("mod alpha;\n\nfn run() {\n    crate::alpha::run();\n}\n");

    assert_eq!(
        got,
        [
            path("alpha", "self::alpha", 8),
            path("deep", "self::beta::deep", 9)
        ]
    );
    assert_eq!(through_the_crate, [path("alpha", "crate::alpha", 4)]);
}

fn resolve(files: &[(&str, &str)], path: &str) -> Extraction {
    let rust = Rust::new();
    let repository = MemoryRepository::new(files.iter().copied());
    let source = files
        .iter()
        .find(|(file, _)| *file == path)
        .map_or("", |(_, source)| source);
    rust.resolver(&repository)
        .resolve(rust.extract(source.as_bytes()), path, &repository)
}

fn binding(target: &str, last_segment: &str) -> ImportBinding {
    ImportBinding::new(None, Some(target.to_owned()), last_segment.to_owned())
}

const WORKSPACE: &[(&str, &str)] = &[
    ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
    (
        "crates/lang/Cargo.toml",
        "[package]\nname = \"kartta-lang\"\n",
    ),
    ("crates/lang/src/lib.rs", ""),
    ("crates/lang/src/tree.rs", ""),
    ("crates/lang/src/path/mod.rs", ""),
    (
        "crates/walk/Cargo.toml",
        "[package]\nname = \"walker\" # the package\n\n[lib]\nname = \"kartta_walk\"\npath = \"lib.rs\"\n\n\
         [[bin]]\nname = \"walk\"\n",
    ),
    ("crates/walk/lib.rs", ""),
    ("crates/walk/walk.rs", ""),
    ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
    (
        "crates/app/src/main.rs",
        "use kartta_lang::tree::text;\nuse kartta_lang::path as paths;\nuse kartta_lang::Language;\n\
         use serde::Serialize;\n\nfn main() {\n    kartta_walk::walk::run();\n    kartta_lang::tree::Node::new();\n}\n",
    ),
    (
        "crates/lang/tests/it.rs",
        "use kartta_lang::tree::text;\n\nfn check() { text(); }\n",
    ),
];

#[test]
fn a_path_into_a_workspace_crate_binds_the_module_file_its_segments_name() {
    let got = resolve(WORKSPACE, "crates/app/src/main.rs").bindings;

    assert_eq!(
        got,
        [
            binding("crates/lang/src/tree.rs", "text"),
            binding("crates/lang/src/path/mod.rs", "paths"),
            binding("crates/walk/walk.rs", "walk"),
        ]
    );
}

#[test]
fn a_crate_s_own_name_binds_its_library_from_its_tests() {
    let got = resolve(WORKSPACE, "crates/lang/tests/it.rs").bindings;

    assert_eq!(got, [binding("crates/lang/src/tree.rs", "text")]);
}

const OWN_CRATE: &[(&str, &str)] = &[
    ("Cargo.toml", "[package]\nname = \"app\"\n"),
    ("src/lib.rs", "mod a;\n"),
    ("src/main.rs", ""),
    (
        "src/a.rs",
        "mod b;\n\nfn run() { crate::root(); super::up(); }\n",
    ),
    (
        "src/a/b.rs",
        "use crate::a::c;\nuse super::helper;\nuse crate::a::Kind;\n\nfn run() {\n    super::super::root();\n\
         \x20   self::d::e();\n    c::f();\n    super::super::super::past();\n    crate::a::Kind::new();\n}\n",
    ),
    ("src/a/c.rs", ""),
    ("src/a/b/d.rs", ""),
    ("src/bin/tool/main.rs", ""),
    ("src/bin/tool/cli.rs", "fn run() { crate::go(); }\n"),
    (
        "tests/common/mod.rs",
        "fn set_up() { crate::x(); super::y(); }\n",
    ),
    ("tests/it/main.rs", ""),
    ("tests/it/util.rs", "fn check() { super::z(); }\n"),
];

#[test]
fn a_path_through_crate_self_or_super_binds_the_module_file_from_the_caller_s_module() {
    let nested = resolve(OWN_CRATE, "src/a/b.rs").bindings;
    let top = resolve(OWN_CRATE, "src/a.rs").bindings;

    assert_eq!(
        nested,
        [
            binding("src/a/c.rs", "c"),
            binding("src/a.rs", "helper"),
            binding("src/lib.rs", "root"),
            binding("src/a/b/d.rs", "d"),
        ]
    );
    assert_eq!(
        top,
        [binding("src/lib.rs", "root"), binding("src/lib.rs", "up")]
    );
}

#[test]
fn a_glob_binds_the_file_of_the_module_it_ends_at_as_a_dot_import() {
    let got = resolve(
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "mod m;\n#[cfg(test)]\nmod tests;\n"),
            ("src/m.rs", ""),
            (
                "src/tests.rs",
                "use super::*;\nuse crate::m::*;\nuse crate::m::item::*;\n",
            ),
        ],
        "src/tests.rs",
    )
    .bindings;

    let glob = |target: &str, last_segment: &str| {
        ImportBinding::new(
            Some(".".to_owned()),
            Some(target.to_owned()),
            last_segment.to_owned(),
        )
    };
    assert_eq!(got, [glob("src/lib.rs", "super"), glob("src/m.rs", "m")]);
}

#[test]
fn a_target_s_own_files_start_crate_paths_at_its_root_and_shared_ones_bind_none() {
    let binary = resolve(OWN_CRATE, "src/bin/tool/cli.rs").bindings;
    let shared = resolve(OWN_CRATE, "tests/common/mod.rs").bindings;
    let in_a_target = resolve(OWN_CRATE, "tests/it/util.rs").bindings;

    assert_eq!(binary, [binding("src/bin/tool/main.rs", "go")]);
    assert_eq!(shared, []);
    assert_eq!(in_a_target, [binding("tests/it/main.rs", "z")]);
}

#[test]
fn a_library_at_its_package_root_starts_crate_paths_of_its_own_files_and_not_of_src() {
    let files: &[(&str, &str)] = &[
        ("Cargo.toml", "[workspace]\nmembers = [\"walk\"]\n"),
        (
            "walk/Cargo.toml",
            "[package]\nname = \"walk\"\n\n[lib]\npath = \"lib.rs\"\n",
        ),
        ("walk/lib.rs", "mod tree;\n"),
        ("walk/tree.rs", "fn run() { crate::top(); }\n"),
        ("walk/src/main.rs", "mod helper;\n"),
        ("walk/src/helper.rs", "fn run() { crate::go(); }\n"),
    ];

    let library = resolve(files, "walk/tree.rs").bindings;
    let binary = resolve(files, "walk/src/helper.rs").bindings;

    assert_eq!(library, [binding("walk/lib.rs", "top")]);
    assert_eq!(binary, [binding("walk/src/main.rs", "go")]);
}

#[test]
fn a_library_at_the_repository_root_starts_crate_paths_of_the_files_beside_it() {
    let files: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            "[package]\nname = \"walk\"\n\n[lib]\npath = \"lib.rs\"\n",
        ),
        ("lib.rs", "mod tree;\n"),
        ("tree.rs", "fn run() { crate::top(); }\n"),
    ];

    let got = resolve(files, "tree.rs").bindings;

    assert_eq!(got, [binding("lib.rs", "top")]);
}

#[test]
fn a_library_root_its_manifest_names_starts_self_paths_at_its_own_directory() {
    let files: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            "[package]\nname = \"cli\"\n\n[lib]\npath = \"src/root.rs\"\n",
        ),
        ("src/root.rs", "fn run() { self::child::f(); }\n"),
        ("src/child.rs", ""),
    ];

    let got = resolve(files, "src/root.rs").bindings;

    assert_eq!(got, [binding("src/child.rs", "child")]);
}

#[test]
fn a_crate_name_several_packages_share_binds_the_nearest_one_or_none() {
    let files = [
        ("app/Cargo.toml", "[package]\nname = \"app\"\n"),
        ("app/src/main.rs", "use shared::f;\n\nfn main() { f(); }\n"),
        ("app/shared/Cargo.toml", "[package]\nname = \"shared\"\n"),
        ("app/shared/src/lib.rs", ""),
        ("vendor/shared/Cargo.toml", "[package]\nname = \"shared\"\n"),
        ("vendor/shared/src/lib.rs", ""),
        ("tools/Cargo.toml", "[package]\nname = \"tools\"\n"),
        (
            "tools/src/main.rs",
            "use shared::f;\n\nfn main() { f(); }\n",
        ),
    ];

    let nearest = resolve(&files, "app/src/main.rs").bindings;
    let tied = resolve(&files, "tools/src/main.rs").bindings;

    assert_eq!(nearest, [binding("app/shared/src/lib.rs", "f")]);
    assert_eq!(tied, []);
}

const MANIFEST: &str =
    "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.0\"\n";

/// Whether the resolver built over a package whose `Cargo.toml` reads `edited`, with the paths `more` beside, resolves
/// as the one built over the same package with [`MANIFEST`].
fn resolves_as_before(edited: &str, more: &[&str]) -> bool {
    let rust = Rust::new();
    let before = MemoryRepository::new([("app/Cargo.toml", MANIFEST), ("app/src/lib.rs", "")]);
    let after = MemoryRepository::new([("app/Cargo.toml", edited), ("app/src/lib.rs", "")])
        .with_paths(more.iter().copied());

    rust.resolver(&after).resolves_as(&*rust.resolver(&before))
}

#[test]
fn a_cargo_toml_that_only_bumps_its_version_or_a_dependency_resolves_as_before() {
    let bumped = MANIFEST.replace("0.1.0", "0.1.1").replace("1.0.0", "1.0.1");

    assert!(resolves_as_before(&bumped, &[]));
}

#[test]
fn a_renamed_or_moved_library_or_a_new_package_resolves_otherwise() {
    let renamed = MANIFEST.replace("\"app\"", "\"other\"");
    let moved = format!("{MANIFEST}\n[lib]\npath = \"lib.rs\"\n");

    assert!(!resolves_as_before(&renamed, &[]));
    assert!(!resolves_as_before(&moved, &[]));
    assert!(!resolves_as_before(MANIFEST, &["app/tools/Cargo.toml"]));
}

const DEPENDING: &[(&str, &str)] = &[
    ("Cargo.toml", "[workspace]\nmembers = [\"app\"]\n"),
    ("build/gen.rs", ""),
    (
        "app/Cargo.toml",
        "[package]\nname = \"app\"\n\n[dependencies]\nshared = \"1\"\nserde = \"1\"\n\n\
         [dev-dependencies]\nkit = { path = \"../tools/helper\", package = \"helper-kit\" }\n",
    ),
    ("app/src/lib.rs", ""),
    (
        "vendor/shared-1/Cargo.toml",
        "[package]\nname = \"shared\"\n",
    ),
    (
        "vendor/shared-2/Cargo.toml",
        "[package]\nname = \"shared\"\n",
    ),
    (
        "tools/helper/Cargo.toml",
        "[package]\nname = \"helper_kit\"\n\n[dependencies]\ndeep = \"1\"\n\
         lazy = { version = \"1\", optional = true }\n\n[dev-dependencies]\nother = \"1\"\n",
    ),
    ("tools/deep/Cargo.toml", "[package]\nname = \"deep\"\n"),
    ("tools/lazy/Cargo.toml", "[package]\nname = \"lazy\"\n"),
    ("other/Cargo.toml", "[package]\nname = \"other\"\n"),
    ("other/src/lib.rs", ""),
];

#[test]
fn a_file_s_package_reaches_every_package_of_each_name_its_build_lists() {
    let depending = resolve(DEPENDING, "app/src/lib.rs").dependencies;
    let depending_on_none = resolve(DEPENDING, "other/src/lib.rs").dependencies;
    let of_no_named_package = resolve(DEPENDING, "build/gen.rs").dependencies;

    assert_eq!(
        depending.as_deref(),
        Some(
            "tools/deep/Cargo.toml\ntools/helper/Cargo.toml\nvendor/shared-1/Cargo.toml\nvendor/shared-2/Cargo.toml"
        )
    );
    assert_eq!(depending_on_none.as_deref(), Some(""));
    assert_eq!(of_no_named_package, None);
}

const INHERITING: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\n\
         walk = { path = \"crates/walker\", package = \"kartta-walker\" }\nplain = { path = \"crates/plain\" }\n\n\
         [workspace.dependencies.ignore]\npath = \"crates/rules\"\npackage = \"ignore-rules\"\n",
    ),
    (
        "crates/fs/Cargo.toml",
        "[package]\nname = \"fs\"\n\n[dependencies]\nwalk.workspace = true\nplain = { workspace = true }\n\n\
         [dev-dependencies]\nignore = { workspace = true }\n",
    ),
    ("crates/fs/src/lib.rs", ""),
    (
        "crates/walker/Cargo.toml",
        "[package]\nname = \"kartta-walker\"\n",
    ),
    (
        "crates/rules/Cargo.toml",
        "[package]\nname = \"ignore-rules\"\n",
    ),
    ("crates/plain/Cargo.toml", "[package]\nname = \"plain\"\n"),
    ("vendor/Cargo.toml", "[workspace]\nmembers = [\"own\"]\n"),
    (
        "vendor/own/Cargo.toml",
        "[package]\nname = \"own\"\n\n[dependencies]\nwalk.workspace = true\n",
    ),
    ("vendor/own/src/lib.rs", ""),
];

#[test]
fn a_member_depends_on_the_package_its_nearest_workspace_s_entry_names() {
    let member = resolve(INHERITING, "crates/fs/src/lib.rs").dependencies;
    let of_another_workspace = resolve(INHERITING, "vendor/own/src/lib.rs").dependencies;

    assert_eq!(
        member.as_deref(),
        Some("crates/plain/Cargo.toml\ncrates/rules/Cargo.toml\ncrates/walker/Cargo.toml")
    );
    assert_eq!(of_another_workspace.as_deref(), Some(""));
}

#[test]
fn a_dependency_on_a_package_the_tree_holds_resolves_otherwise() {
    let rust = Rust::new();
    let held = ("lib/Cargo.toml", "[package]\nname = \"lib\"\n");
    let before = MemoryRepository::new([("app/Cargo.toml", MANIFEST), held]);
    let depending = format!("{MANIFEST}lib = {{ path = \"../lib\" }}\n");
    let after = MemoryRepository::new([("app/Cargo.toml", &depending[..]), held]);

    assert!(!rust.resolver(&after).resolves_as(&*rust.resolver(&before)));
}
