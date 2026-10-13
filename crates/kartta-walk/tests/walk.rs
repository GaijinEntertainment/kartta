//! The public walk contract: shape and order of the recorded tree, the policy's control over descent, cancellation, and
//! partial results on failure.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use kartta_walk::{
    CancelToken, Dir, Entry, EntryKind, Node, NodeId, Policy, Tree, Visit, WalkOptions, walk,
};
use tempfile::TempDir;

/// The fixture tree every shape test walks:
///
/// ```text
/// a.txt
/// dir/b.txt
/// dir/nested/c.txt
/// empty/
/// z.txt
/// ```
fn fixture() -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    let root = dir.path();

    fs::create_dir_all(root.join("dir/nested")).expect("create dir/nested");
    fs::create_dir(root.join("empty")).expect("create empty");
    fs::write(root.join("a.txt"), "a").expect("write a.txt");
    fs::write(root.join("z.txt"), "zz").expect("write z.txt");
    fs::write(root.join("dir/b.txt"), "bbb").expect("write dir/b.txt");
    fs::write(root.join("dir/nested/c.txt"), "cccc").expect("write dir/nested/c.txt");

    dir
}

/// Records what each entry is, descending everywhere.
struct Everything;

impl Policy for Everything {
    type State = ();
    type Node = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
        Visit::Keep(entry.kind())
    }
}

/// Renders the tree as one `path value` line per node, in the order [`Tree::nodes`] reports.
///
/// Rebuilding the paths from names is also the access pattern a consumer uses, so this doubles as a check that the
/// public API carries enough to reconstruct them.
fn render<N: std::fmt::Debug>(tree: &Tree<N>) -> Vec<String> {
    fn descend<N: std::fmt::Debug>(
        tree: &Tree<N>,
        ids: &[NodeId],
        prefix: &str,
        out: &mut Vec<String>,
    ) {
        for &id in ids {
            let node = tree.node(id);
            let path = if prefix.is_empty() {
                node.name().to_owned()
            } else {
                format!("{prefix}/{}", node.name())
            };

            out.push(format!("{path} {:?}", node.value()));
            descend(tree, node.children(), &path, out);
        }
    }

    let mut out = Vec::new();
    descend(tree, tree.roots(), "", &mut out);

    out
}

/// Runs `f` on a rayon pool of exactly `threads` threads, so a test can pin scheduling.
fn on_threads<T: Send>(threads: usize, f: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("build thread pool")
        .install(f)
}

#[test]
fn records_the_tree_depth_first_with_children_name_sorted() {
    let dir = fixture();

    let tree = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        render(tree.tree()),
        [
            "a.txt File",
            "dir Dir",
            "dir/b.txt File",
            "dir/nested Dir",
            "dir/nested/c.txt File",
            "empty Dir",
            "z.txt File",
        ]
    );
    assert!(tree.errors().is_empty());
    assert!(!tree.cancelled());
}

#[test]
fn nodes_are_stored_in_the_order_a_descent_reports_them() {
    let dir = fixture();

    let tree = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    // `nodes()` promises depth-first order — a directory immediately followed by its subtree — so the flat slice and a
    // recursive descent must agree name for name.
    let flat: Vec<&str> = tree.tree().nodes().iter().map(Node::name).collect();
    assert_eq!(
        flat,
        ["a.txt", "dir", "b.txt", "nested", "c.txt", "empty", "z.txt"]
    );
}

#[test]
fn the_tree_is_identical_however_many_threads_walk_it() {
    let dir = fixture();

    let one = on_threads(1, || {
        walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk")
    });
    let many = on_threads(8, || {
        walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk")
    });

    assert_eq!(render(one.tree()), render(many.tree()));
}

#[test]
fn an_empty_directory_is_a_node_without_children() {
    let dir = fixture();

    let tree = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    let empty = tree
        .tree()
        .roots()
        .iter()
        .map(|&id| tree.tree().node(id))
        .find(|node| node.name() == "empty")
        .expect("empty is listed");

    assert_eq!(*empty.value(), EntryKind::Dir);
    assert_eq!(empty.children(), []);
    assert!(
        tree.errors().is_empty(),
        "an empty directory is not a failure"
    );
}

#[test]
fn the_walk_root_is_not_itself_a_node() {
    let dir = fixture();

    let tree = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    let roots: Vec<&str> = tree
        .tree()
        .roots()
        .iter()
        .map(|&id| tree.tree().node(id).name())
        .collect();
    assert_eq!(roots, ["a.txt", "dir", "empty", "z.txt"]);
}

/// Lists a named directory but never descends into it.
struct Prune(&'static str);

impl Policy for Prune {
    type State = ();
    type Node = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
        if entry.kind() == EntryKind::Dir && entry.name() == self.0 {
            return Visit::Prune(entry.kind());
        }

        Visit::Keep(entry.kind())
    }
}

#[test]
fn a_pruned_directory_is_listed_with_no_children() {
    let dir = fixture();

    let tree = walk(dir.path(), &Prune("dir"), &(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        render(tree.tree()),
        ["a.txt File", "dir Dir", "empty Dir", "z.txt File"]
    );
}

/// Drops every entry whose name starts with the given prefix.
struct SkipPrefix(&'static str);

impl Policy for SkipPrefix {
    type State = ();
    type Node = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
        if entry.name().starts_with(self.0) {
            return Visit::Skip;
        }

        Visit::Keep(entry.kind())
    }
}

#[test]
fn a_skipped_directory_leaves_no_trace() {
    let dir = fixture();

    let tree = walk(dir.path(), &SkipPrefix("dir"), &(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        render(tree.tree()),
        ["a.txt File", "empty Dir", "z.txt File"]
    );
}

/// Accumulates the directory names walked so far, proving state descends and does not leak sideways.
struct Trail;

impl Policy for Trail {
    /// The chain of directory names from the walk root to the current directory.
    type State = Vec<String>;
    type Node = String;

    fn enter(&self, dir: &Dir<'_>, parent: &Vec<String>) -> Vec<String> {
        let mut trail = parent.clone();
        if !dir.rel().is_empty() {
            trail.push(dir.rel().to_owned());
        }

        trail
    }

    fn visit(&self, _entry: &Entry<'_>, state: &Vec<String>) -> Visit<String> {
        Visit::Keep(format!("[{}]", state.join(">")))
    }
}

#[test]
fn state_is_inherited_down_the_tree_and_not_across_it() {
    let dir = fixture();

    let tree = walk(dir.path(), &Trail, &Vec::new(), &WalkOptions::default()).expect("walk");

    // `empty` sits beside `dir`, so it must not see `dir` in its trail; `dir/nested/c.txt` sees both ancestors.
    assert_eq!(
        render(tree.tree()),
        [
            r#"a.txt "[]""#,
            r#"dir "[]""#,
            r#"dir/b.txt "[dir]""#,
            r#"dir/nested "[dir]""#,
            r#"dir/nested/c.txt "[dir>dir/nested]""#,
            r#"empty "[]""#,
            r#"z.txt "[]""#,
        ]
    );
}

/// Records the listing facts `enter` can read without a syscall.
struct Control;

impl Policy for Control {
    type State = String;
    type Node = String;

    fn enter(&self, dir: &Dir<'_>, _parent: &String) -> String {
        format!(
            "marker={} count={}",
            dir.contains(".marker"),
            dir.names().count()
        )
    }

    fn visit(&self, _entry: &Entry<'_>, state: &String) -> Visit<String> {
        Visit::Keep(state.clone())
    }
}

#[test]
fn enter_sees_the_whole_listing_before_any_entry_is_classified() {
    let dir = tempfile::tempdir().expect("create temp dir");
    fs::create_dir(dir.path().join("sub")).expect("create sub");
    fs::write(dir.path().join("sub/.marker"), "x").expect("write marker");
    fs::write(dir.path().join("plain"), "x").expect("write plain");

    let tree = walk(
        dir.path(),
        &Control,
        &String::new(),
        &WalkOptions::default(),
    )
    .expect("walk");

    let root_state = tree
        .tree()
        .nodes()
        .iter()
        .find(|n| n.name() == "plain")
        .expect("plain");
    assert_eq!(root_state.value(), "marker=false count=2");

    let sub_state = tree
        .tree()
        .nodes()
        .iter()
        .find(|n| n.name() == ".marker")
        .expect(".marker");
    assert_eq!(
        sub_state.value(),
        "marker=true count=1",
        "the control file is visible to enter"
    );
}

/// Records each entry's relative path exactly as the walk reports it.
struct Rel;

impl Policy for Rel {
    type State = ();
    type Node = String;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<String> {
        Visit::Keep(entry.rel().to_owned())
    }
}

#[test]
fn relative_paths_are_unix_form_on_every_platform() {
    let dir = fixture();

    let tree = walk(dir.path(), &Rel, &(), &WalkOptions::default()).expect("walk");

    let deepest = tree
        .tree()
        .nodes()
        .iter()
        .find(|node| node.name() == "c.txt")
        .expect("c.txt is recorded");
    assert_eq!(deepest.value(), "dir/nested/c.txt");

    for node in tree.tree().nodes() {
        // The reported form is the abstraction: forward slashes whatever the host spells them as, and never the
        // platform's own separator.
        assert!(
            !node.value().contains('\\'),
            "relative path {:?} carries an OS separator",
            node.value()
        );
        assert!(
            !node.name().contains('/') && !node.name().contains('\\'),
            "name {:?} is a single component, so it carries no separator at all",
            node.name()
        );
    }

    // Joining names down the tree must reproduce the reported paths exactly, or the two disagree about the same tree.
    let joined: Vec<String> = render(tree.tree())
        .iter()
        .map(|line| {
            line.split_once(' ')
                .expect("render emits `path value`")
                .0
                .to_owned()
        })
        .collect();
    let reported: Vec<String> = tree
        .tree()
        .nodes()
        .iter()
        .map(|node| node.value().clone())
        .collect();
    let mut sorted_reported = reported;
    sorted_reported.sort();
    let mut sorted_joined = joined;
    sorted_joined.sort();
    assert_eq!(sorted_joined, sorted_reported);
}

/// Records whether the entry's own path resolves to something reachable on this platform.
struct Native;

impl Policy for Native {
    type State = ();
    type Node = bool;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<bool> {
        // `path()` is the platform's own spelling, so it must open what `rel()` merely names.
        Visit::Keep(entry.path().exists())
    }
}

#[test]
fn native_paths_stay_on_the_io_side() {
    let dir = fixture();

    let tree = walk(dir.path(), &Native, &(), &WalkOptions::default()).expect("walk");

    assert!(
        tree.tree().nodes().iter().all(|node| *node.value()),
        "every reported entry is reachable through its native path"
    );
}

/// Cancels the walk once it has classified `after` entries.
struct CancelAfter {
    token: CancelToken,
    after: usize,
    seen: AtomicUsize,
}

impl Policy for CancelAfter {
    type State = ();
    type Node = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
        if self.seen.fetch_add(1, Ordering::Relaxed) >= self.after {
            self.token.cancel();
        }

        Visit::Keep(entry.kind())
    }
}

#[test]
fn cancellation_truncates_the_walk_and_reports_it() {
    let dir = fixture();
    let token = CancelToken::new();
    let policy = CancelAfter {
        token: token.clone(),
        after: 2,
        seen: AtomicUsize::new(0),
    };
    let opts = WalkOptions {
        cancel: Some(token),
        ..Default::default()
    };

    let tree = on_threads(1, || walk(dir.path(), &policy, &(), &opts).expect("walk"));

    assert!(tree.cancelled());
    assert!(
        tree.tree().len() < 7,
        "the walk stopped short: {} of 7",
        tree.tree().len()
    );
    assert!(
        !tree.tree().is_empty(),
        "what was classified before the stop is kept"
    );
}

#[test]
fn a_token_cancelled_up_front_records_nothing() {
    let dir = fixture();
    let token = CancelToken::new();
    token.cancel();
    let opts = WalkOptions {
        cancel: Some(token),
        ..Default::default()
    };

    let tree = walk(dir.path(), &Everything, &(), &opts).expect("walk");

    assert!(tree.tree().is_empty());
    assert!(tree.cancelled());
}

#[test]
fn cancellation_composes_with_a_thread_limit() {
    let dir = fixture();
    let token = CancelToken::new();
    token.cancel();
    let opts = WalkOptions {
        cancel: Some(token),
        threads: NonZeroUsize::new(2),
    };

    let tree = walk(dir.path(), &Everything, &(), &opts).expect("walk");

    assert!(tree.tree().is_empty());
    assert!(tree.cancelled());
}

#[test]
fn an_unreadable_root_fails_the_whole_walk() {
    let missing = Path::new("./no-such-directory-b7f3a1");

    let err =
        walk(missing, &Everything, &(), &WalkOptions::default()).expect_err("missing root fails");

    assert_eq!(err.path(), Some(missing));
    assert!(
        err.to_string().contains("cannot read directory"),
        "the message names the operation: {err}"
    );
}

/// A fixture wide enough that an unbounded walk would spread across several threads: eight sibling directories, each
/// holding a file.
fn wide_fixture(width: usize) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");

    for i in 0..width {
        let sub = dir.path().join(format!("d{i:02}"));
        fs::create_dir(&sub).expect("create subdirectory");
        fs::write(sub.join("f.txt"), "x").expect("write file");
    }

    dir
}

/// Records the thread that classified each entry, so a test can count how many took part.
struct Threads;

impl Policy for Threads {
    type State = ();
    type Node = std::thread::ThreadId;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, _entry: &Entry<'_>, _state: &()) -> Visit<std::thread::ThreadId> {
        Visit::Keep(std::thread::current().id())
    }
}

#[test]
fn threads_bounds_how_many_workers_traverse() {
    let dir = wide_fixture(16);

    for limit in [1_usize, 2, 3] {
        let opts = WalkOptions {
            threads: NonZeroUsize::new(limit),
            ..Default::default()
        };

        let tree = walk(dir.path(), &Threads, &(), &opts).expect("walk");

        let distinct: std::collections::HashSet<_> =
            tree.tree().nodes().iter().map(Node::value).collect();
        assert!(
            distinct.len() <= limit,
            "threads = {limit} admitted {} workers",
            distinct.len()
        );
        assert_eq!(tree.tree().len(), 32, "every entry is still recorded");
    }
}

#[test]
fn one_thread_walks_the_same_tree_as_many() {
    let dir = fixture();

    let single = walk(
        dir.path(),
        &Everything,
        &(),
        &WalkOptions {
            threads: NonZeroUsize::new(1),
            ..Default::default()
        },
    )
    .expect("walk");
    let ambient = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    assert_eq!(render(single.tree()), render(ambient.tree()));
}

#[test]
fn a_thread_limit_records_the_whole_tree() {
    let dir = wide_fixture(8);
    let opts = WalkOptions {
        cancel: None,
        threads: NonZeroUsize::new(2),
    };

    let tree = walk(dir.path(), &Everything, &(), &opts).expect("walk");

    // Bounding the parallelism bounds the workers, never the answer.
    assert_eq!(tree.tree().len(), 16);
    assert!(!tree.cancelled());
    assert!(tree.errors().is_empty());
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn a_node_that_is_neither_directory_file_nor_link_reports_other() {
        let dir = fixture();
        let fifo = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed");

        let tree = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

        let node = tree
            .tree()
            .nodes()
            .iter()
            .find(|node| node.name() == "pipe")
            .expect("the fifo is recorded");

        // The reason `EntryKind` exists: a fifo answers false to is_dir, is_file *and* is_symlink, so "neither a
        // directory nor a file" would have mislabelled it a link.
        assert_eq!(*node.value(), EntryKind::Other);
        assert_eq!(node.children(), []);
    }

    #[test]
    fn a_symlink_is_recorded_and_never_descended() {
        let dir = fixture();
        std::os::unix::fs::symlink(dir.path().join("dir"), dir.path().join("to_dir"))
            .expect("symlink to dir");
        std::os::unix::fs::symlink(dir.path().join("a.txt"), dir.path().join("to_file"))
            .expect("symlink to file");
        std::os::unix::fs::symlink(dir.path().join("nowhere"), dir.path().join("dangling"))
            .expect("dangling symlink");

        let tree = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

        for name in ["to_dir", "to_file", "dangling"] {
            let node = tree
                .tree()
                .nodes()
                .iter()
                .find(|node| node.name() == name)
                .unwrap_or_else(|| panic!("{name} is recorded"));

            assert_eq!(
                *node.value(),
                EntryKind::Symlink,
                "{name} reports as a link, not as its target"
            );
            assert!(node.children().is_empty(), "{name} was not descended");
        }
    }

    #[test]
    fn an_unreadable_subdirectory_costs_its_subtree_and_nothing_else() {
        let dir = fixture();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).expect("create locked");
        fs::write(locked.join("hidden.txt"), "x").expect("write inside locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod 000");

        let tree = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

        // Restore before the assertions so the temp dir can always be cleaned up.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("restore mode");

        if tree.errors().is_empty() {
            // Running as root ignores the permission bits; there is nothing to assert.
            return;
        }

        assert_eq!(tree.errors().len(), 1);
        assert_eq!(tree.errors()[0].path(), Some(locked.as_path()));

        let names: Vec<&str> = tree.tree().nodes().iter().map(Node::name).collect();
        assert!(
            names.contains(&"locked"),
            "the directory itself is still listed"
        );
        assert!(!names.contains(&"hidden.txt"), "its contents are missing");
        assert!(names.contains(&"z.txt"), "the rest of the tree survived");
        assert!(!tree.cancelled(), "a read failure is not a cancellation");
    }
}
