//! How a Rust extraction resolves against its file's path and the repository: the files its `mod` declarations pull
//! in, the crate roots and integration tests that the packages of the repository lay out, and generated file names.

use kartta_lang::{Extraction, Language, MemoryRepository};
use kartta_lang_rust::Rust;

fn resolve(files: &'static [&'static str], source: &[u8], path: &str) -> Extraction {
    let rust = Rust::new();
    let repository = MemoryRepository::of_paths(files);
    let resolver = rust.resolver(&repository);

    resolver.resolve(rust.extract(source), path, &repository)
}

fn includes(files: &'static [&'static str], source: &[u8], path: &str) -> Vec<String> {
    resolve(files, source, path)
        .includes
        .into_iter()
        .map(|include| include.target)
        .collect()
}

fn is_entry(files: &'static [&'static str], path: &str) -> bool {
    let extraction = resolve(files, b"fn main() {}\n", path);
    extraction.symbols[0].flags.is_entry()
}

fn is_test(files: &'static [&'static str], path: &str) -> bool {
    let extraction = resolve(files, b"fn helper() {}\n", path);
    extraction.symbols[0].flags.is_test()
}

const NONE: Vec<String> = Vec::new();

#[test]
fn a_crate_root_or_a_mod_rs_pulls_in_a_sibling_file_or_directory() {
    let sibling = includes(
        &["src/lib.rs", "src/widgets.rs"],
        b"mod widgets;\n",
        "src/lib.rs",
    );
    let directory = includes(
        &["src/lib.rs", "src/widgets/mod.rs"],
        b"mod widgets;\n",
        "src/lib.rs",
    );
    let nested = includes(
        &["src/widgets/mod.rs", "src/widgets/helpers.rs"],
        b"mod helpers;\n",
        "src/widgets/mod.rs",
    );

    assert_eq!(sibling, ["src/widgets.rs"]);
    assert_eq!(directory, ["src/widgets/mod.rs"]);
    assert_eq!(nested, ["src/widgets/helpers.rs"]);
}

#[test]
fn a_plain_module_file_pulls_in_files_under_its_stem() {
    let file = includes(
        &["src/foo.rs", "src/foo/bar.rs"],
        b"mod bar;\n",
        "src/foo.rs",
    );
    let directory = includes(
        &["src/foo.rs", "src/foo/bar/mod.rs"],
        b"mod bar;\n",
        "src/foo.rs",
    );
    let at_root = includes(&["foo.rs", "foo/bar.rs"], b"mod bar;\n", "foo.rs");

    assert_eq!(file, ["src/foo/bar.rs"]);
    assert_eq!(directory, ["src/foo/bar/mod.rs"]);
    assert_eq!(at_root, ["foo/bar.rs"]);
}

#[test]
fn a_mod_inside_an_inline_module_is_one_directory_deeper() {
    let files = &[
        "src/lib.rs",
        "src/inner/child.rs",
        "src/foo.rs",
        "src/foo/inner/child/mod.rs",
        "src/inner/custom.rs",
        "src/foo/inner/custom.rs",
        "src/child.rs",
    ];
    let source = b"mod inner {\n    mod child;\n}\n";
    let overridden = b"mod inner {\n    #[path = \"custom.rs\"]\n    mod child;\n}\n";

    assert_eq!(
        includes(files, source, "src/lib.rs"),
        ["src/inner/child.rs"]
    );
    assert_eq!(
        includes(files, source, "src/foo.rs"),
        ["src/foo/inner/child/mod.rs"]
    );
    assert_eq!(
        includes(files, overridden, "src/lib.rs"),
        ["src/inner/custom.rs"]
    );
    assert_eq!(
        includes(files, overridden, "src/foo.rs"),
        ["src/foo/inner/custom.rs"]
    );
}

#[test]
fn a_crate_root_of_a_package_keeps_its_modules_beside_it() {
    let files = &[
        "Cargo.toml",
        "tests/it.rs",
        "tests/common/mod.rs",
        "src/bin/tool.rs",
        "src/bin/args.rs",
        "build.rs",
        "gen.rs",
    ];

    assert_eq!(
        includes(files, b"mod common;\n", "tests/it.rs"),
        ["tests/common/mod.rs"]
    );
    assert_eq!(
        includes(files, b"mod args;\n", "src/bin/tool.rs"),
        ["src/bin/args.rs"]
    );
    assert_eq!(includes(files, b"mod gen;\n", "build.rs"), ["gen.rs"]);
}

#[test]
fn without_a_package_only_a_lib_main_or_mod_file_keeps_its_modules_beside_it() {
    let files = &["tests/it.rs", "tests/common/mod.rs", "tests/it/common.rs"];

    assert_eq!(
        includes(files, b"mod common;\n", "tests/it.rs"),
        ["tests/it/common.rs"]
    );
}

#[test]
fn a_lib_rs_its_manifest_places_off_the_layout_keeps_its_modules_beside_it() {
    let files = &[
        "Cargo.toml",
        "bindings/rust/lib.rs",
        "bindings/rust/build.rs",
    ];

    assert_eq!(
        includes(files, b"mod build;\n", "bindings/rust/lib.rs"),
        ["bindings/rust/build.rs"]
    );
}

#[test]
fn an_inline_module_or_a_missing_file_adds_no_include() {
    let inline = includes(
        &["src/lib.rs", "src/widgets.rs"],
        b"mod widgets {\n    pub fn f() {}\n}\n",
        "src/lib.rs",
    );
    let missing = includes(&["src/lib.rs"], b"mod absent;\n", "src/lib.rs");

    assert_eq!(inline, NONE);
    assert_eq!(missing, NONE);
}

#[test]
fn a_path_override_wins_relative_to_the_declaring_directory() {
    let files = &[
        "src/lib.rs",
        "src/platform/linux.rs",
        "src/platform.rs",
        "src/shared.rs",
    ];

    assert_eq!(
        includes(
            files,
            b"#[path = \"platform/linux.rs\"]\nmod platform;\n",
            "src/lib.rs"
        ),
        ["src/platform/linux.rs"]
    );
    assert_eq!(
        includes(
            files,
            b"#[path = \"../shared.rs\"]\nmod shared;\n",
            "src/sub/mod.rs"
        ),
        ["src/shared.rs"]
    );
    assert_eq!(
        includes(
            files,
            b"#[path = \"gone.rs\"]\nmod platform;\n",
            "src/lib.rs"
        ),
        NONE
    );
    assert_eq!(
        includes(files, b"#[path = \"../../up.rs\"]\nmod up;\n", "src/lib.rs"),
        NONE
    );
}

#[test]
fn a_path_override_reaches_only_its_own_declaration() {
    let files = &[
        "src/lib.rs",
        "src/custom/first.rs",
        "src/second.rs",
        "src/tests.rs",
    ];

    assert_eq!(
        includes(
            files,
            b"#[path = \"custom/first.rs\"]\nmod first;\nmod second;\n",
            "src/lib.rs"
        ),
        ["src/custom/first.rs", "src/second.rs"]
    );
    assert_eq!(
        includes(files, b"#[cfg(test)]\nmod tests;\n", "src/lib.rs"),
        ["src/tests.rs"]
    );
}

#[test]
fn a_test_gated_mod_or_one_in_a_test_file_includes_test_code() {
    let files = &[
        "Cargo.toml",
        "src/lib.rs",
        "src/tests.rs",
        "tests/it.rs",
        "tests/common/mod.rs",
    ];

    assert_eq!(
        resolve(files, b"#[cfg(test)]\nmod tests;\n", "src/lib.rs").test_includes,
        ["src/tests.rs"]
    );
    assert_eq!(
        resolve(files, b"mod tests;\n", "src/lib.rs")
            .test_includes
            .len(),
        0
    );
    assert_eq!(
        resolve(files, b"mod common;\n", "tests/it.rs").test_includes,
        ["tests/common/mod.rs"]
    );
}

#[test]
fn a_top_level_main_is_the_entry_of_a_crate_root_other_than_the_library() {
    let files = &["Cargo.toml", "crates/app/Cargo.toml"];

    for path in [
        "src/main.rs",
        "crates/app/src/main.rs",
        "src/bin/tool.rs",
        "src/bin/tool/main.rs",
        "examples/demo.rs",
        "examples/demo/main.rs",
        "benches/speed.rs",
        "tests/harnessless.rs",
        "build.rs",
    ] {
        assert!(is_entry(files, path), "{path}");
    }
    for path in [
        "src/lib.rs",
        "src/foo/main.rs",
        "src/bin/tool/helper.rs",
        "tests/common/main.rs/x.rs",
        "crates/app/build/main.rs",
        "crates/other/src/main.rs",
    ] {
        assert!(!is_entry(files, path), "{path}");
    }
}

#[test]
fn a_main_in_a_module_or_without_a_package_is_no_entry() {
    let nested = resolve(
        &["Cargo.toml"],
        b"mod cli {\n    fn main() {}\n}\n",
        "src/main.rs",
    );

    assert!(!nested.symbols[1].flags.is_entry());
    assert!(!is_entry(&[], "src/main.rs"));
}

#[test]
fn a_tests_directory_outside_src_holds_test_code() {
    let files = &[
        "Cargo.toml",
        "crates/foo/Cargo.toml",
        "src/vendor/bar/Cargo.toml",
    ];

    for path in [
        "crates/foo/tests/integration.rs",
        "tests/common/mod.rs",
        "tests/it/main.rs",
        "tests/fixtures/crate/src/lib.rs",
        "src/vendor/bar/tests/smoke.rs",
        "sub/tests/loose.rs",
    ] {
        assert!(is_test(files, path), "{path}");
    }
    for path in [
        "src/tests/helper.rs",
        "crates/foo/src/tests/mod.rs",
        "src/lib.rs",
        "src/widgets/widget.rs",
    ] {
        assert!(!is_test(files, path), "{path}");
    }
}

#[test]
fn a_test_path_keeps_the_flags_the_bytes_gave() {
    let extraction = resolve(
        &["Cargo.toml"],
        b"// @generated\nfn main() {}\n",
        "tests/gen.rs",
    );
    let flags = extraction.symbols[0].flags;

    assert!(flags.is_test() && flags.is_generated() && flags.is_entry());
}

#[test]
fn a_generated_name_marks_every_symbol_generated() {
    for path in ["src/proto.gen.rs", "src/types_generated.rs"] {
        assert!(
            resolve(&[], b"pub fn emitted() {}\n", path).symbols[0]
                .flags
                .is_generated(),
            "{path}"
        );
    }
    assert!(
        !resolve(&[], b"pub fn handwritten() {}\n", "src/lib.rs").symbols[0]
            .flags
            .is_generated()
    );
}

#[test]
fn one_extraction_resolved_at_two_paths_differs_only_by_what_the_paths_show() {
    let rust = Rust::new();
    let files = MemoryRepository::of_paths(["Cargo.toml", "src/util.rs", "src/bin/util.rs"]);
    let resolver = rust.resolver(&files);
    let raw = rust.extract(b"mod util;\nfn main() {}\n");

    let library = resolver.resolve(raw.clone(), "src/lib.rs", &files);
    let binary = resolver.resolve(raw, "src/bin/run.rs", &files);

    assert_eq!(library.includes[0].target, "src/util.rs");
    assert_eq!(binary.includes[0].target, "src/bin/util.rs");
    assert_eq!(library.symbols[1].name, "main");
    assert!(!library.symbols[1].flags.is_entry());
    assert!(binary.symbols[1].flags.is_entry());
    assert_eq!(library.symbols[0], binary.symbols[0]);
}
