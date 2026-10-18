//! The watcher contract: what a `Watcher` reports as the tree changes, and that refreshing the index with each batch it
//! hands over keeps the index what a build would make.
#![cfg(feature = "watch")]
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use kartta_fs::{BuildOptions, EntryKind, Index, Watcher};
use tempfile::TempDir;

fn tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    for (path, content) in spec {
        write(dir.path(), path, content);
    }
    dir
}

fn write(root: &Path, path: &str, content: &str) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(&full, content).expect("write file");
}

fn build(root: &Path) -> Index {
    build_with(root, &BuildOptions::default())
}

fn build_with(root: &Path, options: &BuildOptions) -> Index {
    let outcome = Index::build(root, options).expect("build the index");
    assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    outcome.into_index()
}

fn shown(index: &Index) -> Vec<(String, EntryKind)> {
    index
        .entries()
        .map(|entry| (entry.path().to_owned(), entry.kind()))
        .collect()
}

// constraint: notifications arrive on the operating system's schedule, so a test waits for what it expects up to a
// deadline far past any delivery seen, rather than for a fixed time
fn wait_for(watcher: &mut Watcher, wanted: &str) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        let changes = watcher.wait(Duration::from_millis(500)).expect("wait");
        seen.extend(changes.paths().map(str::to_owned));
        if seen.iter().any(|path| path == wanted) {
            return seen;
        }
    }
    seen
}

fn follow(watcher: &mut Watcher, index: &mut Index) {
    loop {
        let changes = watcher.wait(Duration::from_secs(1)).expect("wait");
        if changes.is_empty() {
            return;
        }
        let outcome = index
            .refresh_paths(changes.paths(), None, None)
            .expect("refresh the changed paths");
        assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    }
}

#[test]
fn a_file_written_after_the_start_is_named_by_its_path() {
    let dir = tree(&[("src/a.rs", "a")]);
    let index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    write(dir.path(), "src/b.rs", "b");

    let seen = wait_for(&mut watcher, "src/b.rs");
    assert!(seen.iter().any(|path| path == "src/b.rs"), "{seen:?}");
}

#[test]
fn a_file_made_in_a_directory_made_after_the_start_is_seen_too() {
    let dir = tree(&[("a.rs", "a")]);
    let mut index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    fs::create_dir(dir.path().join("late")).expect("make a directory");
    follow(&mut watcher, &mut index);
    write(dir.path(), "late/c.rs", "c");

    let seen = wait_for(&mut watcher, "late/c.rs");
    assert!(seen.iter().any(|path| path == "late/c.rs"), "{seen:?}");
}

#[test]
fn refreshing_with_what_the_watcher_hands_over_keeps_the_index_a_build_would_make() {
    let dir = tree(&[
        ("keep.rs", "a"),
        ("gone.rs", "b"),
        ("src/edit.rs", "c"),
        ("old/x.rs", "d"),
        ("old/deep/y.rs", "e"),
        ("from.rs", "f"),
    ]);
    let mut index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    fs::remove_file(dir.path().join("gone.rs")).expect("remove");
    write(dir.path(), "src/edit.rs", "changed");
    write(dir.path(), "new/deeper/z.rs", "g");
    fs::remove_dir_all(dir.path().join("old")).expect("remove a directory");
    fs::rename(dir.path().join("from.rs"), dir.path().join("src/to.rs")).expect("rename");
    follow(&mut watcher, &mut index);

    assert_eq!(shown(&index), shown(&build(dir.path())));
}

#[test]
fn what_the_watcher_hands_over_from_a_pruned_directory_stays_out_of_the_index() {
    let dir = tree(&[("a.rs", "a")]);
    let mut index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    write(dir.path(), ".vscode/settings.json", "{}");
    write(dir.path(), "b.rs", "b");
    follow(&mut watcher, &mut index);

    assert!(index.entry("b.rs").is_some());
    assert!(index.entry(".vscode").is_none());
    assert_eq!(shown(&index), shown(&build(dir.path())));
}

#[test]
fn a_watcher_covers_every_directory_the_index_held_at_its_start() {
    let dir = tree(&[("a.rs", "a"), ("src/deep/b.rs", "b")]);
    let index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    let covered = watcher.cover(&index).expect("cover the index");

    assert!(
        covered.is_empty(),
        "{:?}",
        covered.paths().collect::<Vec<_>>()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_directory_a_refresh_took_in_after_the_start_is_watched_once_covered() {
    let dir = tree(&[("a.rs", "a")]);
    let mut index = build(dir.path());
    write(dir.path(), "early/deep/b.rs", "b");
    let mut watcher = Watcher::new(&index).expect("start watching");
    index.refresh(None, None).expect("refresh the tree");
    write(dir.path(), "early/c.rs", "c");

    let covered = watcher.cover(&index).expect("cover the index");
    index
        .refresh_paths(covered.paths(), None, None)
        .expect("refresh what was covered");
    write(dir.path(), "early/deep/b.rs", "changed");

    assert_eq!(covered.paths().collect::<Vec<_>>(), ["early"]);
    assert!(index.entry("early/c.rs").is_some());
    let seen = wait_for(&mut watcher, "early/deep/b.rs");
    assert!(
        seen.iter().any(|path| path == "early/deep/b.rs"),
        "{seen:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_directory_made_after_the_start_under_a_pruned_name_is_not_watched() {
    let dir = tree(&[("a.rs", "a")]);
    let mut options = BuildOptions::default();
    options.pruned_names = kartta_fs::PrunedNames::builtin()
        .union(&kartta_fs::PrunedNames::new(["node_modules"]).expect("a name"));
    let mut index = build_with(dir.path(), &options);
    let mut watcher = Watcher::new(&index).expect("start watching");

    fs::create_dir(dir.path().join(".idea")).expect("make a directory");
    fs::create_dir_all(dir.path().join("web/node_modules")).expect("make a directory");
    follow(&mut watcher, &mut index);
    write(dir.path(), ".idea/workspace.xml", "<project/>");
    write(dir.path(), "web/node_modules/index.js", "");
    write(dir.path(), "b.rs", "b");

    let seen = wait_for(&mut watcher, "b.rs");
    assert!(seen.iter().any(|path| path == "b.rs"), "{seen:?}");
    assert!(
        !seen
            .iter()
            .any(|path| path.starts_with(".idea/") || path.starts_with("web/node_modules/")),
        "{seen:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_watched_directory_renamed_to_a_pruned_name_is_no_longer_watched() {
    let dir = tree(&[("src/deep/a.rs", "a")]);
    let mut index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    fs::rename(dir.path().join("src"), dir.path().join(".idea")).expect("rename");
    follow(&mut watcher, &mut index);
    write(dir.path(), ".idea/b.rs", "b");
    write(dir.path(), ".idea/deep/c.rs", "c");
    write(dir.path(), "d.rs", "d");

    let seen = wait_for(&mut watcher, "d.rs");
    assert!(seen.iter().any(|path| path == "d.rs"), "{seen:?}");
    assert!(
        !seen
            .iter()
            .any(|path| path.starts_with("src/") || path.starts_with(".idea/")),
        "{seen:?}"
    );
}

#[test]
fn a_tree_nobody_touches_hands_over_nothing_a_refresh_would_change() {
    let dir = tree(&[("a.rs", "a"), ("src/b.rs", "b")]);
    let mut index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    let changes = watcher.wait(Duration::from_millis(300)).expect("wait");
    let outcome = index
        .refresh_paths(changes.paths(), None, None)
        .expect("refresh what was handed over");

    if cfg!(target_os = "linux") {
        assert!(
            changes.is_empty(),
            "{:?}",
            changes.paths().collect::<Vec<_>>()
        );
    }
    assert!(
        outcome.added().is_empty() && outcome.removed().is_empty(),
        "{:?} added {:?} removed {:?}",
        changes.paths().collect::<Vec<_>>(),
        outcome.added(),
        outcome.removed()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_root_spelled_through_the_data_volume_is_watched_by_its_firmlinked_path() {
    let dir = tree(&[("src/a.rs", "a")]);
    let canonical = fs::canonicalize(dir.path()).expect("canonical path");
    let on_data =
        Path::new("/System/Volumes/Data").join(canonical.strip_prefix("/").expect("absolute"));
    let index = build(&on_data);
    let mut watcher = Watcher::new(&index).expect("start watching");

    write(dir.path(), "src/b.rs", "b");

    let seen = wait_for(&mut watcher, "src/b.rs");
    assert!(seen.iter().any(|path| path == "src/b.rs"), "{seen:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_directory_the_rules_ignore_is_not_watched() {
    let dir = tree(&[
        (".gitignore", "build/\n"),
        ("build/old.o", ""),
        ("a.rs", "a"),
    ]);
    let index = build(dir.path());
    let mut watcher = Watcher::new(&index).expect("start watching");

    write(dir.path(), "build/new.o", "");
    write(dir.path(), "b.rs", "b");

    let seen = wait_for(&mut watcher, "b.rs");
    assert!(seen.iter().any(|path| path == "b.rs"), "{seen:?}");
    assert!(
        !seen.iter().any(|path| path.starts_with("build")),
        "{seen:?}"
    );
}
