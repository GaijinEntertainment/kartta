//! The public index contract: what a build records, which entries git's rules remove, and what the listing and the
//! path queries answer.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "clippy's allow-expect-in-tests and allow-panic-in-tests only cover #[cfg(test)] modules, and an \
              integration test crate is built without cfg(test) — so the workspace no-panic posture has to be waived \
              here, where a failed expectation is the assertion"
)]

use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;

use kartta_fs::{
    BinaryExtensions, BuildError, Capped, Config, Entry, EntryKind, Filter, IgnoreSources, Index,
    Inspected, ListDirError, ListOptions,
};
use tempfile::TempDir;

/// Builds a temporary tree from `path -> content` pairs, creating parent directories as needed. A path ending in `/`
/// makes an empty directory and ignores the content.
fn tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    let root = dir.path();

    for (path, content) in spec {
        if let Some(name) = path.strip_suffix('/') {
            fs::create_dir_all(root.join(name)).expect("create directory");
            continue;
        }

        let full = root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&full, content).expect("write file");
    }

    dir
}

/// The index of a complete build, which is what every test but the failure ones expects.
fn index(root: &Path, config: &Config) -> Index {
    let outcome = Index::build(root, config).expect("build the index");
    assert!(
        outcome.errors().is_empty(),
        "unexpected build errors: {:?}",
        outcome.errors()
    );

    outcome.into_index()
}

/// The index of a complete build under the default configuration.
fn default_index(root: &Path) -> Index {
    index(root, &Config::default())
}

fn paths(index: &Index) -> Vec<&str> {
    index.entries().map(Entry::path).collect()
}

fn kind_of(index: &Index, path: &str) -> EntryKind {
    index
        .entry(path)
        .unwrap_or_else(|| panic!("{path} is indexed"))
        .kind()
}

#[test]
fn entries_are_depth_first_with_each_directory_followed_by_its_subtree() {
    let dir = tree(&[
        ("z.txt", "z"),
        ("a.txt", "a"),
        ("dir/nested/c.txt", "c"),
        ("dir/b.txt", "b"),
        ("empty/", ""),
    ]);
    let index = default_index(dir.path());

    assert_eq!(
        paths(&index),
        [
            "a.txt",
            "dir",
            "dir/b.txt",
            "dir/nested",
            "dir/nested/c.txt",
            "empty",
            "z.txt",
        ]
    );
}

#[test]
fn paths_are_forward_slash_separated_and_root_relative() {
    let dir = tree(&[("deep/down/here.txt", "x")]);
    let index = default_index(dir.path());

    let entry = index.entry("deep/down/here.txt").expect("the nested file");
    assert_eq!(entry.path(), "deep/down/here.txt");
    assert_eq!(entry.name(), "here.txt");
    assert_eq!(index.root(), dir.path());
}

#[test]
fn every_entry_is_found_by_its_path_when_names_sort_below_the_separator() {
    let dir = tree(&[
        ("a/b/c.txt", ""),
        ("a b", ""),
        ("a!", ""),
        ("a-c/d.txt", ""),
        ("a.txt", ""),
        ("ab", ""),
        ("a-long-shared-prefix/x", ""),
        ("a-long-shared-prefix.x", ""),
        ("a-long-shared-prefix x", ""),
    ]);
    let index = default_index(dir.path());

    for entry in index.entries() {
        assert_eq!(index.entry(entry.path()), Some(entry), "{}", entry.path());
    }
    for absent in [
        "a/x",
        "a.t",
        "a/b/c",
        "a-long-shared-prefix/",
        "b",
        "a\0b",
        "a-c\0d.txt",
    ] {
        assert_eq!(index.entry(absent), None, "{absent}");
    }
}

#[test]
fn a_root_level_entry_is_its_own_name() {
    let dir = tree(&[("top.txt", "x")]);
    let index = default_index(dir.path());

    assert_eq!(index.entry("top.txt").map(Entry::name), Some("top.txt"));
}

#[test]
fn sizes_are_answered_for_regular_files_only() {
    let dir = tree(&[("empty.txt", ""), ("hello.txt", "hello"), ("sub/", "")]);
    let index = default_index(dir.path());

    assert_eq!(
        index.size("empty.txt"),
        Some(0),
        "an empty file has a known size of zero"
    );
    assert_eq!(index.size("hello.txt"), Some(5));
    assert_eq!(index.size("sub"), None);
    assert_eq!(index.size("nowhere.txt"), None, "nothing is indexed there");
    assert_eq!(kind_of(&index, "sub"), EntryKind::Dir);
    assert_eq!(kind_of(&index, "hello.txt"), EntryKind::File);
}

#[test]
fn a_size_is_read_once_and_answered_from_the_index_after_that() {
    let dir = tree(&[("grows.txt", "five!")]);
    let index = default_index(dir.path());

    assert_eq!(index.size("grows.txt"), Some(5));
    fs::write(dir.path().join("grows.txt"), "much longer now").expect("rewrite the file");

    assert_eq!(
        index.size("grows.txt"),
        Some(5),
        "the first read settled it, and nothing refreshes an answer already taken"
    );
}

#[test]
fn a_size_asked_for_the_first_time_reads_the_file_as_it_is_now() {
    let dir = tree(&[("grows.txt", "five!")]);
    let index = default_index(dir.path());

    fs::write(dir.path().join("grows.txt"), "much longer now").expect("rewrite the file");

    assert_eq!(
        index.size("grows.txt"),
        Some(15),
        "the walk recorded no size, so the first ask sees the file as it stands"
    );
}

#[test]
fn a_file_deleted_after_the_walk_has_no_size_and_is_still_indexed() {
    let dir = tree(&[("gone.txt", "here for now")]);
    let index = default_index(dir.path());

    fs::remove_file(dir.path().join("gone.txt")).expect("remove the file");

    assert_eq!(index.size("gone.txt"), None);
    assert_eq!(
        index.size("gone.txt"),
        None,
        "the failure is remembered too, so a loop pays for one failed stat rather than one per pass"
    );
    assert!(
        index.entry("gone.txt").is_some(),
        "the entry is what the walk saw, and no later read removes it"
    );
}

#[test]
fn entry_lookup_answers_none_for_a_path_that_was_not_indexed() {
    let dir = tree(&[("a.txt", "a")]);
    let index = default_index(dir.path());

    assert!(index.entry("a.txt").is_some());
    assert!(index.entry("missing.txt").is_none());
    assert!(index.entry("").is_none(), "the root itself is not an entry");
    assert_eq!(index.len(), 1);
    assert!(!index.is_empty());
}

#[test]
fn an_empty_tree_indexes_to_nothing() {
    let dir = tree(&[]);
    let index = default_index(dir.path());

    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
}

#[test]
fn a_root_gitignore_hides_matching_entries() {
    let dir = tree(&[
        (".gitignore", "*.log\n"),
        ("build.log", "noise"),
        ("main.rs", "code"),
    ]);
    let index = default_index(dir.path());

    assert_eq!(paths(&index), [".gitignore", "main.rs"]);
}

#[test]
fn a_nested_gitignore_applies_only_below_its_own_directory() {
    let dir = tree(&[
        ("src/.gitignore", "gen.rs\n"),
        ("gen.rs", "x"),
        ("src/gen.rs", "y"),
    ]);
    let index = default_index(dir.path());

    assert_eq!(paths(&index), ["gen.rs", "src", "src/.gitignore"]);
}

#[test]
fn a_deeper_negation_reincludes_a_file_a_shallower_rule_hid() {
    let dir = tree(&[
        (".gitignore", "*.log\n"),
        ("logs/.gitignore", "!keep.log\n"),
        ("logs/keep.log", "keep"),
        ("logs/drop.log", "drop"),
    ]);
    let index = default_index(dir.path());

    assert_eq!(
        paths(&index),
        [".gitignore", "logs", "logs/.gitignore", "logs/keep.log"]
    );
}

#[test]
fn an_ignored_directory_drops_its_whole_subtree_and_no_negation_reaches_inside() {
    let dir = tree(&[
        (".gitignore", "build/\n!build/keep.txt\n"),
        ("build/keep.txt", "keep"),
        ("build/out.o", "binary"),
        ("src/main.rs", "code"),
    ]);
    let index = default_index(dir.path());

    assert_eq!(paths(&index), [".gitignore", "src", "src/main.rs"]);
}

#[test]
fn the_git_directory_is_never_indexed() {
    let dir = tree(&[(".git/config", "[core]\n"), ("a.txt", "a")]);
    let index = default_index(dir.path());

    assert_eq!(paths(&index), ["a.txt"]);
}

#[test]
fn a_nested_git_directory_resets_the_inherited_ignore_stack() {
    let dir = tree(&[
        (".gitignore", "*.txt\n"),
        ("a.txt", "hidden by the outer rule"),
        ("inner/.git/HEAD", "ref: refs/heads/main\n"),
        (
            "inner/b.txt",
            "kept: the outer rule stops at the inner repository",
        ),
    ]);
    let index = default_index(dir.path());

    assert_eq!(paths(&index), [".gitignore", "inner", "inner/b.txt"]);
}

#[test]
fn ignore_sources_none_indexes_everything_except_the_git_directory() {
    let dir = tree(&[
        (".gitignore", "*.log\n"),
        (".git/config", "[core]\n"),
        ("build.log", "noise"),
    ]);

    let mut config = Config::default();
    config.ignore = IgnoreSources::None;
    let index = index(dir.path(), &config);

    assert_eq!(paths(&index), [".gitignore", "build.log"]);
}

#[test]
fn ignore_sources_order_from_none_upwards() {
    assert!(IgnoreSources::None < IgnoreSources::InTree);
    assert_eq!(IgnoreSources::default(), IgnoreSources::InTree);
}

#[test]
fn an_unreadable_gitignore_is_reported_and_its_rules_do_not_apply() {
    let gitignore_that_is_a_directory = (".gitignore/", "");
    let dir = tree(&[gitignore_that_is_a_directory, ("build.log", "noise")]);

    let outcome = Index::build(dir.path(), &Config::default()).expect("build the index");
    let reported: Vec<&Path> = outcome
        .errors()
        .iter()
        .filter(|error| matches!(error, BuildError::Ignore { .. }))
        .filter_map(BuildError::path)
        .collect();

    assert_eq!(reported, [dir.path().join(".gitignore").as_path()]);
    assert_eq!(paths(&outcome.into_index()), [".gitignore", "build.log"]);
}

#[test]
fn two_builds_of_one_tree_produce_identical_indexes() {
    let dir = tree(&[
        ("src/main.rs", "code"),
        ("src/lib.rs", "code"),
        ("docs/readme.md", "docs"),
    ]);

    let first = default_index(dir.path());
    let mut config = Config::default();
    config.threads = NonZeroUsize::new(1);
    let second = index(dir.path(), &config);

    assert!(first.entries().eq(second.entries()));
}

#[test]
fn fuzzy_paths_ranks_a_filename_hit_above_a_directory_hit() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("main/lib.rs", ""),
        ("src/other.rs", ""),
    ]);
    let index = default_index(dir.path());

    let ranked: Vec<&str> = index
        .fuzzy_paths("main", None, None)
        .iter()
        .map(|found| found.path)
        .collect();

    assert_eq!(ranked, ["src/main.rs", "main/lib.rs"]);
}

#[test]
fn fuzzy_paths_never_returns_a_directory() {
    let dir = tree(&[("src/main.rs", ""), ("src/nested/deep.rs", "")]);
    let index = default_index(dir.path());

    let ranked = index.fuzzy_paths("src", None, None);
    assert!(!ranked.is_empty(), "the query matches the files under src");

    for found in &ranked {
        assert_ne!(
            kind_of(&index, found.path),
            EntryKind::Dir,
            "{} is a directory",
            found.path
        );
    }
}

#[test]
fn fuzzy_paths_honours_a_limit_and_scores_descend() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("main/lib.rs", ""),
        ("main/mod.rs", ""),
    ]);
    let index = default_index(dir.path());

    let all = index.fuzzy_paths("main", None, None);
    assert_eq!(all.len(), 3);
    assert!(
        all.windows(2).all(|pair| pair[0].score >= pair[1].score),
        "best first"
    );

    let capped = index.fuzzy_paths("main", None, NonZeroUsize::new(2));
    assert_eq!(capped.len(), 2);
    assert_eq!(capped[0].path, all[0].path);
}

#[test]
fn an_empty_query_matches_every_indexed_file_in_index_order() {
    let dir = tree(&[("b.txt", ""), ("a.txt", ""), ("sub/", "")]);
    let index = default_index(dir.path());

    let ranked: Vec<&str> = index
        .fuzzy_paths("", None, None)
        .iter()
        .map(|found| found.path)
        .collect();

    assert_eq!(ranked, ["a.txt", "b.txt"]);
}

#[test]
fn fuzzy_paths_answers_nothing_for_a_query_no_path_contains() {
    let dir = tree(&[("src/main.rs", "")]);
    let index = default_index(dir.path());

    assert_eq!(index.fuzzy_paths("qqzz", None, None), []);
}

#[test]
fn glob_paths_lists_the_selected_paths_in_index_order() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("src/deep/mod.rs", ""),
        ("Cargo.toml", ""),
        ("README.md", ""),
    ]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.{rs,toml}"]).expect("compile the patterns");

    assert_eq!(
        index.glob_paths(&filter, None).items(),
        ["Cargo.toml", "src/deep/mod.rs", "src/main.rs"]
    );
}

#[test]
fn glob_paths_never_returns_a_directory() {
    let dir = tree(&[("src/main.rs", ""), ("src/deep/", "")]);
    let index = default_index(dir.path());

    let filter = Filter::new(["src/*"]).expect("compile the pattern");

    assert_eq!(index.glob_paths(&filter, None).items(), ["src/main.rs"]);
    assert_eq!(
        kind_of(&index, "src/deep"),
        EntryKind::Dir,
        "the directory is indexed"
    );
}

#[test]
fn glob_paths_honours_a_limit_by_taking_the_first_paths_in_index_order() {
    let dir = tree(&[("a.rs", ""), ("b.rs", ""), ("c.rs", "")]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.rs"]).expect("compile the pattern");

    let capped = index.glob_paths(&filter, NonZeroUsize::new(2));

    assert_eq!(capped.items(), ["a.rs", "b.rs"]);
    assert!(capped.truncated(), "c.rs matched and was turned away");
}

#[test]
fn glob_paths_answers_nothing_when_no_pattern_selects_anything() {
    let dir = tree(&[("src/main.rs", "")]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.go"]).expect("compile the pattern");

    assert_eq!(index.glob_paths(&filter, None).items(), Vec::<&str>::new());
}

#[test]
fn a_filter_narrows_what_fuzzy_paths_ranks() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("main/lib.rs", ""),
        ("doc/main.md", ""),
    ]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.rs"]).expect("compile the pattern");
    let ranked: Vec<&str> = index
        .fuzzy_paths("main", Some(&filter), None)
        .iter()
        .map(|found| found.path)
        .collect();

    assert_eq!(ranked, ["src/main.rs", "main/lib.rs"]);
}

#[test]
fn a_glob_query_and_a_fuzzy_query_select_from_the_same_set() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("build.log", ""),
        (".gitignore", "*.log\n"),
    ]);
    let index = default_index(dir.path());

    let filter = Filter::new(["**/*"]).expect("compile the pattern");
    let selected = index.glob_paths(&filter, None);
    let listed = selected.items();
    let ranked: Vec<&str> = index
        .fuzzy_paths("", None, None)
        .iter()
        .map(|found| found.path)
        .collect();

    assert_eq!(listed, ranked);
    assert!(
        !listed.contains(&"build.log"),
        "the .gitignore applied to both"
    );
}

#[cfg(unix)]
#[test]
fn a_gitignore_that_is_a_fifo_does_not_block_the_build() {
    let dir = tree(&[("build.log", "noise")]);
    mkfifo(&dir.path().join(".gitignore"));

    let root = dir.path().to_path_buf();
    let (report, errors) = within_deadline(move || {
        let outcome = Index::build(&root, &Config::default()).expect("build the index");
        let errors = format!("{:?}", outcome.errors());
        let carries_ignore = outcome
            .errors()
            .iter()
            .any(|error| matches!(error, BuildError::Ignore { .. }));

        let index = outcome.into_index();
        let listed: Vec<String> = paths(&index).iter().map(|&p| p.to_owned()).collect();

        (
            (carries_ignore, listed, kind_of(&index, ".gitignore")),
            errors,
        )
    });

    let (carries_ignore, listed, gitignore_kind) = report;
    assert!(
        carries_ignore,
        "a name that carries no rules is reported: {errors}"
    );
    assert_eq!(listed, [".gitignore", "build.log"]);
    assert_eq!(gitignore_kind, EntryKind::Other);
}

/// Runs `work` on its own thread and fails the test if it does not finish in time.
///
/// The failure this guards is a build that never returns: opening a fifo for reading waits for a writer that never
/// comes. Calling straight through would hang the whole test binary rather than fail one test, and the workspace test
/// job has no per-test deadline to catch it. A thread left blocked on a leaked fifo dies with the process.
#[cfg(unix)]
fn within_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    use std::sync::mpsc;
    use std::time::Duration;

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || sender.send(work()));

    receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("the build finished rather than blocking on the fifo")
}

#[cfg(unix)]
#[test]
fn a_symlinked_gitignore_applies_its_rules_like_git_does() {
    let dir = tree(&[("rules.txt", "*.log\n"), ("build.log", "noise")]);
    std::os::unix::fs::symlink("rules.txt", dir.path().join(".gitignore"))
        .expect("create a symlinked .gitignore");

    let index = default_index(dir.path());

    assert_eq!(
        paths(&index),
        [".gitignore", "rules.txt"],
        "git opens .gitignore through a symlink, and so does this"
    );
    assert_eq!(kind_of(&index, ".gitignore"), EntryKind::Symlink);
}

#[cfg(unix)]
#[test]
fn a_gitignore_symlinked_out_of_the_tree_is_refused_and_reported() {
    // Following it would let rules that do not travel with the tree decide the index, which is exactly what the
    // in-tree level promises to keep out.
    let outside = tree(&[("rules.txt", "*.log\n")]);
    let dir = tree(&[("build.log", "noise")]);
    std::os::unix::fs::symlink(
        outside.path().join("rules.txt"),
        dir.path().join(".gitignore"),
    )
    .expect("create an out-of-tree .gitignore symlink");

    let outcome = Index::build(dir.path(), &Config::default()).expect("build the index");

    assert!(
        outcome
            .errors()
            .iter()
            .any(|error| matches!(error, BuildError::Ignore { .. })),
        "the refusal is reported: {:?}",
        outcome.errors()
    );
    assert_eq!(
        paths(&outcome.into_index()),
        [".gitignore", "build.log"],
        "the outside rules never applied"
    );
}

#[cfg(unix)]
#[test]
fn a_gitignore_symlinked_within_the_tree_is_followed() {
    let dir = tree(&[("ci/rules.txt", "*.log\n"), ("build.log", "noise")]);
    std::os::unix::fs::symlink("ci/rules.txt", dir.path().join(".gitignore"))
        .expect("create an in-tree .gitignore symlink");

    let index = default_index(dir.path());

    assert_eq!(
        paths(&index),
        [".gitignore", "ci", "ci/rules.txt"],
        "an in-tree target travels with the tree, so git's follow is kept"
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_gitignore_symlink_is_reported_and_its_rules_do_not_apply() {
    let dir = tree(&[("build.log", "noise")]);
    std::os::unix::fs::symlink("nowhere", dir.path().join(".gitignore"))
        .expect("create a dangling .gitignore");

    let outcome = Index::build(dir.path(), &Config::default()).expect("build the index");

    assert!(
        outcome
            .errors()
            .iter()
            .any(|error| matches!(error, BuildError::Ignore { .. })),
        "the rules could not be read, so the build says so: {:?}",
        outcome.errors()
    );
    assert_eq!(paths(&outcome.into_index()), [".gitignore", "build.log"]);
}

#[cfg(target_os = "linux")]
#[test]
fn an_entry_whose_name_is_not_utf8_is_reported_rather_than_indexed() {
    // Two names that differ only in an invalid byte both decode to one replacement character, so indexing either would
    // give the two a single lookup key and a path that opens neither.
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = tree(&[("plain.txt", "x")]);
    for raw in [&b"\x80.txt"[..], &b"\x81.txt"[..]] {
        fs::write(dir.path().join(OsStr::from_bytes(raw)), "y").expect("write a non-UTF-8 name");
    }

    let outcome = Index::build(dir.path(), &Config::default()).expect("build the index");
    let reported = outcome
        .errors()
        .iter()
        .filter(|error| matches!(error, BuildError::Name { .. }))
        .count();

    assert_eq!(
        reported,
        2,
        "both names are reported: {:?}",
        outcome.errors()
    );
    assert_eq!(paths(&outcome.into_index()), ["plain.txt"]);
}

#[cfg(unix)]
#[test]
fn a_backslash_in_a_filename_is_not_a_path_separator_when_ranking() {
    // Both candidates match, and both hold `main`. Under the scorer's default separators `\` splits `main\x.rs` so its
    // basename is `x.rs` alone, which drops `main` out of the basename and lets the directory hit win. Only the
    // narrowed separator set puts the whole filename in the basename and ranks it first.
    let dir = tree(&[("main/x.rs", ""), ("main\\x.rs", "")]);
    let index = default_index(dir.path());

    let ranked: Vec<&str> = index
        .fuzzy_paths("main", None, None)
        .iter()
        .map(|found| found.path)
        .collect();

    assert_eq!(
        ranked,
        ["main\\x.rs", "main/x.rs"],
        "one unix filename outranks a directory hit of the same spelling"
    );
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed for {}", path.display());
}

#[cfg(unix)]
#[test]
fn a_symlink_is_indexed_as_itself_and_never_descended() {
    let dir = tree(&[("target/inner.txt", "x"), ("plain.txt", "y")]);
    std::os::unix::fs::symlink("target", dir.path().join("link"))
        .expect("create a directory symlink");

    let index = default_index(dir.path());

    assert_eq!(
        paths(&index),
        ["link", "plain.txt", "target", "target/inner.txt"],
        "the link is listed once and its target is not indexed a second time under it"
    );
    assert_eq!(kind_of(&index, "link"), EntryKind::Symlink);
    assert_eq!(
        index.size("link"),
        None,
        "a symlink is never followed, so it carries no size"
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_indexed_like_any_other() {
    let dir = tree(&[]);
    std::os::unix::fs::symlink("nowhere", dir.path().join("dangling"))
        .expect("create a dangling symlink");

    let index = default_index(dir.path());

    assert_eq!(paths(&index), ["dangling"]);
    assert_eq!(kind_of(&index, "dangling"), EntryKind::Symlink);
}

#[cfg(unix)]
#[test]
fn a_symlink_is_ignored_by_name_without_being_followed() {
    let dir = tree(&[(".gitignore", "link\n"), ("target/inner.txt", "x")]);
    std::os::unix::fs::symlink("target", dir.path().join("link"))
        .expect("create a directory symlink");

    let index = default_index(dir.path());

    assert_eq!(paths(&index), [".gitignore", "target", "target/inner.txt"]);
}

#[cfg(unix)]
#[test]
fn a_symlink_reports_the_target_it_stores_without_resolving_it() {
    let dir = tree(&[("nested/target.txt", "x")]);
    std::os::unix::fs::symlink("../nested/target.txt", dir.path().join("nested/link"))
        .expect("create a relative symlink");

    let index = default_index(dir.path());

    assert_eq!(
        index.entry("nested/link").and_then(Entry::target),
        Some("../nested/target.txt"),
        "the target is the link's own string, not a path resolved against the index root"
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_reports_the_target_that_resolves_to_nothing() {
    let dir = tree(&[]);
    std::os::unix::fs::symlink("nowhere", dir.path().join("dangling"))
        .expect("create a dangling symlink");

    let outcome = Index::build(dir.path(), &Config::default()).expect("build the index");

    assert!(
        outcome.errors().is_empty(),
        "a view onto nothing is what the tree holds, not a build failure: {:?}",
        outcome.errors()
    );
    assert_eq!(
        outcome.index().entry("dangling").and_then(Entry::target),
        Some("nowhere")
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_tree_reports_its_target_like_any_other() {
    let outside = tree(&[("elsewhere.txt", "x")]);
    let dir = tree(&[]);
    let target = outside.path().join("elsewhere.txt");
    std::os::unix::fs::symlink(&target, dir.path().join("link"))
        .expect("create an escaping symlink");

    let index = default_index(dir.path());

    assert_eq!(
        index.entry("link").and_then(Entry::target),
        target.to_str(),
        "where the target leads is never consulted, so leaving the tree changes nothing"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_whose_target_is_not_utf8_reports_no_target_and_no_error() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = tree(&[]);
    std::os::unix::fs::symlink(OsStr::from_bytes(b"\xff"), dir.path().join("link"))
        .expect("create a symlink to an unrepresentable target");

    let outcome = Index::build(dir.path(), &Config::default()).expect("build the index");

    assert!(
        outcome.errors().is_empty(),
        "an unresolvable view is a property of the link, never a gap in the index: {:?}",
        outcome.errors()
    );

    let index = outcome.into_index();
    assert_eq!(kind_of(&index, "link"), EntryKind::Symlink, "still indexed");
    assert_eq!(
        index.entry("link").and_then(Entry::target),
        None,
        "a lossy decode would read as a target rather than as the absence of one"
    );
}

#[test]
fn only_a_symlink_carries_a_target() {
    let dir = tree(&[("dir/file.txt", "x")]);
    let index = default_index(dir.path());

    assert_eq!(index.entry("dir").and_then(Entry::target), None);
    assert_eq!(index.entry("dir/file.txt").and_then(Entry::target), None);
}

fn listed<'a>(outcome: &Capped<Inspected<'a>>) -> Vec<&'a str> {
    outcome
        .items()
        .iter()
        .map(|found| found.entry().path())
        .collect()
}

fn list<'a>(index: &'a Index, dir: &str) -> Capped<Inspected<'a>> {
    index
        .list_dir(dir, &ListOptions::default())
        .unwrap_or_else(|error| panic!("list {dir}: {error}"))
}

#[test]
fn a_listing_holds_the_whole_subtree_depth_first_with_directories_included() {
    let dir = tree(&[
        ("src/net/tcp.rs", "t"),
        ("src/main.rs", "m"),
        ("src/lib.rs", "l"),
        ("outside.txt", "o"),
    ]);
    let index = default_index(dir.path());

    assert_eq!(
        listed(&list(&index, "src")),
        ["src/lib.rs", "src/main.rs", "src/net", "src/net/tcp.rs"],
        "the directory itself is not listed, its descendants are, and src/net comes before its own child"
    );
}

#[test]
fn the_empty_path_lists_the_index_root() {
    let dir = tree(&[("src/main.rs", "m"), ("README.md", "r")]);
    let index = default_index(dir.path());

    assert_eq!(listed(&list(&index, "")), paths(&index));
}

#[test]
fn a_depth_takes_only_that_many_levels_below_the_listed_directory() {
    let dir = tree(&[("a/b/c/d.txt", "d"), ("a/top.txt", "t")]);
    let index = default_index(dir.path());

    let one = index
        .list_dir("a", &ListOptions::default().depth(1))
        .expect("list a");
    assert_eq!(listed(&one), ["a/b", "a/top.txt"]);

    let two = index
        .list_dir("a", &ListOptions::default().depth(2))
        .expect("list a");
    assert_eq!(listed(&two), ["a/b", "a/b/c", "a/top.txt"]);

    assert_eq!(
        listed(&list(&index, "a")),
        ["a/b", "a/b/c", "a/b/c/d.txt", "a/top.txt"],
        "no depth takes the whole subtree"
    );
}

#[test]
fn a_limit_cuts_the_listing_and_says_so() {
    let dir = tree(&[("a/one.txt", "1"), ("a/two.txt", "2"), ("a/three.txt", "3")]);
    let index = default_index(dir.path());

    let capped = index
        .list_dir("a", &ListOptions::default().limit(2))
        .expect("list a");

    assert_eq!(listed(&capped), ["a/one.txt", "a/three.txt"]);
    assert!(capped.truncated());
}

#[test]
fn a_directory_holding_exactly_the_limit_is_not_truncated() {
    let dir = tree(&[("a/one.txt", "1"), ("a/two.txt", "2")]);
    let index = default_index(dir.path());

    let capped = index
        .list_dir("a", &ListOptions::default().limit(2))
        .expect("list a");

    assert_eq!(listed(&capped).len(), 2);
    assert!(
        !capped.truncated(),
        "the cap was reached, and nothing was turned away"
    );
}

#[test]
fn an_entry_a_depth_dropped_never_counts_as_truncation() {
    let dir = tree(&[("a/deep/buried.txt", "b"), ("a/top.txt", "t")]);
    let index = default_index(dir.path());

    let capped = index
        .list_dir("a", &ListOptions::default().depth(1).limit(2))
        .expect("list a");

    assert_eq!(listed(&capped), ["a/deep", "a/top.txt"]);
    assert!(
        !capped.truncated(),
        "a/deep/buried.txt was out of depth, which the limit never saw"
    );
}

#[test]
fn a_directory_with_nothing_indexed_under_it_lists_nothing() {
    let dir = tree(&[("empty/", ""), ("hidden/.gitignore", "*\n"), ("z.txt", "z")]);
    let index = default_index(dir.path());

    assert_eq!(listed(&list(&index, "empty")), [] as [&str; 0]);
    assert_eq!(
        listed(&list(&index, "hidden")),
        [] as [&str; 0],
        "`*` hides the .gitignore that carries it, as it does for git, and an emptied directory still lists"
    );
}

#[test]
fn a_listing_stops_at_a_sibling_whose_name_extends_the_directory_name() {
    let dir = tree(&[
        ("src/main.rs", "m"),
        ("src-gen/built.rs", "b"),
        ("src.bak/old.rs", "o"),
    ]);
    let index = default_index(dir.path());

    assert_eq!(
        listed(&list(&index, "src")),
        ["src/main.rs"],
        "src-gen and src.bak are siblings of src, not entries beneath it"
    );
}

#[test]
fn listing_a_path_that_is_not_indexed_is_not_found() {
    let dir = tree(&[(".gitignore", "secret/\n"), ("secret/key.txt", "k")]);
    let index = default_index(dir.path());

    assert_eq!(
        index
            .list_dir("nowhere", &ListOptions::default())
            .unwrap_err(),
        ListDirError::NotFound {
            path: "nowhere".to_owned()
        }
    );
    assert_eq!(
        index
            .list_dir("secret", &ListOptions::default())
            .unwrap_err(),
        ListDirError::NotFound {
            path: "secret".to_owned()
        },
        "an ignored directory reads the same as one that was never there"
    );
}

#[test]
fn listing_a_file_reports_what_is_indexed_there_instead() {
    let dir = tree(&[("main.rs", "m")]);
    let index = default_index(dir.path());

    assert_eq!(
        index
            .list_dir("main.rs", &ListOptions::default())
            .unwrap_err(),
        ListDirError::NotADirectory {
            path: "main.rs".to_owned(),
            kind: EntryKind::File,
        }
    );
}

#[cfg(unix)]
#[test]
fn listing_a_symlink_to_a_directory_is_not_a_listing_of_its_target() {
    let dir = tree(&[("target/inner.txt", "x")]);
    std::os::unix::fs::symlink("target", dir.path().join("link"))
        .expect("create a directory symlink");

    let index = default_index(dir.path());

    assert_eq!(
        index.list_dir("link", &ListOptions::default()).unwrap_err(),
        ListDirError::NotADirectory {
            path: "link".to_owned(),
            kind: EntryKind::Symlink,
        },
        "the walk never descended the link, so the index holds nothing under it"
    );
}

#[cfg(unix)]
#[test]
fn a_listing_carries_the_symlink_target_and_answers_sizes_beside_it() {
    let dir = tree(&[("a/file.txt", "twelve chars")]);
    std::os::unix::fs::symlink("file.txt", dir.path().join("a/link"))
        .expect("create a file symlink");

    let index = default_index(dir.path());
    let entries = list(&index, "a").into_items();

    let sizes: Vec<Option<u64>> = entries.iter().map(Inspected::size).collect();
    let targets: Vec<Option<&str>> = entries.iter().map(|found| found.entry().target()).collect();

    assert_eq!(listed(&list(&index, "a")), ["a/file.txt", "a/link"]);
    assert_eq!(sizes, [Some(12), None]);
    assert_eq!(targets, [None, Some("file.txt")]);
}

#[test]
fn listing_the_root_one_level_deep_takes_only_the_top_level_entries() {
    let dir = tree(&[("a/b/c/deep.txt", "d"), ("z.txt", "z"), ("m/one.txt", "1")]);
    let index = default_index(dir.path());

    let top = index
        .list_dir("", &ListOptions::default().depth(1))
        .expect("list the root");

    assert_eq!(
        listed(&top),
        ["a", "m", "z.txt"],
        "each top-level directory is listed and none of them is descended"
    );
}

#[test]
fn a_skipped_subtree_that_runs_to_the_end_of_the_index_ends_the_listing() {
    let dir = tree(&[("a/b/c/deep.txt", "d")]);
    let index = default_index(dir.path());

    let top = index
        .list_dir("", &ListOptions::default().depth(1))
        .expect("list the root");

    assert_eq!(listed(&top), ["a"]);
    assert!(!top.truncated());
}

#[test]
fn an_extension_settles_a_binary_file_without_the_file_being_read() {
    let dir = tree(&[("logo.png", "not really a png, and it does not matter")]);
    let index = default_index(dir.path());

    assert_eq!(
        index.is_binary("logo.png"),
        Some(true),
        "the name decides, so the contents are never consulted"
    );
}

/// The index of a complete build that classifies names by `binary` instead of the built-in set.
fn index_with_binary_extensions(root: &Path, binary: BinaryExtensions) -> Index {
    let mut config = Config::default();
    config.binary_extensions = binary;

    index(root, &config)
}

#[test]
fn an_extension_the_caller_adds_settles_its_files_without_them_being_read() {
    let dir = tree(&[("pack.vromfs", "plain lines, and the name still decides")]);
    let added = BinaryExtensions::new(["vromfs"]).expect("an extension");
    let index = index_with_binary_extensions(dir.path(), BinaryExtensions::builtin().union(&added));
    fs::remove_file(dir.path().join("pack.vromfs")).expect("remove the file");

    assert_eq!(
        index.is_binary("pack.vromfs"),
        Some(true),
        "the name decides, so a file that is gone still answers"
    );
}

#[test]
fn a_built_in_extension_the_caller_takes_out_is_read_instead() {
    let dir = tree(&[("logo.png", "not really a png, and now the contents decide")]);
    let taken_out = BinaryExtensions::new(["png"]).expect("an extension");
    let index = index_with_binary_extensions(
        dir.path(),
        BinaryExtensions::builtin().difference(&taken_out),
    );

    assert_eq!(index.is_binary("logo.png"), Some(false));

    let swept: Vec<Option<bool>> = index.inspected().map(|found| found.is_binary()).collect();
    assert_eq!(
        swept,
        [Some(false)],
        "the sweep classifies with the same set"
    );
}

#[test]
fn binary_extension_counts_show_an_extension_no_file_carries() {
    let dir = tree(&[
        ("a.png", ""),
        ("b.PNG", ""),
        ("pack.vromfs", ""),
        ("notes.txt", ""),
        ("shots.png/", ""),
    ]);
    let own = BinaryExtensions::new(["vromfs", "vromsf"]).expect("two extensions");
    let index = index_with_binary_extensions(
        dir.path(),
        own.union(&BinaryExtensions::new(["png"]).expect("an extension")),
    );

    assert_eq!(
        index.binary_extension_counts(),
        [("png", 2), ("vromfs", 1), ("vromsf", 0)],
        "a directory named like a file is not a file, and a misspelled extension matches nothing"
    );
}

#[test]
fn a_nul_settles_a_file_whose_extension_does_not() {
    let dir = tree(&[("notes.txt", "plain lines\n")]);
    fs::write(dir.path().join("blob.txt"), b"lines\0with a nul").expect("write a file with a nul");
    let index = default_index(dir.path());

    assert_eq!(index.is_binary("notes.txt"), Some(false));
    assert_eq!(index.is_binary("blob.txt"), Some(true));
}

#[test]
fn a_nul_past_the_probe_leaves_the_file_reported_as_text() {
    let dir = tree(&[]);
    let mut content = vec![b'a'; 64 * 1024];
    content.push(0);
    fs::write(dir.path().join("late.txt"), &content).expect("write a file with a late nul");

    let index = default_index(dir.path());

    assert_eq!(
        index.is_binary("late.txt"),
        Some(false),
        "the verdict is about the probed prefix, and the nul sits one byte past it"
    );
}

#[test]
fn a_nul_at_the_last_probed_byte_still_settles_the_file() {
    let dir = tree(&[]);
    let mut content = vec![b'a'; 64 * 1024 - 1];
    content.push(0);
    fs::write(dir.path().join("edge.txt"), &content).expect("write a file with a nul at the edge");

    let index = default_index(dir.path());

    assert_eq!(index.is_binary("edge.txt"), Some(true));
}

#[test]
fn only_a_regular_file_is_classified() {
    let dir = tree(&[("sub/inner.txt", "x")]);
    let index = default_index(dir.path());

    assert_eq!(
        index.is_binary("sub"),
        None,
        "a directory has no contents to probe"
    );
    assert_eq!(
        index.is_binary("nowhere.txt"),
        None,
        "nothing is indexed there"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_is_not_classified_through_its_target() {
    let dir = tree(&[("target.txt", "plain lines\n")]);
    std::os::unix::fs::symlink("target.txt", dir.path().join("link"))
        .expect("create a file symlink");

    let index = default_index(dir.path());

    assert_eq!(
        index.is_binary("link"),
        None,
        "classifying a link would mean reading through it, which no surface here does"
    );
}

#[test]
fn a_file_that_will_not_open_is_unclassified_and_stays_indexed() {
    let dir = tree(&[("gone.bin.txt", "here for now")]);
    let index = default_index(dir.path());

    fs::remove_file(dir.path().join("gone.bin.txt")).expect("remove the file");

    assert_eq!(index.is_binary("gone.bin.txt"), None);
    assert_eq!(
        index.is_binary("gone.bin.txt"),
        None,
        "the failure is remembered, so a second ask costs no second open"
    );
    assert!(index.entry("gone.bin.txt").is_some());
}

#[test]
fn a_classification_is_settled_once_and_answered_from_the_index_after_that() {
    let dir = tree(&[("mutates.txt", "plain lines\n")]);
    let index = default_index(dir.path());

    assert_eq!(index.is_binary("mutates.txt"), Some(false));
    fs::write(dir.path().join("mutates.txt"), b"now\0binary").expect("rewrite the file");

    assert_eq!(
        index.is_binary("mutates.txt"),
        Some(false),
        "the first read settled it, and nothing refreshes an answer already taken"
    );
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_fifo_does_not_block_a_classification() {
    let dir = tree(&[("notes.txt", "plain lines\n")]);
    let index = default_index(dir.path());
    assert_eq!(kind_of(&index, "notes.txt"), EntryKind::File);

    fs::remove_file(dir.path().join("notes.txt")).expect("remove the file");
    mkfifo(&dir.path().join("notes.txt"));

    let verdict = within_deadline(move || index.is_binary("notes.txt"));

    assert_eq!(
        verdict, None,
        "the walk recorded a regular file, and what the probe opened is not one"
    );
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_directory_is_not_read_as_content() {
    let dir = tree(&[("notes.txt", "plain lines\n")]);
    let index = default_index(dir.path());

    fs::remove_file(dir.path().join("notes.txt")).expect("remove the file");
    fs::create_dir(dir.path().join("notes.txt")).expect("put a directory in its place");

    assert_eq!(index.is_binary("notes.txt"), None);
}

#[test]
fn glob_paths_matching_exactly_the_limit_is_not_truncated() {
    let dir = tree(&[("a.rs", ""), ("b.rs", ""), ("c.md", "")]);
    let index = default_index(dir.path());
    let filter = Filter::new(["*.rs"]).expect("compile the pattern");

    let capped = index.glob_paths(&filter, NonZeroUsize::new(2));

    assert_eq!(capped.items(), ["a.rs", "b.rs"]);
    assert!(
        !capped.truncated(),
        "the limit was reached, and no matching path was turned away"
    );
}

#[test]
fn glob_paths_without_a_limit_is_never_truncated() {
    let dir = tree(&[("a.rs", ""), ("b.rs", "")]);
    let index = default_index(dir.path());
    let filter = Filter::new(["*.rs"]).expect("compile the pattern");

    assert!(!index.glob_paths(&filter, None).truncated());
}

#[test]
fn glob_paths_selecting_nothing_is_an_empty_answer_rather_than_a_cut_one() {
    let dir = tree(&[("a.rs", "")]);
    let index = default_index(dir.path());
    let filter = Filter::new(["*.toml"]).expect("compile the pattern");

    let found = index.glob_paths(&filter, NonZeroUsize::new(5));

    assert!(found.is_empty());
    assert!(!found.truncated());
}

#[test]
fn list_options_default_to_no_bounds_and_zero_lifts_one() {
    let dir = tree(&[("a/b/c.txt", "c"), ("a/top.txt", "t")]);
    let index = default_index(dir.path());

    assert_eq!(
        listed(&list(&index, "a")),
        listed(
            &index
                .list_dir("a", &ListOptions::default().depth(0))
                .expect("list a")
        ),
        "a zero depth reads as no depth, which is what NonZeroUsize makes unrepresentable"
    );
    assert_eq!(
        listed(&list(&index, "a")),
        listed(
            &index
                .list_dir("a", &ListOptions::default().limit(0))
                .expect("list a")
        ),
    );
}

#[test]
fn a_sweep_answers_the_same_facts_as_the_keyed_lookups() {
    /// One entry's path beside every fact the index reads on demand for it.
    type Facts<'a> = (&'a str, Option<u64>, Option<bool>);

    let dir = tree(&[("a.txt", "hello"), ("sub/b.bin", "x"), ("empty/", "")]);
    fs::write(dir.path().join("sub/blob.txt"), b"has\0a nul").expect("write a binary file");
    let index = default_index(dir.path());

    let swept: Vec<Facts<'_>> = index
        .inspected()
        .map(|found| (found.entry().path(), found.size(), found.is_binary()))
        .collect();
    let keyed: Vec<Facts<'_>> = index
        .entries()
        .map(|entry| {
            (
                entry.path(),
                index.size(entry.path()),
                index.is_binary(entry.path()),
            )
        })
        .collect();

    assert_eq!(
        swept, keyed,
        "one index, one set of answers, two routes to them"
    );
    assert_eq!(swept.len(), index.len());
}

#[test]
fn a_sweep_reads_nothing_for_an_entry_it_never_asks_about() {
    let dir = tree(&[("gone.txt", "here for now")]);
    let index = default_index(dir.path());

    let names: Vec<&str> = index
        .inspected()
        .map(|found| found.entry().name())
        .collect();
    assert_eq!(names, ["gone.txt"]);

    fs::remove_file(dir.path().join("gone.txt")).expect("remove the file");

    assert_eq!(
        index.size("gone.txt"),
        None,
        "walking past an entry leaves its slot unread, so the first ask is still the first read"
    );
}

#[test]
fn a_file_replaced_by_a_directory_has_no_size() {
    let dir = tree(&[("notes.txt", "hello")]);
    let index = default_index(dir.path());

    fs::remove_file(dir.path().join("notes.txt")).expect("remove the file");
    fs::create_dir(dir.path().join("notes.txt")).expect("put a directory in its place");

    assert_eq!(
        index.size("notes.txt"),
        None,
        "a directory reports a length of its own, which is not a file size and must never be published as one"
    );
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_symlink_is_measured_through_neither_surface() {
    let dir = tree(&[
        ("notes.txt", "plain lines\n"),
        ("elsewhere.txt", "much longer content here"),
    ]);
    let index = default_index(dir.path());
    assert_eq!(kind_of(&index, "notes.txt"), EntryKind::File);

    fs::remove_file(dir.path().join("notes.txt")).expect("remove the file");
    std::os::unix::fs::symlink("elsewhere.txt", dir.path().join("notes.txt"))
        .expect("put a symlink in its place");

    assert_eq!(
        index.size("notes.txt"),
        None,
        "the stat does not follow, so the target's size is never reported as this entry's"
    );
    assert_eq!(
        index.is_binary("notes.txt"),
        None,
        "and the probe refuses the link rather than reading through it"
    );
}
