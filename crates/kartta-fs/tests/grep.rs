//! The content search contract: what `Index::grep` finds, in what order, and how its answer accounts for every file it
//! did not search.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::Path;

use kartta_fs::{
    BuildOptions, CancelToken, Case, Cut, Filter, GrepOptions, Index, LineKind, Matches, Patterns,
};
use tempfile::TempDir;

fn tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");

    for (path, content) in spec {
        write(dir.path(), path, content.as_bytes());
    }

    dir
}

fn write(root: &Path, path: &str, content: &[u8]) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(&full, content).expect("write file");
}

fn index(root: &Path) -> Index {
    let outcome = Index::build(root, &BuildOptions::default()).expect("build the index");
    assert!(
        outcome.errors.is_empty(),
        "unexpected build errors: {:?}",
        outcome.errors
    );

    outcome.index
}

fn patterns(patterns: &[&str]) -> Patterns {
    Patterns::new(patterns, Case::Sensitive).expect("compile the patterns")
}

fn printed(index: &Index, matches: &Matches<'_>) -> Vec<String> {
    let mut paths = index.paths();

    matches
        .files()
        .iter()
        .flat_map(|file| {
            let path = paths.get(file.position()).to_owned();
            file.lines().iter().map(move |line| {
                let separator = match line.kind() {
                    LineKind::Match => ':',
                    LineKind::Context => '-',
                };
                format!(
                    "{path}{separator}{}{separator}{}",
                    line.number(),
                    line.text()
                )
            })
        })
        .collect()
}

fn assert_accounted(matches: &Matches<'_>) {
    assert_eq!(
        matches.candidates(),
        matches.searched()
            + matches.binary()
            + matches.oversized()
            + matches.ruled_out()
            + matches.failures().len()
            + matches.unsearched(),
        "every candidate lands in exactly one count"
    );
}

fn needles(count: usize) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    for at in 0..count {
        write(dir.path(), &format!("f{at:04}.txt"), b"needle\n");
    }

    dir
}

#[test]
fn files_answer_in_index_order_with_lines_in_file_order() {
    let dir = tree(&[
        ("a.txt", "hit\n"),
        ("a/z.txt", "miss\nhit\n"),
        ("b.txt", "hit\nhit\n"),
    ]);
    let index = index(dir.path());

    let found = index.grep(&patterns(&["hit"]), &GrepOptions::default());

    assert_eq!(
        printed(&index, &found),
        ["a/z.txt:2:hit", "a.txt:1:hit", "b.txt:1:hit", "b.txt:2:hit"],
        "a directory's subtree follows it, ahead of a sibling file that sorts after it byte by byte"
    );
    assert_eq!(found.lines(), 4);
    assert!(!found.truncated() && found.unsearched() == 0);
    assert_eq!(found.searched(), 3);
    assert_accounted(&found);
}

#[test]
fn patterns_are_a_union_and_a_line_several_match_is_reported_once() {
    let dir = tree(&[("f.txt", "alpha\nbeta\nalpha beta\ngamma\n")]);
    let index = index(dir.path());

    let found = index.grep(&patterns(&["alpha", "beta"]), &GrepOptions::default());

    assert_eq!(
        printed(&index, &found),
        ["f.txt:1:alpha", "f.txt:2:beta", "f.txt:3:alpha beta"]
    );
}

#[test]
fn case_insensitive_patterns_fold_case() {
    let dir = tree(&[("f.txt", "Needle\nNEEDLE\nnoodle\n")]);
    let index = index(dir.path());

    let folded = Patterns::new(["needle"], Case::Insensitive).expect("compile");
    let found = index.grep(&folded, &GrepOptions::default());

    assert_eq!(
        printed(&index, &found),
        ["f.txt:1:Needle", "f.txt:2:NEEDLE"]
    );
    assert!(
        index
            .grep(&patterns(&["needle"]), &GrepOptions::default())
            .files()
            .is_empty(),
        "the sensitive set finds none of them"
    );
}

#[test]
fn context_lines_surround_matches_and_merge_where_they_meet() {
    let dir = tree(&[("f.txt", "1\n2\nhit\n4\nhit\n6\n7\n8\n")]);
    let index = index(dir.path());

    let found = index.grep(&patterns(&["hit"]), &GrepOptions::default().context(1));

    assert_eq!(
        printed(&index, &found),
        [
            "f.txt-2-2",
            "f.txt:3:hit",
            "f.txt-4-4",
            "f.txt:5:hit",
            "f.txt-6-6"
        ]
    );
    assert_eq!(found.lines(), 2, "context lines are not matches");
}

#[test]
fn a_dollar_matches_before_a_windows_line_end_and_the_text_drops_it() {
    let dir = tree(&[("f.txt", "first end\r\nsecond end\r\n")]);
    let index = index(dir.path());

    let found = index.grep(&patterns(&["end$"]), &GrepOptions::default());

    assert_eq!(
        printed(&index, &found),
        ["f.txt:1:first end", "f.txt:2:second end"]
    );
}

#[test]
fn the_limit_is_spent_in_index_order_and_an_exact_fit_is_whole() {
    let dir = tree(&[("a.txt", "hit\n"), ("b.txt", "hit\n"), ("c.txt", "miss\n")]);
    let index = index(dir.path());
    let hits = patterns(&["hit"]);

    let exact = index.grep(&hits, &GrepOptions::default().limit(2));
    assert_eq!(printed(&index, &exact), ["a.txt:1:hit", "b.txt:1:hit"]);
    assert!(
        !exact.truncated() && exact.unsearched() == 0,
        "every file ran in one batch, so the answer knows nothing was cut"
    );

    let cut = index.grep(&hits, &GrepOptions::default().limit(1));
    assert_eq!(printed(&index, &cut), ["a.txt:1:hit"]);
    assert!(cut.truncated(), "b.txt's match was seen and turned away");
    assert_accounted(&cut);
}

#[test]
fn a_cut_inside_a_file_keeps_the_last_kept_matchs_trailing_context_only() {
    let dir = tree(&[("f.txt", "hit\n2\n3\n4\n5\nhit\n7\n")]);
    let index = index(dir.path());

    let found = index.grep(
        &patterns(&["hit"]),
        &GrepOptions::default().context(1).limit(1),
    );

    assert_eq!(
        printed(&index, &found),
        ["f.txt:1:hit", "f.txt-2-2"],
        "line 5 leads into the match the limit turned away, so it is not the kept match's context"
    );
    assert!(found.truncated());
}

#[test]
fn a_file_that_crosses_the_limit_another_file_left_it_is_cut_where_the_limit_runs_out() {
    let dir = tree(&[("a.txt", "hit\n"), ("b.txt", "hit\nx\ny\nz\nhit\nq\nhit\n")]);
    let index = index(dir.path());

    let found = index.grep(
        &patterns(&["hit"]),
        &GrepOptions::default().context(1).limit(2),
    );

    assert_eq!(
        printed(&index, &found),
        ["a.txt:1:hit", "b.txt:1:hit", "b.txt-2-x"],
        "both files ran in one batch with room for two, and a.txt spent one of them"
    );
    assert!(found.truncated());
    assert_eq!(found.lines(), 2);
}

#[test]
fn a_limit_that_fills_before_the_last_batch_reports_the_files_it_left_unread() {
    let dir = needles(300);
    let index = index(dir.path());
    let needle = patterns(&["needle"]);

    let filled = index.grep(&needle, &GrepOptions::default().limit(256));
    assert_eq!(filled.lines(), 256);
    assert!(
        !filled.truncated(),
        "no match past the limit was seen, so none is claimed"
    );
    assert_eq!(
        filled.unsearched(),
        44,
        "the files past the first batch were never read"
    );
    assert_accounted(&filled);

    let counted = index.grep(&needle, &GrepOptions::default().limit(256).count_all());
    assert_eq!(counted.lines(), 256);
    assert!(counted.truncated(), "counting reads on and proves the cut");
    assert_eq!(counted.unsearched(), 0);
    assert_eq!(counted.total_lines(), Some(300));
    assert_eq!(counted.total_files(), Some(300));
    assert_accounted(&counted);
}

#[test]
fn counting_past_a_spent_limit_returns_no_context_for_the_matches_it_omits() {
    let dir = tempfile::tempdir().expect("create temp dir");
    for at in 0..300 {
        write(dir.path(), &format!("f{at:04}.txt"), b"before\nneedle\n");
    }
    let index = index(dir.path());

    let found = index.grep(
        &patterns(&["needle"]),
        &GrepOptions::default().context(1).limit(256).count_all(),
    );

    assert_eq!(
        found.files().len(),
        256,
        "the files read once the limit was spent hold only omitted matches, so none of their lines belong"
    );
    assert_eq!(found.total_lines(), Some(300));
}

#[test]
fn the_largest_context_is_every_line_around_a_match() {
    let dir = tree(&[("f.txt", "1\n2\nhit\n4\n5\nhit\n7\n")]);
    let index = index(dir.path());

    let found = index.grep(
        &patterns(&["hit"]),
        &GrepOptions::default().context(usize::MAX).limit(1),
    );

    assert_eq!(
        printed(&index, &found),
        [
            "f.txt-1-1",
            "f.txt-2-2",
            "f.txt:3:hit",
            "f.txt-4-4",
            "f.txt-5-5"
        ],
        "everything up to the match the limit turned away"
    );
}

#[test]
fn a_cut_proven_early_leaves_the_rest_unsearched() {
    let dir = needles(300);
    let index = index(dir.path());

    let found = index.grep(&patterns(&["needle"]), &GrepOptions::default().limit(10));

    assert_eq!(found.lines(), 10);
    assert!(found.truncated());
    assert_eq!(found.searched(), 11, "the eleventh file proved the cut");
    assert_eq!(found.unsearched(), 289);
    assert_eq!(found.total_lines(), None, "nothing was asked to count");
    assert_accounted(&found);
}

#[test]
fn repeated_searches_answer_identically_across_batches() {
    let dir = needles(600);
    let index = index(dir.path());
    let needle = patterns(&["needle"]);
    let options = GrepOptions::default().limit(500);

    let first = printed(&index, &index.grep(&needle, &options));
    for _ in 0..5 {
        assert_eq!(printed(&index, &index.grep(&needle, &options)), first);
    }
    assert_eq!(
        first.first().map(String::as_str),
        Some("f0000.txt:1:needle")
    );
    assert_eq!(first.len(), 500);
}

#[test]
fn a_filter_selects_and_an_exclusion_removes_files_and_whole_directories() {
    let dir = tree(&[
        ("src/main.rs", "hit\n"),
        ("src/notes.md", "hit\n"),
        ("vendor/dep/lib.rs", "hit\n"),
        ("vendor/dep/more.rs", "hit\n"),
        ("generated.rs", "hit\n"),
    ]);
    let index = index(dir.path());
    let sources = Filter::new(["*.rs"]).expect("compile");
    let left_out = Filter::new(["vendor", "generated.rs"]).expect("compile");

    let found = index.grep(
        &patterns(&["hit"]),
        &GrepOptions::default().filter(&sources).exclude(&left_out),
    );

    assert_eq!(printed(&index, &found), ["src/main.rs:1:hit"]);
    assert_eq!(
        found.excluded(),
        3,
        "both files under vendor and generated.rs; notes.md was never selected"
    );
    assert_eq!(found.candidates(), 1);
}

#[test]
fn a_binary_extension_skips_a_file_unopened_and_a_nul_skips_one_after_reading() {
    let mut late_nul = vec![b'a'; 70 * 1024];
    late_nul.extend_from_slice(b"\0\nneedle\n");
    let dir = tree(&[("image.png", "needle\n"), ("text.txt", "needle\n")]);
    write(dir.path(), "blob.dat", b"\0needle\n");
    write(dir.path(), "late.txt", &late_nul);
    let index = index(dir.path());
    fs::remove_file(dir.path().join("image.png")).expect("remove");

    let found = index.grep(&patterns(&["needle"]), &GrepOptions::default());

    assert_eq!(
        printed(&index, &found)
            .iter()
            .map(|line| line.split(':').next().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["late.txt", "text.txt"],
        "a NUL past the first 64 KiB does not make a file binary, as is_binary would not either"
    );
    assert_eq!(found.binary(), 2);
    assert!(
        found.failures().is_empty(),
        "the removed image.png was never opened"
    );
    assert_accounted(&found);
}

#[test]
fn a_class_that_could_match_a_line_break_matches_within_a_line_only() {
    let dir = tree(&[("one.txt", "left right\n"), ("two.txt", "left\nright\n")]);
    let index = index(dir.path());

    let found = index.grep(&patterns(&[r"left\s+right"]), &GrepOptions::default());

    assert_eq!(printed(&index, &found), ["one.txt:1:left right"]);
}

#[test]
fn the_verdict_and_the_size_a_search_learns_are_kept() {
    let dir = tree(&[("text.txt", "needle\n")]);
    write(dir.path(), "blob.dat", b"\0needle\n");
    let index = index(dir.path());

    let first = index.grep(&patterns(&["needle"]), &GrepOptions::default());
    assert_eq!(first.binary(), 1);

    fs::remove_file(dir.path().join("blob.dat")).expect("remove");
    fs::remove_file(dir.path().join("text.txt")).expect("remove");

    assert_eq!(index.is_binary("blob.dat"), Some(true));
    assert_eq!(index.is_binary("text.txt"), Some(false));
    assert_eq!(index.size("text.txt"), Some(7));

    let second = index.grep(&patterns(&["needle"]), &GrepOptions::default());
    assert_eq!(
        second.binary(),
        1,
        "a known binary file is skipped without being opened"
    );
    assert_eq!(second.failures().len(), 1, "the text file is gone");
}

#[test]
fn a_file_over_the_size_limit_is_skipped_and_counted() {
    let dir = tree(&[
        ("small.txt", "needle\n"),
        ("large.txt", "needle\nand more\n"),
    ]);
    let index = index(dir.path());

    let found = index.grep(
        &patterns(&["needle"]),
        &GrepOptions::default().max_file_bytes(10),
    );

    assert_eq!(printed(&index, &found), ["small.txt:1:needle"]);
    assert_eq!(found.oversized(), 1);
    assert_accounted(&found);

    let again = index.grep(
        &patterns(&["needle"]),
        &GrepOptions::default().max_file_bytes(10),
    );
    assert_eq!(
        again.oversized(),
        1,
        "the second search knows the size and does not open the file"
    );
}

#[test]
fn a_file_gone_since_the_walk_is_a_failure_naming_its_position() {
    let dir = tree(&[("gone.txt", "needle\n"), ("kept.txt", "needle\n")]);
    let index = index(dir.path());
    fs::remove_file(dir.path().join("gone.txt")).expect("remove");

    let found = index.grep(&patterns(&["needle"]), &GrepOptions::default());

    assert_eq!(printed(&index, &found), ["kept.txt:1:needle"]);
    let [failure] = found.failures() else {
        panic!("one failure expected, got {:?}", found.failures());
    };
    assert_eq!(index.paths().get(failure.position()), "gone.txt");
    assert_eq!(failure.error().kind(), std::io::ErrorKind::NotFound);
    assert_accounted(&found);
}

#[cfg(unix)]
#[test]
fn a_symlink_is_counted_and_never_read_through() {
    let dir = tree(&[("real.txt", "needle\n")]);
    std::os::unix::fs::symlink("real.txt", dir.path().join("link.txt")).expect("symlink");
    let index = index(dir.path());

    let found = index.grep(&patterns(&["needle"]), &GrepOptions::default());

    assert_eq!(
        printed(&index, &found),
        ["real.txt:1:needle"],
        "the target is searched under its own path only"
    );
    assert_eq!(found.symlinks(), 1);
    assert_eq!(found.candidates(), 1);

    let links = Filter::new(["link.txt"]).expect("compile");
    let excluded = index.grep(
        &patterns(&["needle"]),
        &GrepOptions::default().exclude(&links),
    );
    assert_eq!(
        excluded.symlinks(),
        1,
        "an excluded symlink is still a selected one, and still unsearched"
    );
    assert_eq!(excluded.excluded(), 0, "the exclusion counts regular files");
}

#[test]
fn a_long_matching_line_is_cut_around_its_match_and_a_long_context_line_keeps_its_start() {
    let long_match = format!("{}needle{}", "a".repeat(100), "b".repeat(100));
    let long_context = "c".repeat(50);
    let dir = tree(&[("f.txt", &format!("{long_match}\n{long_context}\n"))]);
    let index = index(dir.path());

    let found = index.grep(
        &patterns(&["needle"]),
        &GrepOptions::default().context(1).max_line_bytes(16),
    );
    let lines = found.files()[0].lines();

    assert_eq!(lines[0].text(), "aaaaaneedlebbbbb");
    assert_eq!(
        lines[0].cut(),
        Some(Cut {
            before: 95,
            after: 95
        })
    );
    assert_eq!(lines[1].text(), "c".repeat(16));
    assert_eq!(
        lines[1].cut(),
        Some(Cut {
            before: 0,
            after: 34
        })
    );
}

#[test]
fn bytes_that_are_not_utf8_read_as_replacement_characters() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write(dir.path(), "f.txt", b"needle \xFF here\n");
    let index = index(dir.path());

    let found = index.grep(&patterns(&["needle"]), &GrepOptions::default());

    assert_eq!(printed(&index, &found), ["f.txt:1:needle \u{FFFD} here"]);
}

#[test]
fn a_cancelled_search_reports_what_it_covered_and_an_uncancelled_one_is_whole() {
    let dir = needles(10);
    let index = index(dir.path());
    let needle = patterns(&["needle"]);

    let cancel = CancelToken::new();
    cancel.cancel();
    let stopped = index.grep(&needle, &GrepOptions::default().cancel(&cancel));
    assert!(stopped.cancelled());
    assert!(stopped.files().is_empty());
    assert_eq!(stopped.unsearched(), 10);
    assert_accounted(&stopped);

    let quiet = CancelToken::new();
    let whole = index.grep(&needle, &GrepOptions::default().cancel(&quiet));
    assert!(
        !whole.cancelled(),
        "a token nobody cancelled turns nothing away"
    );
    assert_eq!(whole.lines(), 10);
}

#[test]
fn an_empty_index_searches_nothing() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let index = index(dir.path());

    let found = index.grep(&patterns(&["anything"]), &GrepOptions::default());

    assert!(found.files().is_empty());
    assert_eq!(found.candidates(), 0);
    assert!(!found.truncated() && found.unsearched() == 0);
}
