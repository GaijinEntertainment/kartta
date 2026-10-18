//! What a refreshed engine answers: every count and answer a build of the edited tree gives, whether the engine
//! followed the change through what its files asked of the tree, walked only the named paths, or built every graph
//! again.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build or edit is the assertion"
)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::Duration;

use kartta::{BuildOptions, Engine, EngineRefresh, Limits, Widened};
use tempfile::TempDir;

const TREE: &[(&str, &str)] = &[
    ("go.mod", "module example.com/m\n"),
    (
        "lib/lib.go",
        "package lib\n\nfunc Helper() int { return 1 }\n",
    ),
    (
        "main.go",
        "package main\n\nimport \"example.com/m/lib\"\n\nfunc main() {\n\tlib.Helper()\n\tlib.Extra()\n}\n",
    ),
    (
        "src/a.cpp",
        "#include \"util.h\"\n#include \"later.h\"\n\nvoid run() {\n  util();\n  later();\n  extra();\n}\n",
    ),
    ("src/util.h", "void util();\n"),
    ("src/util.cpp", "#include \"util.h\"\n\nvoid util() {}\n"),
    ("src/old.cpp", "void old() {}\n"),
    (
        "src/gone.cpp",
        "#include \"util.h\"\n\nvoid gone() {\n  util();\n}\n",
    ),
    (
        "web/app.ts",
        "import { button } from \"./button\";\n\nexport function render() {\n  return button();\n}\n",
    ),
    (
        "web/button.ts",
        "export function button() {\n  return 1;\n}\n",
    ),
];

/// Each edit a refresh must follow, and the paths a watcher would name for it.
const EDITED: &[&str] = &[
    "src/util.cpp",
    "src/later.h",
    "src/gone.cpp",
    "src/old.cpp",
    "src/renamed.cpp",
    "src/util.h",
    "src/extra.h",
    "lib/extra.go",
    "web/button.ts",
];

fn write(root: &Path, path: &str, content: &str) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create the parent directory");
    }
    fs::write(full, content).expect("write the file");
}

// constraint: a stamp is trusted only when its file last changed over two seconds before the walk began, so a tree
// whose files a refresh is to carry over waits that out before it is built
fn settled_tree() -> TempDir {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, content) in TREE {
        write(tree.path(), path, content);
    }
    std::thread::sleep(Duration::from_millis(2100));
    tree
}

fn edit(root: &Path) {
    write(
        root,
        "src/util.cpp",
        "#include \"util.h\"\n\nvoid util() {}\n\nvoid twice() {\n  util();\n}\n",
    );
    write(root, "src/later.h", "void later();\n");
    fs::remove_file(root.join("src/gone.cpp")).expect("remove gone.cpp");
    fs::rename(root.join("src/old.cpp"), root.join("src/renamed.cpp")).expect("rename old.cpp");
    write(root, "src/util.h", "#include \"extra.h\"\n\nvoid util();\n");
    write(root, "src/extra.h", "void extra();\n");
    write(
        root,
        "lib/extra.go",
        "package lib\n\nfunc Extra() int { return 2 }\n",
    );
    write(
        root,
        "web/button.ts",
        "export function button() {\n  return render();\n}\n\nexport function render() {\n  return 2;\n}\n",
    );
}

fn build(root: &Path, stamps: bool) -> Engine {
    let mut options = BuildOptions::default();
    options.stamps = stamps;
    let outcome =
        Engine::build(root, &options, kartta_languages::all(), None).expect("the tree builds");
    assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    outcome.into_engine()
}

/// Every count the engine keeps and an answer to each query a change can move, written out to compare.
fn answers(engine: &Engine) -> String {
    let mut answers = format!(
        "{:#?}\n{:#?}\n",
        engine.stats(),
        engine.resolution(usize::MAX)
    );
    for (anchor, name) in [
        ("src/a.cpp", "util"),
        ("src/a.cpp", "later"),
        ("src/a.cpp", "extra"),
        ("main.go", "Helper"),
        ("main.go", "Extra"),
        ("web/app.ts", "button"),
        ("web/app.ts", "render"),
    ] {
        let graph = engine
            .graph_of(anchor)
            .expect("a language claims the anchor");
        writeln!(
            answers,
            "callers of {name}: {:#?}",
            graph.callers(name, Limits::default())
        )
        .expect("write to a string");
    }
    for path in [
        "src/util.h",
        "src/later.h",
        "src/extra.h",
        "lib/lib.go",
        "lib/extra.go",
    ] {
        let graph = engine.graph_of(path).expect("a language claims the path");
        writeln!(
            answers,
            "dependents of {path}: {:#?}",
            graph.dependents(path, 3, 100)
        )
        .expect("write to a string");
    }
    answers
}

fn refreshed(refresh: Result<EngineRefresh, kartta::BuildError>) -> EngineRefresh {
    let refresh = refresh.expect("the refresh completes");
    assert_eq!(
        refresh.errors().count(),
        0,
        "{:?}",
        refresh.errors().collect::<Vec<_>>()
    );
    refresh
}

#[test]
fn an_engine_refreshed_after_edits_answers_as_a_build_of_the_edited_tree() {
    let tree = settled_tree();
    let mut followed = build(tree.path(), true);
    let mut scoped = build(tree.path(), true);
    let mut inert = build(tree.path(), false);
    let before = answers(&followed);

    edit(tree.path());
    let fresh = answers(&build(tree.path(), false));
    let whole = refreshed(followed.refresh(None));
    let named = refreshed(scoped.refresh_paths(EDITED.iter().copied(), None));
    let rebuilt = refreshed(inert.refresh(None));

    assert_ne!(before, fresh, "the edits change what the engine answers");
    assert_eq!(
        answers(&followed),
        fresh,
        "a refresh that follows the change"
    );
    assert_eq!(answers(&scoped), fresh, "a refresh of the named paths");
    assert_eq!(
        answers(&inert),
        fresh,
        "a refresh that builds every graph again"
    );
    assert!(!whole.rebuilt() && !named.rebuilt() && rebuilt.rebuilt());
    assert_eq!((whole.widened(), named.widened()), (None, None));
    assert_ne!(
        named.index().walked,
        [""],
        "the named paths are walked apart"
    );
    assert!(
        whole.resolved_again() > 0 && named.resolved_again() > 0,
        "files whose includes or packages changed are resolved again: {} and {}",
        whole.resolved_again(),
        named.resolved_again()
    );
    assert_eq!(
        (whole.dropped(), named.dropped(), rebuilt.dropped()),
        (Some(2), Some(2), None),
        "gone.cpp and old.cpp leave"
    );
}

#[test]
fn more_named_paths_than_the_limit_refresh_the_whole_tree() {
    let tree = settled_tree();
    let mut engine = build(tree.path(), true);
    let limit = engine.refresh_paths_limit();
    let vanished: Vec<String> = (0..limit).map(|at| format!("tmp/{at}.cpp")).collect();
    let named = EDITED
        .iter()
        .copied()
        .chain(vanished.iter().map(String::as_str));

    edit(tree.path());
    let refresh = refreshed(engine.refresh_paths(named, None));

    assert_eq!(
        refresh.widened(),
        Some(Widened {
            named: EDITED.len() + limit,
            limit
        })
    );
    assert_eq!(refresh.index().walked, [""]);
    assert!(!refresh.rebuilt());
    assert_eq!(answers(&engine), answers(&build(tree.path(), false)));
}

#[test]
fn a_path_outside_the_tree_fails_a_refresh_that_would_walk_the_whole_tree() {
    let tree = settled_tree();
    let mut widened = build(tree.path(), true);
    let mut unstamped = build(tree.path(), false);
    let before = (answers(&widened), answers(&unstamped));
    let limit = widened.refresh_paths_limit();
    let vanished: Vec<String> = (0..limit).map(|at| format!("tmp/{at}.cpp")).collect();
    let named = EDITED
        .iter()
        .copied()
        .chain(vanished.iter().map(String::as_str))
        .chain(["../past-the-limit.cpp"]);

    edit(tree.path());
    let past_the_limit = widened.refresh_paths(named, None);
    let without_stamps = unstamped.refresh_paths(["src/util.cpp", "src/../../out.cpp"], None);

    assert!(
        matches!(&past_the_limit, Err(kartta::BuildError::OutsideTree { path }) if path == "../past-the-limit.cpp"),
        "{past_the_limit:?}"
    );
    assert!(
        matches!(&without_stamps, Err(kartta::BuildError::OutsideTree { path }) if path == "src/../../out.cpp"),
        "{without_stamps:?}"
    );
    assert_eq!(
        (answers(&widened), answers(&unstamped)),
        before,
        "a refused refresh leaves each engine answering as it did"
    );
}

#[test]
fn a_refresh_of_an_unchanged_tree_reads_and_resolves_nothing() {
    let tree = settled_tree();
    let mut engine = build(tree.path(), true);
    let before = answers(&engine);

    let refresh = refreshed(engine.refresh(None));

    assert_eq!(
        (
            refresh.extracted(),
            refresh.resolved_again(),
            refresh.dropped()
        ),
        (0, 0, Some(0))
    );
    assert_eq!(refresh.reconfigured(), &[] as &[String]);
    assert_eq!(answers(&engine), before);
}

#[test]
fn a_changed_go_mod_resolves_every_go_file_again() {
    let tree = settled_tree();
    let mut engine = build(tree.path(), true);

    write(tree.path(), "go.mod", "module example.com/n\n");
    let refresh = refreshed(engine.refresh_paths(["go.mod"], None));

    assert_eq!(refresh.reconfigured(), ["go"]);
    assert_eq!(refresh.resolved_again(), 2, "main.go and lib/lib.go");
    assert_eq!(answers(&engine), answers(&build(tree.path(), false)));
}
