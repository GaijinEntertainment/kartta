//! The watcher contract: what a `Watcher` reports as the tree changes, and that refreshing the index with each batch it
//! hands over keeps the index what a build would make.
#![cfg(feature = "watch")]
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

mod common;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::{tree, write};
use kartta_fs::{BuildOptions, Content, ContentVisitor, EntryKind, Index, Watcher};

fn build(root: &Path) -> Index {
    let outcome = Index::build(root, &BuildOptions::default()).expect("build the index");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    outcome.index
}

fn watched(root: &Path) -> (Index, Watcher) {
    watched_with(root, &BuildOptions::default(), None)
}

fn watched_with(
    root: &Path,
    options: &BuildOptions,
    visitor: Option<&dyn ContentVisitor>,
) -> (Index, Watcher) {
    let (outcome, watcher) =
        Index::build_watched(root, options, visitor, None).expect("build the index");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    (outcome.index, watcher.expect("start watching"))
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
        seen.extend(changes.iter().map(String::as_str).map(str::to_owned));
        if seen.iter().any(|path| path == wanted) {
            return seen;
        }
    }
    seen
}

fn refresh_under(index: &mut Index, watcher: &mut Watcher, changes: &[String]) {
    let (outcome, followed) = index
        .refresh_paths_watched(changes.iter().map(String::as_str), watcher, None, None)
        .expect("refresh the changed paths");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    followed.expect("watch what the refresh took in");
}

fn follow(watcher: &mut Watcher, index: &mut Index) -> Vec<String> {
    let mut handed = Vec::new();
    loop {
        let changes = watcher.wait(Duration::from_secs(1)).expect("wait");
        if changes.is_empty() {
            return handed;
        }
        refresh_under(index, watcher, &changes);
        handed.extend(changes);
    }
}

// constraint: a drain leaves to a later wait what the system reports after it, as FSEvents reports a change made in a
// stream's first moments, so a test of a change made during the walk reads on until `wanted` came and the tree is quiet
fn caught_up(watcher: &mut Watcher, index: &mut Index, wanted: &str) -> Vec<String> {
    let mut handed = watcher.drain().expect("drain");
    refresh_under(index, watcher, &handed);
    if !handed.iter().any(|path| path == wanted) {
        let late = wait_for(watcher, wanted);
        refresh_under(index, watcher, &late);
        handed.extend(late);
    }
    handed.extend(follow(watcher, index));
    handed
}

/// Writes `writes` into the tree the first time the build asks it about the file at `at`, while the walk is in that
/// file's directory, and wants no file read.
struct WritesMidWalk<'a> {
    root: &'a Path,
    at: &'a str,
    writes: &'a [(&'a str, &'a str)],
    written: AtomicBool,
}

impl<'a> WritesMidWalk<'a> {
    const fn new(root: &'a Path, at: &'a str, writes: &'a [(&'a str, &'a str)]) -> Self {
        Self {
            root,
            at,
            writes,
            written: AtomicBool::new(false),
        }
    }
}

impl ContentVisitor for WritesMidWalk<'_> {
    fn wants(&self, path: &str) -> bool {
        if path == self.at && !self.written.swap(true, Ordering::Relaxed) {
            for (path, content) in self.writes {
                write(self.root, path, content);
            }
        }
        false
    }

    fn visit(&self, _content: Content<'_>) {}
}

#[test]
fn a_change_made_while_the_build_walks_is_handed_over_once_it_is_built() {
    let dir = tree(&[("a.rs", "a"), ("src/b.rs", "b"), ("src/deep/c.rs", "c")]);
    let writes = [
        ("a.rs", "changed"),
        ("late.rs", "d"),
        ("src/new/deeper/e.rs", "e"),
        ("src/deep/f.rs", "f"),
    ];
    let hook = WritesMidWalk::new(dir.path(), "src/b.rs", &writes);
    let (mut index, mut watcher) = watched_with(dir.path(), &BuildOptions::default(), Some(&hook));

    let handed = caught_up(&mut watcher, &mut index, "a.rs");

    assert!(handed.iter().any(|path| path == "a.rs"), "{handed:?}");
    assert_eq!(shown(&index), shown(&build(dir.path())));
}

#[test]
fn a_directory_made_while_the_build_walks_is_watched_once_it_is_built() {
    let dir = tree(&[("a.rs", "a"), ("src/b.rs", "b")]);
    let writes = [("src/new/deeper/c.rs", "c")];
    let hook = WritesMidWalk::new(dir.path(), "src/b.rs", &writes);
    let (mut index, mut watcher) = watched_with(dir.path(), &BuildOptions::default(), Some(&hook));
    follow(&mut watcher, &mut index);

    write(dir.path(), "src/new/deeper/d.rs", "d");

    let seen = wait_for(&mut watcher, "src/new/deeper/d.rs");
    assert!(
        seen.iter().any(|path| path == "src/new/deeper/d.rs"),
        "{seen:?}"
    );
}

#[test]
fn a_file_written_after_the_start_is_named_by_its_path() {
    let dir = tree(&[("src/a.rs", "a")]);
    let (_, mut watcher) = watched(dir.path());

    write(dir.path(), "src/b.rs", "b");

    let seen = wait_for(&mut watcher, "src/b.rs");
    assert!(seen.iter().any(|path| path == "src/b.rs"), "{seen:?}");
}

#[test]
fn a_file_made_in_a_directory_made_after_the_start_is_seen_too() {
    let dir = tree(&[("a.rs", "a")]);
    let (mut index, mut watcher) = watched(dir.path());

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
    let (mut index, mut watcher) = watched(dir.path());

    fs::remove_file(dir.path().join("gone.rs")).expect("remove");
    write(dir.path(), "src/edit.rs", "changed");
    write(dir.path(), "new/deeper/z.rs", "g");
    fs::remove_dir_all(dir.path().join("old")).expect("remove a directory");
    fs::rename(dir.path().join("from.rs"), dir.path().join("src/to.rs")).expect("rename");
    follow(&mut watcher, &mut index);

    assert_eq!(shown(&index), shown(&build(dir.path())));
}

#[cfg(unix)]
#[test]
fn a_name_a_refresh_refuses_is_followed_through_the_directory_that_holds_it() {
    let dir = tree(&[("src/a\\b.rs", "a"), ("C:x", "b"), ("keep.rs", "c")]);
    let (mut index, mut watcher) = watched(dir.path());

    write(dir.path(), "src/a\\b.rs", "changed");
    write(dir.path(), "src/c\\d.rs", "new");
    write(dir.path(), "C:x", "changed");
    write(dir.path(), "e\\f.rs", "new");
    follow(&mut watcher, &mut index);

    assert_eq!(shown(&index), shown(&build(dir.path())));
    assert!(index.entry("e\\f.rs").is_some());
}

#[test]
fn what_the_watcher_hands_over_from_a_pruned_directory_stays_out_of_the_index() {
    let dir = tree(&[("a.rs", "a")]);
    let (mut index, mut watcher) = watched(dir.path());

    write(dir.path(), ".vscode/settings.json", "{}");
    write(dir.path(), "b.rs", "b");
    follow(&mut watcher, &mut index);

    assert!(index.entry("b.rs").is_some());
    assert!(index.entry(".vscode").is_none());
    assert_eq!(shown(&index), shown(&build(dir.path())));
}

#[test]
fn every_directory_the_build_kept_is_watched() {
    let dir = tree(&[("a.rs", "a"), ("src/deep/b.rs", "b")]);
    let (_, mut watcher) = watched(dir.path());

    write(dir.path(), "src/deep/c.rs", "c");

    let seen = wait_for(&mut watcher, "src/deep/c.rs");
    assert!(seen.iter().any(|path| path == "src/deep/c.rs"), "{seen:?}");
}

#[test]
fn a_watched_refresh_hands_over_what_changed_while_it_walked_and_watches_what_it_took_in() {
    let dir = tree(&[("a.rs", "a")]);
    let mut index = build(dir.path());
    write(dir.path(), "early/deep/b.rs", "b");
    let writes = [("early/deep/c.rs", "c"), ("early/new/d.rs", "d")];
    let hook = WritesMidWalk::new(dir.path(), "early/deep/b.rs", &writes);

    let (outcome, watcher) = index
        .refresh_watched(Some(&hook), None)
        .expect("refresh the tree");
    let mut watcher = watcher.expect("start watching");
    let handed = caught_up(&mut watcher, &mut index, "early/deep/c.rs");

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert!(
        handed.iter().any(|path| path == "early/deep/c.rs"),
        "{handed:?}"
    );
    assert_eq!(shown(&index), shown(&build(dir.path())));
    write(dir.path(), "early/deep/b.rs", "changed");
    write(dir.path(), "early/new/e.rs", "e");
    let seen = wait_for(&mut watcher, "early/new/e.rs");
    assert!(
        seen.iter().any(|path| path == "early/deep/b.rs"),
        "{seen:?}"
    );
    assert!(seen.iter().any(|path| path == "early/new/e.rs"), "{seen:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_directory_made_after_the_start_under_a_pruned_name_is_not_watched() {
    let dir = tree(&[("a.rs", "a")]);
    let mut options = BuildOptions::default();
    options.pruned_names = kartta_fs::PrunedNames::builtin()
        .union(&kartta_fs::PrunedNames::new(["node_modules"]).expect("a name"));
    let (mut index, mut watcher) = watched_with(dir.path(), &options, None);

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
    let (mut index, mut watcher) = watched(dir.path());

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
    let (mut index, mut watcher) = watched(dir.path());

    let changes = watcher.wait(Duration::from_millis(300)).expect("wait");
    let outcome = index
        .refresh_paths(changes.iter().map(String::as_str), None, None)
        .expect("refresh what was handed over");

    if cfg!(target_os = "linux") {
        assert!(
            changes.is_empty(),
            "{:?}",
            changes.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }
    assert!(
        outcome.added.is_empty() && outcome.removed.is_empty(),
        "{:?} added {:?} removed {:?}",
        changes.iter().map(String::as_str).collect::<Vec<_>>(),
        outcome.added,
        outcome.removed
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_root_spelled_through_the_data_volume_is_watched_by_its_firmlinked_path() {
    let dir = tree(&[("src/a.rs", "a")]);
    let canonical = fs::canonicalize(dir.path()).expect("canonical path");
    let on_data =
        Path::new("/System/Volumes/Data").join(canonical.strip_prefix("/").expect("absolute"));
    let (_, mut watcher) = watched(&on_data);

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
    let (_, mut watcher) = watched(dir.path());

    write(dir.path(), "build/new.o", "");
    write(dir.path(), "b.rs", "b");

    let seen = wait_for(&mut watcher, "b.rs");
    assert!(seen.iter().any(|path| path == "b.rs"), "{seen:?}");
    assert!(
        !seen.iter().any(|path| path.starts_with("build")),
        "{seen:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_watched_directory_renamed_in_the_tree_is_followed_by_its_new_name() {
    let dir = tree(&[("src/deep/a.rs", "a")]);
    let (mut index, mut watcher) = watched(dir.path());

    fs::rename(dir.path().join("src"), dir.path().join("lib")).expect("rename");
    let renamed = watcher.drain().expect("drain");
    refresh_under(&mut index, &mut watcher, &renamed);
    write(dir.path(), "lib/deep/b.rs", "b");
    let seen = watcher.drain().expect("drain");

    assert_eq!(renamed, ["lib", "src"]);
    assert_eq!(seen, ["lib/deep/b.rs"]);
}

#[cfg(target_os = "linux")]
#[test]
fn a_watched_directory_moved_under_an_ignored_one_is_no_longer_followed() {
    let dir = tree(&[
        (".gitignore", "build/\n"),
        ("build/old.o", ""),
        ("src/deep/a.rs", "a"),
    ]);
    let (mut index, mut watcher) = watched(dir.path());

    fs::rename(dir.path().join("src"), dir.path().join("build/src")).expect("rename");
    let moved = watcher.drain().expect("drain");
    refresh_under(&mut index, &mut watcher, &moved);
    write(dir.path(), "build/src/deep/b.rs", "b");
    let seen = watcher.drain().expect("drain");

    assert_eq!(moved, ["src"]);
    assert_eq!(seen, Vec::<String>::new());
}

#[cfg(target_os = "linux")]
#[test]
fn a_directory_a_gitignore_edit_shows_is_watched_by_the_refresh_that_takes_it_in() {
    let dir = tree(&[
        (".gitignore", "build/\n"),
        ("build/deep/old.o", ""),
        ("sub/.gitignore", "out/\n"),
        ("sub/out/deep/old.o", ""),
        ("a.rs", "a"),
    ]);
    let (mut index, mut watcher) = watched(dir.path());

    write(dir.path(), "sub/.gitignore", "");
    let shown_below = watcher.drain().expect("drain");
    refresh_under(&mut index, &mut watcher, &shown_below);
    write(dir.path(), ".gitignore", "");
    let shown_at_the_root = watcher.drain().expect("drain");
    refresh_under(&mut index, &mut watcher, &shown_at_the_root);
    write(dir.path(), "build/deep/new.o", "");
    write(dir.path(), "sub/out/deep/new.o", "");
    let seen = watcher.drain().expect("drain");
    refresh_under(&mut index, &mut watcher, &seen);

    assert_eq!(shown_below, ["sub/.gitignore"]);
    assert_eq!(shown_at_the_root, [".gitignore"]);
    assert_eq!(seen, ["build/deep/new.o", "sub/out/deep/new.o"]);
    assert_eq!(shown(&index), shown(&build(dir.path())));
}
