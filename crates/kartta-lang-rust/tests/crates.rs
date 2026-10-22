//! The paths into other crates a Rust file names through its `use` declarations and path calls.

use kartta_lang::Language;
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
         \x20   self::h();\n    crate::m::i();\n    Vec::new();\n    std::fs::read();\n}\n",
    );

    assert_eq!(
        got,
        [
            path("tree", "kartta_lang::tree", 1),
            path("kartta_walk", "kartta_walk", 4),
            path("m", "other::m", 5),
            path("sub", "kartta_lang::tree::sub", 6),
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
fn a_kept_path_includes_no_file_its_name_spells() {
    let rust = Rust::new();
    let repository = kartta_lang::MemoryRepository::new([("src/lib.rs", ""), ("src/walk", "")]);

    let extraction = rust
        .resolver(&repository)
        .resolve(rust.extract(b"use other::walk;\n"), "src/lib.rs");

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
