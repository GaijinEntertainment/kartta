//! How a Go extraction resolves against its file's path and the repository: the package an import names, the files
//! it includes, and the flags a path gives a file's symbols.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use std::io;

use kartta_lang::path::{base, dir};
use kartta_lang::{Extraction, Language, Repository, Symbol};
use kartta_lang_go::Go;

struct Files(&'static [(&'static str, &'static str)]);

impl Repository for Files {
    fn contains(&self, path: &str) -> bool {
        self.0.iter().any(|&(file, _)| file == path)
    }

    fn files_in_dir<'a>(&'a self, in_dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let in_dir = in_dir.to_owned();
        Box::new(
            self.0
                .iter()
                .map(|&(file, _)| file)
                .filter(move |file| dir(file) == in_dir)
                .map(str::to_owned),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        Box::new(
            self.0
                .iter()
                .map(|&(file, _)| file)
                .filter(move |file| base(file) == name)
                .map(str::to_owned),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.0
            .iter()
            .find(|&&(file, _)| file == path)
            .map(|&(_, contents)| contents.as_bytes().to_vec())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

const APP: Files = Files(&[
    ("go.mod", "module example.com/app\n\ngo 1.26\n"),
    ("main.go", ""),
    ("lib/cache/cache.go", ""),
    ("lib/cache/keys.go", ""),
    ("ext/go-openai/client.go", ""),
    ("vendor/github.com/pkg/errors/errors.go", ""),
]);

fn resolve(files: &Files, source: &[u8], path: &str) -> Extraction {
    let go = Go::new();
    let resolver = go.resolver(files);

    resolver.resolve(go.extract(source), path)
}

fn package_dir(files: &Files, spec: &str) -> Option<String> {
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
        package_dir(&APP, "example.com/app/lib/cache").as_deref(),
        Some("lib/cache")
    );
    assert_eq!(package_dir(&APP, "example.com/app").as_deref(), Some("."));
}

#[test]
fn a_comment_after_the_module_path_is_not_part_of_it() {
    let files = Files(&[
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
        package_dir(&APP, "github.com/pkg/errors").as_deref(),
        Some("vendor/github.com/pkg/errors")
    );
}

#[test]
fn a_replace_onto_a_local_directory_names_that_directory() {
    let files = Files(&[
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
    let files = Files(&[
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
fn a_file_sees_the_replaces_of_its_own_module_only() {
    let files = Files(&[
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
    let files = Files(&[(
        "go.mod",
        "module example.com/app\n\nreplace github.com/x/openai => ../openai\n",
    )]);

    assert_eq!(package_dir(&files, "github.com/x/openai"), None);
}

#[test]
fn a_nested_module_wins_by_the_longest_prefix() {
    let files = Files(&[
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
fn a_go_mod_under_vendor_declares_no_module() {
    let files = Files(&[
        ("go.mod", "module example.com/app\n"),
        ("vendor/modules/go.mod", "module other.com/lib\n"),
        ("vendor/modules/lib.go", ""),
    ]);

    assert_eq!(package_dir(&files, "other.com/lib"), None);
}

#[test]
fn an_external_or_empty_package_stays_unresolved() {
    assert_eq!(package_dir(&APP, "fmt"), None);
    assert_eq!(package_dir(&APP, "golang.org/x/net/http2"), None);
    assert_eq!(package_dir(&APP, "example.com/app/lib/missing"), None);
    assert_eq!(package_dir(&APP, "example.com/application"), None);
}

#[test]
fn imports_become_bindings_and_package_includes() {
    let files = Files(&[
        ("go.mod", "module example.com/app\n"),
        ("lib/cache/cache.go", ""),
        ("lib/cache/keys.go", ""),
        ("lib/cache/cache_test.go", ""),
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
fn an_importing_file_does_not_include_itself() {
    let files = Files(&[("go.mod", "module m\n"), ("a/a.go", ""), ("a/b.go", "")]);

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

    let test = resolve(&APP, source, "sample_test.go");
    let ordinary = resolve(&APP, source, "sample.go");

    assert!(qualified(&test, "sample.Helper").flags.is_test());
    assert!(qualified(&test, "sample.TestThing").flags.is_test());
    assert!(!qualified(&ordinary, "sample.Helper").flags.is_test());
}

#[test]
fn a_pb_go_file_is_generated_without_the_banner() {
    let source = b"\npackage sample\n\nfunc FromProto() {}\n";

    let protobuf = resolve(&APP, source, "sample.pb.go");
    let ordinary = resolve(&APP, source, "sample.go");

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
    let resolver = go.resolver(&APP);
    let raw =
        go.extract(b"package app\n\nimport \"example.com/app/lib/cache\"\n\nfunc Handle() {}\n");

    let mut handler = resolver.resolve(raw.clone(), "app/handler.go");
    let test = resolver.resolve(raw, "app/handler_test.go");

    assert_eq!(handler.file, "app/handler.go");
    assert_eq!(test.file, "app/handler_test.go");
    assert!(!handler.symbols[0].flags.is_test());
    assert!(test.symbols[0].flags.is_test());
    handler.file.clone_from(&test.file);
    handler.symbols[0].flags = test.symbols[0].flags;
    assert_eq!(handler, test);
}
