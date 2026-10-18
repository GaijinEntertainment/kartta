//! The public walk contract: shape and order of the recorded tree, the policy's control over descent, cancellation, and
//! partial results on failure.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use kartta_walk::{
    CancelToken, Children, Dir, Entry, EntryKind, Node, Policy, Tree, Visit, WalkError,
    WalkOptions, walk,
};
use tempfile::TempDir;

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

struct Everything;

impl Policy for Everything {
    type State = ();
    type Value = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
        Visit::Keep(entry.kind())
    }
}

fn render<N: std::fmt::Debug>(tree: &Tree<N>) -> Vec<String> {
    fn descend<N: std::fmt::Debug>(nodes: Children<'_, N>, prefix: &str, out: &mut Vec<String>) {
        for node in nodes {
            let path = if prefix.is_empty() {
                node.name().to_owned()
            } else {
                format!("{prefix}/{}", node.name())
            };

            out.push(format!("{path} {:?}", node.value()));
            descend(node.children(), &path, out);
        }
    }

    let mut out = Vec::new();
    descend(tree.children(), "", &mut out);

    out
}

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

    let outcome = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        render(&outcome.tree),
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
    assert!(outcome.errors.is_empty());
    assert!(!outcome.cancelled);
}

#[derive(Default)]
struct Seen {
    dirs: Mutex<Vec<String>>,
    entries: Mutex<Vec<String>>,
}

impl Policy for Seen {
    type State = ();
    type Value = EntryKind;

    fn enter(&self, dir: &Dir<'_>, _parent_state: &()) {
        self.dirs.lock().expect("lock").push(dir.path().to_owned());
    }

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
        self.entries
            .lock()
            .expect("lock")
            .push(entry.path().to_owned());
        Visit::Keep(entry.kind())
    }
}

fn sorted(seen: &Mutex<Vec<String>>) -> Vec<String> {
    let mut seen = seen.lock().expect("lock").clone();
    seen.sort();
    seen
}

#[test]
fn a_subtree_walk_shows_the_policy_the_paths_the_whole_walk_would() {
    let dir = fixture();
    let seen = Seen::default();
    let options = WalkOptions {
        subtree: Some("dir".to_owned()),
        ..WalkOptions::default()
    };

    let outcome = walk(dir.path(), &seen, &(), &options).expect("walk");

    assert_eq!(
        render(&outcome.tree),
        ["b.txt File", "nested Dir", "nested/c.txt File"],
        "the tree holds the directory's contents"
    );
    assert_eq!(sorted(&seen.dirs), ["dir", "dir/nested"]);
    assert_eq!(
        sorted(&seen.entries),
        ["dir/b.txt", "dir/nested", "dir/nested/c.txt"]
    );
}

#[test]
fn a_nested_subtree_spelled_loosely_walks_as_the_whole_walk_sees_it() {
    let dir = fixture();
    let seen = Seen::default();
    let options = WalkOptions {
        subtree: Some("/dir//./nested/".to_owned()),
        ..WalkOptions::default()
    };

    let outcome = walk(dir.path(), &seen, &(), &options).expect("walk");

    assert_eq!(render(&outcome.tree), ["c.txt File"]);
    assert_eq!(sorted(&seen.dirs), ["dir/nested"]);
    assert_eq!(sorted(&seen.entries), ["dir/nested/c.txt"]);
}

#[test]
fn a_subtree_that_may_leave_the_root_fails_the_walk() {
    let dir = fixture();
    let options = WalkOptions {
        subtree: Some("dir/../dir".to_owned()),
        ..WalkOptions::default()
    };

    let walked = walk(dir.path(), &Everything, &(), &options);

    assert!(matches!(
        walked,
        Err(WalkError::Subtree { subtree }) if subtree == "dir/../dir"
    ));
}

#[test]
fn a_subtree_that_cannot_be_listed_fails_the_walk_naming_it() {
    let dir = fixture();
    let options = WalkOptions {
        subtree: Some("dir/missing".to_owned()),
        ..WalkOptions::default()
    };

    let walked = walk(dir.path(), &Everything, &(), &options);

    let missing = dir.path().join("dir/missing");
    assert!(matches!(
        walked,
        Err(WalkError::ReadDir { native_path, .. }) if native_path == missing
    ));
}

#[test]
fn nodes_are_stored_in_the_order_a_descent_reports_them() {
    let dir = fixture();

    let outcome = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    let flat: Vec<&str> = outcome.tree.nodes().map(Node::name).collect();
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

    assert_eq!(render(&one.tree), render(&many.tree));
}

#[test]
fn an_empty_directory_is_a_node_without_children() {
    let dir = fixture();

    let outcome = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    let empty = outcome
        .tree
        .children()
        .find(|node| node.name() == "empty")
        .expect("empty is listed");

    assert_eq!(*empty.value(), EntryKind::Dir);
    assert_eq!(empty.children().count(), 0);
    assert!(
        outcome.errors.is_empty(),
        "an empty directory is not a failure"
    );
}

#[test]
fn the_walk_root_is_not_itself_a_node() {
    let dir = fixture();

    let outcome = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

    let children: Vec<&str> = outcome.tree.children().map(Node::name).collect();
    assert_eq!(children, ["a.txt", "dir", "empty", "z.txt"]);
}

struct PruneDir(&'static str);

impl Policy for PruneDir {
    type State = ();
    type Value = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

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

    let outcome = walk(dir.path(), &PruneDir("dir"), &(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        render(&outcome.tree),
        ["a.txt File", "dir Dir", "empty Dir", "z.txt File"]
    );
}

struct SkipPrefix(&'static str);

impl Policy for SkipPrefix {
    type State = ();
    type Value = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

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

    let outcome = walk(dir.path(), &SkipPrefix("dir"), &(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        render(&outcome.tree),
        ["a.txt File", "empty Dir", "z.txt File"]
    );
}

struct Trail;

impl Policy for Trail {
    type State = Vec<String>;
    type Value = String;

    fn enter(&self, dir: &Dir<'_>, parent_state: &Vec<String>) -> Vec<String> {
        let mut trail = parent_state.clone();
        if !dir.path().is_empty() {
            trail.push(dir.path().to_owned());
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

    let outcome = walk(dir.path(), &Trail, &Vec::new(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        render(&outcome.tree),
        [
            r#"a.txt "[]""#,
            r#"dir "[]""#,
            r#"dir/b.txt "[dir]""#,
            r#"dir/nested "[dir]""#,
            r#"dir/nested/c.txt "[dir>dir/nested]""#,
            r#"empty "[]""#,
            r#"z.txt "[]""#,
        ],
        "`empty` sits beside `dir` and sees none of it, and `dir/nested/c.txt` sees both its ancestors"
    );
}

struct Control;

impl Policy for Control {
    type State = String;
    type Value = String;

    fn enter(&self, dir: &Dir<'_>, _parent_state: &String) -> String {
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

    let outcome = walk(
        dir.path(),
        &Control,
        &String::new(),
        &WalkOptions::default(),
    )
    .expect("walk");

    let root_state = outcome
        .tree
        .nodes()
        .find(|n| n.name() == "plain")
        .expect("plain");
    assert_eq!(root_state.value(), "marker=false count=2");

    let sub_state = outcome
        .tree
        .nodes()
        .find(|n| n.name() == ".marker")
        .expect(".marker");
    assert_eq!(
        sub_state.value(),
        "marker=true count=1",
        "the control file is visible to enter"
    );
}

struct Paths;

impl Policy for Paths {
    type State = ();
    type Value = String;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<String> {
        Visit::Keep(entry.path().to_owned())
    }
}

#[test]
fn paths_are_forward_slash_on_every_platform() {
    let dir = fixture();

    let outcome = walk(dir.path(), &Paths, &(), &WalkOptions::default()).expect("walk");

    let deepest = outcome
        .tree
        .nodes()
        .find(|node| node.name() == "c.txt")
        .expect("c.txt is recorded");
    assert_eq!(deepest.value(), "dir/nested/c.txt");

    for node in outcome.tree.nodes() {
        assert!(
            !node.value().contains('\\'),
            "path {:?} carries a native separator rather than the forward slash every host reports",
            node.value()
        );
        assert!(
            !node.name().contains('/') && !node.name().contains('\\'),
            "name {:?} is a single component, so it carries no separator at all",
            node.name()
        );
    }

    let joined: Vec<String> = render(&outcome.tree)
        .iter()
        .map(|line| {
            line.split_once(' ')
                .expect("render emits `path value`")
                .0
                .to_owned()
        })
        .collect();
    let reported: Vec<String> = outcome
        .tree
        .nodes()
        .map(|node| node.value().clone())
        .collect();
    let mut sorted_reported = reported;
    sorted_reported.sort();
    let mut sorted_joined = joined;
    sorted_joined.sort();
    assert_eq!(
        sorted_joined, sorted_reported,
        "names joined down the tree reproduce the reported paths"
    );
}

struct Native;

impl Policy for Native {
    type State = ();
    type Value = bool;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<bool> {
        Visit::Keep(entry.native_path().exists())
    }
}

#[test]
fn native_paths_stay_on_the_io_side() {
    let dir = fixture();

    let outcome = walk(dir.path(), &Native, &(), &WalkOptions::default()).expect("walk");

    assert!(
        outcome.tree.nodes().all(|node| *node.value()),
        "every reported entry is reachable through its native path"
    );
}

struct Facts;

impl Policy for Facts {
    type State = ();
    type Value = (EntryKind, PathBuf, Option<u64>);

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<Self::Value> {
        Visit::Keep((
            entry.kind(),
            entry.native_path(),
            entry.metadata().ok().map(|metadata| metadata.len()),
        ))
    }
}

type RenderedWithErrors = (Vec<String>, Vec<Option<PathBuf>>);

fn with_and_without_listed_stats(root: &Path) -> [RenderedWithErrors; 2] {
    [false, true].map(|listed_stats| {
        let options = WalkOptions {
            listed_stats,
            ..WalkOptions::default()
        };
        let outcome = walk(root, &Facts, &(), &options).expect("walk");
        let errors = outcome
            .errors
            .iter()
            .map(|error| error.native_path().map(Path::to_path_buf))
            .collect();

        (render(&outcome.tree), errors)
    })
}

#[test]
fn a_walk_asked_for_listed_stats_records_the_tree_a_walk_without_them_records() {
    let dir = fixture();

    let [without, with] = with_and_without_listed_stats(dir.path());

    assert_eq!(with, without, "kinds, native paths and metadata all agree");
}

struct Stats;

type PathAndStat = (String, Option<(u64, i64, i64)>);

impl Policy for Stats {
    type State = ();
    type Value = PathAndStat;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<Self::Value> {
        let stat = entry
            .listed_stat()
            .map(|stat| (stat.size, stat.ctime, stat.ctime_nsec));

        Visit::Keep((entry.path().to_owned(), stat))
    }
}

fn stated(tree: &Tree<PathAndStat>) -> usize {
    tree.nodes().filter(|node| node.value().1.is_some()).count()
}

#[test]
fn a_walk_not_asked_for_listed_stats_hands_none() {
    let dir = fixture();

    let outcome = walk(dir.path(), &Stats, &(), &WalkOptions::default()).expect("walk");

    assert_eq!(
        stated(&outcome.tree),
        0,
        "only a walk that asks lists stats"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn a_listing_that_carries_no_stats_hands_none_when_asked() {
    let dir = fixture();
    let options = WalkOptions {
        listed_stats: true,
        ..WalkOptions::default()
    };

    let outcome = walk(dir.path(), &Stats, &(), &options).expect("walk");

    assert_eq!(
        stated(&outcome.tree),
        0,
        "only macOS lists stats with the names"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn listed_stats_give_each_regular_file_what_a_stat_of_it_gives() {
    use std::os::unix::fs::MetadataExt as _;

    let dir = fixture();
    std::os::unix::fs::symlink(dir.path().join("a.txt"), dir.path().join("to_file"))
        .expect("symlink to file");
    let options = WalkOptions {
        listed_stats: true,
        ..WalkOptions::default()
    };

    let outcome = walk(dir.path(), &Stats, &(), &options).expect("walk");

    for node in outcome.tree.nodes() {
        let (path, stat) = node.value();
        let metadata = fs::symlink_metadata(dir.path().join(path)).expect("stat the entry");
        let stated = metadata
            .is_file()
            .then(|| (metadata.size(), metadata.ctime(), metadata.ctime_nsec()));
        assert_eq!(*stat, stated, "{path} lists what a stat of it reports");
    }
    assert_eq!(stated(&outcome.tree), 4, "every regular file has its stat");
}

struct CancelAfter {
    token: CancelToken,
    after: usize,
    seen: AtomicUsize,
}

impl Policy for CancelAfter {
    type State = ();
    type Value = EntryKind;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

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
    let options = WalkOptions {
        cancel_token: Some(token),
        ..Default::default()
    };

    let outcome = on_threads(1, || {
        walk(dir.path(), &policy, &(), &options).expect("walk")
    });

    assert!(outcome.cancelled);
    assert!(
        outcome.tree.len() < 7,
        "the walk stopped short: {} of 7",
        outcome.tree.len()
    );
    assert!(
        !outcome.tree.is_empty(),
        "what was classified before the stop is recorded"
    );
}

#[test]
fn a_token_cancelled_after_the_last_entry_leaves_a_complete_walk_reported_complete() {
    let dir = fixture();
    let token = CancelToken::new();
    let policy = CancelAfter {
        token: token.clone(),
        after: 6,
        seen: AtomicUsize::new(0),
    };
    let options = WalkOptions {
        cancel_token: Some(token.clone()),
        ..Default::default()
    };

    let outcome = on_threads(1, || {
        walk(dir.path(), &policy, &(), &options).expect("walk")
    });

    assert!(
        token.is_cancelled(),
        "the seventh and last entry cancelled it"
    );
    assert_eq!(outcome.tree.len(), 7, "every entry was recorded");
    assert!(
        !outcome.cancelled,
        "no check turned anything away, so the tree is complete and says so"
    );
}

#[test]
fn a_token_cancelled_up_front_records_nothing() {
    let dir = fixture();
    let token = CancelToken::new();
    token.cancel();
    let options = WalkOptions {
        cancel_token: Some(token),
        ..Default::default()
    };

    let outcome = walk(dir.path(), &Everything, &(), &options).expect("walk");

    assert!(outcome.tree.is_empty());
    assert!(outcome.cancelled);
}

#[test]
fn an_unreadable_root_fails_the_whole_walk() {
    let missing = Path::new("./no-such-directory-b7f3a1");

    let err =
        walk(missing, &Everything, &(), &WalkOptions::default()).expect_err("missing root fails");

    assert_eq!(err.native_path(), Some(missing));
    assert!(
        err.to_string().contains("cannot read directory"),
        "the message names the operation: {err}"
    );
}

fn wide_fixture(width: usize) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");

    for i in 0..width {
        let sub = dir.path().join(format!("d{i:02}"));
        fs::create_dir(&sub).expect("create subdirectory");
        fs::write(sub.join("f.txt"), "x").expect("write file");
    }

    dir
}

struct Threads;

impl Policy for Threads {
    type State = ();
    type Value = std::thread::ThreadId;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, _entry: &Entry<'_>, _state: &()) -> Visit<std::thread::ThreadId> {
        Visit::Keep(std::thread::current().id())
    }
}

#[test]
fn the_pool_a_walk_is_called_from_bounds_how_many_workers_traverse() {
    let dir = wide_fixture(16);

    for limit in [1_usize, 2, 3] {
        let outcome = on_threads(limit, || {
            walk(dir.path(), &Threads, &(), &WalkOptions::default()).expect("walk")
        });

        let distinct: std::collections::HashSet<_> =
            outcome.tree.nodes().map(Node::value).collect();
        assert!(
            distinct.len() <= limit,
            "a pool of {limit} admitted {} workers",
            distinct.len()
        );
        assert_eq!(outcome.tree.len(), 32, "every entry is still recorded");
    }
}

// constraint: a pool gives no thread a reason to steal from a classifier that finishes trivial entries before the
// others wake, so the first classification waits, for a bounded time, until a second thread classifies too
struct WaitsForCompany {
    seen: Mutex<std::collections::HashSet<std::thread::ThreadId>>,
    joined: Condvar,
}

impl Policy for WaitsForCompany {
    type State = ();
    type Value = std::thread::ThreadId;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, _entry: &Entry<'_>, _state: &()) -> Visit<std::thread::ThreadId> {
        let id = std::thread::current().id();
        self.join(id);

        Visit::Keep(id)
    }
}

impl WaitsForCompany {
    fn join(&self, id: std::thread::ThreadId) {
        let mut seen = self.seen.lock().expect("lock");
        let first_visit = seen.insert(id);
        self.joined.notify_all();
        let first_thread = first_visit && seen.len() == 1;
        if !first_thread {
            return;
        }

        let alone = |seen: &mut std::collections::HashSet<_>| seen.len() < 2;
        let _company =
            self.joined
                .wait_timeout_while(seen, std::time::Duration::from_secs(5), alone);
    }
}

#[test]
fn a_directory_large_enough_to_classify_in_parallel_walks_to_the_same_tree_and_spreads() {
    let dir = tempfile::tempdir().expect("create temp dir");
    for i in 0..500 {
        fs::write(dir.path().join(format!("f{i:03}.txt")), "x").expect("write file");
    }
    let company = WaitsForCompany {
        seen: Mutex::new(std::collections::HashSet::new()),
        joined: Condvar::new(),
    };

    let single = on_threads(1, || {
        walk(dir.path(), &Threads, &(), &WalkOptions::default()).expect("walk")
    });
    let many = on_threads(4, || {
        walk(dir.path(), &company, &(), &WalkOptions::default()).expect("walk")
    });

    let names = |tree: &Tree<std::thread::ThreadId>| {
        tree.nodes()
            .map(|node| node.name().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&single.tree),
        names(&many.tree),
        "listing order, whatever ran where"
    );
    assert_eq!(many.tree.len(), 500);
    let classifiers: std::collections::HashSet<_> = many.tree.nodes().map(Node::value).collect();
    assert!(
        classifiers.len() > 1,
        "one flat directory's entries were classified on more than one thread"
    );
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn a_node_that_is_neither_directory_file_nor_symlink_reports_other() {
        let dir = fixture();
        let fifo = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed");

        let outcome = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

        let node = outcome
            .tree
            .nodes()
            .find(|node| node.name() == "pipe")
            .expect("the fifo is recorded");

        assert_eq!(
            *node.value(),
            EntryKind::Other,
            "a fifo is none of a directory, a file and a symlink"
        );
        assert_eq!(node.children().count(), 0);
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

        let outcome = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

        for name in ["to_dir", "to_file", "dangling"] {
            let node = outcome
                .tree
                .nodes()
                .find(|node| node.name() == name)
                .unwrap_or_else(|| panic!("{name} is recorded"));

            assert_eq!(
                *node.value(),
                EntryKind::Symlink,
                "{name} reports as a symlink, not as its target"
            );
            assert_eq!(node.children().count(), 0, "{name} was not descended");
        }
    }

    #[test]
    fn listed_stats_change_nothing_a_walk_records_or_misses_among_odd_entries() {
        let dir = fixture();
        let root = dir.path();
        let status = std::process::Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed");
        std::os::unix::fs::symlink(root.join("dir"), root.join("to_dir")).expect("symlink to dir");
        std::os::unix::fs::symlink(root.join("nowhere"), root.join("dangling"))
            .expect("dangling symlink");
        let locked = root.join("locked");
        fs::create_dir(&locked).expect("create locked");
        fs::write(locked.join("hidden.txt"), "x").expect("write inside locked");
        let unsearchable = root.join("unsearchable");
        fs::create_dir_all(unsearchable.join("below")).expect("create unsearchable/below");
        fs::write(unsearchable.join("named.txt"), "x").expect("write inside unsearchable");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod 000");
        fs::set_permissions(&unsearchable, fs::Permissions::from_mode(0o400)).expect("chmod 400");

        let [without, with] = with_and_without_listed_stats(root);

        // constraint: a directory left unreadable cannot be removed, so the modes are restored before any assertion
        // fails
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("restore mode");
        fs::set_permissions(&unsearchable, fs::Permissions::from_mode(0o755))
            .expect("restore mode");

        assert_eq!(
            with, without,
            "a fifo, symlinks, an unreadable directory and one whose names can be read but not searched all walk alike"
        );
    }

    #[test]
    fn an_unreadable_subdirectory_costs_its_subtree_and_nothing_else() {
        let dir = fixture();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).expect("create locked");
        fs::write(locked.join("hidden.txt"), "x").expect("write inside locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod 000");

        let outcome = walk(dir.path(), &Everything, &(), &WalkOptions::default()).expect("walk");

        // constraint: a directory left unreadable cannot be removed, so the mode is restored before any assertion fails
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("restore mode");

        if outcome.errors.is_empty() {
            // Running as root ignores the permission bits; there is nothing to assert.
            return;
        }

        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].native_path(), Some(locked.as_path()));

        let names: Vec<&str> = outcome.tree.nodes().map(Node::name).collect();
        assert!(
            names.contains(&"locked"),
            "the directory itself is still listed"
        );
        assert!(!names.contains(&"hidden.txt"), "its contents are missing");
        assert!(names.contains(&"z.txt"), "the rest of the tree survived");
        assert!(!outcome.cancelled, "a read failure is not a cancellation");
    }
}
