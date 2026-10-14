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
    BuildError, BuildOptions, BuildOutcome, Content, ContentIndexStorage, ContentVisitor, Index,
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
