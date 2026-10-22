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
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use kartta::{
    BuildOptions, Conventions, DocComments, Engine, EngineRefresh, Language, Limits, RawExtraction,
    Repository, Resolver, Widened,
};
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
    settled_tree_with(&[])
}

fn settled_tree_with(more: &[(&str, &str)]) -> TempDir {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, content) in TREE.iter().chain(more) {
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
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    outcome.engine
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
        let (_, graph) = engine
            .graph_of(anchor)
            .expect("a language claims the anchor");
        writeln!(
            answers,
            "callers of {name}: {:#?}",
            graph.callers(name, Limits::new(1))
        )
        .expect("write to a string");
    }
    for path in [
        "src/util.h",
        "src/later.h",
        "src/extra.h",
        "lib/lib.go",
        "lib/extra.go",
        "web/button.ts",
    ] {
        let (_, graph) = engine.graph_of(path).expect("a language claims the path");
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
    assert!(!whole.rebuilt && !named.rebuilt && rebuilt.rebuilt);
    assert_eq!((whole.widened, named.widened), (None, None));
    assert_ne!(named.index.walked, [""], "the named paths are walked apart");
    assert!(
        whole.resolved_again > 0 && named.resolved_again > 0,
        "files whose includes or packages changed are resolved again: {} and {}",
        whole.resolved_again,
        named.resolved_again
    );
    assert_eq!(
        (whole.dropped, named.dropped, rebuilt.dropped),
        (Some(2), Some(2), None),
        "gone.cpp and old.cpp leave"
    );
}

#[test]
fn named_paths_that_come_to_more_parts_than_the_limit_refresh_the_whole_tree() {
    let tree = settled_tree();
    let mut engine = build(tree.path(), true);
    let limit = engine.refresh_paths_limit();
    let vanished: Vec<String> = (0..limit).map(|at| format!("src/tmp-{at}.cpp")).collect();
    let unheld = ["build/out/a.o", "build/out/b.o", "build/c.o"];
    let named = EDITED
        .iter()
        .copied()
        .chain(vanished.iter().map(String::as_str))
        .chain(unheld);

    edit(tree.path());
    let refresh = refreshed(engine.refresh_paths(named, None));

    assert_eq!(
        refresh.widened,
        Some(Widened {
            named: EDITED.len() + limit + unheld.len(),
            parts: EDITED.len() + limit + 1,
            limit
        }),
        "the paths inside a directory the index does not hold come to one part"
    );
    assert_eq!(refresh.index.walked, [""]);
    assert!(!refresh.rebuilt);
    assert_eq!(answers(&engine), answers(&build(tree.path(), false)));
}

#[test]
fn named_paths_walk_apart_up_to_one_part_for_every_30_entries() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let notes: Vec<String> = (0..1_530).map(|at| format!("notes/{at:04}.txt")).collect();
    for path in &notes {
        write(tree.path(), path, "");
    }
    let mut engine = build(tree.path(), true);
    let parts = engine.index().len() / 30;

    let refresh = refreshed(engine.refresh_paths(notes[..parts].iter().map(String::as_str), None));

    assert_eq!(engine.refresh_paths_limit(), parts);
    assert_eq!(refresh.widened, None);
    assert_eq!(refresh.index.walked.len(), parts);
}

#[test]
fn paths_past_the_limit_inside_an_ignored_directory_walk_only_that_directory() {
    let tree = settled_tree();
    write(tree.path(), ".gitignore", "target/\n");
    let mut engine = build(tree.path(), true);
    let before = answers(&engine);
    let limit = engine.refresh_paths_limit();
    let built: Vec<String> = (0..=limit)
        .map(|at| format!("target/debug/deps/{at}.o"))
        .collect();
    for path in &built {
        write(tree.path(), path, "");
    }

    let refresh = refreshed(engine.refresh_paths(built.iter().map(String::as_str), None));

    assert_eq!(refresh.widened, None);
    assert_eq!(refresh.index.walked, ["target"]);
    assert_eq!(answers(&engine), before);
}

#[test]
fn paths_past_the_limit_that_come_to_the_limit_in_parts_walk_apart() {
    let tree = settled_tree();
    write(tree.path(), ".gitignore", "target/\n");
    let mut engine = build(tree.path(), true);
    let limit = engine.refresh_paths_limit();
    let vanished: Vec<String> = (1..limit).map(|at| format!("src/tmp-{at}.cpp")).collect();
    let named = vanished
        .iter()
        .map(String::as_str)
        .chain(["target/a.o", "target/b.o"]);

    let refresh = refreshed(engine.refresh_paths(named, None));

    assert_eq!(refresh.widened, None);
    assert_eq!(refresh.index.walked.len(), limit);
}

#[test]
fn paths_past_the_limit_that_a_gitignore_edit_shows_are_taken_in() {
    let tree = settled_tree();
    write(tree.path(), "web/.gitignore", "dist/\n");
    write(
        tree.path(),
        "web/dist/bundle.ts",
        "export function bundle() {\n  return button();\n}\n",
    );
    let mut with_gitignore = build(tree.path(), true);
    let mut without_gitignore = build(tree.path(), true);
    let limit = with_gitignore.refresh_paths_limit();
    let built: Vec<String> = (0..limit).map(|at| format!("web/dist/{at}.ts")).collect();
    write(tree.path(), "web/.gitignore", "");
    for path in &built {
        write(tree.path(), path, "export const built = 1;\n");
    }
    let named = || built.iter().map(String::as_str);

    let named_with_gitignore = refreshed(with_gitignore.refresh_paths(
        named().chain(["web/.gitignore", "web/dist/bundle.ts"]),
        None,
    ));
    let named_without_gitignore =
        refreshed(without_gitignore.refresh_paths(named().chain(["web/dist/bundle.ts"]), None));

    assert_eq!(
        (
            named_with_gitignore.widened,
            named_without_gitignore.widened
        ),
        (None, None)
    );
    assert_eq!(named_with_gitignore.index.walked, ["web"]);
    assert_eq!(
        named_without_gitignore.index.walked,
        ["web/dist"],
        "the part the index last saw ignored is walked under the rules read now"
    );
    let fresh = answers(&build(tree.path(), false));
    for engine in [&with_gitignore, &without_gitignore] {
        assert!(engine.index().entry("web/dist/bundle.ts").is_some());
        assert_eq!(answers(engine), fresh);
    }
}

#[test]
fn a_path_outside_the_tree_fails_a_refresh_that_would_walk_the_whole_tree() {
    let tree = settled_tree();
    let mut widened = build(tree.path(), true);
    let mut unstamped = build(tree.path(), false);
    let before = (answers(&widened), answers(&unstamped));
    let limit = widened.refresh_paths_limit();
    let vanished: Vec<String> = (0..limit).map(|at| format!("src/tmp-{at}.cpp")).collect();
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
        (refresh.extracted, refresh.resolved_again, refresh.dropped),
        (0, 0, Some(0))
    );
    assert!(
        refresh.reconfigured.is_empty(),
        "{:?}",
        refresh.reconfigured
    );
    assert_eq!(answers(&engine), before);
}

#[test]
fn a_changed_go_mod_resolves_every_go_file_again() {
    let tree = settled_tree();
    let mut engine = build(tree.path(), true);

    write(tree.path(), "go.mod", "module example.com/n\n");
    let refresh = refreshed(engine.refresh_paths(["go.mod"], None));

    assert_eq!(refresh.reconfigured, ["go"]);
    assert_eq!(refresh.resolved_again, 2, "main.go and lib/lib.go");
    assert_eq!(answers(&engine), answers(&build(tree.path(), false)));
}

#[test]
fn a_crate_renamed_in_its_cargo_toml_resolves_every_rust_file_again() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, content) in [
        ("crates/other/Cargo.toml", "[package]\nname = \"other\"\n"),
        ("crates/other/src/lib.rs", "pub fn f() {}\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/main.rs",
            "fn main() {\n    other::f();\n}\n",
        ),
    ] {
        write(tree.path(), path, content);
    }
    std::thread::sleep(Duration::from_millis(2100));
    let callees = |engine: &Engine| {
        let (_, graph) = engine
            .graph_of("crates/app/src/main.rs")
            .expect("Rust claims the file");
        format!("{:#?}", graph.callees("main", Limits::new(1)))
    };
    let mut engine = build(tree.path(), true);
    let before = callees(&engine);

    write(
        tree.path(),
        "crates/other/Cargo.toml",
        "[package]\nname = \"renamed\"\n",
    );
    let refresh = refreshed(engine.refresh_paths(["crates/other/Cargo.toml"], None));

    assert_eq!(refresh.reconfigured, ["rust"]);
    assert_eq!(refresh.resolved_again, 2, "main.rs and lib.rs");
    assert_ne!(callees(&engine), before, "the call binds through no crate");
    assert_eq!(callees(&engine), callees(&build(tree.path(), false)));
}

#[test]
fn an_engine_refreshed_again_and_again_follows_every_change() {
    let tree = settled_tree();
    let mut engine = build(tree.path(), true);

    for round in 0..6 {
        if round % 2 == 0 {
            write(tree.path(), "src/later.h", "void later();\n");
        } else {
            fs::remove_file(tree.path().join("src/later.h")).expect("remove later.h");
        }
        let refresh = refreshed(engine.refresh_paths(["src/later.h"], None));

        assert_eq!(
            refresh.resolved_again, 1,
            "src/a.cpp includes later.h, round {round}"
        );
        assert_eq!(
            answers(&engine),
            answers(&build(tree.path(), false)),
            "round {round}"
        );
    }
}

/// A shipped language that counts the resolvers it builds.
#[derive(Debug)]
struct Counted {
    language: Box<dyn Language>,
    resolvers: Arc<AtomicUsize>,
}

impl Language for Counted {
    fn name(&self) -> &str {
        self.language.name()
    }

    fn family(&self) -> &str {
        self.language.family()
    }

    fn claims(&self, path: &str) -> bool {
        self.language.claims(path)
    }

    fn fingerprint(&self) -> &[u8] {
        self.language.fingerprint()
    }

    fn conventions(&self) -> Conventions {
        self.language.conventions()
    }

    fn doc_comments(&self) -> DocComments {
        self.language.doc_comments()
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        self.language.extract(source)
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        self.resolvers.fetch_add(1, Ordering::Relaxed);
        self.language.resolver(repository)
    }
}

const ALIASED: &[(&str, &str)] = &[
    (
        "tsconfig.json",
        "{\"compilerOptions\": {\"paths\": {\"@ui/*\": [\"./web/*\"]}}}\n",
    ),
    (
        "web/page.ts",
        "import { button } from \"@ui/button\";\n\nexport function page() {\n  return button();\n}\n",
    ),
    (
        "web/view.tsx",
        "import { page } from \"./page\";\n\nexport function View() {\n  return <div>{page()}</div>;\n}\n",
    ),
    (
        "web/legacy.js",
        "export function legacy() {\n  return 0;\n}\n",
    ),
];

#[test]
fn a_changed_tsconfig_resolves_the_javascript_family_through_one_resolver_built_again_once() {
    let tree = settled_tree_with(ALIASED);
    let resolvers = Arc::new(AtomicUsize::new(0));
    let languages = kartta_languages::all()
        .into_iter()
        .map(|language| -> Box<dyn Language> {
            if language.family() == "jsts" {
                Box::new(Counted {
                    language,
                    resolvers: Arc::clone(&resolvers),
                })
            } else {
                language
            }
        })
        .collect();
    let mut options = BuildOptions::default();
    options.stamps = true;
    let outcome = Engine::build(tree.path(), &options, languages, None).expect("the tree builds");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let mut engine = outcome.engine;
    let built = resolvers.load(Ordering::Relaxed);
    let before = answers(&engine);

    write(
        tree.path(),
        "tsconfig.json",
        "{\"compilerOptions\": {\"paths\": {\"@ui/*\": [\"./absent/*\"]}}}\n",
    );
    let refresh = refreshed(engine.refresh_paths(["tsconfig.json"], None));
    let fresh = answers(&build(tree.path(), false));

    assert_eq!(
        built, 1,
        "TypeScript, TSX and JavaScript share one resolver"
    );
    assert_eq!(resolvers.load(Ordering::Relaxed), 2);
    assert_eq!(refresh.reconfigured, ["jsts"]);
    assert_eq!(
        refresh.resolved_again, 5,
        "every file under web/, of each of the three languages"
    );
    assert_ne!(
        before, fresh,
        "web/page.ts no longer includes web/button.ts"
    );
    assert_eq!(answers(&engine), fresh);
}
