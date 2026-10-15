//! The refresh contract: what an index shows after `Index::refresh`, which ids the refresh hands out and retires, and
//! what a refresh that does not finish leaves behind.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

use kartta_fs::{
    BuildError, BuildOptions, CancelToken, Case, Content, ContentId, ContentIndexStorage,
    ContentVisitor, GrepOptions, Index, Patterns,
};
use tempfile::TempDir;

fn tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    for (path, content) in spec {
        write(dir.path(), path, content);
    }
    dir
}

fn write(root: &Path, path: &str, content: &str) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(&full, content).expect("write file");
}

fn paths(index: &Index) -> Vec<String> {
    index
        .entries()
        .map(|entry| entry.path().to_owned())
        .collect()
}

fn build(root: &Path, options: &BuildOptions, visitor: Option<&dyn ContentVisitor>) -> Index {
    let outcome = Index::build_with(root, options, visitor, None).expect("build the index");
    assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    outcome.into_index()
}

#[derive(Default)]
struct Ids(Mutex<BTreeMap<String, ContentId>>);

impl Ids {
    fn take(&self) -> BTreeMap<String, ContentId> {
        std::mem::take(&mut *self.0.lock().expect("lock"))
    }
}

impl ContentVisitor for Ids {
    fn wants(&self, _path: &str) -> bool {
        true
    }

    fn visit(&self, content: Content<'_>) {
        self.0
            .lock()
            .expect("lock")
            .insert(content.path().to_owned(), content.id());
    }
}

#[test]
fn a_refresh_shows_what_was_added_and_removed_since_the_build() {
    let dir = tree(&[("keep.rs", "a"), ("gone.rs", "b"), ("src/lib.rs", "c")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    fs::remove_file(dir.path().join("gone.rs")).expect("remove");
    write(dir.path(), "src/new.rs", "d");
    let outcome = index.refresh(None, None).expect("refresh");

    assert!(outcome.errors().is_empty(), "{:?}", outcome.errors());
    assert_eq!(
        paths(&index),
        ["keep.rs", "src", "src/lib.rs", "src/new.rs"]
    );
}

#[test]
fn a_refresh_applies_ignore_rules_as_they_are_now() {
    let dir = tree(&[(".gitignore", "*.log\n"), ("a.log", ""), ("b.txt", "")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    assert!(index.entry("a.log").is_none());

    write(dir.path(), ".gitignore", "*.txt\n");
    index.refresh(None, None).expect("refresh");

    assert!(
        index.entry("a.log").is_some(),
        "the rule that hid it is gone"
    );
    assert!(index.entry("b.txt").is_none(), "the new rule hides it");
}

#[test]
fn a_refresh_names_each_file_by_an_id_no_earlier_read_used_and_retires_the_old_ones() {
    let dir = tree(&[("a.rs", "a"), ("b.rs", "b")]);
    let ids = Ids::default();
    let mut index = build(dir.path(), &BuildOptions::default(), Some(&ids));
    let built = ids.take();

    let outcome = index.refresh(Some(&ids), None).expect("refresh");
    let refreshed = ids.take();

    assert_eq!(
        refreshed.keys().collect::<Vec<_>>(),
        ["a.rs", "b.rs"],
        "every wanted file is handed over again"
    );
    let earliest_new = refreshed.values().min().expect("ids");
    assert!(
        built.values().all(|old| old < earliest_new),
        "new ids follow every old one"
    );

    let retired: Vec<ContentId> = outcome.retired().collect();
    let mut old: Vec<ContentId> = built.values().copied().collect();
    old.sort();
    assert_eq!(retired, old);
    for id in &retired {
        assert_eq!(
            index.content_position(*id),
            None,
            "a retired id names nothing"
        );
    }
    for (path, id) in &refreshed {
        let position = index
            .content_position(*id)
            .expect("a new id names its file");
        assert_eq!(index.paths().get(position), path);
    }
}

#[test]
fn a_refresh_searches_content_as_it_is_now() {
    let dir = tree(&[("a.txt", "before\n"), ("b.txt", "other\n")]);
    let mut options = BuildOptions::default();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), "a.txt", "after\n");
    index.refresh(None, None).expect("refresh");

    let search = |pattern: &str| {
        let patterns = Patterns::new([pattern], Case::Sensitive).expect("pattern");
        let found = index.grep(&patterns, &GrepOptions::default());
        (found.files().len(), found.ruled_out())
    };
    assert_eq!(search("after"), (1, 1), "the new content is indexed");
    assert_eq!(
        search("before"),
        (0, 2),
        "the old content is gone from the index"
    );
}

#[test]
fn a_cancelled_refresh_leaves_the_index_as_it_was() {
    let dir = tree(&[("a.rs", "a")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    write(dir.path(), "b.rs", "b");

    let cancel = CancelToken::new();
    cancel.cancel();
    let refreshed = index.refresh(None, Some(&cancel));

    assert!(matches!(refreshed, Err(BuildError::Cancelled)));
    assert_eq!(paths(&index), ["a.rs"]);
}

struct CancelAtFirstVisit {
    ids: Ids,
    cancel: CancelToken,
}

impl ContentVisitor for CancelAtFirstVisit {
    fn wants(&self, _path: &str) -> bool {
        true
    }

    fn visit(&self, content: Content<'_>) {
        self.ids.visit(content);
        self.cancel.cancel();
    }
}

#[test]
fn a_refresh_never_hands_out_again_an_id_a_cancelled_one_handed_its_visitor() {
    let names: Vec<String> = (0..16).map(|at| format!("file{at:02}.rs")).collect();
    let spec: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "x")).collect();
    let dir = tree(&spec);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    let cancelling = CancelAtFirstVisit {
        ids: Ids::default(),
        cancel: CancelToken::new(),
    };
    let cancelled = index.refresh(Some(&cancelling), Some(&cancelling.cancel));

    assert!(matches!(cancelled, Err(BuildError::Cancelled)));
    refresh_after_one_that_stopped(&mut index, &cancelling.ids.take(), "cancelled");
}

struct PanicAtFirstVisit(Ids);

impl ContentVisitor for PanicAtFirstVisit {
    fn wants(&self, _path: &str) -> bool {
        true
    }

    #[expect(clippy::panic, reason = "a visitor that panics is the case under test")]
    fn visit(&self, content: Content<'_>) {
        self.0.visit(content);
        panic!("a visitor that fails on the first file it is handed");
    }
}

#[test]
fn a_refresh_never_hands_out_again_an_id_a_visitor_saw_before_it_panicked() {
    let dir = tree(&[("a.rs", "a"), ("b.rs", "b")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    let panicking = PanicAtFirstVisit(Ids::default());

    let unwound = panic::catch_unwind(AssertUnwindSafe(|| index.refresh(Some(&panicking), None)));

    assert!(unwound.is_err(), "the visitor's panic reaches the caller");
    refresh_after_one_that_stopped(&mut index, &panicking.0.take(), "unwound");
}

fn refresh_after_one_that_stopped(
    index: &mut Index,
    stopped: &BTreeMap<String, ContentId>,
    how: &str,
) {
    assert!(!stopped.is_empty(), "the {how} refresh handed out an id");
    let ids = Ids::default();
    let outcome = index.refresh(Some(&ids), None).expect("refresh");

    assert!(
        ids.take()
            .values()
            .all(|id| !stopped.values().any(|then| then == id)),
        "no id names two files after the {how} refresh"
    );
    let retired: Vec<ContentId> = outcome.retired().collect();
    assert!(
        stopped.values().all(|id| retired.contains(id)),
        "the ids the {how} refresh handed out are retired"
    );
}

#[test]
fn a_refresh_runs_on_the_threads_the_index_was_built_with() {
    let dir = tree(&[("a.rs", "a")]);
    let mut options = BuildOptions::default();
    options.threads = NonZeroUsize::new(2);
    let mut index = build(dir.path(), &options, None);

    index.refresh(None, None).expect("refresh");

    assert_eq!(index.threads(), 2);
}
