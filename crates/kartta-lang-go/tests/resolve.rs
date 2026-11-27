//! How a Go extraction resolves against its file's path and the repository: the package an import names, the files
//! it includes, and the flags a path gives a file's symbols.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{Extraction, Language, MemoryRepository, Symbol};
use kartta_lang_go::Go;

fn app() -> MemoryRepository {
    MemoryRepository::new([
        ("go.mod", "module example.com/app\n\ngo 1.26\n"),
        ("main.go", ""),
        ("lib/cache/cache.go", ""),
        ("lib/cache/keys.go", ""),
        ("ext/go-openai/client.go", ""),
        ("vendor/github.com/pkg/errors/errors.go", ""),
    ])
}

fn resolve(files: &MemoryRepository, source: &[u8], path: &str) -> Extraction {
    let go = Go::new();
    let resolver = go.resolver(files);

    resolver.resolve(go.extract(source), path, files)
}

fn package_dir(files: &MemoryRepository, spec: &str) -> Option<String> {
    let source = format!("package main\n\nimport \"{spec}\"\n");
    let mut extraction = resolve(files, source.as_bytes(), "main.go");

    assert_eq!(extraction.bindings.len(), 1);
    extraction.bindings.remove(0).target_dir
}

fn qualified<'a>(extraction: &'a Extraction, qualified_name: &str) -> &'a Symbol {
    extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("no symbol {qualified_name}"))
}

#[test]
fn an_import_under_the_module_path_names_the_package_directory() {
    assert_eq!(
        package_dir(&app(), "example.com/app/lib/cache").as_deref(),
        Some("lib/cache")
    );
    assert_eq!(package_dir(&app(), "example.com/app").as_deref(), Some("."));
}

#[test]
fn an_import_of_a_module_path_two_modules_declare_names_the_importer_s_own_module() {
    let files = MemoryRepository::new([
        (".copies/old/go.mod", "module example.com/app\n"),
        (".copies/old/lib/cache/cache.go", ""),
        ("go.mod", "module example.com/app\n"),
        ("lib/cache/cache.go", ""),
    ]);
    let source = b"package main\n\nimport \"example.com/app/lib/cache\"\n";

    let root = resolve(&files, source, "main.go");
    let copy = resolve(&files, source, ".copies/old/main.go");

    assert_eq!(root.bindings[0].target_dir.as_deref(), Some("lib/cache"));
    assert_eq!(
        copy.bindings[0].target_dir.as_deref(),
        Some(".copies/old/lib/cache")
    );
}

#[test]
fn a_comment_after_the_module_path_is_not_part_of_it() {
    let files = MemoryRepository::new([
        ("go.mod", "module example.com/app // toolchain note\n"),
        ("lib/cache/cache.go", ""),
    ]);

    assert_eq!(
        package_dir(&files, "example.com/app/lib/cache").as_deref(),
        Some("lib/cache")
    );
}

#[test]
fn the_vendored_copy_of_a_package_wins() {
    assert_eq!(
        package_dir(&app(), "github.com/pkg/errors").as_deref(),
        Some("vendor/github.com/pkg/errors")
    );
}

#[test]
fn a_replace_onto_a_local_directory_names_that_directory() {
    let files = MemoryRepository::new([
        (
            "go.mod",
            "module example.com/app\n\nreplace (\n\tgithub.com/x/openai => ./ext/go-openai\n)\n",
        ),
        ("ext/go-openai/client.go", ""),
    ]);

    assert_eq!(
        package_dir(&files, "github.com/x/openai").as_deref(),
        Some("ext/go-openai")
    );
}

#[test]
fn a_file_of_a_nested_module_takes_its_module_s_vendored_copy() {
    let files = MemoryRepository::new([
        ("go.mod", "module example.com/app\n"),
        ("tools/go.mod", "module example.com/tools\n"),
        ("tools/vendor/github.com/pkg/errors/errors.go", ""),
        ("tools/main.go", ""),
    ]);
    let source = b"package main\n\nimport \"github.com/pkg/errors\"\n";

    let in_tools = resolve(&files, source, "tools/main.go");
    let in_app = resolve(&files, source, "main.go");

    assert_eq!(
        in_tools.bindings[0].target_dir.as_deref(),
        Some("tools/vendor/github.com/pkg/errors")
    );
    assert_eq!(in_app.bindings[0].target_dir, None);
}

#[test]
fn a_replaced_module_without_a_vendor_directory_is_built_by_the_module_that_replaces_it() {
    let files = MemoryRepository::new([
        (
            "go.mod",
            "module example.com/app\n\nreplace (\n\texample.com/lib => ./lib\n\
             \tgithub.com/x/openai => ./ext/openai\n)\n",
        ),
        ("vendor/modules.txt", ""),
        ("vendor/github.com/pkg/errors/errors.go", ""),
        ("ext/openai/client.go", ""),
        ("lib/go.mod", "module example.com/lib\n"),
        ("lib/lib.go", ""),
        ("tools/go.mod", "module example.com/tools\n"),
        ("tools/main.go", ""),
    ]);
    let source =
        b"package lib\n\nimport (\n\t\"github.com/pkg/errors\"\n\t\"github.com/x/openai\"\n)\n";

    let in_lib = resolve(&files, source, "lib/lib.go");
    let in_tools = resolve(&files, source, "tools/main.go");
    let lib_dirs: Vec<Option<&str>> = in_lib
        .bindings
        .iter()
        .map(|binding| binding.target_dir.as_deref())
        .collect();
    let tools_dirs: Vec<Option<&str>> = in_tools
        .bindings
        .iter()
        .map(|binding| binding.target_dir.as_deref())
        .collect();

    assert_eq!(
        lib_dirs,
        [Some("vendor/github.com/pkg/errors"), Some("ext/openai")]
    );
    assert_eq!(tools_dirs, [None, None]);
}

#[test]
fn a_replaced_module_with_its_own_vendor_directory_keeps_it() {
    let files = MemoryRepository::new([
        (
            "go.mod",
            "module example.com/app\n\nreplace example.com/lib => ./lib\n",
        ),
        ("vendor/modules.txt", ""),
        ("vendor/github.com/pkg/errors/errors.go", ""),
        ("lib/go.mod", "module example.com/lib\n"),
        ("lib/vendor/modules.txt", ""),
        ("lib/vendor/github.com/pkg/errors/errors.go", ""),
    ]);
    let source = b"package lib\n\nimport \"github.com/pkg/errors\"\n";

    let in_lib = resolve(&files, source, "lib/lib.go");

    assert_eq!(
        in_lib.bindings[0].target_dir.as_deref(),
        Some("lib/vendor/github.com/pkg/errors")
    );
}

#[test]
fn a_file_sees_the_replaces_of_its_own_module_only() {
    let files = MemoryRepository::new([
        (
            "go.mod",
            "module example.com/app\n\nreplace github.com/x/openai => ./ext/app-openai\n",
        ),
        (
            "tools/go.mod",
            "module example.com/tools\n\nreplace github.com/x/openai => ./openai\n",
        ),
        ("ext/app-openai/client.go", ""),
        ("tools/openai/client.go", ""),
        ("tools/main.go", ""),
    ]);
    let source = b"package main\n\nimport \"github.com/x/openai\"\n";

    let in_app = resolve(&files, source, "main.go");
    let in_tools = resolve(&files, source, "tools/main.go");

    assert_eq!(
        in_app.bindings[0].target_dir.as_deref(),
        Some("ext/app-openai")
    );
    assert_eq!(
        in_tools.bindings[0].target_dir.as_deref(),
        Some("tools/openai")
    );
}

#[test]
fn a_replace_onto_a_directory_above_the_root_names_nothing() {
    let files = MemoryRepository::new([(
        "go.mod",
        "module example.com/app\n\nreplace github.com/x/openai => ../openai\n",
    )]);

    assert_eq!(package_dir(&files, "github.com/x/openai"), None);
}

#[test]
fn a_nested_module_wins_by_the_longest_prefix() {
    let files = MemoryRepository::new([
        ("go.mod", "module example.com/app\n"),
        ("ext/go-openai/go.mod", "module github.com/x/openai\n"),
        ("ext/go-openai/client.go", ""),
    ]);

    assert_eq!(
        package_dir(&files, "github.com/x/openai").as_deref(),
        Some("ext/go-openai")
    );
}

#[test]
fn an_import_of_a_package_of_the_std_module_names_its_directory_without_a_prefix() {
    let files = MemoryRepository::new([
        ("src/go.mod", "module std\n"),
        ("src/strings/builder.go", ""),
        ("src/go/parser/interface.go", ""),
        ("src/net/http/server.go", ""),
        ("src/cmd/go.mod", "module cmd\n"),
        ("src/cmd/cover/cover.go", ""),
        ("src/cmd/internal/obj/link.go", ""),
    ]);
    let source =
        b"package cover\n\nimport (\n\t\"strings\"\n\t\"go/parser\"\n\t\"cmd/internal/obj\"\n)\n";
    let dirs = |path: &str| -> Vec<Option<String>> {
        resolve(&files, source, path)
            .bindings
            .into_iter()
            .map(|binding| binding.target_dir)
            .collect()
    };
    let expected = [
        Some("src/strings".to_owned()),
        Some("src/go/parser".to_owned()),
        Some("src/cmd/internal/obj".to_owned()),
    ];

    assert_eq!(dirs("src/net/http/server.go"), expected);
    assert_eq!(dirs("src/cmd/cover/cover.go"), expected);
}

#[test]
fn a_go_mod_under_vendor_declares_no_module() {
    let files = MemoryRepository::new([
        ("go.mod", "module example.com/app\n"),
        ("vendor/modules/go.mod", "module other.com/lib\n"),
        ("vendor/modules/lib.go", ""),
    ]);

    assert_eq!(package_dir(&files, "other.com/lib"), None);
}

#[test]
fn an_external_or_empty_package_stays_unresolved() {
    assert_eq!(package_dir(&app(), "fmt"), None);
    assert_eq!(package_dir(&app(), "golang.org/x/net/http2"), None);
    assert_eq!(package_dir(&app(), "example.com/app/lib/missing"), None);
    assert_eq!(package_dir(&app(), "example.com/application"), None);
}

#[test]
fn imports_become_bindings_and_package_includes() {
    let files = MemoryRepository::new([
        ("go.mod", "module example.com/app\n"),
        ("lib/cache/cache.go", ""),
        ("lib/cache/keys.go", ""),
        ("lib/cache/cache_test.go", ""),
        ("lib/cache/_skipped.go", ""),
        ("lib/util/util.go", ""),
        ("main.go", ""),
    ]);
    let source = br#"
package main

import (
    "fmt"
    "example.com/app/lib/cache"
    u "example.com/app/lib/util"
    . "example.com/app/lib/util"
    _ "example.com/app/lib/util"
)

func main() {}
"#;

    let extraction = resolve(&files, source, "main.go");

    assert_eq!(extraction.file, "main.go");
    assert_eq!(extraction.package.as_deref(), Some("main"));
    let bindings: Vec<_> = extraction
        .bindings
        .iter()
        .map(|binding| {
            (
                binding.alias.as_deref(),
                binding.target_dir.as_deref(),
                binding.last_segment.as_str(),
            )
        })
        .collect();
    assert_eq!(
        bindings,
        [
            (None, None, "fmt"),
            (None, Some("lib/cache"), "cache"),
            (Some("u"), Some("lib/util"), "util"),
            (Some("."), Some("lib/util"), "util"),
            (Some("_"), Some("lib/util"), "util"),
        ]
    );
    let includes: Vec<&str> = extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect();
    assert_eq!(
        includes,
        [
            "lib/cache/cache.go",
            "lib/cache/keys.go",
            "lib/util/util.go",
            "lib/util/util.go",
            "lib/util/util.go"
        ],
        "a test file is not included, and each import includes its package once"
    );
}

#[test]
fn an_import_with_a_major_version_suffix_binds_the_element_before_it() {
    let files = MemoryRepository::new([("go.mod", "module example.com/app\n"), ("main.go", "")]);
    let source = br#"
package main

import (
    "math/rand/v2"
    "gopkg.in/yaml.v3"
    "example.com/kit/v10"
    "example.com/vendor.version"
)

func main() {}
"#;

    let extraction = resolve(&files, source, "main.go");

    let names: Vec<&str> = extraction
        .bindings
        .iter()
        .map(|binding| binding.last_segment.as_str())
        .collect();
    assert_eq!(names, ["rand", "yaml", "kit", "vendor.version"]);
}

#[test]
fn an_importing_file_does_not_include_itself() {
    let files = MemoryRepository::new([("go.mod", "module m\n"), ("a/a.go", ""), ("a/b.go", "")]);

    let extraction = resolve(&files, b"\npackage a\n\nimport \"m/a\"\n", "a/a.go");

    let includes: Vec<&str> = extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect();
    assert_eq!(includes, ["a/b.go"]);
}

#[test]
fn a_test_file_marks_every_symbol_as_test() {
    let source = b"\npackage sample\n\nfunc Helper() {}\n\nfunc TestThing(t *T) {}\n";

    let test = resolve(&app(), source, "sample_test.go");
    let ordinary = resolve(&app(), source, "sample.go");

    assert!(qualified(&test, "sample.Helper").flags.is_test());
    assert!(qualified(&test, "sample.TestThing").flags.is_test());
    assert!(!qualified(&ordinary, "sample.Helper").flags.is_test());
}

#[test]
fn a_file_under_a_vendor_directory_marks_every_symbol_as_vendored() {
    let source = b"\npackage sample\n\nfunc Helper() {}\n\nfunc Other() {}\n";

    for path in ["vendor/x/sample.go", "app/vendor/x/sample.go"] {
        let vendored = resolve(&app(), source, path);
        for name in ["sample.Helper", "sample.Other"] {
            assert!(qualified(&vendored, name).flags.is_vendored(), "{path}");
        }
    }
    for path in ["sample.go", "vendors/sample.go", "app/myvendor/sample.go"] {
        let ordinary = resolve(&app(), source, path);
        assert!(
            !qualified(&ordinary, "sample.Helper").flags.is_vendored(),
            "{path}"
        );
    }
}

#[test]
fn a_pb_go_file_is_generated_without_the_banner() {
    let source = b"\npackage sample\n\nfunc FromProto() {}\n";

    let protobuf = resolve(&app(), source, "sample.pb.go");
    let ordinary = resolve(&app(), source, "sample.go");

    assert!(
        qualified(&protobuf, "sample.FromProto")
            .flags
            .is_generated()
    );
    assert!(
        !qualified(&ordinary, "sample.FromProto")
            .flags
            .is_generated()
    );
}

#[test]
fn one_extraction_resolved_at_two_paths_differs_only_by_what_the_paths_show() {
    let go = Go::new();
    let app = app();
    let resolver = go.resolver(&app);
    let raw =
        go.extract(b"package app\n\nimport \"example.com/app/lib/cache\"\n\nfunc Handle() {}\n");

    let mut handler = resolver.resolve(raw.clone(), "app/handler.go", &app);
    let test = resolver.resolve(raw, "app/handler_test.go", &app);

    assert_eq!(handler.file, "app/handler.go");
    assert_eq!(test.file, "app/handler_test.go");
    assert!(!handler.symbols[0].flags.is_test());
    assert!(test.symbols[0].flags.is_test());
    handler.file.clone_from(&test.file);
    handler.symbols[0].flags = test.symbols[0].flags;
    assert_eq!(handler, test);
}

#[test]
fn a_resolver_answers_from_the_repository_it_is_handed_not_the_one_it_was_built_over() {
    let go = Go::new();
    let built_over = app();
    let resolver = go.resolver(&built_over);
    let grown = app().with_paths(["lib/store/store.go"]);
    let raw = go.extract(b"package main\n\nimport \"example.com/app/lib/store\"\n");

    let before = resolver.resolve(raw.clone(), "main.go", &built_over);
    let after = resolver.resolve(raw, "main.go", &grown);

    assert_eq!(before.bindings[0].target_dir, None);
    assert_eq!(before.includes, []);
    assert_eq!(after.bindings[0].target_dir.as_deref(), Some("lib/store"));
    assert_eq!(after.includes[0].target, "lib/store/store.go");
}

const GO_MOD: &str = "module example.com/app\n\ngo 1.26\n\nrequire github.com/pkg/errors v0.9.0\n";

/// Whether the resolver built over a module whose `go.mod` reads `edited`, with the paths `more` beside, resolves as
/// the one built over the same module with [`GO_MOD`].
fn resolves_as_before(edited: &str, more: &[&str]) -> bool {
    let go = Go::new();
    let before = MemoryRepository::new([("go.mod", GO_MOD), ("main.go", "")]);
    let after = MemoryRepository::new([("go.mod", edited), ("main.go", "")])
        .with_paths(more.iter().copied());

    go.resolver(&after).resolves_as(&*go.resolver(&before))
}

#[test]
fn a_go_mod_that_only_bumps_a_requirement_resolves_as_before() {
    let bumped = GO_MOD.replace("v0.9.0", "v0.9.1");

    assert!(resolves_as_before(&bumped, &[]));
}

#[test]
fn a_vendored_go_mod_that_appears_resolves_as_before() {
    assert!(resolves_as_before(
        GO_MOD,
        &["vendor/github.com/pkg/errors/go.mod"]
    ));
}

#[test]
fn a_changed_module_line_an_added_local_replace_or_a_vendor_list_resolves_otherwise() {
    let renamed = GO_MOD.replace("example.com/app", "example.com/other");
    let replaced = format!("{GO_MOD}\nreplace github.com/pkg/errors => ./third_party/errors\n");

    assert!(!resolves_as_before(&renamed, &[]));
    assert!(!resolves_as_before(&replaced, &[]));
    assert!(!resolves_as_before(GO_MOD, &["vendor/modules.txt"]));
}
