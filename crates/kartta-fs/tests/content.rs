//! The content contract: what a build reads for a `ContentVisitor`, what the visitor is handed, and what the reads
//! leave behind in the index.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use kartta_fs::{
    BuildError, BuildOptions, BuildOutcome, CancelToken, Content, ContentId, ContentVisitor, Index,
    PrunedNames,
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

type Seen = BTreeMap<String, (ContentId, Vec<u8>)>;

struct Recorder {
    wanted: fn(&str) -> bool,
    asked: Mutex<Vec<String>>,
    seen: Mutex<Seen>,
    cancel_after_first: Option<CancelToken>,
}

impl Recorder {
    fn new(wanted: fn(&str) -> bool) -> Self {
        Self {
            wanted,
            asked: Mutex::new(Vec::new()),
            seen: Mutex::new(BTreeMap::new()),
            cancel_after_first: None,
        }
    }

    fn asked(&self) -> Vec<String> {
        let mut asked = self.asked.lock().expect("lock").clone();
        asked.sort();
        asked
    }

    fn seen(&self) -> Seen {
        self.seen.lock().expect("lock").clone()
    }
}

impl ContentVisitor for Recorder {
    fn wants(&self, path: &str) -> bool {
        self.asked.lock().expect("lock").push(path.to_owned());
        (self.wanted)(path)
    }

    fn visit(&self, content: Content<'_>) {
        let previous = self.seen.lock().expect("lock").insert(
            content.path().to_owned(),
            (content.id(), content.bytes().to_vec()),
        );
        assert!(previous.is_none(), "{} was visited twice", content.path());
        if let Some(cancel) = &self.cancel_after_first {
            cancel.cancel();
        }
    }
}

fn build_with(root: &Path, visitor: &Recorder) -> BuildOutcome {
    Index::build_with(root, &BuildOptions::default(), Some(visitor), None).expect("build the index")
}

const fn everything(_path: &str) -> bool {
    true
}

#[test]
fn a_wanted_text_file_is_handed_over_whole_under_an_id_that_finds_its_position() {
    let large = vec![b'x'; 200 * 1024];
    let dir = tree(&[
        ("src/main.rs", b"fn main() {}\n"),
        ("src/large.txt", &large),
    ]);
    let visitor = Recorder::new(everything);

    let outcome = build_with(dir.path(), &visitor);
    assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    let index = outcome.into_index();

    let seen = visitor.seen();
    assert_eq!(
        seen.keys().map(String::as_str).collect::<Vec<_>>(),
        ["src/large.txt", "src/main.rs"]
    );
    assert_eq!(seen["src/main.rs"].1, b"fn main() {}\n");
    assert_eq!(
        seen["src/large.txt"].1.len(),
        large.len(),
        "content past the binary probe is read too"
    );

    let mut paths = index.paths();
    for (path, (id, _)) in &seen {
        let position = index
            .content_position(*id)
            .expect("an id this build handed out has a position");
        assert_eq!(paths.get(position), path);
    }
}

#[test]
fn only_wanted_files_are_read_and_their_facts_are_kept() {
    let dir = tree(&[("read.rs", b"fn read() {}\n"), ("skipped.md", b"skipped\n")]);
    let visitor = Recorder::new(|path| path == "read.rs");

    let index = build_with(dir.path(), &visitor).into_index();
    fs::remove_file(dir.path().join("read.rs")).expect("remove");
    fs::remove_file(dir.path().join("skipped.md")).expect("remove");

    assert_eq!(
        visitor
            .seen()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["read.rs"]
    );
    assert_eq!(
        index.size("read.rs"),
        Some(13),
        "the build's read settled the size"
    );
    assert_eq!(index.is_binary("read.rs"), Some(false));
    assert_eq!(
        index.size("skipped.md"),
        None,
        "an unwanted file was never read, and is gone now"
    );
}

#[test]
fn a_wanted_ignore_file_is_handed_over_from_the_read_its_rules_came_from() {
    let dir = tree(&[("sub/.gitignore", b"*.log\n"), ("sub/kept.rs", b"x")]);
    let visitor = Recorder::new(everything);

    let index = build_with(dir.path(), &visitor).into_index();

    let seen = visitor.seen();
    let (id, bytes) = &seen["sub/.gitignore"];
    assert_eq!(bytes, b"*.log\n");
    let position = index.content_position(*id).expect("the read was kept");
    assert_eq!(index.paths().get(position), "sub/.gitignore");
    assert_eq!(index.size("sub/.gitignore"), Some(6));
}

#[test]
fn an_ignored_ignore_file_still_applies_its_rules_and_is_never_offered() {
    let dir = tree(&[
        (".gitignore", b"sub/.gitignore\n"),
        ("sub/.gitignore", b"*.log\n"),
        ("sub/debug.log", b"x"),
        ("sub/kept.rs", b"x"),
    ]);
    let visitor = Recorder::new(everything);

    let index = build_with(dir.path(), &visitor).into_index();

    assert!(index.entry("sub/.gitignore").is_none());
    assert!(
        index.entry("sub/debug.log").is_none(),
        "its rules applied all the same"
    );
    assert_eq!(visitor.asked(), [".gitignore", "sub/kept.rs"]);
    assert!(!visitor.seen().contains_key("sub/.gitignore"));
}

#[test]
fn a_pruned_ignore_file_still_applies_its_rules_and_is_never_offered() {
    let dir = tree(&[
        (".gitignore", b"*.log\n"),
        (".vscode/settings.json", b"{}"),
        ("debug.log", b"x"),
        ("kept.rs", b"x"),
    ]);
    let mut options = BuildOptions::default();
    options.pruned_names =
        PrunedNames::builtin().union(&PrunedNames::new([".gitignore"]).expect("a name"));
    let visitor = Recorder::new(everything);

    let outcome = Index::build_with(dir.path(), &options, Some(&visitor), None).expect("build");
    let index = outcome.into_index();

    assert!(index.entry(".gitignore").is_none());
    assert!(
        index.entry("debug.log").is_none(),
        "its rules applied all the same"
    );
    assert_eq!(visitor.asked(), ["kept.rs"], "nothing pruned is offered");
    assert_eq!(visitor.seen().keys().collect::<Vec<_>>(), ["kept.rs"]);
}

#[test]
fn ignored_files_and_directories_are_never_offered() {
    let dir = tree(&[
        (".gitignore", b"target/\n*.log\n"),
        ("kept.rs", b"x"),
        ("debug.log", b"x"),
        ("target/out.rs", b"x"),
    ]);
    let visitor = Recorder::new(everything);

    build_with(dir.path(), &visitor);

    assert_eq!(visitor.asked(), [".gitignore", "kept.rs"]);
}

#[test]
fn binary_content_is_never_visited_and_a_binary_extension_is_never_opened() {
    let dir = tree(&[
        ("blob.dat", b"\0binary"),
        ("image.png", b"looks like text"),
        ("text.txt", b"text"),
    ]);
    let visitor = Recorder::new(everything);

    let index = build_with(dir.path(), &visitor).into_index();
    fs::remove_file(dir.path().join("blob.dat")).expect("remove");
    fs::remove_file(dir.path().join("image.png")).expect("remove");

    assert_eq!(
        visitor
            .seen()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["text.txt"]
    );
    assert_eq!(
        index.is_binary("blob.dat"),
        Some(true),
        "the build read the NUL and kept the verdict"
    );
    assert_eq!(
        index.size("image.png"),
        None,
        "never opened by the build, so no size was kept and the ask finds the file gone"
    );
}

#[cfg(unix)]
#[test]
fn a_wanted_file_that_will_not_read_is_reported_and_still_indexed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tree(&[("locked.rs", b"x"), ("open.rs", b"y")]);
    let locked = dir.path().join("locked.rs");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }
    let visitor = Recorder::new(everything);

    let outcome = build_with(dir.path(), &visitor);

    assert!(
        matches!(outcome.errors(), [BuildError::Read { native_path, .. }] if native_path.ends_with("locked.rs")),
        "{:?}",
        outcome.errors()
    );
    assert!(outcome.index().entry("locked.rs").is_some());
    assert_eq!(
        visitor
            .seen()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["open.rs"]
    );
}

#[cfg(unix)]
#[test]
fn a_gitignore_that_will_not_read_for_its_rules_is_still_offered_to_the_visitor() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tree(&[(".gitignore", b"*.log\n"), ("main.rs", b"x")]);
    let locked = dir.path().join(".gitignore");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }
    let visitor = Recorder::new(everything);

    let outcome = build_with(dir.path(), &visitor);

    assert_eq!(visitor.asked(), [".gitignore", "main.rs"]);
    assert!(
        matches!(
            outcome.errors(),
            [BuildError::Ignore { .. }, BuildError::Read { .. }]
        ),
        "the rules and the content each report the failed read: {:?}",
        outcome.errors()
    );
}

#[test]
fn the_visitor_runs_on_the_threads_the_build_was_given() {
    struct Threads(Mutex<Vec<Option<String>>>);

    impl ContentVisitor for Threads {
        fn wants(&self, _path: &str) -> bool {
            true
        }

        fn visit(&self, _content: Content<'_>) {
            let name = std::thread::current().name().map(str::to_owned);
            self.0.lock().expect("lock").push(name);
        }
    }

    let dir = tree(&[("a.rs", b"a"), ("b.rs", b"b")]);
    let threads = Threads(Mutex::new(Vec::new()));
    let mut options = BuildOptions::default();
    options.threads = std::num::NonZeroUsize::new(2);

    let index = Index::build_with(dir.path(), &options, Some(&threads), None)
        .expect("build")
        .into_index();

    let names = threads.0.into_inner().expect("lock");
    assert_eq!(names.len(), 2);
    assert!(
        names.iter().all(|name| name
            .as_deref()
            .is_some_and(|name| name.starts_with("kartta-fs-"))),
        "every read ran on the index's own pool, which the thread count bounds: {names:?}"
    );
    assert_eq!(index.threads(), 2);
}

// constraint: only unix filesystems keep a file extended by `set_len` sparse, so elsewhere this writes 1 TiB
#[cfg(unix)]
#[test]
fn a_binary_file_too_large_to_hold_is_classified_without_reserving_room_for_it() {
    let dir = tree(&[]);
    fs::File::create(dir.path().join("huge.blob"))
        .and_then(|file| file.set_len(1 << 40))
        .expect("create a sparse file");
    let visitor = Recorder::new(everything);

    let outcome = build_with(dir.path(), &visitor);

    assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    assert!(visitor.seen().is_empty());
    assert_eq!(outcome.index().is_binary("huge.blob"), Some(true));
    assert_eq!(outcome.index().size("huge.blob"), Some(1 << 40));
}

#[test]
fn a_build_without_a_visitor_reads_no_file() {
    let dir = tree(&[("main.rs", b"fn main() {}\n")]);

    let index = Index::build(dir.path(), &BuildOptions::default())
        .expect("build")
        .into_index();
    fs::remove_file(dir.path().join("main.rs")).expect("remove");

    assert_eq!(index.size("main.rs"), None, "nothing was read at the walk");
}

#[test]
fn a_cancelled_build_returns_no_index() {
    let dir = tree(&[("a.rs", b"a"), ("b.rs", b"b"), ("c.rs", b"c")]);
    let cancel = CancelToken::new();
    let mut visitor = Recorder::new(everything);
    visitor.cancel_after_first = Some(cancel.clone());
    let mut options = BuildOptions::default();
    options.threads = std::num::NonZeroUsize::new(1);

    let built = Index::build_with(dir.path(), &options, Some(&visitor), Some(&cancel));

    assert!(matches!(built, Err(BuildError::Cancelled)));
    assert_eq!(
        visitor.seen().len(),
        1,
        "the first file was handed over before the stop"
    );
}

#[test]
fn a_build_without_a_visitor_can_still_be_cancelled() {
    let dir = tree(&[("a.rs", b"a"), ("b.rs", b"b")]);
    let cancel = CancelToken::new();

    let quiet = Index::build_with(dir.path(), &BuildOptions::default(), None, Some(&cancel))
        .expect("an uncancelled token builds the whole index");
    cancel.cancel();
    let stopped = Index::build_with(dir.path(), &BuildOptions::default(), None, Some(&cancel));

    assert_eq!(quiet.index().len(), 2);
    assert!(matches!(stopped, Err(BuildError::Cancelled)));
}
