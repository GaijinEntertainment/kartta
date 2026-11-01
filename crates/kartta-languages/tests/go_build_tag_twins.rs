//! What a Go read reaches where build constraints give two files of a package one name: every declaration, whichever
//! file a build selects and whatever path the files hold, and only the caller's own where its file declares the name.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Selector, SiteQuery};
use tempfile::TempDir;

const OVER_AN_IMPORT: &str = "//go:build amd64

package dec

import \"net/http\"

func New() *http.Client { return nil }
";

const OVER_A_HELD_TYPE: &str = "//go:build !amd64

package dec

type Dec struct{}

func (Dec) Set() {}

func New() *Dec { return nil }
";

const READER: &str = "package m

import \"x/dec\"

func run() {
	d := dec.New()
	d.Set()
}
";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("go.mod"), "module x\n\ngo 1.22\n").expect("the file is written");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["go"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// What `caller`, in `file`, binds: each callee's qualified name and file's name, sorted, so a renamed file reads by
/// the role a test gave it.
fn callees(engine: &Engine, file: &str, caller: &str) -> Vec<(String, String)> {
    let (_, graph) = engine.graph_of(file).expect("Go claims the file");
    let walk = graph
        .callees(caller, Limits::new(1))
        .expect("the caller is held");
    let mut bound: Vec<(String, String)> = (walk.reached.iter())
        .map(|reached| {
            (
                reached.symbol.qualified_name.to_owned(),
                reached.symbol.file.to_owned(),
            )
        })
        .collect();
    bound.sort();
    bound
}

fn pair(qualified: &str, file: &str) -> (String, String) {
    (qualified.to_owned(), file.to_owned())
}

#[test]
fn a_read_from_another_package_answers_alike_whichever_file_comes_first_in_path_order() {
    for first in ["dec/a.go", "dec/z.go"] {
        let (_tree, engine) = build(&[
            (first, OVER_AN_IMPORT),
            ("dec/b.go", OVER_A_HELD_TYPE),
            ("m.go", READER),
        ]);

        let mut expected = [
            pair("dec.Dec.Set", "dec/b.go"),
            pair("dec.New", "dec/b.go"),
            pair("dec.New", first),
        ];
        expected.sort();
        assert_eq!(
            callees(&engine, "m.go", "run"),
            expected,
            "with the import's file at {first}"
        );
    }
}

#[test]
fn a_read_from_a_third_file_of_the_package_reaches_every_declaration() {
    let reader = "package dec\n\nfunc run() {\n\td := New()\n\td.Set()\n}\n";
    for first in ["dec/a.go", "dec/z.go"] {
        let (_tree, engine) = build(&[
            (first, OVER_AN_IMPORT),
            ("dec/b.go", OVER_A_HELD_TYPE),
            ("dec/c.go", reader),
        ]);

        let mut expected = [
            pair("dec.Dec.Set", "dec/b.go"),
            pair("dec.New", "dec/b.go"),
            pair("dec.New", first),
        ];
        expected.sort();
        assert_eq!(callees(&engine, "dec/c.go", "dec.run"), expected);
    }
}

#[test]
fn a_method_two_files_declare_binds_both_from_another_file_and_its_own_from_one_of_them() {
    let one = "//go:build linux

package dec

type Dec struct{}

func (Dec) Set() {}

func (d Dec) Run() {
	d.Set()
}
";
    let other = one.replace("linux", "!linux");
    let reader = "package m\n\nimport \"x/dec\"\n\nfunc run(d dec.Dec) {\n\td.Set()\n}\n";
    let (_tree, engine) = build(&[
        ("dec/a.go", one),
        ("dec/b.go", other.as_str()),
        ("m.go", reader),
    ]);

    assert_eq!(
        callees(&engine, "m.go", "run"),
        [
            pair("dec.Dec.Set", "dec/a.go"),
            pair("dec.Dec.Set", "dec/b.go"),
        ]
    );
    let own = Selector::new("dec.Dec.Run", Some("dec/b.go"));
    let (_, graph) = engine.graph_of("m.go").expect("Go claims the file");
    let walk = graph
        .callees(own, Limits::new(1))
        .expect("the caller is held");
    let bound: Vec<&str> = (walk.reached.iter())
        .map(|reached| reached.symbol.file)
        .collect();
    assert_eq!(bound, ["dec/b.go"], "a file's own method stands alone");
}

#[test]
fn a_caller_a_file_name_constrains_binds_no_declaration_a_build_never_compiles_with_it() {
    let declaring = "package dec\n\nfunc New() {}\n";
    let reader = "package m\n\nimport \"x/dec\"\n\nfunc run() {\n\tdec.New()\n}\n";
    let (_tree, engine) = build(&[
        ("dec/a_darwin.go", declaring),
        ("dec/b_linux_amd64.go", declaring),
        ("dec/c_linux_arm64.go", declaring),
        ("dec/d_linux.go", declaring),
        ("dec/e.go", declaring),
        ("m_linux_amd64.go", reader),
    ]);

    assert_eq!(
        callees(&engine, "m_linux_amd64.go", "run"),
        [
            pair("dec.New", "dec/b_linux_amd64.go"),
            pair("dec.New", "dec/d_linux.go"),
            pair("dec.New", "dec/e.go"),
        ]
    );
}

#[test]
fn an_alias_each_file_points_elsewhere_reads_the_type_the_graph_holds() {
    let over_a_held_type = "//go:build !tinygo

package dec

import \"x/held\"

type Dec = held.Dec

func New() *Dec { return nil }
";
    let over_an_import = "//go:build tinygo

package dec

import \"net/http\"

type Dec = http.Client

func New() *Dec { return nil }
";
    let held = "package held\n\ntype Dec struct{}\n\nfunc (Dec) Set() {}\n";
    for (first, second) in [("dec/a.go", "dec/b.go"), ("dec/b.go", "dec/a.go")] {
        let (_tree, engine) = build(&[
            (first, over_a_held_type),
            (second, over_an_import),
            ("held/held.go", held),
            ("m.go", READER),
        ]);

        let bound = callees(&engine, "m.go", "run");
        assert!(
            bound.contains(&pair("held.Dec.Set", "held/held.go")),
            "with the held alias at {first}: {bound:?}"
        );
    }
}

#[test]
fn two_held_types_read_alike_whichever_file_comes_first_in_path_order() {
    let one = "//go:build amd64\n\npackage dec\n\ntype One struct{}\n\nfunc (One) Set() {}\n\nfunc New() *One { return nil }\n";
    let other = "//go:build !amd64\n\npackage dec\n\ntype Two struct{}\n\nfunc (Two) Set() {}\n\nfunc New() *Two { return nil }\n";
    let bound_with = |first: &str, second: &str| {
        let (_tree, engine) = build(&[(first, one), (second, other), ("m.go", READER)]);
        let bound = callees(&engine, "m.go", "run");
        bound
            .into_iter()
            .map(|(qualified, _)| qualified)
            .collect::<Vec<_>>()
    };

    let bound = bound_with("dec/a.go", "dec/b.go");
    assert_eq!(
        bound,
        ["dec.New", "dec.New", "dec.Two.Set"],
        "the greater qualified name stands"
    );
    assert_eq!(bound, bound_with("dec/b.go", "dec/a.go"));
}

#[test]
fn a_declaration_of_another_kind_in_another_build_is_a_declaration_of_the_name() {
    let (_tree, engine) = build(&[
        ("dec/a_zos.go", "package dec\n\nvar New = func() {}\n"),
        ("dec/z_linux.go", "package dec\n\nfunc New() {}\n"),
        (
            "dec/m_linux.go",
            "package dec\n\nfunc run() {\n\tNew()\n}\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "dec/m_linux.go", "run"),
        [pair("dec.New", "dec/z_linux.go")]
    );
}

#[test]
fn a_name_no_build_compiles_with_the_caller_binds_nothing() {
    let (_tree, engine) = build(&[
        ("dec/a_linux.go", "package dec\n\nfunc New() {}\n"),
        (
            "dec/m_windows.go",
            "package dec\n\nfunc run() {\n\tNew()\n}\n",
        ),
    ]);

    assert_eq!(callees(&engine, "dec/m_windows.go", "run"), []);
}

#[test]
fn a_caller_a_header_constrains_binds_no_declaration_a_build_never_compiles_with_it() {
    let declaring = |constraint: &str| format!("{constraint}\n\npackage dec\n\nfunc New() {{}}\n");
    let reader = "//go:build linux && !purego\n\npackage m\n\nimport \"x/dec\"\n\nfunc run() {\n\tdec.New()\n}\n";
    let not_windows = declaring("//go:build !windows");
    let linux_amd64 = declaring("// +build amd64,linux");
    let (_tree, engine) = build(&[
        ("dec/a.go", &declaring("//go:build windows")),
        ("dec/b.go", &declaring("// +build darwin")),
        ("dec/c.go", &declaring("//go:build purego")),
        ("dec/d.go", &not_windows),
        ("dec/e_amd64.go", &linux_amd64),
        ("dec/f_arm64.go", &linux_amd64),
        ("dec/g_windows.go", &not_windows),
        ("m.go", reader),
    ]);

    assert_eq!(
        callees(&engine, "m.go", "run"),
        [
            pair("dec.New", "dec/d.go"),
            pair("dec.New", "dec/e_amd64.go")
        ]
    );
}

/// The lines of the file `reader` at which the declaration in `file` of `name` is used.
fn used_at(engine: &Engine, name: &str, file: &str, reader: &str) -> Vec<u32> {
    let (_, graph) = engine.graph_of(file).expect("Go claims the file");
    let selector = Selector::new(name, Some(file));
    let references = graph
        .references(selector, SiteQuery::new(100))
        .expect("the declaration is held");
    assert_eq!(references.symbol.file, file);
    (references.sites.iter())
        .filter(|site| site.site_of.file == reader)
        .map(|site| site.line)
        .collect()
}

const NO_LINE: [u32; 0] = [];

const DEC_OF_LINUX: &str = "//go:build linux\n\npackage dec\n\ntype Dec struct{}\n";

const DEC_OF_WINDOWS: &str = "//go:build windows\n\npackage dec\n\ntype Dec struct{}\n";

#[test]
fn a_use_of_a_name_two_files_declare_reaches_both_from_a_file_either_builds_with() {
    let reader = "package m\n\nimport \"x/dec\"\n\nvar held dec.Dec\n";
    let beside = "package dec\n\nvar held Dec\n";
    let (_tree, engine) = build(&[
        ("dec/a.go", DEC_OF_LINUX),
        ("dec/b.go", DEC_OF_WINDOWS),
        ("dec/c.go", beside),
        ("m.go", reader),
    ]);

    for file in ["dec/a.go", "dec/b.go"] {
        assert_eq!(used_at(&engine, "dec.Dec", file, "m.go"), [5], "{file}");
        assert_eq!(used_at(&engine, "dec.Dec", file, "dec/c.go"), [3], "{file}");
    }
}

#[test]
fn a_use_reaches_no_declaration_a_build_never_compiles_with_it_and_stands_on_its_file_s_own() {
    let reader = "//go:build windows\n\npackage m\n\nimport \"x/dec\"\n\nvar held dec.Dec\n";
    let beside = "//go:build windows\n\npackage dec\n\nvar held Dec\n";
    let declaring = "//go:build windows\n\npackage dec\n\ntype Dec struct{}\n\nvar own Dec\n";
    let (_tree, engine) = build(&[
        ("dec/a.go", DEC_OF_LINUX),
        ("dec/b.go", declaring),
        ("dec/c.go", beside),
        ("dec/d.go", "package dec\n\ntype Dec struct{}\n"),
        ("m.go", reader),
    ]);

    assert_eq!(used_at(&engine, "dec.Dec", "dec/a.go", "m.go"), NO_LINE);
    assert_eq!(used_at(&engine, "dec.Dec", "dec/a.go", "dec/c.go"), NO_LINE);
    assert_eq!(used_at(&engine, "dec.Dec", "dec/a.go", "dec/b.go"), NO_LINE);
    assert_eq!(used_at(&engine, "dec.Dec", "dec/b.go", "m.go"), [7]);
    assert_eq!(used_at(&engine, "dec.Dec", "dec/b.go", "dec/b.go"), [7]);
    assert_eq!(used_at(&engine, "dec.Dec", "dec/d.go", "dec/b.go"), NO_LINE);
}

#[test]
fn a_use_no_declaration_is_built_with_binds_nothing() {
    let reader = "//go:build windows\n\npackage m\n\nimport \"x/dec\"\n\nvar held dec.Dec\n";
    let (_tree, engine) = build(&[("dec/a.go", DEC_OF_LINUX), ("m.go", reader)]);

    assert_eq!(used_at(&engine, "dec.Dec", "dec/a.go", "m.go"), NO_LINE);
}

#[test]
fn a_use_outside_test_code_reaches_no_declaration_of_a_test_file() {
    let reader = "package m\n\nimport \"x/dec\"\n\nvar held dec.Dec\n";
    let (_tree, engine) = build(&[
        ("dec/a.go", "package dec\n\ntype Dec struct{}\n"),
        ("dec/b_test.go", "package dec\n\ntype Dec struct{}\n"),
        ("m.go", reader),
    ]);

    assert_eq!(used_at(&engine, "dec.Dec", "dec/a.go", "m.go"), [5]);
    assert_eq!(
        used_at(&engine, "dec.Dec", "dec/b_test.go", "m.go"),
        NO_LINE
    );
}

#[test]
fn a_file_the_go_command_skips_declares_nothing() {
    let declaring = "package dec\n\nfunc New() {}\n";
    let reader = "package m\n\nimport \"x/dec\"\n\nfunc run() {\n\tdec.New()\n}\n";
    let (_tree, engine) = build(&[
        ("dec/_a.go", declaring),
        ("dec/.b.go", declaring),
        ("dec/c.go", declaring),
        ("m.go", reader),
    ]);

    assert_eq!(
        callees(&engine, "m.go", "run"),
        [pair("dec.New", "dec/c.go")]
    );
    assert!(engine.graph_of("dec/_a.go").is_none());
}

#[test]
fn the_methods_of_two_types_of_one_package_are_no_declarations_of_one_name() {
    let one = "package dec\n\ntype One struct{}\n\nfunc (One) Set() {}\n";
    let other = "package dec\n\ntype Two struct{}\n\nfunc (Two) Set() {}\n";
    let reader = "package m\n\nimport \"x/dec\"\n\nfunc run(d dec.One) {\n\td.Set()\n}\n";
    let (_tree, engine) = build(&[("dec/a.go", one), ("dec/b.go", other), ("m.go", reader)]);

    assert_eq!(
        callees(&engine, "m.go", "run"),
        [pair("dec.One.Set", "dec/a.go")]
    );
}

#[test]
fn a_namesake_package_s_declaration_stands_beside_none_of_the_caller_s_own() {
    let own = "package dec\n\nimport other \"x/far/dec\"\n\nfunc New() {}\n\nfunc run() {\n\tother.New()\n}\n";
    let far = "package dec\n\nfunc New() {}\n";
    let (_tree, engine) = build(&[("near/dec/a.go", own), ("far/dec/b.go", far)]);

    assert_eq!(
        callees(&engine, "near/dec/a.go", "dec.run"),
        [pair("dec.New", "far/dec/b.go")]
    );
}

#[test]
fn a_test_file_s_declaration_stands_beside_none_for_a_caller_outside_test_code() {
    let declaring = "package dec\n\nfunc New() {}\n";
    let reader = "package m\n\nimport \"x/dec\"\n\nfunc run() {\n\tdec.New()\n}\n";
    let (_tree, engine) = build(&[
        ("dec/a.go", declaring),
        ("dec/b_test.go", declaring),
        ("m.go", reader),
    ]);

    assert_eq!(
        callees(&engine, "m.go", "run"),
        [pair("dec.New", "dec/a.go")]
    );
}
