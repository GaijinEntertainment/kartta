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
use std::time::Duration;

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

// constraint: a stamp is trusted only when its file last changed over two seconds before the walk began, and nothing
// sets a change time back, so a tree whose files a refresh is to carry over waits that out before it is built
fn settled_tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tree(spec);
    std::thread::sleep(Duration::from_millis(2100));
    dir
}

fn stamped() -> BuildOptions {
    let mut options = BuildOptions::default();
    options.stamps = true;
    options
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

    assert_eq!(
        search(&index, "after"),
        (1, 1),
        "the new content is indexed"
    );
    assert_eq!(
        search(&index, "before"),
        (0, 2),
        "the old content is gone from the index"
    );
}

fn search(index: &Index, pattern: &str) -> (usize, usize) {
    let patterns = Patterns::new([pattern], Case::Sensitive).expect("pattern");
    let found = index.grep(&patterns, &GrepOptions::default());
    (found.files().len(), found.ruled_out())
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

    for (options, how) in [
        (BuildOptions::default(), "cancelled"),
        (stamped(), "cancelled stamped"),
    ] {
        let mut index = build(dir.path(), &options, Some(&Ids::default()));
        let cancelling = CancelAtFirstVisit {
            ids: Ids::default(),
            cancel: CancelToken::new(),
        };
        let cancelled = index.refresh(Some(&cancelling), Some(&cancelling.cancel));

        assert!(matches!(cancelled, Err(BuildError::Cancelled)));
        refresh_after_one_that_stopped(&mut index, &cancelling.ids.take(), how);
    }
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

#[test]
fn a_stamped_refresh_reads_again_only_the_files_that_changed() {
    let dir = settled_tree(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
    let ids = Ids::default();
    let mut index = build(dir.path(), &stamped(), Some(&ids));
    let built = ids.take();

    write(dir.path(), "b.rs", "changed");
    fs::remove_file(dir.path().join("c.rs")).expect("remove");
    write(dir.path(), "d.rs", "new");
    let outcome = index.refresh(Some(&ids), None).expect("refresh");

    assert_eq!(
        ids.take().keys().collect::<Vec<_>>(),
        ["b.rs", "d.rs"],
        "only the changed file and the new one are read"
    );
    let at = index
        .content_position(built["a.rs"])
        .expect("the unchanged file keeps its id");
    assert_eq!(index.paths().get(at), "a.rs");
    assert_eq!(
        index.size("a.rs"),
        Some(1),
        "and what the build learned of it"
    );
    let mut gone = vec![built["b.rs"], built["c.rs"]];
    gone.sort();
    assert_eq!(outcome.retired().collect::<Vec<_>>(), gone);
}

#[test]
fn a_file_written_just_before_the_build_is_read_again_by_a_stamped_refresh() {
    let dir = tree(&[("a.rs", "a")]);
    let ids = Ids::default();
    let mut index = build(dir.path(), &stamped(), Some(&ids));
    let built = ids.take();

    index.refresh(Some(&ids), None).expect("refresh");

    let refreshed = ids.take();
    assert_eq!(refreshed.keys().collect::<Vec<_>>(), ["a.rs"]);
    assert_ne!(refreshed["a.rs"], built["a.rs"]);
}

#[test]
fn a_stamped_refresh_hands_the_visitor_a_file_it_was_never_handed() {
    let dir = settled_tree(&[("a.rs", "a")]);
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);

    let ids = Ids::default();
    index.refresh(Some(&ids), None).expect("refresh");

    assert_eq!(ids.take().keys().collect::<Vec<_>>(), ["a.rs"]);
}

#[test]
fn a_stamped_refresh_searches_the_files_it_carried_as_those_it_read() {
    let dir = settled_tree(&[("a.txt", "kept\n"), ("b.txt", "before\n")]);
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), "b.txt", "after\n");
    index.refresh(None, None).expect("refresh");

    assert_eq!(search(&index, "kept"), (1, 1));
    assert_eq!(search(&index, "after"), (1, 1));
    assert_eq!(search(&index, "before"), (0, 2));
    assert_eq!(
        index
            .content_index_stats()
            .expect("a content index")
            .files(),
        2,
        "each file the index answers for counts once"
    );
}

#[cfg(unix)]
#[test]
fn a_stamped_refresh_reads_a_same_size_edit_whose_modification_time_was_set_back() {
    let dir = settled_tree(&[("a.txt", "before\n")]);
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);
    let path = dir.path().join("a.txt");
    let modified = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .expect("the modification time");

    write(dir.path(), "a.txt", "after!\n");
    fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(modified))
        .expect("set the modification time back");
    index.refresh(None, None).expect("refresh");

    assert_eq!(search(&index, "after"), (1, 0), "the edit is read");
    assert_eq!(search(&index, "before"), (0, 1));
}

#[test]
fn an_index_built_with_stamps_that_read_nothing_carries_files_once_a_refresh_read_them() {
    let dir = settled_tree(&[("a.rs", "a")]);
    let mut index = build(dir.path(), &stamped(), None);
    let ids = Ids::default();

    index.refresh(Some(&ids), None).expect("refresh");
    assert_eq!(ids.take().keys().collect::<Vec<_>>(), ["a.rs"]);
    index.refresh(Some(&ids), None).expect("refresh");

    assert!(ids.take().is_empty(), "the second refresh carries the file");
}

#[test]
fn a_stamped_refresh_of_an_unchanged_tree_reads_nothing_and_retires_nothing() {
    let dir = settled_tree(&[(".gitignore", "*.log\n"), ("a.rs", "a"), ("src/b.rs", "b")]);
    let ids = Ids::default();
    let mut index = build(dir.path(), &stamped(), Some(&ids));
    let built = ids.take();
    assert!(
        built.contains_key(".gitignore"),
        "the visitor is handed the rules too"
    );

    let outcome = index.refresh(Some(&ids), None).expect("refresh");

    assert!(ids.take().is_empty(), "nothing is handed over again");
    assert_eq!(outcome.retired().count(), 0);
    for (path, id) in &built {
        let position = index
            .content_position(*id)
            .expect("every id still names its file");
        assert_eq!(index.paths().get(position), path);
    }
}

#[test]
fn a_refresh_that_leaves_too_many_files_gone_from_the_lists_merges_them_into_one() {
    let names: Vec<String> = (0..12).map(|at| format!("file{at:02}.txt")).collect();
    let spec: Vec<(&str, String)> = names
        .iter()
        .map(|name| (name.as_str(), format!("kept {name}\n")))
        .collect();
    let spec: Vec<(&str, &str)> = spec
        .iter()
        .map(|(name, text)| (*name, text.as_str()))
        .collect();
    let dir = settled_tree(&spec);
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);

    for name in &names[..5] {
        write(dir.path(), name, &format!("changed {name}\n"));
    }
    index.refresh(None, None).expect("refresh");

    let fresh = build(dir.path(), &options, None);
    let stats = index.content_index_stats().expect("a content index");
    let built = fresh.content_index_stats().expect("a content index");
    assert_eq!(
        (stats.files(), stats.trigrams()),
        (built.files(), built.trigrams()),
        "one segment, each trigram counted once, as a build makes"
    );
    assert_eq!(search(&index, "changed"), (5, 7));
    assert_eq!(search(&index, "kept"), (7, 5));
    assert_eq!(search(&index, "file00"), (1, 11));
}
