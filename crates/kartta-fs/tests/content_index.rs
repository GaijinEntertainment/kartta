//! The content index contract: what a build asked for one reads, and what the index then holds.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use kartta_fs::{
    BuildError, BuildOptions, BuildOutcome, Case, Content, ContentIndexStorage, ContentVisitor,
    GrepOptions, Index, Matches, Patterns,
};
use tempfile::TempDir;

fn tree(spec: &[(&str, &[u8])]) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");

    for (path, content) in spec {
        let full = dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&full, content).expect("write file");
    }

    dir
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
        ("src/main.rs", b"fn main() {}\n"),
        ("notes.md", b"some notes\n"),
        ("blob.dat", b"\0binary"),
        ("image.png", b"looks like text"),
    ]);

    let outcome = Index::build(dir.path(), &indexed()).expect("build the index");
    assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    let index = outcome.into_index();
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
    let dir = tree(&[("a.txt", b"abcd"), ("b.txt", b"BCDE")]);

    let stats = build(dir.path(), None)
        .index()
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
    let dir = tree(&[("main.rs", b"fn main() {}\n"), ("notes.md", b"notes\n")]);
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
            .index()
            .content_index_stats()
            .map(|stats| stats.files()),
        Some(2),
        "the index read the file the visitor turned down"
    );
}

#[test]
fn a_build_not_asked_for_one_has_no_content_index() {
    let dir = tree(&[("main.rs", b"fn main() {}\n")]);

    let outcome = Index::build(dir.path(), &BuildOptions::default()).expect("build the index");

    assert_eq!(outcome.index().content_index_stats(), None);
}

#[cfg(unix)]
#[test]
fn a_text_file_that_will_not_read_is_reported_and_left_out_of_the_index() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tree(&[("locked.rs", b"locked"), ("open.rs", b"open")]);
    let locked = dir.path().join("locked.rs");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }

    let outcome = build(dir.path(), None);

    assert!(
        matches!(outcome.errors(), [BuildError::Read { native_path, .. }] if native_path.ends_with("locked.rs")),
        "{:?}",
        outcome.errors()
    );
    assert_eq!(
        outcome
            .index()
            .content_index_stats()
            .map(|stats| stats.files()),
        Some(1)
    );
}

fn plain(root: &Path) -> Index {
    Index::build(root, &BuildOptions::default())
        .expect("build the index")
        .into_index()
}

fn found(index: &Index, matches: &Matches<'_>) -> Vec<String> {
    let mut paths = index.paths();

    matches
        .files()
        .iter()
        .flat_map(|file| {
            let path = paths.get(file.position()).to_owned();
            file.lines()
                .iter()
                .map(move |line| format!("{path}:{}:{}", line.number(), line.text()))
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

#[test]
fn a_search_through_the_index_finds_what_a_full_search_finds() {
    let dir = tree(&[
        ("src/main.rs", b"fn main() {\n    run();\n}\n"),
        ("src/run.rs", b"pub fn run() {}\n"),
        ("src/Kelvin.rs", "// 7 \u{212A}elvin\n".as_bytes()),
        ("docs/notes.md", b"run the tests, then RUN them again\n"),
        ("docs/other.md", b"nothing to see\n"),
        ("blob.dat", b"\0run"),
    ]);
    let indexed = build(dir.path(), None).into_index();
    let plain = plain(dir.path());
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
            found(&indexed, &through),
            found(&plain, &full),
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
        ("hit.txt", b"needle\n"),
        ("near.txt", b"need le\n"),
        ("miss.txt", b"haystack\n"),
    ]);
    let index = build(dir.path(), None).into_index();
    let patterns = Patterns::new(["needle"], Case::Sensitive).expect("compile");

    let matches = index.grep(&patterns, &GrepOptions::default());

    assert_eq!(found(&index, &matches), ["hit.txt:1:needle"]);
    assert_eq!(matches.searched(), 1);
    assert_eq!(matches.ruled_out(), 2);
}

#[test]
fn a_pattern_that_needs_no_trigram_reads_every_candidate() {
    let dir = tree(&[("a.txt", b"alpha\n"), ("b.txt", b"beta\n")]);
    let index = build(dir.path(), None).into_index();
    let patterns = Patterns::new([r"\w+", "ab"], Case::Sensitive).expect("compile");

    let matches = index.grep(&patterns, &GrepOptions::default());

    assert_eq!(matches.ruled_out(), 0);
    assert_eq!(matches.searched(), 2);
}

#[test]
fn a_file_changed_since_the_build_is_judged_by_the_content_the_build_read() {
    let dir = tree(&[("changed.txt", b"old words\n")]);
    let index = build(dir.path(), None).into_index();
    fs::write(dir.path().join("changed.txt"), "needle\n").expect("rewrite");
    let patterns = Patterns::new(["needle"], Case::Sensitive).expect("compile");

    let matches = index.grep(&patterns, &GrepOptions::default());

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

    let dir = tree(&[("locked.txt", b"needle\n"), ("open.txt", b"haystack\n")]);
    let locked = dir.path().join("locked.txt");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }
    let index = build(dir.path(), None).into_index();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("unlock");
    let patterns = Patterns::new(["needle"], Case::Sensitive).expect("compile");

    let matches = index.grep(&patterns, &GrepOptions::default());

    assert_eq!(found(&index, &matches), ["locked.txt:1:needle"]);
    assert_eq!(matches.ruled_out(), 1, "only the file the index holds");
}

const CHUNK: usize = 64 * 1024;

fn straddling(text: &str) -> Vec<u8> {
    let mut content = vec![b'x'; CHUNK - 1];
    content.extend_from_slice(text.as_bytes());
    content.extend_from_slice(b"\n");
    content
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
    let mut late = vec![b'y'; 3 * CHUNK];
    late.extend_from_slice(b"needle far into the file\n");
    let dir = tree(&[
        ("straddle.txt", &straddling("needle")),
        ("kelvin.txt", &straddling("\u{212A}elvin")),
        ("long_s.txt", &straddling("\u{17F}tring")),
        ("late.txt", &late),
    ]);

    let streamed = build(dir.path(), None)
        .index()
        .content_index_stats()
        .expect("indexed");
    let whole = build(dir.path(), Some(&Everything))
        .index()
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
    let index = build(dir.path(), None).into_index();

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
