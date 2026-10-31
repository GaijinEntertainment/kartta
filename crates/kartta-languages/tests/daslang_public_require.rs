//! What a daslang `require x public` lets a name reach: a file that requires its file sees `x` as well, at any depth
//! of public requires, so a name binds through it where a plain `require` of the same file binds nothing.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
use tempfile::TempDir;

const QUOTING: (&str, &str) = (
    "boost.das",
    "options gen2\nmodule boost shared\n\ndef visit {\n    return <- qmacro(helper())\n}\n",
);
const HELPER: &str = "options gen2\n\ndef helper {\n    return 1\n}\n";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["daslang"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// Each site of `helper` as `path` defines it: its holder and confidence.
fn sites<'e>(engine: &'e Engine, path: &str) -> Vec<(&'e str, u8)> {
    let (_, graph) = engine.graph_of(path).expect("daslang claims the file");
    let selector = kartta::Selector::new("helper", Some(path));
    let references = graph
        .references(selector, SiteQuery::new(usize::MAX))
        .expect("the file defines the name");
    references
        .sites
        .iter()
        .map(|site| (site.site_of.name, site.confidence))
        .collect()
}

#[test]
fn a_quoted_name_binds_through_a_public_require_of_a_file_the_expanding_file_requires() {
    let (_tree, engine) = build(&[
        QUOTING,
        (
            "client.das",
            "options gen2\nrequire boost\nrequire facade\n",
        ),
        ("facade.das", "options gen2\nrequire lib public\n"),
        ("lib.das", HELPER),
    ]);

    assert_eq!(sites(&engine, "lib.das"), [("visit", 60)]);
}

#[test]
fn a_quoted_name_binds_through_a_chain_of_public_requires() {
    let (_tree, engine) = build(&[
        QUOTING,
        (
            "client.das",
            "options gen2\nrequire boost\nrequire facade\n",
        ),
        ("facade.das", "options gen2\nrequire middle public\n"),
        ("middle.das", "options gen2\nrequire lib public\n"),
        ("lib.das", HELPER),
    ]);

    assert_eq!(sites(&engine, "lib.das"), [("visit", 60)]);
}

#[test]
fn a_require_that_is_not_public_stops_a_quoted_name() {
    let (_tree, engine) = build(&[
        QUOTING,
        (
            "client.das",
            "options gen2\nrequire boost\nrequire facade\n",
        ),
        ("facade.das", "options gen2\nrequire lib\n"),
        ("lib.das", HELPER),
    ]);

    assert_eq!(sites(&engine, "lib.das"), []);
}

#[test]
fn two_definitions_past_two_required_files_bind_a_quoted_name_to_neither() {
    let (_tree, engine) = build(&[
        QUOTING,
        (
            "client.das",
            "options gen2\nrequire boost\nrequire one\nrequire two\n",
        ),
        ("one.das", "options gen2\nrequire a/lib public\n"),
        ("two.das", "options gen2\nrequire b/lib public\n"),
        ("a/lib.das", HELPER),
        ("b/lib.das", HELPER),
    ]);

    assert_eq!(
        (sites(&engine, "a/lib.das"), sites(&engine, "b/lib.das")),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn a_quoted_name_the_module_seen_everywhere_declares_binds_there_over_a_namesake_an_expanding_file_reaches()
 {
    let (_tree, engine) = build(&[
        QUOTING,
        ("client.das", "options gen2\nrequire boost\nrequire lib\n"),
        ("lib.das", HELPER),
        (
            "daslib/builtin.das",
            "options gen2\nmodule builtin shared public !inscope\n\ndef helper {\n    return 2\n}\n",
        ),
    ]);

    assert_eq!(
        (
            sites(&engine, "daslib/builtin.das"),
            sites(&engine, "lib.das")
        ),
        (vec![("visit", 70)], Vec::new())
    );
}

#[test]
fn a_name_the_macro_s_own_file_requires_binds_there_over_the_module_seen_everywhere() {
    let quoting = "options gen2\nmodule boost shared\nrequire lib\n\ndef visit {\n    return <- qmacro(helper())\n}\n";
    let (_tree, engine) = build(&[
        ("boost.das", quoting),
        ("lib.das", HELPER),
        (
            "daslib/builtin.das",
            "options gen2\nmodule builtin shared public !inscope\n\ndef helper {\n    return 2\n}\n",
        ),
    ]);

    assert_eq!(
        (
            sites(&engine, "daslib/builtin.das"),
            sites(&engine, "lib.das")
        ),
        (Vec::new(), vec![("visit", 70)])
    );
}

#[test]
fn a_quoted_name_the_runtime_answers_first_binds_nothing_past_its_own_file() {
    let source = |use_require: &str| {
        format!(
            "options gen2\nmodule boost shared\n{use_require}\n\ndef visit {{\n    return <- qmacro(length(1))\n}}\n"
        )
    };
    let library = "options gen2\n\ndef length(a) {\n    return 1\n}\n";
    let builtin =
        "options gen2\nmodule builtin shared public !inscope\n\ndef length(a) {\n    return 2\n}\n";
    let length_sites = |boost: &str, client: &str| {
        let (_tree, engine) = build(&[
            ("boost.das", boost),
            ("client.das", client),
            ("lib.das", library),
            ("daslib/builtin.das", builtin),
        ]);
        let (_, graph) = engine.graph_of("lib.das").expect("daslang claims the file");
        let site_count = |path| {
            graph
                .references(
                    kartta::Selector::new("length", Some(path)),
                    SiteQuery::new(usize::MAX),
                )
                .expect("the file defines the name")
                .sites
                .len()
        };
        (site_count("lib.das"), site_count("daslib/builtin.das"))
    };

    assert_eq!(
        length_sites(&source(""), "options gen2\nrequire boost\nrequire lib\n"),
        (0, 0)
    );
    assert_eq!(
        length_sites(&source("require lib"), "options gen2\nrequire boost\n"),
        (1, 0)
    );
}

#[test]
fn a_public_require_reaches_no_call_past_the_file_that_writes_it() {
    let (_tree, engine) = build(&[
        (
            "client.das",
            "options gen2\nrequire facade\n\ndef run {\n    helper()\n}\n",
        ),
        ("facade.das", "options gen2\nrequire a/lib public\n"),
        ("a/lib.das", HELPER),
        ("b/lib.das", HELPER),
    ]);

    assert_eq!(sites(&engine, "a/lib.das"), []);
}
