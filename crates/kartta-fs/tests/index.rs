//! The public index contract: what a build records, which entries git's rules remove, and what the listing and the
//! path queries answer.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "clippy's allow-expect-in-tests and allow-panic-in-tests only cover #[cfg(test)] modules, and an \
              integration test crate is built without cfg(test) — so the workspace no-panic posture has to be waived \
              here, where a failed expectation is the assertion"
)]

mod common;

use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;

use common::tree;
use kartta_fs::{
    BinaryExtensions, BuildError, BuildOptions, BuildOutcome, Entry, EntryKind, Filter,
    FuzzyOptions, GlobOptions, IgnoreSources, Index, ListError, ListOptions, Listed, Listing,
    PrunedNames,
};

fn index(root: &Path, options: &BuildOptions) -> Index {
    let outcome = Index::build(root, options).expect("build the index");
    assert!(
        outcome.errors.is_empty(),
        "unexpected build errors: {:?}",
        outcome.errors
    );

    outcome.index
}

fn default_index(root: &Path) -> Index {
    index(root, &BuildOptions::default())
}

fn paths(index: &Index) -> Vec<String> {
    index
        .entries()
        .map(|entry| entry.path().to_owned())
        .collect()
}

fn paths_at(index: &Index, positions: impl IntoIterator<Item = usize>) -> Vec<String> {
    let mut paths = index.paths();
    positions
        .into_iter()
        .map(|at| paths.get(at).to_owned())
        .collect()
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
        assert_eq!(
            index.entry(entry.path()).as_ref(),
            Some(&entry),
            "{}",
            entry.path()
        );
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
fn paths_read_in_any_order_match_the_entries() {
    // constraint: `é` and `ê` share their first UTF-8 byte, so a name of one after a name of the other shares a prefix
    // that ends inside a character; 150 of them span several restarts
    let names: Vec<String> = (0..150)
        .map(|n| format!("d{}/{}{n}.txt", n % 3, ['é', 'ê'][n % 2]))
        .collect();
    let spec: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "")).collect();
    let dir = tree(&spec);
    let index = default_index(dir.path());
    let entries = paths(&index);

    // constraint: no name here holds a byte below `/`, so a plain byte sort gives the index order
    let mut expected: Vec<String> = ["d0", "d1", "d2"].map(String::from).into();
    expected.extend(names.iter().cloned());
    expected.sort();
    assert_eq!(entries, expected);

    let mut reader = index.paths();
    for at in (0..index.len()).rev().chain((0..index.len()).step_by(7)) {
        assert_eq!(reader.get(at), entries[at], "position {at}");
    }
    for path in &entries {
        assert_eq!(
            index.entry(path).map(|entry| entry.path().to_owned()),
            Some(path.clone())
        );
    }
}

#[test]
fn paths_sharing_a_prefix_too_long_for_a_byte_read_and_list_whole() {
    // constraint: every file under `long` shares more than 255 bytes with the entry before it, past the index's one-byte
    // shared length
    let long = "l".repeat(250);
    let names: Vec<String> = (0..140)
        .map(|n| format!("{long}/{}{n:03}", "f".repeat(10)))
        .chain(["after".to_owned()])
        .collect();
    let spec: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "")).collect();
    let dir = tree(&spec);
    let index = default_index(dir.path());
    let entries = paths(&index);

    let mut expected = vec!["after".to_owned(), long.clone()];
    expected.extend(names[..140].iter().cloned());
    assert_eq!(entries, expected);

    let mut reader = index.paths();
    for at in (0..index.len()).rev() {
        assert_eq!(reader.get(at), entries[at], "position {at}");
    }
    for path in &entries {
        assert!(index.entry(path).is_some(), "{path}");
    }

    let listed = index
        .list(&long, &ListOptions::default())
        .expect("list the long directory");
    assert_eq!(listed.total(), 140);
    assert_eq!(self::listed(&listed), names[..140]);
}

#[test]
fn a_root_level_entry_is_its_own_name() {
    let dir = tree(&[("top.txt", "x")]);
    let index = default_index(dir.path());

    assert_eq!(
        index.entry("top.txt").as_ref().map(Entry::name),
        Some("top.txt")
    );
}

#[test]
#[should_panic(expected = "out of bounds")]
fn a_fresh_reader_panics_on_a_position_past_the_last_entry() {
    let dir = tree(&[("only.txt", "")]);
    let index = default_index(dir.path());

    index.paths().get(usize::MAX);
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

    let mut options = BuildOptions::default();
    options.ignore_sources = IgnoreSources::None;
    let index = index(dir.path(), &options);

    assert_eq!(paths(&index), [".gitignore", "build.log"]);
}

#[test]
fn editor_and_os_clutter_is_never_indexed_or_descended_at_any_depth() {
    let dir = tree(&[
        (".idea/workspace.xml", "<project/>"),
        (".vscode/settings.json", "{}"),
        (".fleet/settings.json", "{}"),
        (".DS_Store", "\0"),
        ("src/.DS_Store", "\0"),
        ("src/.idea/modules.xml", "<project/>"),
        ("src/main.rs", "fn main() {}"),
    ]);
    let index = default_index(dir.path());

    assert_eq!(paths(&index), ["src", "src/main.rs"]);
}

#[test]
fn a_pruned_name_prunes_an_entry_of_any_kind() {
    let dir = tree(&[
        (".idea", "a file"),
        (".DS_Store/inside.txt", "x"),
        ("a.rs", ""),
    ]);
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.rs", dir.path().join(".vscode")).expect("create a symlink");

    let index = default_index(dir.path());

    assert_eq!(paths(&index), ["a.rs"]);
}

#[test]
fn no_ignore_rule_brings_a_pruned_name_back() {
    let dir = tree(&[
        (".gitignore", "!.idea/\n!.idea/**\n"),
        (".idea/workspace.xml", "<project/>"),
        ("a.rs", ""),
    ]);
    let mut none = BuildOptions::default();
    none.ignore_sources = IgnoreSources::None;

    assert_eq!(paths(&default_index(dir.path())), [".gitignore", "a.rs"]);
    assert_eq!(
        paths(&index(dir.path(), &none)),
        [".gitignore", "a.rs"],
        "pruning is not an ignore rule, so turning the rules off keeps it"
    );
}

#[test]
fn a_caller_composes_the_names_it_prunes() {
    let dir = tree(&[
        (".git/config", "[core]\n"),
        (".idea/workspace.xml", "<project/>"),
        (".vscode/settings.json", "{}"),
        ("node_modules/left-pad/index.js", ""),
        ("a.rs", ""),
    ]);
    let mut composed = BuildOptions::default();
    composed.pruned_names =
        PrunedNames::builtin().union(&PrunedNames::new(["node_modules"]).expect("a name"));
    let mut stated = BuildOptions::default();
    stated.pruned_names = PrunedNames::new([".idea"]).expect("a name");
    let mut none = BuildOptions::default();
    none.pruned_names = PrunedNames::new([""; 0]).expect("an empty set");

    assert_eq!(paths(&index(dir.path(), &composed)), ["a.rs"]);
    assert_eq!(
        paths(&index(dir.path(), &stated)),
        [
            ".vscode",
            ".vscode/settings.json",
            "a.rs",
            "node_modules",
            "node_modules/left-pad",
            "node_modules/left-pad/index.js"
        ]
    );
    assert_eq!(
        paths(&index(dir.path(), &none)),
        [
            ".idea",
            ".idea/workspace.xml",
            ".vscode",
            ".vscode/settings.json",
            "a.rs",
            "node_modules",
            "node_modules/left-pad",
            "node_modules/left-pad/index.js",
        ],
        "`.git` is pruned whatever the set holds"
    );
}

#[test]
fn the_root_is_never_pruned_whatever_its_name() {
    let dir = tree(&[(".idea/workspace.xml", "<project/>")]);
    let index = default_index(&dir.path().join(".idea"));

    assert_eq!(paths(&index), ["workspace.xml"]);
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

    let outcome = Index::build(dir.path(), &BuildOptions::default()).expect("build the index");
    let reported: Vec<&Path> = outcome
        .errors
        .iter()
        .filter(|error| matches!(error, BuildError::Ignore { .. }))
        .filter_map(BuildError::native_path)
        .collect();

    assert_eq!(reported, [dir.path().join(".gitignore").as_path()]);
    assert_eq!(paths(&outcome.index), [".gitignore", "build.log"]);
}

#[test]
fn two_builds_of_one_tree_produce_identical_indexes() {
    let dir = tree(&[
        ("src/main.rs", "code"),
        ("src/lib.rs", "code"),
        ("docs/readme.md", "docs"),
    ]);

    let first = default_index(dir.path());
    let mut options = BuildOptions::default();
    options.threads = NonZeroUsize::new(1);
    let second = index(dir.path(), &options);

    assert!(first.entries().eq(second.entries()));
}

#[test]
fn an_index_runs_on_exactly_the_thread_count_its_build_named() {
    let dir = tree(&[("main.rs", "code")]);
    let mut options = BuildOptions::default();
    options.threads = NonZeroUsize::new(3);

    assert_eq!(index(dir.path(), &options).threads(), 3);
}

#[test]
fn an_index_with_no_thread_count_runs_on_the_ambient_pool() {
    let dir = tree(&[("main.rs", "code")]);

    assert_eq!(
        default_index(dir.path()).threads(),
        rayon::current_num_threads()
    );
}

fn ambient_pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("start a thread pool")
}

#[test]
fn a_consumer_runs_its_parallel_work_on_the_threads_the_build_named() {
    let dir = tree(&[("main.rs", "code")]);
    let mut options = BuildOptions::default();
    options.threads = NonZeroUsize::new(3);
    let index = index(dir.path(), &options);

    let threads = ambient_pool(1).install(|| index.on_threads(rayon::current_num_threads));

    assert_eq!(threads, 3);
}

#[test]
fn a_consumer_of_an_index_with_no_thread_count_runs_on_the_ambient_pool() {
    let dir = tree(&[("main.rs", "code")]);
    let index = default_index(dir.path());

    let threads = ambient_pool(2).install(|| index.on_threads(rayon::current_num_threads));

    assert_eq!(threads, 2);
}

#[test]
fn a_build_outcome_hands_over_its_index_and_its_report_together() {
    let gitignore_that_is_a_directory = (".gitignore/", "");
    let dir = tree(&[gitignore_that_is_a_directory, ("main.rs", "code")]);

    let BuildOutcome { index, errors, .. } =
        Index::build(dir.path(), &BuildOptions::default()).expect("build the index");

    assert!(index.entry("main.rs").is_some());
    assert!(matches!(errors.as_slice(), [BuildError::Ignore { .. }]));
}

#[test]
fn fuzzy_ranks_a_filename_hit_above_a_directory_hit() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("main/lib.rs", ""),
        ("src/other.rs", ""),
    ]);
    let index = default_index(dir.path());

    let ranked = index.fuzzy("main", &FuzzyOptions::default());

    assert_eq!(
        paths_at(&index, ranked.items().iter().map(|found| found.position)),
        ["src/main.rs", "main/lib.rs"]
    );
}

#[test]
fn fuzzy_never_returns_a_directory() {
    let dir = tree(&[("src/main.rs", ""), ("src/nested/deep.rs", "")]);
    let index = default_index(dir.path());

    let ranked = index.fuzzy("src", &FuzzyOptions::default());
    assert!(
        !ranked.items().is_empty(),
        "the query matches the files under src"
    );

    for path in paths_at(&index, ranked.items().iter().map(|found| found.position)) {
        assert_ne!(
            kind_of(&index, &path),
            EntryKind::Dir,
            "{path} is a directory"
        );
    }
    assert_eq!(ranked.matched(), Some(2), "nor counts one");
}

#[test]
fn fuzzy_honours_a_limit_and_scores_descend() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("main/lib.rs", ""),
        ("main/mod.rs", ""),
    ]);
    let index = default_index(dir.path());

    let all = index.fuzzy("main", &FuzzyOptions::default());
    assert_eq!(all.items().len(), 3);
    assert!(
        all.items()
            .windows(2)
            .all(|pair| pair[0].score >= pair[1].score),
        "best first"
    );
    assert_eq!(all.matched(), Some(3));

    let capped = index.fuzzy("main", &FuzzyOptions::default().limit(2));
    assert_eq!(capped.items().len(), 2);
    assert_eq!(capped.items()[0].position, all.items()[0].position);
    assert_eq!(
        capped.matched(),
        Some(3),
        "the count holds the match the limit left out"
    );
}

#[test]
fn a_ranked_answer_is_truncated_only_when_the_limit_turned_a_match_away() {
    let dir = tree(&[("a/main.rs", ""), ("b/main.rs", ""), ("c/main.rs", "")]);
    let index = default_index(dir.path());

    for query in ["main", ""] {
        let truncated = |limit| {
            index
                .fuzzy(query, &FuzzyOptions::default().limit(limit))
                .truncated()
        };
        assert!(truncated(2), "query {query:?} under the matches");
        assert!(!truncated(3), "query {query:?} at exactly the matches");
        assert!(!truncated(0), "query {query:?} with no limit");
    }
}

#[test]
fn an_empty_query_matches_every_indexed_file_in_index_order() {
    let dir = tree(&[("b.txt", ""), ("a.txt", ""), ("sub/", "")]);
    let index = default_index(dir.path());

    let ranked = index.fuzzy("", &FuzzyOptions::default());

    assert_eq!(
        paths_at(&index, ranked.items().iter().map(|found| found.position)),
        ["a.txt", "b.txt"]
    );
}

#[test]
fn an_empty_query_counts_nothing_with_or_without_a_limit() {
    let dir = tree(&[("b.txt", ""), ("a.txt", "")]);
    let index = default_index(dir.path());

    for (query, limit) in ["", " "]
        .iter()
        .flat_map(|&query| [0, 1, 3].map(|limit| (query, limit)))
    {
        let ranked = index.fuzzy(query, &FuzzyOptions::default().limit(limit));
        assert_eq!(ranked.matched(), None, "query {query:?}, limit {limit}");
    }
    assert_eq!(
        index
            .fuzzy("", &FuzzyOptions::default().limit(1))
            .items()
            .len(),
        1,
        "it stops at the limit"
    );
}

#[test]
fn fuzzy_answers_nothing_for_a_query_no_path_contains() {
    let dir = tree(&[("src/main.rs", "")]);
    let index = default_index(dir.path());

    let ranked = index.fuzzy("qqzz", &FuzzyOptions::default());

    assert_eq!(ranked.items(), []);
    assert_eq!(ranked.matched(), Some(0), "counted, and none matched");
}

#[test]
fn glob_returns_the_selected_paths_in_index_order() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("src/deep/mod.rs", ""),
        ("Cargo.toml", ""),
        ("README.md", ""),
    ]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.{rs,toml}"]).expect("compile the patterns");
    let selected = index.glob(&filter, &GlobOptions::default());

    assert_eq!(
        paths_at(&index, selected.items().iter().copied()),
        ["Cargo.toml", "src/deep/mod.rs", "src/main.rs"]
    );
}

#[test]
fn glob_never_returns_a_directory() {
    let dir = tree(&[("src/main.rs", ""), ("src/deep/", "")]);
    let index = default_index(dir.path());

    let filter = Filter::new(["src/*"]).expect("compile the pattern");
    let selected = index.glob(&filter, &GlobOptions::default());

    assert_eq!(
        paths_at(&index, selected.items().iter().copied()),
        ["src/main.rs"]
    );
    assert_eq!(
        kind_of(&index, "src/deep"),
        EntryKind::Dir,
        "the directory is indexed"
    );
}

#[test]
fn glob_honours_a_limit_by_taking_the_first_paths_in_index_order() {
    let dir = tree(&[("a.rs", ""), ("b.rs", ""), ("c.rs", "")]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.rs"]).expect("compile the pattern");

    let capped = index.glob(&filter, &GlobOptions::default().limit(2));

    assert_eq!(
        paths_at(&index, capped.items().iter().copied()),
        ["a.rs", "b.rs"]
    );
    assert!(capped.truncated(), "c.rs matched and was turned away");
}

#[test]
fn a_cut_glob_counts_every_match_and_exclusion_past_its_limit() {
    let dir = tree(&[
        ("a.rs", ""),
        ("b.rs", ""),
        ("c/x.rs", ""),
        ("d.md", ""),
        ("e/skipped.rs", ""),
        ("f.rs", ""),
    ]);
    let index = default_index(dir.path());
    let sources = filter(&["*.rs"]);
    let skipped = filter(&["e"]);

    let cut = index.glob(&sources, &GlobOptions::default().exclude(&skipped).limit(1));

    assert_eq!(paths_at(&index, cut.items().iter().copied()), ["a.rs"]);
    assert_eq!(
        cut.matched(),
        4,
        "a.rs, b.rs, c/x.rs and f.rs, read past the cut"
    );
    assert!(cut.truncated());
    assert_eq!(cut.excluded(), 1, "e/skipped.rs, past the cut");
    let whole = index.glob(&sources, &GlobOptions::default().exclude(&skipped));
    assert_eq!(
        (cut.matched(), cut.excluded()),
        (whole.matched(), whole.excluded()),
        "the same counts as a glob the limit does not cut"
    );
}

#[test]
fn glob_answers_nothing_when_no_pattern_selects_anything() {
    let dir = tree(&[("src/main.rs", "")]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.go"]).expect("compile the pattern");

    assert_eq!(
        index.glob(&filter, &GlobOptions::default()).items(),
        Vec::<usize>::new()
    );
}

#[test]
fn a_filter_narrows_what_fuzzy_ranks() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("main/lib.rs", ""),
        ("doc/main.md", ""),
    ]);
    let index = default_index(dir.path());

    let filter = Filter::new(["*.rs"]).expect("compile the pattern");
    let ranked = index.fuzzy("main", &FuzzyOptions::default().filter(&filter));

    assert_eq!(
        paths_at(&index, ranked.items().iter().map(|found| found.position)),
        ["src/main.rs", "main/lib.rs"]
    );
    assert_eq!(
        ranked.matched(),
        Some(2),
        "a path the filter does not select is never counted"
    );
}

#[test]
fn a_glob_exclusion_removes_a_file_and_a_directory_with_everything_beneath_it() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("src/notes.md", ""),
        ("vendor/dep/lib.rs", ""),
        ("vendor/dep/more.rs", ""),
        ("vendor/dep/README.md", ""),
        ("generated.rs", ""),
    ]);
    let index = default_index(dir.path());
    let sources = filter(&["*.rs"]);
    let left_out = filter(&["vendor", "generated.rs"]);

    let found = index.glob(&sources, &GlobOptions::default().exclude(&left_out));

    assert_eq!(
        paths_at(&index, found.items().iter().copied()),
        ["src/main.rs"],
        "vendor names neither file beneath it, and still removes both"
    );
    assert_eq!(
        found.excluded(),
        3,
        "both .rs files under vendor and generated.rs; the .md files were never selected"
    );
}

#[test]
fn a_glob_limit_stays_exact_under_an_exclusion() {
    let dir = tree(&[
        ("a/skipped.rs", ""),
        ("b.rs", ""),
        ("c.rs", ""),
        ("d/skipped.rs", ""),
    ]);
    let index = default_index(dir.path());
    let sources = filter(&["*.rs"]);
    let skipped = filter(&["a", "d"]);

    let whole = index.glob(&sources, &GlobOptions::default().exclude(&skipped).limit(2));
    assert_eq!(
        paths_at(&index, whole.items().iter().copied()),
        ["b.rs", "c.rs"]
    );
    assert!(
        !whole.truncated(),
        "the excluded paths neither count toward the limit nor stand past it"
    );
    assert_eq!(whole.excluded(), 2);

    let cut = index.glob(&sources, &GlobOptions::default().exclude(&skipped).limit(1));
    assert_eq!(paths_at(&index, cut.items().iter().copied()), ["b.rs"]);
    assert!(cut.truncated(), "c.rs matched and was turned away");
    assert_eq!(
        cut.matched(),
        2,
        "b.rs and c.rs, never a path the exclusion removed"
    );
    assert_eq!(
        cut.excluded(),
        2,
        "a/skipped.rs before the cut and d/skipped.rs past it"
    );
}

#[test]
fn a_fuzzy_exclusion_removes_a_file_and_a_directory_with_everything_beneath_it() {
    let dir = tree(&[
        ("src/main.rs", ""),
        ("src/domain.rs", ""),
        ("vendor/main.rs", ""),
        ("vendor/deep/main.c", ""),
        ("main.log", ""),
    ]);
    let index = default_index(dir.path());
    let left_out = filter(&["vendor", "*.log"]);

    let everything = index.fuzzy("main", &FuzzyOptions::default().limit(1));
    assert_eq!(
        paths_at(
            &index,
            everything.items().iter().map(|found| found.position)
        ),
        ["main.log"],
        "the shortest filename hit ranks first when nothing is excluded"
    );
    assert_eq!(everything.matched(), Some(5));

    let ranked = index.fuzzy("main", &FuzzyOptions::default().exclude(&left_out));
    assert_eq!(
        paths_at(&index, ranked.items().iter().map(|found| found.position)),
        ["src/main.rs", "src/domain.rs"]
    );

    let capped = index.fuzzy("main", &FuzzyOptions::default().exclude(&left_out).limit(1));
    assert_eq!(
        paths_at(&index, capped.items().iter().map(|found| found.position)),
        ["src/main.rs"],
        "the limit is spent on what the exclusion leaves"
    );
    assert_eq!(
        capped.matched(),
        Some(2),
        "and the count covers only what it leaves"
    );
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
    let selected = index.glob(&filter, &GlobOptions::default());
    let listed = paths_at(&index, selected.items().iter().copied());
    let ranked = paths_at(
        &index,
        index
            .fuzzy("", &FuzzyOptions::default())
            .items()
            .iter()
            .map(|found| found.position),
    );

    assert_eq!(listed, ranked);
    assert!(
        !listed.iter().any(|path| path == "build.log"),
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
        let outcome = Index::build(&root, &BuildOptions::default()).expect("build the index");
        let errors = format!("{:?}", outcome.errors);
        let carries_ignore = outcome
            .errors
            .iter()
            .any(|error| matches!(error, BuildError::Ignore { .. }));

        let index = outcome.index;
        let listed = paths(&index);

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

// constraint: the test job has no per-test deadline, so a build blocked on a fifo would hang the whole binary rather
// than fail one test; a thread left blocked dies with the process
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
    let outside = tree(&[("rules.txt", "*.log\n")]);
    let dir = tree(&[("build.log", "noise")]);
    std::os::unix::fs::symlink(
        outside.path().join("rules.txt"),
        dir.path().join(".gitignore"),
    )
    .expect("create an out-of-tree .gitignore symlink");

    let outcome = Index::build(dir.path(), &BuildOptions::default()).expect("build the index");

    assert!(
        outcome
            .errors
            .iter()
            .any(|error| matches!(error, BuildError::Ignore { .. })),
        "the refusal is reported: {:?}",
        outcome.errors
    );
    assert_eq!(
        paths(&outcome.index),
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

    let outcome = Index::build(dir.path(), &BuildOptions::default()).expect("build the index");

    assert!(
        outcome
            .errors
            .iter()
            .any(|error| matches!(error, BuildError::Ignore { .. })),
        "the rules could not be read, so the build says so: {:?}",
        outcome.errors
    );
    assert_eq!(paths(&outcome.index), [".gitignore", "build.log"]);
}

#[cfg(target_os = "linux")]
#[test]
fn an_entry_whose_name_is_not_utf8_is_reported_rather_than_indexed() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = tree(&[("plain.txt", "x")]);
    for raw in [&b"\x80.txt"[..], &b"\x81.txt"[..]] {
        fs::write(dir.path().join(OsStr::from_bytes(raw)), "y").expect("write a non-UTF-8 name");
    }

    let outcome = Index::build(dir.path(), &BuildOptions::default()).expect("build the index");
    let reported = outcome
        .errors
        .iter()
        .filter(|error| matches!(error, BuildError::Name { .. }))
        .count();

    assert_eq!(reported, 2, "both names are reported: {:?}", outcome.errors);
    assert_eq!(paths(&outcome.index), ["plain.txt"]);
}

#[cfg(unix)]
#[test]
fn a_backslash_in_a_filename_is_not_a_path_separator_when_ranking() {
    let dir = tree(&[("main/x.rs", ""), ("main\\x.rs", "")]);
    let index = default_index(dir.path());

    let ranked = index.fuzzy("main", &FuzzyOptions::default());

    assert_eq!(
        paths_at(&index, ranked.items().iter().map(|found| found.position)),
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
        index.entry("nested/link").and_then(|entry| entry.target()),
        Some("../nested/target.txt"),
        "the target is the link's own string, not a path resolved against the index root"
    );
}

#[cfg(unix)]
#[test]
fn each_symlink_of_a_directory_classified_in_parallel_reports_its_own_target() {
    let dir = tree(&[]);
    for at in 0..200 {
        std::os::unix::fs::symlink(
            format!("to{at:03}"),
            dir.path().join(format!("link{at:03}")),
        )
        .expect("create a symlink");
    }

    let index = default_index(dir.path());

    let targets: Vec<(String, Option<String>)> = index
        .entries()
        .map(|entry| (entry.path().to_owned(), entry.target().map(str::to_owned)))
        .collect();
    let expected: Vec<(String, Option<String>)> = (0..200)
        .map(|at| (format!("link{at:03}"), Some(format!("to{at:03}"))))
        .collect();
    assert_eq!(
        targets, expected,
        "a directory of 64 entries or more is decided on several threads, in whatever order they read the targets"
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_reports_the_target_that_resolves_to_nothing() {
    let dir = tree(&[]);
    std::os::unix::fs::symlink("nowhere", dir.path().join("dangling"))
        .expect("create a dangling symlink");

    let outcome = Index::build(dir.path(), &BuildOptions::default()).expect("build the index");

    assert!(
        outcome.errors.is_empty(),
        "a view onto nothing is what the tree holds, not a build failure: {:?}",
        outcome.errors
    );
    assert_eq!(
        outcome
            .index
            .entry("dangling")
            .and_then(|entry| entry.target()),
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
        index.entry("link").and_then(|entry| entry.target()),
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

    let outcome = Index::build(dir.path(), &BuildOptions::default()).expect("build the index");

    assert!(
        outcome.errors.is_empty(),
        "an unresolvable view is a property of the link, never a gap in the index: {:?}",
        outcome.errors
    );

    let index = outcome.index;
    assert_eq!(kind_of(&index, "link"), EntryKind::Symlink, "still indexed");
    assert_eq!(
        index.entry("link").and_then(|entry| entry.target()),
        None,
        "a lossy decode would read as a target rather than as the absence of one"
    );
}

#[test]
fn only_a_symlink_carries_a_target() {
    let dir = tree(&[("dir/file.txt", "x")]);
    let index = default_index(dir.path());

    assert_eq!(index.entry("dir").and_then(|entry| entry.target()), None);
    assert_eq!(
        index.entry("dir/file.txt").and_then(|entry| entry.target()),
        None
    );
}

fn listed(listing: &Listing<'_>) -> Vec<String> {
    listing
        .items()
        .iter()
        .map(|found| found.entry().path().to_owned())
        .collect()
}

fn unlisted(listing: &Listing<'_>) -> Vec<(String, Option<usize>)> {
    listing
        .items()
        .iter()
        .map(|found| (found.entry().path().to_owned(), found.unlisted()))
        .collect()
}

fn owned(rows: &[(&str, Option<usize>)]) -> Vec<(String, Option<usize>)> {
    rows.iter()
        .map(|&(path, unlisted)| (path.to_owned(), unlisted))
        .collect()
}

fn list<'a>(index: &'a Index, dir: &str) -> Listing<'a> {
    index
        .list(dir, &ListOptions::default())
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
fn a_listing_bounded_by_nothing_but_its_limit_allocates_exactly_what_it_returns() {
    let names: Vec<String> = (0..40).map(|n| format!("d{}/f{n:02}.txt", n % 4)).collect();
    let spec: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "")).collect();
    let dir = tree(&spec);
    let index = default_index(dir.path());

    for dir in ["", "d1"] {
        for options in [ListOptions::default(), ListOptions::default().limit(7)] {
            let items = index
                .list(dir, &options)
                .unwrap_or_else(|error| panic!("list {dir:?}: {error}"))
                .into_items();
            assert_eq!(items.capacity(), items.len(), "{dir:?} {options:?}");
        }
    }
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
        .list("a", &ListOptions::default().depth(1))
        .expect("list a");
    assert_eq!(listed(&one), ["a/b", "a/top.txt"]);

    let two = index
        .list("a", &ListOptions::default().depth(2))
        .expect("list a");
    assert_eq!(listed(&two), ["a/b", "a/b/c", "a/top.txt"]);

    assert_eq!(
        listed(&list(&index, "a")),
        ["a/b", "a/b/c", "a/b/c/d.txt", "a/top.txt"],
        "no depth takes the whole subtree"
    );
}

#[test]
fn a_directory_at_the_depth_bound_says_how_many_entries_lie_beneath_it() {
    let dir = tree(&[("a/b/c/d.txt", "d"), ("a/empty/", ""), ("a/top.txt", "t")]);
    let index = default_index(dir.path());

    let one = index
        .list("a", &ListOptions::default().depth(1))
        .expect("list a");
    assert_eq!(
        unlisted(&one),
        owned(&[("a/b", Some(2)), ("a/empty", Some(0)), ("a/top.txt", None)]),
        "a/b holds c and c/d.txt; a/empty holds nothing, which reads differently from not being looked at"
    );

    let two = index
        .list("a", &ListOptions::default().depth(2))
        .expect("list a");
    assert_eq!(
        unlisted(&two),
        owned(&[
            ("a/b", None),
            ("a/b/c", Some(1)),
            ("a/empty", None),
            ("a/top.txt", None)
        ]),
        "a directory whose contents the listing holds has nothing unlisted to report"
    );
}

#[test]
fn a_listing_reports_how_many_entries_the_index_holds_under_the_directory() {
    let dir = tree(&[("a/b/c/d.txt", "d"), ("a/top.txt", "t"), ("z.txt", "z")]);
    let index = default_index(dir.path());

    for options in [
        ListOptions::default(),
        ListOptions::default().depth(1),
        ListOptions::default().limit(1),
    ] {
        let listing = index.list("a", &options).expect("list a");
        assert_eq!(
            listing.total(),
            4,
            "a/b, a/b/c, a/b/c/d.txt and a/top.txt, whatever {options:?} cut"
        );
    }

    assert_eq!(list(&index, "").total(), index.len());
    assert_eq!(list(&index, "a/b/c").total(), 1);
}

fn filter(patterns: &[&str]) -> Filter {
    Filter::new(patterns).expect("compile the collapse filter")
}

#[test]
fn a_collapsed_directory_is_listed_with_its_contents_counted_at_any_level() {
    let dir = tree(&[
        ("lib/vendor/c.rs", "c"),
        ("src/main.rs", "m"),
        ("vendor/a/x.rs", "x"),
        ("vendor/b.rs", "b"),
    ]);
    let index = default_index(dir.path());
    let vendor = filter(&["**/vendor"]);

    let listing = index
        .list("", &ListOptions::default().collapse(&vendor))
        .expect("list the root");

    assert_eq!(
        unlisted(&listing),
        owned(&[
            ("lib", None),
            ("lib/vendor", Some(1)),
            ("src", None),
            ("src/main.rs", None),
            ("vendor", Some(3)),
        ]),
        "both vendor directories are listed, neither is entered, and each says what it holds"
    );
}

#[test]
fn the_listed_directory_is_never_collapsed() {
    let dir = tree(&[("vendor/a/x.rs", "x"), ("vendor/b.rs", "b")]);
    let index = default_index(dir.path());
    let vendor = filter(&["**/vendor"]);

    let listing = index
        .list("vendor", &ListOptions::default().collapse(&vendor))
        .expect("list vendor");

    assert_eq!(
        listed(&listing),
        ["vendor/a", "vendor/a/x.rs", "vendor/b.rs"],
        "naming a collapsed directory by path is how a caller sees inside it"
    );
}

#[test]
fn a_collapsed_directory_counts_once_toward_the_limit() {
    let dir = tree(&[("big/one.rs", "1"), ("big/two.rs", "2"), ("small.rs", "s")]);
    let index = default_index(dir.path());
    let big = filter(&["big"]);

    let listing = index
        .list("", &ListOptions::default().collapse(&big).limit(2))
        .expect("list the root");

    assert_eq!(
        unlisted(&listing),
        owned(&[("big", Some(2)), ("small.rs", None)])
    );
    assert!(
        !listing.truncated(),
        "big's two files were counted rather than listed, so the limit never saw them"
    );
}

#[test]
fn an_excluded_entry_leaves_the_listing_with_its_subtree_and_is_counted() {
    let dir = tree(&[
        (".github/workflows/ci.yml", "c"),
        (".hidden.txt", "h"),
        ("src/.cache/blob", "b"),
        ("src/main.rs", "m"),
    ]);
    let index = default_index(dir.path());
    let dotfiles = filter(&["**/.*"]);

    let listing = index
        .list("", &ListOptions::default().exclude(&dotfiles))
        .expect("list the root");

    assert_eq!(listed(&listing), ["src", "src/main.rs"]);
    assert_eq!(
        listing.excluded(),
        6,
        ".github with its two descendants, .hidden.txt, and src/.cache with its blob"
    );
}

#[test]
fn exclusion_wins_over_collapse_and_spares_the_listed_directory() {
    let dir = tree(&[(".github/workflows/ci.yml", "c"), ("vendor/dep.rs", "d")]);
    let index = default_index(dir.path());
    let dotfiles = filter(&["**/.*"]);
    let vendor = filter(&["vendor"]);

    let listing = index
        .list(
            "",
            &ListOptions::default().exclude(&vendor).collapse(&vendor),
        )
        .expect("list the root");
    assert_eq!(
        listed(&listing),
        [".github", ".github/workflows", ".github/workflows/ci.yml"]
    );

    let inside = index
        .list(".github", &ListOptions::default().exclude(&dotfiles))
        .expect("list .github");
    assert_eq!(
        listed(&inside),
        [".github/workflows", ".github/workflows/ci.yml"],
        "naming an excluded directory by path still shows what is inside it"
    );
}

#[test]
fn an_excluded_entry_never_counts_toward_the_limit() {
    let dir = tree(&[("a.txt", "a"), ("b.txt", "b"), ("z.tmp", "z")]);
    let index = default_index(dir.path());
    let scratch = filter(&["*.tmp"]);

    let listing = index
        .list("", &ListOptions::default().exclude(&scratch).limit(2))
        .expect("list the root");

    assert_eq!(listed(&listing), ["a.txt", "b.txt"]);
    assert!(
        !listing.truncated(),
        "z.tmp was excluded, so the limit turned nothing away"
    );
    assert_eq!(listing.excluded(), 1);
}

#[test]
fn a_listing_through_a_pruned_name_says_which_name_pruned_it() {
    let dir = tree(&[
        (".git/config", "[core]\n"),
        (".idea/scopes/all.xml", "<project/>"),
        ("src/main.rs", ""),
    ]);
    let index = default_index(dir.path());
    let pruned = |path: &str, name: &str| ListError::Pruned {
        path: path.to_owned(),
        name: name.to_owned(),
    };

    assert_eq!(
        index.list(".idea/scopes", &ListOptions::default()).err(),
        Some(pruned(".idea/scopes", ".idea"))
    );
    assert_eq!(
        index.list("src/.git", &ListOptions::default()).err(),
        Some(pruned("src/.git", ".git"))
    );
    assert_eq!(
        pruned("src/.git", ".git").label(),
        "not_found",
        "an answer spells a pruned path as any path the index holds nothing at"
    );
    let absent = index.list("src/absent", &ListOptions::default()).err();
    assert_eq!(
        absent,
        Some(ListError::NotFound {
            path: "src/absent".to_owned()
        }),
        "a path through no pruned name is merely not indexed"
    );
    assert_eq!(absent.map(|error| error.label()), Some("not_found"));
    assert_eq!(
        index
            .list("src/main.rs", &ListOptions::default())
            .err()
            .map(|error| error.label()),
        Some("not_a_directory")
    );
}

#[test]
fn a_limit_cuts_the_listing_and_says_so() {
    let dir = tree(&[("a/one.txt", "1"), ("a/two.txt", "2"), ("a/three.txt", "3")]);
    let index = default_index(dir.path());

    let capped = index
        .list("a", &ListOptions::default().limit(2))
        .expect("list a");

    assert_eq!(listed(&capped), ["a/one.txt", "a/three.txt"]);
    assert!(capped.truncated());
}

#[test]
fn a_directory_holding_exactly_the_limit_is_not_truncated() {
    let dir = tree(&[("a/one.txt", "1"), ("a/two.txt", "2")]);
    let index = default_index(dir.path());

    let capped = index
        .list("a", &ListOptions::default().limit(2))
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
        .list("a", &ListOptions::default().depth(1).limit(2))
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
fn a_root_the_walk_cannot_list_fails_as_a_walk() {
    let dir = tempfile::tempdir().expect("a temporary directory");

    let error = Index::build(dir.path().join("gone"), &BuildOptions::default())
        .expect_err("the root is missing");

    assert_eq!(error.label(), "walk");
}

#[test]
fn listing_a_path_that_is_not_indexed_is_not_found() {
    let dir = tree(&[(".gitignore", "secret/\n"), ("secret/key.txt", "k")]);
    let index = default_index(dir.path());

    assert_eq!(
        index.list("nowhere", &ListOptions::default()).unwrap_err(),
        ListError::NotFound {
            path: "nowhere".to_owned()
        }
    );
    assert_eq!(
        index.list("secret", &ListOptions::default()).unwrap_err(),
        ListError::NotFound {
            path: "secret".to_owned()
        },
        "an ignored directory reads the same as one that was never there"
    );
}

#[test]
fn listing_a_file_reports_what_is_indexed_there_instead() {
    let dir = tree(&[("main.rs", "m")]);
    let index = default_index(dir.path());

    let error = index.list("main.rs", &ListOptions::default()).unwrap_err();

    assert_eq!(
        error,
        ListError::NotADirectory {
            path: "main.rs".to_owned(),
            kind: EntryKind::File,
        }
    );
    assert_eq!(
        error.to_string(),
        "\"main.rs\" is indexed as a file, which has no listing"
    );
}

#[cfg(unix)]
#[test]
fn listing_a_symlink_to_a_directory_is_not_a_listing_of_its_target() {
    let dir = tree(&[("target/inner.txt", "x")]);
    std::os::unix::fs::symlink("target", dir.path().join("link"))
        .expect("create a directory symlink");

    let index = default_index(dir.path());

    let error = index.list("link", &ListOptions::default()).unwrap_err();

    assert_eq!(
        error,
        ListError::NotADirectory {
            path: "link".to_owned(),
            kind: EntryKind::Symlink,
        },
        "the walk never descended the link, so the index holds nothing under it"
    );
    assert_eq!(
        error.to_string(),
        "\"link\" is indexed as a symlink, which has no listing"
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

    let sizes: Vec<Option<u64>> = entries.iter().map(Listed::size).collect();
    let binary: Vec<Option<bool>> = entries.iter().map(Listed::is_binary).collect();
    let targets: Vec<Option<&str>> = entries.iter().map(|found| found.entry().target()).collect();

    assert_eq!(listed(&list(&index, "a")), ["a/file.txt", "a/link"]);
    assert_eq!(sizes, [Some(12), None]);
    assert_eq!(
        binary,
        [Some(false), None],
        "a link is reported, never read through"
    );
    assert_eq!(targets, [None, Some("file.txt")]);
}

#[test]
fn listing_the_root_one_level_deep_takes_only_the_top_level_entries() {
    let dir = tree(&[("a/b/c/deep.txt", "d"), ("z.txt", "z"), ("m/one.txt", "1")]);
    let index = default_index(dir.path());

    let top = index
        .list("", &ListOptions::default().depth(1))
        .expect("list the root");

    assert_eq!(
        listed(&top),
        ["a", "m", "z.txt"],
        "each top-level directory is listed and none of them is descended"
    );
    assert_eq!(
        unlisted(&top),
        owned(&[("a", Some(3)), ("m", Some(1)), ("z.txt", None)]),
        "the root's entries carry no prefix, and their counts come out the same as anywhere else"
    );
}

#[test]
fn a_skipped_subtree_that_runs_to_the_end_of_the_index_ends_the_listing() {
    let dir = tree(&[("a/b/c/deep.txt", "d")]);
    let index = default_index(dir.path());

    let top = index
        .list("", &ListOptions::default().depth(1))
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

fn index_with_binary_extensions(root: &Path, binary: BinaryExtensions) -> Index {
    let mut options = BuildOptions::default();
    options.binary_extensions = binary;

    index(root, &options)
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
fn a_built_in_extension_a_set_of_the_caller_s_own_leaves_out_is_read_instead() {
    let dir = tree(&[("logo.png", "not really a png, and now the contents decide")]);
    let own = BinaryExtensions::new(["vromfs"]).expect("an extension");
    let index = index_with_binary_extensions(dir.path(), own);

    assert_eq!(index.is_binary("logo.png"), Some(false));

    let listed: Vec<Option<bool>> = list(&index, "")
        .items()
        .iter()
        .map(Listed::is_binary)
        .collect();
    assert_eq!(
        listed,
        [Some(false)],
        "a listing classifies with the same set"
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
fn glob_matching_exactly_the_limit_is_not_truncated() {
    let dir = tree(&[("a.rs", ""), ("b.rs", ""), ("c.md", "")]);
    let index = default_index(dir.path());
    let filter = Filter::new(["*.rs"]).expect("compile the pattern");

    let capped = index.glob(&filter, &GlobOptions::default().limit(2));

    assert_eq!(
        paths_at(&index, capped.items().iter().copied()),
        ["a.rs", "b.rs"]
    );
    assert!(
        !capped.truncated(),
        "the limit was reached, and no matching path was turned away"
    );
}

#[test]
fn glob_without_a_limit_is_never_truncated() {
    let dir = tree(&[("a.rs", ""), ("b.rs", "")]);
    let index = default_index(dir.path());
    let filter = Filter::new(["*.rs"]).expect("compile the pattern");

    assert!(!index.glob(&filter, &GlobOptions::default()).truncated());
}

#[test]
fn glob_selecting_nothing_is_an_empty_answer_rather_than_a_cut_one() {
    let dir = tree(&[("a.rs", "")]);
    let index = default_index(dir.path());
    let filter = Filter::new(["*.toml"]).expect("compile the pattern");

    let found = index.glob(&filter, &GlobOptions::default().limit(5));

    assert_eq!(found.items(), []);
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
                .list("a", &ListOptions::default().depth(0))
                .expect("list a")
        ),
        "a zero depth reads as no depth, which is what NonZeroUsize makes unrepresentable"
    );
    assert_eq!(
        listed(&list(&index, "a")),
        listed(
            &index
                .list("a", &ListOptions::default().limit(0))
                .expect("list a")
        ),
    );
}

#[test]
fn path_search_options_default_to_no_limit_and_zero_lifts_it() {
    let dir = tree(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
    let index = default_index(dir.path());
    let filter = Filter::new(["*.rs"]).expect("compile the pattern");

    let globbed = index.glob(&filter, &GlobOptions::default().limit(0));
    assert_eq!(
        paths_at(&index, globbed.items().iter().copied()),
        ["a.rs", "b.rs", "c.rs"],
        "a zero limit reads as no limit"
    );
    assert!(!globbed.truncated());

    let ranked = index.fuzzy("rs", &FuzzyOptions::default().limit(0));
    assert_eq!(ranked.items().len(), 3, "a zero limit reads as no limit");
    assert_eq!(ranked, index.fuzzy("rs", &FuzzyOptions::default()));
}

#[test]
fn a_listing_answers_the_same_facts_as_the_keyed_lookups() {
    type Facts = (String, Option<u64>, Option<bool>);

    let dir = tree(&[("a.txt", "hello"), ("sub/b.bin", "x"), ("empty/", "")]);
    fs::write(dir.path().join("sub/blob.txt"), b"has\0a nul").expect("write a binary file");
    let index = default_index(dir.path());

    let listed: Vec<Facts> = list(&index, "")
        .items()
        .iter()
        .map(|found| {
            (
                found.entry().path().to_owned(),
                found.size(),
                found.is_binary(),
            )
        })
        .collect();
    let keyed: Vec<Facts> = index
        .entries()
        .map(|entry| {
            (
                entry.path().to_owned(),
                index.size(entry.path()),
                index.is_binary(entry.path()),
            )
        })
        .collect();

    assert_eq!(
        listed, keyed,
        "one index, one set of answers, two routes to them"
    );
    assert_eq!(listed.len(), index.len());
}

#[test]
fn a_listing_reads_nothing_for_an_entry_it_never_asks_about() {
    let dir = tree(&[("gone.txt", "here for now")]);
    let index = default_index(dir.path());

    assert_eq!(listed(&list(&index, "")), ["gone.txt"]);

    fs::remove_file(dir.path().join("gone.txt")).expect("remove the file");

    assert_eq!(
        index.size("gone.txt"),
        None,
        "listing an entry leaves its slot unread, so the first ask is still the first read"
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
