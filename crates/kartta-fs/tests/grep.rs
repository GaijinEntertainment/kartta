//! The content search contract: what `Index::grep` finds, in what order, and how its answer accounts for every file it
//! did not search; and what a build asked for a content index reads, and what the index then holds.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

mod common;

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use common::{tree, write};
use kartta_fs::{
    BuildError, BuildOptions, BuildOutcome, CancelToken, Case, Content, ContentIndexStorage,
    ContentVisitor, Cut, Filter, GrepOptions, Index, LineKind, Matches, Patterns,
};
use tempfile::TempDir;

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
fn a_limit_that_fills_with_its_batch_reads_on_until_a_further_match_proves_the_cut() {
    let dir = needles(300);
    let index = index(dir.path());
    let needle = patterns(&["needle"]);

    let filled = index.grep(&needle, &GrepOptions::default().limit(256));
    assert_eq!(filled.lines(), 256);
    assert!(
        filled.truncated(),
        "the first batch filled the limit exactly, and the next one holds the match that proves the cut"
    );
    assert_eq!(filled.searched(), 257);
    assert_eq!(
        filled.unsearched(),
        43,
        "the rest of the second batch was read after the cut was proven"
    );
    assert_accounted(&filled);

    let exact = index.grep(&needle, &GrepOptions::default().limit(300));
    assert_eq!(exact.lines(), 300);
    assert!(
        !exact.truncated() && exact.unsearched() == 0,
        "a limit equal to every match reads on, finds none past it, and answers whole"
    );

    let counted = index.grep(&needle, &GrepOptions::default().limit(256).count_all());
    assert_eq!(counted.lines(), 256);
    assert!(counted.truncated(), "counting reads on and proves the cut");
    assert_eq!(counted.unsearched(), 0);
    assert_eq!(counted.total_lines(), Some(300));
    assert_eq!(counted.total_files(), Some(300));
    assert_accounted(&counted);
}

#[test]
fn a_limited_search_answers_truncated_alike_whatever_shares_the_batch_that_fills_it() {
    let dir = tempfile::tempdir().expect("create temp dir");
    for at in 0..512 {
        let text = if at % 2 == 0 { "needle\n" } else { "hay\n" };
        write(dir.path(), &format!("f{at:04}.txt"), text);
    }
    let needle = patterns(&["needle"]);
    let plain = index(dir.path());
    let content_indexed = build(dir.path(), None).index;

    for (build, index) in [
        ("no content index", &plain),
        ("a content index", &content_indexed),
    ] {
        let found = index.grep(&needle, &GrepOptions::default().limit(128));

        assert_eq!(found.lines(), 128, "{build}");
        assert!(
            found.truncated(),
            "with {build}, a match past the limit proves the cut, though the index rules the hay out of the batch"
        );
        assert_accounted(&found);
    }
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
fn a_binary_file_over_the_size_limit_is_oversized_whatever_read_it_before() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write(dir.path(), "blob.txt", b"\0needle and more\n");
    let fresh = index(dir.path());
    let content_indexed = build(dir.path(), None).index;
    let classified = index(dir.path());
    assert_eq!(classified.is_binary("blob.txt"), Some(true));

    for (read_before, index) in [
        ("nothing", &fresh),
        ("the build", &content_indexed),
        ("is_binary", &classified),
    ] {
        let found = index.grep(
            &patterns(&["needle"]),
            &GrepOptions::default().max_file_bytes(10),
        );
        assert_eq!(
            (found.oversized(), found.binary()),
            (1, 0),
            "after {read_before} read the file, the size bound is still checked before the binary verdict"
        );
    }
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

fn indexed() -> BuildOptions {
    let mut options = BuildOptions::default();
    options.content_index = Some(ContentIndexStorage::Memory);
    options
}

fn build(root: &Path, visitor: Option<&dyn ContentVisitor>) -> BuildOutcome {
    Index::build_with(root, &indexed(), visitor, None).expect("build the index")
}

struct Recorder {
    asked: Mutex<Vec<String>>,
    seen: Mutex<Vec<String>>,
}

impl ContentVisitor for Recorder {
    fn wants(&self, path: &str) -> bool {
        self.asked.lock().expect("lock").push(path.to_owned());
        path == "main.rs"
    }

    fn visit(&self, content: Content<'_>) {
        self.seen
            .lock()
            .expect("lock")
            .push(content.path().to_owned());
    }
}

#[test]
fn a_build_asked_for_a_content_index_reads_every_text_file_and_indexes_it() {
    let dir = tree(&[
        ("src/main.rs", "fn main() {}\n"),
        ("notes.md", "some notes\n"),
        ("blob.dat", "\0binary"),
        ("image.png", "looks like text"),
    ]);

    let outcome = Index::build(dir.path(), &indexed()).expect("build the index");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let index = outcome.index;
    for path in ["src/main.rs", "notes.md", "blob.dat", "image.png"] {
        fs::remove_file(dir.path().join(path)).expect("remove");
    }

    let stats = index
        .content_index_stats()
        .expect("the build was asked for one");
    assert_eq!(stats.files(), 2, "the two text files");
    assert!(stats.trigrams() > 0 && stats.bytes() > 0);
    assert_eq!(
        index.size("notes.md"),
        Some(11),
        "the build read it, and kept what the read learned"
    );
    assert_eq!(index.is_binary("blob.dat"), Some(true));
    assert_eq!(
        index.size("image.png"),
        None,
        "a binary extension is never opened"
    );
}

#[test]
fn the_index_counts_each_distinct_trigram_once() {
    let dir = tree(&[("a.txt", "abcd"), ("b.txt", "BCDE")]);

    let stats = build(dir.path(), None)
        .index
        .content_index_stats()
        .expect("the build was asked for one");

    assert_eq!(
        stats.trigrams(),
        3,
        "abc, bcd and cde, the letters folded to one case"
    );
}

#[test]
fn a_visitor_beside_the_index_is_still_handed_only_what_it_wants() {
    let dir = tree(&[("main.rs", "fn main() {}\n"), ("notes.md", "notes\n")]);
    let visitor = Recorder {
        asked: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
    };

    let outcome = build(dir.path(), Some(&visitor));

    let mut asked = visitor.asked.into_inner().expect("lock");
    asked.sort();
    assert_eq!(asked, ["main.rs", "notes.md"], "asked once per file");
    assert_eq!(visitor.seen.into_inner().expect("lock"), ["main.rs"]);
    assert_eq!(
        outcome
            .index
            .content_index_stats()
            .map(|stats| stats.files()),
        Some(2),
        "the index read the file the visitor turned down"
    );
}

#[test]
fn a_build_not_asked_for_one_has_no_content_index() {
    let dir = tree(&[("main.rs", "fn main() {}\n")]);

    assert_eq!(index(dir.path()).content_index_stats(), None);
}

#[cfg(unix)]
#[test]
fn a_text_file_that_will_not_read_is_reported_and_left_out_of_the_index() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tree(&[("locked.rs", "locked"), ("open.rs", "open")]);
    let locked = dir.path().join("locked.rs");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }

    let outcome = build(dir.path(), None);

    assert!(
        matches!(outcome.errors.as_slice(), [BuildError::Read { native_path, .. }] if native_path.ends_with("locked.rs")),
        "{:?}",
        outcome.errors
    );
    assert_eq!(
        outcome
            .index
            .content_index_stats()
            .map(|stats| stats.files()),
        Some(1)
    );
}

#[test]
fn a_search_through_the_index_finds_what_a_full_search_finds() {
    let dir = tree(&[
        ("src/main.rs", "fn main() {\n    run();\n}\n"),
        ("src/run.rs", "pub fn run() {}\n"),
        ("src/Kelvin.rs", "// 7 \u{212A}elvin\n"),
        ("docs/notes.md", "run the tests, then RUN them again\n"),
        ("docs/other.md", "nothing to see\n"),
        ("blob.dat", "\0run"),
    ]);
    let indexed = build(dir.path(), None).index;
    let plain = index(dir.path());
    let searches: [(&[&str], Case); 8] = [
        (&[r"\brun\("], Case::Sensitive),
        (&["run"], Case::Insensitive),
        (&["kelvin"], Case::Insensitive),
        (&["fn main", "see"], Case::Sensitive),
        (&["(main|other)"], Case::Sensitive),
        (&[r"t[eh]+"], Case::Sensitive),
        (&["absent from every file"], Case::Sensitive),
        (&[r"\w+"], Case::Sensitive),
    ];

    for (patterns, case) in searches {
        let patterns = Patterns::new(patterns, case).expect("compile the patterns");
        let through = indexed.grep(&patterns, &GrepOptions::default());
        let full = plain.grep(&patterns, &GrepOptions::default());

        assert_eq!(
            printed(&indexed, &through),
            printed(&plain, &full),
            "{patterns:?}"
        );
        assert_eq!(through.candidates(), full.candidates(), "{patterns:?}");
        assert_eq!(
            through.searched() + through.ruled_out(),
            full.searched(),
            "{patterns:?}: what the index ruled out is what the full search read and found nothing in"
        );
        assert_accounted(&through);
    }
}

#[test]
fn a_literal_rules_out_every_file_missing_one_of_its_trigrams() {
    let dir = tree(&[
        ("hit.txt", "needle\n"),
        ("near.txt", "need le\n"),
        ("miss.txt", "haystack\n"),
    ]);
    let index = build(dir.path(), None).index;

    let matches = index.grep(&patterns(&["needle"]), &GrepOptions::default());

    assert_eq!(printed(&index, &matches), ["hit.txt:1:needle"]);
    assert_eq!(matches.searched(), 1);
    assert_eq!(matches.ruled_out(), 2);
}

#[test]
fn a_pattern_that_needs_no_trigram_reads_every_candidate() {
    let dir = tree(&[("a.txt", "alpha\n"), ("b.txt", "beta\n")]);
    let index = build(dir.path(), None).index;

    let matches = index.grep(&patterns(&[r"\w+", "ab"]), &GrepOptions::default());

    assert_eq!(matches.ruled_out(), 0);
    assert_eq!(matches.searched(), 2);
}

#[test]
fn a_file_changed_since_the_build_is_judged_by_the_content_the_build_read() {
    let dir = tree(&[("changed.txt", "old words\n")]);
    let index = build(dir.path(), None).index;
    fs::write(dir.path().join("changed.txt"), "needle\n").expect("rewrite");

    let matches = index.grep(&patterns(&["needle"]), &GrepOptions::default());

    assert!(
        matches.files().is_empty(),
        "the index rules the file out by what it held then"
    );
    assert_eq!(matches.ruled_out(), 1);
}

#[cfg(unix)]
#[test]
fn a_file_the_build_could_not_read_is_searched_whatever_the_pattern() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tree(&[("locked.txt", "needle\n"), ("open.txt", "haystack\n")]);
    let locked = dir.path().join("locked.txt");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }
    let index = build(dir.path(), None).index;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("unlock");

    let matches = index.grep(&patterns(&["needle"]), &GrepOptions::default());

    assert_eq!(printed(&index, &matches), ["locked.txt:1:needle"]);
    assert_eq!(matches.ruled_out(), 1, "only the file the index holds");
}

const CHUNK: usize = 64 * 1024;

fn straddling(text: &str) -> String {
    format!("{}{text}\n", "x".repeat(CHUNK - 1))
}

struct Everything;

impl ContentVisitor for Everything {
    fn wants(&self, _path: &str) -> bool {
        true
    }

    fn visit(&self, _content: Content<'_>) {}
}

#[test]
fn a_file_only_the_index_reads_is_indexed_as_a_file_read_whole_is() {
    let late = format!("{}needle far into the file\n", "y".repeat(3 * CHUNK));
    let dir = tree(&[
        ("straddle.txt", &straddling("needle")),
        ("kelvin.txt", &straddling("\u{212A}elvin")),
        ("long_s.txt", &straddling("\u{17F}tring")),
        ("late.txt", &late),
    ]);

    let streamed = build(dir.path(), None)
        .index
        .content_index_stats()
        .expect("indexed");
    let whole = build(dir.path(), Some(&Everything))
        .index
        .content_index_stats()
        .expect("indexed");

    assert_eq!(
        streamed.trigrams(),
        whole.trigrams(),
        "a chunk boundary loses no trigram and folds no character apart"
    );
}

#[test]
fn a_match_straddling_a_chunk_of_the_build_read_is_not_ruled_out() {
    let dir = tree(&[
        ("straddle.txt", &straddling("needle")),
        ("kelvin.txt", &straddling("\u{212A}elvin")),
    ]);
    let index = build(dir.path(), None).index;

    for (pattern, case, path) in [
        ("needle", Case::Sensitive, "straddle.txt"),
        ("kelvin", Case::Insensitive, "kelvin.txt"),
    ] {
        let patterns = Patterns::new([pattern], case).expect("compile");
        let matches = index.grep(&patterns, &GrepOptions::default());

        assert_eq!(
            matches.files().len(),
            1,
            "{pattern} straddles the first chunk of {path}"
        );
        assert_eq!(matches.ruled_out(), 1, "the other file");
    }
}

#[test]
fn an_index_kept_in_a_file_or_a_mapping_answers_as_one_in_memory_does() {
    let dir = tree(&[
        ("src/main.rs", "fn main() {\n    run();\n}\n"),
        ("src/run.rs", "pub fn run() {}\n"),
        ("docs/notes.md", "run the tests\n"),
    ]);
    let scratch = tempfile::tempdir().expect("create scratch dir");
    let in_memory = build(dir.path(), None).index;
    let patterns = patterns(&[r"\brun\("]);
    let expected = printed(
        &in_memory,
        &in_memory.grep(&patterns, &GrepOptions::default()),
    );

    for storage in [
        ContentIndexStorage::File {
            dir: scratch.path().to_path_buf(),
        },
        ContentIndexStorage::Mapped {
            dir: scratch.path().to_path_buf(),
        },
    ] {
        let mut options = BuildOptions::default();
        options.content_index = Some(storage.clone());
        let index = Index::build(dir.path(), &options)
            .expect("build the index")
            .index;

        let matches = index.grep(&patterns, &GrepOptions::default());

        assert_eq!(printed(&index, &matches), expected, "{storage:?}");
        assert_eq!(matches.ruled_out(), 1, "{storage:?}");
        assert_eq!(
            index.content_index_stats(),
            in_memory.content_index_stats(),
            "{storage:?}"
        );
    }
}

#[test]
fn a_scratch_directory_that_does_not_exist_fails_the_build_naming_it() {
    let dir = tree(&[("main.rs", "fn main() {}\n")]);
    let missing = dir.path().join("no such directory");
    let mut options = BuildOptions::default();
    options.content_index = Some(ContentIndexStorage::Mapped {
        dir: missing.clone(),
    });

    let built = Index::build(dir.path(), &options);

    assert!(
        matches!(&built, Err(BuildError::ContentIndex { native_path, .. }) if *native_path == missing),
        "{built:?}"
    );
}
