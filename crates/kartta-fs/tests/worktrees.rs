//! The git layouts a build meets: a linked worktree of the indexed repository inside the root is held as an empty
//! directory and named, and every other layout is indexed as its ignore rules say.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

mod common;

use std::fs;
use std::path::Path;
#[cfg(feature = "watch")]
use std::time::{Duration, Instant};

use common::{tree, write};
#[cfg(feature = "watch")]
use kartta_fs::Watcher;
use kartta_fs::{BuildOptions, IgnoreSources, Index};

const AGENT: &str = ".claude/worktrees/agent1";
const NONE: [&str; 0] = [];

fn build(root: &Path, options: &BuildOptions) -> Index {
    let outcome = Index::build(root, options).expect("build the index");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    outcome.index
}

fn paths(index: &Index) -> Vec<String> {
    index
        .entries()
        .map(|entry| entry.path().to_owned())
        .collect()
}

fn pruned(index: &Index) -> Vec<&str> {
    index.pruned_worktrees().collect()
}

/// Makes `root` the top of a repository whose git directory is `root/.git`.
fn init(root: &Path) {
    write(root, ".git/HEAD", "ref: refs/heads/main\n");
    fs::create_dir_all(root.join(".git/objects")).expect("make the object store");
    fs::create_dir_all(root.join(".git/refs")).expect("make the refs");
}

/// Makes `worktree` a linked worktree, under the id `id`, of the repository whose common git directory is `common`,
/// with the files `git worktree add` writes, each ended by a newline (git v2.56.1 `builtin/worktree.c:540-544`,
/// `worktree.c:1129-1133`, `wrapper.c:778`): `worktrees/<id>/gitdir` and `worktrees/<id>/commondir` holding
/// `../..`, and the worktree's `.git` gitfile naming `worktrees/<id>` as `named_as` spells it.
fn link_worktree(common: &Path, id: &str, worktree: &Path, named_as: &str) {
    let git_dir = common.join("worktrees").join(id);
    fs::create_dir_all(&git_dir).expect("make the worktree's git directory");
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/agent\n").expect("write HEAD");
    fs::write(
        git_dir.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .expect("write gitdir");
    fs::write(git_dir.join("commondir"), "../..\n").expect("write commondir");
    fs::create_dir_all(worktree).expect("make the worktree");
    fs::write(worktree.join(".git"), format!("gitdir: {named_as}\n")).expect("write the gitfile");
}

fn link_nested_worktree(root: &Path) {
    let gitdir = root.join(".git/worktrees/agent1");
    link_worktree(
        &root.join(".git"),
        "agent1",
        &root.join(AGENT),
        &gitdir.display().to_string(),
    );
}

fn clone_with_nested_worktree() -> tempfile::TempDir {
    let dir = tree(&[
        ("src/lib.rs", "pub fn alpha() {}"),
        (".claude/worktrees/agent1/src/lib.rs", "pub fn alpha() {}"),
    ]);
    init(dir.path());
    link_nested_worktree(dir.path());
    dir
}

#[test]
fn a_linked_worktree_of_the_indexed_repository_inside_the_root_is_held_empty() {
    let dir = clone_with_nested_worktree();
    let common = dir.path().join(".git");
    link_worktree(
        &common,
        "vendored",
        &dir.path().join("vendor/wt"),
        &common.join("worktrees/vendored").display().to_string(),
    );
    write(dir.path(), "vendor/wt/src/lib.rs", "");

    let index = build(dir.path(), &BuildOptions::default());

    assert_eq!(
        paths(&index),
        [
            ".claude",
            ".claude/worktrees",
            AGENT,
            "src",
            "src/lib.rs",
            "vendor",
            "vendor/wt"
        ]
    );
    assert_eq!(pruned(&index), [AGENT, "vendor/wt"]);
}

#[test]
fn a_worktree_whose_gitfile_names_its_git_directory_relatively_is_held_empty() {
    let dir = tree(&[
        ("src/lib.rs", ""),
        (".claude/worktrees/agent1/src/lib.rs", ""),
    ]);
    init(dir.path());
    link_worktree(
        &dir.path().join(".git"),
        "agent1",
        &dir.path().join(AGENT),
        "../../../.git/worktrees/agent1",
    );

    let index = build(dir.path(), &BuildOptions::default());

    assert_eq!(
        paths(&index),
        [".claude", ".claude/worktrees", AGENT, "src", "src/lib.rs"]
    );
    assert_eq!(pruned(&index), [AGENT]);
}

#[test]
fn a_root_that_is_itself_a_linked_worktree_holds_another_worktree_of_its_repository_empty() {
    let main = tree(&[("src/lib.rs", "")]);
    init(main.path());
    let dir = tree(&[("src/lib.rs", ""), ("wt/src/lib.rs", "")]);
    let common = main.path().join(".git");
    link_worktree(
        &common,
        "root",
        dir.path(),
        &common.join("worktrees/root").display().to_string(),
    );
    link_worktree(
        &common,
        "wt",
        &dir.path().join("wt"),
        &common.join("worktrees/wt").display().to_string(),
    );

    let index = build(dir.path(), &BuildOptions::default());

    assert_eq!(paths(&index), ["src", "src/lib.rs", "wt"]);
    assert_eq!(pruned(&index), ["wt"]);
}

#[test]
fn a_submodule_is_indexed_under_its_own_rules() {
    let dir = tree(&[
        (".gitignore", "*.log\n"),
        ("sub/a.rs", ""),
        ("sub/b.log", ""),
    ]);
    init(dir.path());
    write(
        dir.path(),
        ".git/modules/sub/HEAD",
        "ref: refs/heads/main\n",
    );
    write(dir.path(), "sub/.git", "gitdir: ../.git/modules/sub\n");

    let index = build(dir.path(), &BuildOptions::default());

    assert_eq!(
        paths(&index),
        [".gitignore", "sub", "sub/a.rs", "sub/b.log"]
    );
    assert_eq!(pruned(&index), NONE);
}

#[test]
fn a_worktree_of_another_repository_is_indexed_as_any_nested_checkout() {
    let other = tree(&[("lib.rs", "")]);
    init(other.path());
    let dir = tree(&[("src/lib.rs", ""), ("vendor/other/lib.rs", "")]);
    init(dir.path());
    let common = other.path().join(".git");
    link_worktree(
        &common,
        "other",
        &dir.path().join("vendor/other"),
        &common.join("worktrees/other").display().to_string(),
    );

    let index = build(dir.path(), &BuildOptions::default());

    assert_eq!(
        paths(&index),
        [
            "src",
            "src/lib.rs",
            "vendor",
            "vendor/other",
            "vendor/other/lib.rs"
        ]
    );
    assert_eq!(pruned(&index), NONE);
}

#[test]
fn a_root_with_no_repository_of_its_own_indexes_a_worktree_inside_it() {
    let main = tree(&[("src/lib.rs", "")]);
    init(main.path());
    let dir = tree(&[("wt/src/lib.rs", "")]);
    let common = main.path().join(".git");
    link_worktree(
        &common,
        "wt",
        &dir.path().join("wt"),
        &common.join("worktrees/wt").display().to_string(),
    );

    let index = build(dir.path(), &BuildOptions::default());

    assert_eq!(paths(&index), ["wt", "wt/src", "wt/src/lib.rs"]);
    assert_eq!(pruned(&index), NONE);
}

#[test]
fn without_ignore_rules_a_nested_worktree_is_indexed() {
    let dir = clone_with_nested_worktree();
    let mut options = BuildOptions::default();
    options.ignore_sources = IgnoreSources::None;

    let mut index = build(dir.path(), &options);
    write(dir.path(), ".claude/worktrees/agent1/src/new.rs", "");
    index
        .refresh_paths([".claude/worktrees/agent1/src/new.rs"], None, None)
        .expect("refresh the new file");

    assert!(index.entry(".claude/worktrees/agent1/src/lib.rs").is_some());
    assert!(index.entry(".claude/worktrees/agent1/src/new.rs").is_some());
    assert_eq!(pruned(&index), NONE);
}

#[test]
fn a_whole_refresh_holds_a_nested_worktree_empty_as_the_build_did() {
    let dir = clone_with_nested_worktree();
    let mut index = build(dir.path(), &BuildOptions::default());

    write(dir.path(), ".claude/worktrees/agent1/src/new.rs", "");
    index.refresh(None, None).expect("refresh");

    assert_eq!(
        paths(&index),
        paths(&build(dir.path(), &BuildOptions::default()))
    );
    assert!(index.entry(".claude/worktrees/agent1/src").is_none());
    assert_eq!(pruned(&index), [AGENT]);
}

#[test]
fn a_refreshed_path_inside_a_nested_worktree_stays_out_of_the_index() {
    let dir = clone_with_nested_worktree();
    let mut index = build(dir.path(), &BuildOptions::default());

    write(dir.path(), ".claude/worktrees/agent1/src/new.rs", "");
    write(dir.path(), ".claude/worktrees/agent1/README.md", "");
    let outcome = index
        .refresh_paths(
            [
                ".claude/worktrees/agent1/src/new.rs",
                ".claude/worktrees/agent1/README.md",
            ],
            None,
            None,
        )
        .expect("refresh the paths");

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(
        paths(&index),
        [".claude", ".claude/worktrees", AGENT, "src", "src/lib.rs"]
    );
    assert_eq!(pruned(&index), [AGENT]);
}

#[test]
fn a_directory_that_becomes_a_nested_worktree_is_emptied_by_the_refresh_of_its_gitfile() {
    let dir = tree(&[
        ("src/lib.rs", ""),
        (".claude/worktrees/agent1/src/lib.rs", ""),
    ]);
    init(dir.path());
    let mut index = build(dir.path(), &BuildOptions::default());
    assert!(index.entry(".claude/worktrees/agent1/src/lib.rs").is_some());

    link_nested_worktree(dir.path());
    index
        .refresh_paths([".claude/worktrees/agent1/.git"], None, None)
        .expect("refresh the gitfile");

    assert_eq!(
        paths(&index),
        [".claude", ".claude/worktrees", AGENT, "src", "src/lib.rs"]
    );
    assert_eq!(pruned(&index), [AGENT]);
}

#[test]
fn a_part_refreshed_below_a_directory_that_became_a_worktree_names_that_worktree() {
    let dir = tree(&[("src/lib.rs", ""), (".claude/worktrees/agent1/", "")]);
    init(dir.path());
    let mut index = build(dir.path(), &BuildOptions::default());

    link_nested_worktree(dir.path());
    write(dir.path(), ".claude/worktrees/agent1/src/new.rs", "");
    index
        .refresh_paths([".claude/worktrees/agent1/src/new.rs"], None, None)
        .expect("refresh the new file");

    assert!(index.entry(".claude/worktrees/agent1/src").is_none());
    assert_eq!(pruned(&index), [AGENT]);
}

#[test]
fn a_removed_worktree_is_no_longer_named() {
    let dir = clone_with_nested_worktree();
    let mut index = build(dir.path(), &BuildOptions::default());

    fs::remove_dir_all(dir.path().join(AGENT)).expect("remove the worktree");
    index
        .refresh_paths([AGENT], None, None)
        .expect("refresh the worktree's path");

    assert_eq!(
        paths(&index),
        [".claude", ".claude/worktrees", "src", "src/lib.rs"]
    );
    assert_eq!(pruned(&index), NONE);
}

#[test]
fn a_nested_worktree_an_ignore_rule_hides_is_not_entered() {
    let dir = clone_with_nested_worktree();
    write(dir.path(), ".gitignore", ".claude/\n");

    let index = build(dir.path(), &BuildOptions::default());

    assert_eq!(paths(&index), [".gitignore", "src", "src/lib.rs"]);
    assert_eq!(pruned(&index), NONE);
}

#[cfg(feature = "watch")]
fn refresh_what_comes(index: &mut Index, watcher: &mut Watcher, wait: Duration) -> bool {
    let changes = watcher.wait(wait).expect("wait");
    if changes.is_empty() {
        return false;
    }
    let (outcome, followed) = index
        .refresh_paths_watched(changes.iter().map(String::as_str), watcher, None, None)
        .expect("refresh the changed paths");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    followed.expect("watch what the refresh took in");
    true
}

// constraint: notifications arrive on the operating system's schedule, so the test follows until the worktree is named
// or a deadline far past any delivery seen, then until the tree is quiet
#[cfg(feature = "watch")]
#[test]
fn a_worktree_added_while_the_index_follows_the_tree_is_held_empty() {
    let dir = tree(&[("src/lib.rs", "")]);
    init(dir.path());
    let (outcome, watcher) = Index::build_watched(dir.path(), &BuildOptions::default(), None, None)
        .expect("build the index");
    let (mut index, mut watcher) = (outcome.index, watcher.expect("start watching"));

    link_nested_worktree(dir.path());
    write(dir.path(), ".claude/worktrees/agent1/src/lib.rs", "");
    write(dir.path(), ".claude/worktrees/agent1/README.md", "");
    let deadline = Instant::now() + Duration::from_secs(10);
    while pruned(&index).is_empty() && Instant::now() < deadline {
        refresh_what_comes(&mut index, &mut watcher, Duration::from_millis(500));
    }
    while refresh_what_comes(&mut index, &mut watcher, Duration::from_secs(1)) {}

    assert_eq!(
        paths(&index),
        [".claude", ".claude/worktrees", AGENT, "src", "src/lib.rs"]
    );
    assert_eq!(pruned(&index), [AGENT]);
}
