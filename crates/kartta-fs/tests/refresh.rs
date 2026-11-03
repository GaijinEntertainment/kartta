//! The refresh contract: what an index shows after `Index::refresh` and `Index::refresh_paths`, which ids a refresh
//! hands out and retires, and what a refresh that does not finish leaves behind.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;
use std::thread::ThreadId;
use std::time::Duration;

use common::{tree, write};
use kartta_fs::{
    BuildError, BuildOptions, CancelToken, Case, Content, ContentId, ContentIndexStorage,
    ContentVisitor, EntryKind, FileStamp, GrepOptions, IgnoreSources, Index, PathFault, Patterns,
    PrunedNames, RefreshOutcome,
};
use tempfile::TempDir;

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
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    outcome.index
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

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(
        paths(&index),
        ["keep.rs", "src", "src/lib.rs", "src/new.rs"]
    );
}

#[test]
fn a_refresh_lists_the_regular_files_it_added_and_removed_and_not_those_it_changed() {
    let dir = tree(&[
        ("edited.rs", "a"),
        ("gone.rs", "b"),
        ("became_dir", "c"),
        ("became_file/inner.rs", "d"),
        ("hidden.log", "e"),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "edited.rs", "changed");
    fs::remove_file(dir.path().join("gone.rs")).expect("remove");
    fs::remove_file(dir.path().join("became_dir")).expect("remove");
    write(dir.path(), "became_dir/inner.rs", "f");
    fs::remove_dir_all(dir.path().join("became_file")).expect("remove");
    write(dir.path(), "became_file", "g");
    write(dir.path(), ".gitignore", "*.log\n");
    let outcome = index.refresh(None, None).expect("refresh");

    assert_eq!(
        outcome.added,
        [".gitignore", "became_dir/inner.rs", "became_file"]
    );
    assert_eq!(
        outcome.removed,
        [
            "became_dir",
            "became_file/inner.rs",
            "gone.rs",
            "hidden.log"
        ]
    );
}

#[test]
fn refreshing_paths_lists_the_regular_files_added_and_removed_in_the_parts_it_walked() {
    let dir = tree(&[("keep.rs", "a"), ("gone.rs", "b"), ("old/one.rs", "c")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    fs::remove_file(dir.path().join("gone.rs")).expect("remove");
    fs::remove_dir_all(dir.path().join("old")).expect("remove");
    write(dir.path(), "new/deep/two.rs", "d");
    write(dir.path(), "unnamed.rs", "e");
    let outcome = index
        .refresh_paths(["gone.rs", "old", "new/deep/two.rs"], None, None)
        .expect("refresh the paths");

    assert_eq!(outcome.walked, ["gone.rs", "new", "old"]);
    assert_eq!(outcome.added, ["new/deep/two.rs"]);
    assert_eq!(outcome.removed, ["gone.rs", "old/one.rs"]);
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

// constraint: Windows refuses to remove a directory while the index's scratch file is open in it
#[cfg(unix)]
#[test]
fn a_refresh_that_cannot_keep_the_content_index_leaves_it_answering_as_it_did() {
    let dir = tree(&[("a.txt", "before\n"), ("b.txt", "other\n")]);
    let scratch = TempDir::new().expect("a scratch directory");
    let gone = scratch.path().join("gone");
    fs::create_dir(&gone).expect("the directory");
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::File { dir: gone.clone() });
    let mut index = build(dir.path(), &options, None);
    fs::remove_dir(&gone).expect("the directory goes");

    write(dir.path(), "a.txt", "after\n");
    let whole = index.refresh(None, None);
    let named = index.refresh_paths(["a.txt"], None, None);

    assert!(
        matches!(whole, Err(BuildError::ContentIndex { .. })),
        "{whole:?}"
    );
    assert!(
        matches!(named, Err(BuildError::ContentIndex { .. })),
        "{named:?}"
    );
    assert_eq!(index.content_index_failure(), None);
    assert_eq!(
        index.content_index_stats().map(|stats| stats.files()),
        Some(2)
    );
    assert_eq!(search(&index, "before"), (0, 1), "the old index narrows");
}

#[cfg(unix)]
struct RemovesDir(std::path::PathBuf);

#[cfg(unix)]
impl ContentVisitor for RemovesDir {
    fn wants(&self, _path: &str) -> bool {
        true
    }

    fn visit(&self, _content: Content<'_>) {
        drop(fs::remove_dir_all(&self.0));
    }
}

#[cfg(unix)]
#[test]
fn a_refresh_that_loses_its_scratch_directory_during_the_walk_leaves_the_index_answering() {
    let dir = tree(&[("a.txt", "before\n"), ("b.txt", "other\n")]);
    let scratch = TempDir::new().expect("a scratch directory");
    let gone = scratch.path().join("gone");
    fs::create_dir(&gone).expect("the directory");
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::File { dir: gone.clone() });
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), "a.txt", "after\n");
    let refreshed = index.refresh(Some(&RemovesDir(gone)), None);

    assert!(
        matches!(refreshed, Err(BuildError::ContentIndex { .. })),
        "{refreshed:?}"
    );
    assert_eq!(index.content_index_failure(), None);
    assert_eq!(
        index.content_index_stats().map(|stats| stats.files()),
        Some(2)
    );
    assert_eq!(search(&index, "before"), (0, 1), "the old index narrows");
}

#[test]
fn a_content_index_a_build_gave_up_stays_absent_and_explained_through_refreshes() {
    let dir = tree(&[("a.txt", "before\n"), ("b.txt", "other\n")]);
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::File {
        dir: dir.path().join("no such directory"),
    });
    let outcome = Index::build(dir.path(), &options).expect("the build goes on");
    let failure = outcome
        .errors
        .first()
        .expect("the build collected it")
        .to_string();
    let mut index = outcome.index;

    write(dir.path(), "a.txt", "after\n");
    let named = index
        .refresh_paths(["a.txt"], None, None)
        .expect("refresh the path");
    write(dir.path(), "a.txt", "last\n");
    let whole = index.refresh(None, None).expect("refresh the tree");

    assert!(named.errors.is_empty(), "{:?}", named.errors);
    assert!(whole.errors.is_empty(), "{:?}", whole.errors);
    assert_eq!(index.content_index_failure(), Some(failure.as_str()));
    assert_eq!(index.content_index_stats(), None);
    assert_eq!(search(&index, "last"), (1, 0));
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
fn a_file_stamp_is_trusted_once_the_file_settled_and_tells_every_write_after_it() {
    let dir = tree(&[("a.rs", "a")]);
    let a = dir.path().join("a.rs");
    let fresh = FileStamp::of(&a);
    std::thread::sleep(Duration::from_millis(2100));

    let settled = FileStamp::of(&a).expect("a settled file has a stamp");
    let again = FileStamp::of(&a);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&a, dir.path().join("link.rs")).expect("link to a.rs");
        assert_eq!(
            FileStamp::of(&dir.path().join("link.rs")),
            Some(settled),
            "a link to the file"
        );
    }
    write(dir.path(), "a.rs", "z");
    let rewritten = FileStamp::of(&a);
    std::thread::sleep(Duration::from_millis(2100));
    let edited = FileStamp::of(&a).expect("a settled edit has a stamp");

    assert_eq!(fresh, None, "a file written within two seconds");
    assert_eq!(again, Some(settled));
    assert_eq!(rewritten, None, "a same-size write within two seconds");
    assert_ne!(edited, settled);
    assert_eq!(FileStamp::of(dir.path()), None, "a directory");
    assert_eq!(FileStamp::of(&dir.path().join("gone.rs")), None);
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
        write(dir.path(), name, format!("changed {name}\n"));
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

type Shown = (String, EntryKind, Option<String>);

fn shown(index: &Index) -> Vec<Shown> {
    index
        .entries()
        .map(|entry| {
            (
                entry.path().to_owned(),
                entry.kind(),
                entry.target().map(str::to_owned),
            )
        })
        .collect()
}

fn assert_as_built(index: &Index, options: &BuildOptions) {
    let built = build(index.root(), options, None);
    assert_eq!(shown(index), shown(&built));
    for entry in built.entries() {
        assert!(
            index.entry(entry.path()).is_some(),
            "{} is found by its path",
            entry.path()
        );
    }
}

fn refresh_paths<'p>(index: &mut Index, paths: impl IntoIterator<Item = &'p str>) -> Vec<String> {
    let outcome = index
        .refresh_paths(paths, None, None)
        .expect("refresh the paths");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    outcome.walked
}

#[test]
fn refreshing_the_paths_that_changed_makes_the_index_a_build_would_make() {
    let dir = tree(&[
        ("a.rs", "a"),
        ("src/gone.rs", "b"),
        ("src/edit.rs", "c"),
        ("old/x.rs", "d"),
        ("old/deep/y.rs", "e"),
        ("swap", "f"),
        ("z.rs", "g"),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    fs::remove_file(dir.path().join("src/gone.rs")).expect("remove");
    write(dir.path(), "src/edit.rs", "changed");
    write(dir.path(), "src/added.rs", "h");
    write(dir.path(), "new/deeper/still/w.rs", "i");
    fs::remove_dir_all(dir.path().join("old")).expect("remove the directory");
    fs::remove_file(dir.path().join("swap")).expect("remove");
    write(dir.path(), "swap/inside.rs", "j");
    let walked = refresh_paths(
        &mut index,
        [
            "src/gone.rs",
            "src/edit.rs",
            "src/added.rs",
            "new/deeper/still/w.rs",
            "old",
            "swap/inside.rs",
        ],
    );

    assert_as_built(&index, &BuildOptions::default());
    assert_eq!(
        walked,
        [
            "new",
            "old",
            "src/added.rs",
            "src/edit.rs",
            "src/gone.rs",
            "swap"
        ],
        "a path below a directory the index lacks walks that directory"
    );
}

#[test]
fn paths_given_twice_out_of_order_or_below_one_another_are_walked_once() {
    let dir = tree(&[("a/b/c.rs", "a"), ("a/d.rs", "b"), ("e.rs", "c")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "a/b/new.rs", "d");
    let walked = refresh_paths(
        &mut index,
        ["e.rs", "./a/b/new.rs", "a//b", "a/b/", "a/d.rs", "e.rs"],
    );

    assert_eq!(walked, ["a/b", "a/d.rs", "e.rs"]);
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn a_refresh_walked_over_the_paths_inside_its_parts_and_no_other() {
    let dir = tree(&[
        ("a/b/c.rs", "a"),
        ("a/b-x/d.rs", "b"),
        ("a/b.rs", "c"),
        ("a/bc/e.rs", "d"),
        ("e.rs", "e"),
        ("z/f.rs", "f"),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    let named = index
        .refresh_paths(["e.rs", "a/b-x", "a/b"], None, None)
        .expect("refresh the paths");
    let whole = index.refresh(None, None).expect("refresh the tree");
    let none = index
        .refresh_paths([], None, None)
        .expect("refresh no path");

    assert_eq!(named.walked, ["a/b", "a/b-x", "e.rs"]);
    let over: Vec<&str> = [
        "a/b",
        "a/b/c.rs",
        "a/b/deep/new.rs",
        "a/b-x/d.rs",
        "a/b.rs",
        "a/bc/e.rs",
        "a",
        "",
        "e.rs",
        "z/f.rs",
    ]
    .into_iter()
    .filter(|path| named.walked_over(path))
    .collect();
    assert_eq!(
        over,
        ["a/b", "a/b/c.rs", "a/b/deep/new.rs", "a/b-x/d.rs", "e.rs"]
    );
    assert!(whole.walked_over("z/f.rs") && whole.walked_over(""));
    assert!(none.walked.is_empty() && !none.walked_over("z/f.rs") && !none.walked_over(""));
}

/// Asserts that every entry before `outcome` either sits after it where [`RefreshOutcome::position_after`] says, or lay
/// inside a part it walked, and that every position after it holds a kept entry or lies inside a walked part.
fn assert_placed(before: &[String], after: &[String], outcome: &RefreshOutcome) {
    let mut placed = vec![false; after.len()];
    for (position, path) in before.iter().enumerate() {
        match outcome.position_after(position) {
            Some(moved) => {
                assert_eq!(after[moved], *path, "{path} moved to {moved}");
                placed[moved] = true;
            }
            None => assert!(outcome.walked_over(path), "{path} lay in a walked part"),
        }
    }
    for at in outcome.walked_positions().flatten() {
        assert!(!placed[at], "{} is both kept and walked", after[at]);
        placed[at] = true;
    }
    assert!(placed.iter().all(|&placed| placed), "{placed:?}");
}

#[test]
fn a_refresh_says_where_each_entry_it_kept_sits_now_and_what_its_parts_hold() {
    let dir = tree(&[
        ("a/b.rs", "a"),
        ("a/c.rs", "b"),
        ("d/e.rs", "c"),
        ("f.rs", "d"),
        ("g/h.rs", "e"),
        ("z.rs", "f"),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    let before = paths(&index);

    fs::remove_file(dir.path().join("a/b.rs")).expect("remove");
    write(dir.path(), "d/new.rs", "g");
    write(dir.path(), "n/deep/x.rs", "h");
    fs::remove_dir_all(dir.path().join("g")).expect("remove the directory");
    let outcome = index
        .refresh_paths(["a/b.rs", "d/new.rs", "n/deep/x.rs", "g"], None, None)
        .expect("refresh the paths");

    assert!(!outcome.kept_positions());
    let after = paths(&index);
    let walked: Vec<Vec<&str>> = outcome
        .walked_positions()
        .map(|run| run.map(|at| after[at].as_str()).collect())
        .collect();
    let gone: [&str; 0] = [];
    assert_eq!(
        walked,
        [&gone[..], &["d/new.rs"], &["n", "n/deep", "n/deep/x.rs"]],
        "a run for a/b.rs, for d/new.rs, and one for g and n, which lay side by side"
    );
    assert_placed(&before, &after, &outcome);
    assert_eq!(outcome.position_after(before.len()), None);
}

#[test]
fn an_entry_that_changed_kind_in_place_is_not_kept() {
    let dir = tree(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    let before = paths(&index);

    fs::remove_file(dir.path().join("b.rs")).expect("remove");
    fs::create_dir(dir.path().join("b.rs")).expect("make a directory in its place");
    let outcome = index
        .refresh_paths(["b.rs"], None, None)
        .expect("refresh the path");

    assert_eq!(index.len(), before.len());
    assert_eq!(index.kind_at(1), EntryKind::Dir);
    assert!(!outcome.kept_positions());
    assert_eq!(
        outcome.position_after(1),
        None,
        "b.rs is not the entry it was"
    );
    assert_placed(&before, &paths(&index), &outcome);
}

#[test]
fn a_refresh_that_only_read_files_again_kept_every_position() {
    let dir = tree(&[("a/b.rs", "a"), ("c.rs", "b"), ("d/e.rs", "c")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    let before = paths(&index);

    write(dir.path(), "a/b.rs", "edited");
    let named = index
        .refresh_paths(["a/b.rs", "d"], None, None)
        .expect("refresh the paths");
    let whole = index.refresh(None, None).expect("refresh the tree");
    write(dir.path(), "new.rs", "d");
    let grown = index.refresh(None, None).expect("refresh the tree");

    assert!(named.kept_positions() && whole.kept_positions());
    assert_eq!(paths(&index).len(), before.len() + 1);
    assert_placed(&before, &before, &named);
    assert!(!grown.kept_positions());
    assert_eq!(
        grown.position_after(0),
        None,
        "a whole refresh walks every entry again"
    );
    let walked: Vec<usize> = grown.walked_positions().flatten().collect();
    assert_eq!(walked, (0..=before.len()).collect::<Vec<_>>());
    assert_eq!(grown.walked_positions().len(), 1);
}

#[test]
fn the_named_files_a_refresh_reads_are_read_on_the_index_threads() {
    #[derive(Default)]
    struct Threads(Mutex<Vec<(String, ThreadId)>>);

    impl ContentVisitor for Threads {
        fn wants(&self, _path: &str) -> bool {
            true
        }

        fn visit(&self, content: Content<'_>) {
            let thread = std::thread::current().id();
            self.0
                .lock()
                .expect("lock")
                .push((content.path().to_owned(), thread));
        }
    }

    let dir = tree(&[("a.rs", "a"), ("b.rs", "b"), ("src/c.rs", "c")]);
    let mut options = BuildOptions::default();
    options.threads = NonZeroUsize::new(2);
    let mut index = build(dir.path(), &options, None);
    let pool: Vec<ThreadId> =
        index.on_threads(|| rayon::broadcast(|_| std::thread::current().id()));
    let threads = Threads::default();

    let outcome = index
        .refresh_paths(["a.rs", "b.rs", "src/c.rs"], Some(&threads), None)
        .expect("refresh the paths");

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let mut read = threads.0.into_inner().expect("lock");
    read.sort_by(|(left, _), (right, _)| left.cmp(right));
    assert_eq!(
        read.iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["a.rs", "b.rs", "src/c.rs"]
    );
    assert!(
        read.iter().all(|(_, thread)| pool.contains(thread)),
        "every named file is read on the index's own pool {pool:?}: {read:?}"
    );
}

#[test]
fn the_parts_of_named_paths_are_what_a_refresh_of_them_walks() {
    let dir = tree(&[
        (".gitignore", "target/\n"),
        ("sub/.gitignore", ""),
        ("sub/a.rs", ""),
        ("b.rs", ""),
    ]);
    fs::create_dir(dir.path().join(".git")).expect("make a repository");
    let mut index = build(dir.path(), &pruning_node_modules(), None);
    let named = [
        "target/debug/x.o",
        "target/debug/deps/y.o",
        "sub/.gitignore",
        "sub/a.rs",
        "node_modules/m/index.js",
        ".git/HEAD",
        "b.rs",
        "c.rs",
    ];

    let parts = index.parts_of(named).expect("work out the parts");
    let walked = refresh_paths(&mut index, named);

    assert_eq!(parts, ["b.rs", "c.rs", "sub", "target"]);
    assert_eq!(walked, parts);
}

#[test]
fn a_changed_gitignore_walks_again_the_directory_it_governs() {
    let dir = tree(&[
        ("sub/.gitignore", "*.log\n"),
        ("sub/a.log", ""),
        ("sub/b.txt", ""),
        ("c.txt", ""),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "sub/.gitignore", "*.txt\n");
    let walked = refresh_paths(&mut index, ["sub/.gitignore"]);

    assert_eq!(walked, ["sub"]);
    assert!(index.entry("sub/a.log").is_some());
    assert!(index.entry("sub/b.txt").is_none());
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn a_part_walked_again_keeps_the_rules_of_the_directories_above_it() {
    let dir = tree(&[
        (".gitignore", "*.log\n"),
        ("sub/.gitignore", "*.tmp\n"),
        ("sub/deep/a.rs", ""),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    for path in ["sub/deep/b.log", "sub/deep/c.tmp", "sub/deep/d.rs", "e.log"] {
        write(dir.path(), path, "");
    }
    refresh_paths(
        &mut index,
        ["sub/deep/b.log", "sub/deep/c.tmp", "sub/deep/d.rs", "e.log"],
    );

    assert_eq!(
        paths(&index),
        [
            ".gitignore",
            "sub",
            "sub/.gitignore",
            "sub/deep",
            "sub/deep/a.rs",
            "sub/deep/d.rs"
        ]
    );
}

#[test]
fn parts_in_sibling_directories_each_keep_their_own_rules() {
    let dir = tree(&[
        ("a/deep/keep.rs", ""),
        ("b/.gitignore", "*.log\n"),
        ("b/deep/keep.rs", ""),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    for path in ["a/deep/x.log", "b/deep/y.log", "b/z.log"] {
        write(dir.path(), path, "");
    }
    refresh_paths(&mut index, ["a/deep/x.log", "b/deep/y.log", "b/z.log"]);

    assert!(index.entry("a/deep/x.log").is_some());
    assert!(index.entry("b/deep/y.log").is_none());
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn a_gitignore_above_the_parts_that_will_not_read_is_reported_once_a_call() {
    let dir = tree(&[(".gitignore", "*.log\n"), ("sub/deep/a.rs", "")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    fs::create_dir(dir.path().join("sub/.gitignore")).expect("make a directory of the name");
    for path in ["sub/deep/b.rs", "sub/deep/c.log", "sub/d.rs"] {
        write(dir.path(), path, "");
    }
    let outcome = index
        .refresh_paths(["sub/deep/b.rs", "sub/deep/c.log", "sub/d.rs"], None, None)
        .expect("refresh the paths");

    assert!(
        matches!(
            outcome.errors.as_slice(),
            [BuildError::Ignore { native_path, .. }] if native_path.ends_with("sub/.gitignore")
        ),
        "{:?}",
        outcome.errors
    );
    assert!(index.entry("sub/deep/b.rs").is_some());
    assert!(index.entry("sub/d.rs").is_some());
    assert!(index.entry("sub/deep/c.log").is_none());
}

#[test]
fn a_nested_repository_walked_again_drops_the_rules_from_above_it() {
    let dir = tree(&[(".gitignore", "*.log\n"), ("nested/a.rs", "")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    fs::create_dir(dir.path().join("nested/.git")).expect("make a repository");
    write(dir.path(), "nested/b.log", "");
    let walked = refresh_paths(&mut index, ["nested/.git"]);

    assert_eq!(walked, ["nested"]);
    assert!(index.entry("nested/b.log").is_some());
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn a_part_inside_a_nested_repository_is_walked_without_the_rules_from_above_it() {
    let dir = tree(&[(".gitignore", "*.log\n"), ("nested/sub/a.rs", "")]);
    fs::create_dir(dir.path().join("nested/.git")).expect("make a repository");
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "nested/sub/b.log", "");
    let walked = refresh_paths(&mut index, ["nested/sub/b.log"]);

    assert_eq!(walked, ["nested/sub/b.log"]);
    assert!(index.entry("nested/sub/b.log").is_some());
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn a_gitignore_is_an_ordinary_file_where_no_rules_apply() {
    let dir = tree(&[(".gitignore", "*.log\n"), ("a.log", "")]);
    let mut options = BuildOptions::default();
    options.ignore_sources = IgnoreSources::None;
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), ".gitignore", "*.rs\n");
    let walked = refresh_paths(&mut index, [".gitignore", ".git/HEAD", "sub/.git"]);

    assert_eq!(walked, [".gitignore"]);
    assert_as_built(&index, &options);
}

#[test]
fn a_path_that_widens_to_the_root_refreshes_the_whole_tree() {
    let dir = tree(&[(".gitignore", "*.log\n"), ("a.log", ""), ("b.rs", "")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), ".gitignore", "*.rs\n");
    let walked = refresh_paths(&mut index, ["b.rs", ".gitignore"]);

    assert_eq!(walked, [""]);
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn a_path_inside_git_or_naming_the_root_repository_changes_nothing() {
    let dir = tree(&[("a.rs", "")]);
    fs::create_dir(dir.path().join(".git")).expect("make a repository");
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "b.rs", "");
    let walked = refresh_paths(&mut index, [".git/HEAD", ".git", "sub/.git/index"]);

    assert_eq!(walked, Vec::<String>::new());
    assert_eq!(paths(&index), ["a.rs"], "nothing was walked");
}

fn pruning_node_modules() -> BuildOptions {
    let mut options = BuildOptions::default();
    options.pruned_names =
        PrunedNames::builtin().union(&PrunedNames::new(["node_modules"]).expect("a name"));
    options
}

#[test]
fn a_refresh_prunes_the_names_the_build_was_given() {
    let dir = tree(&[("a.rs", "")]);
    let options = pruning_node_modules();
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), ".idea/workspace.xml", "<project/>");
    write(dir.path(), "web/node_modules/left-pad/index.js", "");
    write(dir.path(), "web/app.js", "");
    index.refresh(None, None).expect("refresh");

    assert_eq!(paths(&index), ["a.rs", "web", "web/app.js"]);
    assert_as_built(&index, &options);
}

#[test]
fn a_path_through_a_pruned_name_changes_nothing() {
    let dir = tree(&[("a.rs", ""), ("web/app.js", "")]);
    let options = pruning_node_modules();
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), ".idea/workspace.xml", "<project/>");
    write(dir.path(), ".vscode/.gitignore", "*\n");
    write(dir.path(), "web/node_modules/left-pad/index.js", "");
    write(dir.path(), "web/.DS_Store", "\0");
    let walked = refresh_paths(
        &mut index,
        [
            ".idea/workspace.xml",
            ".idea",
            ".vscode/.gitignore",
            "web/node_modules/left-pad/index.js",
            "web/.DS_Store",
        ],
    );

    assert_eq!(walked, Vec::<String>::new());
    assert_eq!(paths(&index), ["a.rs", "web", "web/app.js"]);
}

#[test]
fn a_changed_pruned_gitignore_walks_again_the_directory_its_rules_govern() {
    let dir = tree(&[
        ("sub/.gitignore", "*.log\n"),
        ("sub/a.log", ""),
        ("sub/b.rs", ""),
    ]);
    let mut options = BuildOptions::default();
    options.pruned_names =
        PrunedNames::builtin().union(&PrunedNames::new([".gitignore"]).expect("a name"));
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), "sub/.gitignore", "*.rs\n");
    let walked = refresh_paths(&mut index, ["sub/.gitignore"]);

    assert_eq!(walked, ["sub"], "the file is pruned, and its rules are not");
    assert_eq!(paths(&index), ["sub", "sub/a.log"]);
    assert_as_built(&index, &options);
}

#[test]
fn a_path_that_may_leave_the_tree_is_refused_before_anything_is_walked() {
    let dir = tree(&[("a.rs", "")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "b.rs", "");
    let refreshed = index.refresh_paths(["b.rs", "sub/../../outside"], None, None);

    assert!(matches!(
        refreshed,
        Err(BuildError::Misspelled { path, fault: PathFault::ParentName }) if path == "sub/../../outside"
    ));
    assert_eq!(paths(&index), ["a.rs"]);
}

#[test]
fn a_path_not_spelled_from_the_root_with_slashes_is_refused_by_its_fault_before_anything_is_walked()
{
    let dir = tree(&[("a.rs", ""), ("src/a.rs", "")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "b.rs", "");
    for (path, fault) in [
        ("/b.rs", PathFault::Absolute),
        ("\\repo\\b.rs", PathFault::Absolute),
        ("C:\\repo\\b.rs", PathFault::Drive('C')),
        ("C:b.rs", PathFault::Drive('C')),
        ("src\\a.rs", PathFault::Backslash),
    ] {
        let refreshed = index.refresh_paths(["b.rs", path], None, None);
        let parts = index.parts_of(["b.rs", path]);

        for refused in [refreshed.map(|_| ()), parts.map(|_| ())] {
            assert!(
                matches!(&refused, Err(BuildError::Misspelled { path: given, fault: found }) if given == path && *found == fault),
                "{path:?}: {refused:?}"
            );
        }
    }
    assert_eq!(paths(&index), ["a.rs", "src", "src/a.rs"]);

    let walked = refresh_paths(&mut index, ["src/a.rs", "./b.rs"]);
    assert_eq!(walked, ["b.rs", "src/a.rs"]);
}

#[cfg(any(target_os = "macos", windows))]
fn folding_tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tree(spec);
    let (first, _) = spec.first().expect("a path");
    assert!(
        dir.path().join(first.to_lowercase()).exists(),
        "the temporary directory sits on a volume that folds case"
    );
    dir
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn a_path_spelled_in_another_case_refreshes_the_entry_the_disk_holds() {
    let dir = folding_tree(&[("Dir/f.rs", "a"), ("Dir/Sub/g.rs", "b")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    let entries = index.len();

    write(dir.path(), "Dir/f.rs", "changed");
    let parts = index.parts_of(["dir/f.rs"]).expect("work out the parts");
    let walked = refresh_paths(&mut index, ["dir/f.rs"]);

    assert_eq!(parts, ["Dir/f.rs"]);
    assert_eq!(walked, parts);
    assert_eq!(index.len(), entries);
    assert_as_built(&index, &BuildOptions::default());
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn every_name_of_a_path_spelled_in_another_case_takes_the_disk_s_spelling() {
    let dir = folding_tree(&[("Dir/Sub/g.rs", "a"), ("Dir/f.rs", "b")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "Dir/Sub/new.rs", "c");
    let walked = refresh_paths(&mut index, ["DIR/SUB/G.RS", "dir/sub/new.rs", "DIR/F.RS"]);

    assert_eq!(walked, ["Dir/Sub/g.rs", "Dir/Sub/new.rs", "Dir/f.rs"]);
    assert_as_built(&index, &BuildOptions::default());
}

#[cfg(target_os = "macos")]
#[test]
fn a_path_in_either_unicode_normalization_form_refreshes_the_entry_the_disk_holds() {
    let dir = folding_tree(&[("Dir/caf\u{e9}.rs", "a")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    let held = paths(&index).pop().expect("the file below its directory");

    let walked = refresh_paths(&mut index, ["Dir/caf\u{e9}.rs", "Dir/cafe\u{301}.rs"]);

    assert_eq!(walked, [held]);
    assert_as_built(&index, &BuildOptions::default());
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn a_rename_that_changes_only_case_leaves_the_new_spelling_alone() {
    let dir = folding_tree(&[("Dir/g.rs", "a"), ("Dir/Sub/h.rs", "b")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    fs::rename(dir.path().join("Dir/g.rs"), dir.path().join("Dir/G.rs")).expect("rename");
    fs::rename(dir.path().join("Dir/Sub"), dir.path().join("Dir/sub")).expect("rename");
    refresh_paths(&mut index, ["Dir/g.rs", "Dir/G.rs", "Dir/Sub", "Dir/sub"]);

    assert_eq!(
        paths(&index),
        ["Dir", "Dir/G.rs", "Dir/sub", "Dir/sub/h.rs"]
    );
    assert_as_built(&index, &BuildOptions::default());
}

#[cfg(target_os = "linux")]
#[test]
fn paths_that_differ_only_in_case_stay_apart_where_the_disk_holds_both() {
    let dir = tree(&[("Dir/f.rs", "a"), ("dir/f.rs", "b")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "dir/f.rs", "changed");
    write(dir.path(), "DIR/f.rs", "c");
    let walked = refresh_paths(&mut index, ["dir/f.rs", "DIR/f.rs", "Dir/F.rs"]);

    assert_eq!(walked, ["DIR", "Dir/F.rs", "dir/f.rs"]);
    assert_eq!(
        paths(&index),
        ["DIR", "DIR/f.rs", "Dir", "Dir/f.rs", "dir", "dir/f.rs"]
    );
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn a_path_the_rules_ignore_stays_out_of_the_index() {
    let dir = tree(&[(".gitignore", "build/\n"), ("a.rs", "")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "build/out.rs", "");
    let walked = refresh_paths(&mut index, ["build/out.rs"]);

    assert_eq!(walked, ["build"]);
    assert_eq!(paths(&index), [".gitignore", "a.rs"]);
}

#[test]
fn a_file_below_an_ignored_directory_stays_out_even_where_a_rule_below_takes_it_back() {
    let dir = tree(&[
        (".gitignore", "hidden/\n"),
        ("hidden/.gitignore", "!file.rs\n"),
        ("a.rs", ""),
    ]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "hidden/file.rs", "");
    let walked = refresh_paths(&mut index, ["hidden/file.rs"]);

    assert_eq!(walked, ["hidden"]);
    assert_eq!(paths(&index), [".gitignore", "a.rs"]);
}

#[cfg(unix)]
#[test]
fn a_symlink_pointed_elsewhere_shows_its_new_target() {
    let dir = tree(&[("a.rs", ""), ("b.rs", "")]);
    std::os::unix::fs::symlink("a.rs", dir.path().join("link")).expect("link");
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    fs::remove_file(dir.path().join("link")).expect("unlink");
    std::os::unix::fs::symlink("b.rs", dir.path().join("link")).expect("link again");
    refresh_paths(&mut index, ["link"]);

    assert_eq!(
        index.entry("link").and_then(|entry| entry.target()),
        Some("b.rs")
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_below_a_directory_walked_again_keeps_its_target() {
    let dir = tree(&[("dir/a.rs", ""), ("dir/sub/b.rs", "")]);
    std::os::unix::fs::symlink("../a.rs", dir.path().join("dir/sub/kept")).expect("link");
    let mut index = build(dir.path(), &BuildOptions::default(), None);

    write(dir.path(), "dir/new.rs", "");
    for at in 0..100 {
        std::os::unix::fs::symlink(
            format!("to{at:03}"),
            dir.path().join(format!("dir/sub/l{at:03}")),
        )
        .expect("link");
    }
    let walked = refresh_paths(&mut index, ["dir"]);

    assert_eq!(walked, ["dir"]);
    let target = |path: &str| {
        index
            .entry(path)
            .and_then(|entry| entry.target().map(str::to_owned))
    };
    assert_eq!(target("dir/sub/kept").as_deref(), Some("../a.rs"));
    let added: Vec<Option<String>> = (0..100)
        .map(|at| target(&format!("dir/sub/l{at:03}")))
        .collect();
    let expected: Vec<Option<String>> = (0..100).map(|at| Some(format!("to{at:03}"))).collect();
    assert_eq!(
        added, expected,
        "a directory of 64 entries or more is decided on several threads, in whatever order they read the targets"
    );
    assert_as_built(&index, &BuildOptions::default());
}

#[test]
fn refreshing_paths_reads_only_what_they_name_and_retires_what_it_replaced() {
    let dir = tree(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
    let ids = Ids::default();
    let mut index = build(dir.path(), &BuildOptions::default(), Some(&ids));
    let built = ids.take();

    write(dir.path(), "a.rs", "changed");
    fs::remove_file(dir.path().join("c.rs")).expect("remove");
    let outcome = index
        .refresh_paths(["a.rs", "c.rs"], Some(&ids), None)
        .expect("refresh the paths");
    let refreshed = ids.take();

    assert_eq!(refreshed.keys().collect::<Vec<_>>(), ["a.rs"]);
    assert!(built.values().all(|old| *old < refreshed["a.rs"]));
    let mut gone = vec![built["a.rs"], built["c.rs"]];
    gone.sort();
    assert_eq!(outcome.retired().collect::<Vec<_>>(), gone);
    for (path, id) in [("a.rs", refreshed["a.rs"]), ("b.rs", built["b.rs"])] {
        let position = index.content_position(id).expect("the id names its file");
        assert_eq!(index.paths().get(position), path);
    }
}

#[test]
fn a_stamped_refresh_of_a_directory_reads_again_only_the_files_that_changed() {
    let dir = settled_tree(&[("src/a.rs", "a"), ("src/b.rs", "b"), ("c.rs", "c")]);
    let ids = Ids::default();
    let mut index = build(dir.path(), &stamped(), Some(&ids));
    let built = ids.take();

    write(dir.path(), "src/b.rs", "changed");
    let outcome = index
        .refresh_paths(["src"], Some(&ids), None)
        .expect("refresh the paths");

    assert_eq!(ids.take().keys().collect::<Vec<_>>(), ["src/b.rs"]);
    assert_eq!(outcome.retired().collect::<Vec<_>>(), [built["src/b.rs"]]);
    for path in ["src/a.rs", "c.rs"] {
        let position = index
            .content_position(built[path])
            .expect("an unchanged file keeps its id");
        assert_eq!(index.paths().get(position), path);
    }
    assert_eq!(
        index.size("src/a.rs"),
        Some(1),
        "and what was learned of it"
    );
}

#[test]
fn refreshed_paths_are_searched_as_they_are_now() {
    let dir = settled_tree(&[("a.txt", "kept\n"), ("sub/b.txt", "before\n")]);
    let mut options = stamped();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);

    write(dir.path(), "sub/b.txt", "after\n");
    write(dir.path(), "sub/c.txt", "added\n");
    refresh_paths(&mut index, ["sub"]);

    assert_eq!(search(&index, "kept"), (1, 2));
    assert_eq!(search(&index, "after"), (1, 2));
    assert_eq!(search(&index, "added"), (1, 2));
    assert_eq!(search(&index, "before"), (0, 3));
    let built = build(dir.path(), &options, None);
    assert_eq!(
        index.content_index_stats().map(|stats| stats.files()),
        built.content_index_stats().map(|stats| stats.files()),
        "each file the index answers for counts once"
    );
}

#[cfg(unix)]
#[test]
fn a_refreshed_file_that_will_not_read_is_reported_and_never_ruled_out() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tree(&[("a.txt", "kept\n"), ("b.txt", "other\n")]);
    let mut options = BuildOptions::default();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);
    let locked = dir.path().join("b.txt");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }

    let outcome = index
        .refresh_paths(["b.txt"], None, None)
        .expect("refresh the paths");

    assert!(
        matches!(outcome.errors.as_slice(), [BuildError::Read { native_path, .. }] if native_path.ends_with("b.txt")),
        "{:?}",
        outcome.errors
    );
    assert_eq!(
        search(&index, "kept"),
        (1, 0),
        "a file the index holds no list for may hold anything"
    );
    assert_eq!(
        index.content_index_stats().map(|stats| stats.files()),
        Some(1)
    );
}

#[test]
fn many_refreshes_of_single_files_keep_searches_and_counts_a_build_would_give() {
    let names: Vec<String> = (0..100).map(|at| format!("file{at:02}.txt")).collect();
    let spec: Vec<(&str, String)> = names
        .iter()
        .map(|name| (name.as_str(), format!("kept {name}\n")))
        .collect();
    let spec: Vec<(&str, &str)> = spec
        .iter()
        .map(|(name, text)| (*name, text.as_str()))
        .collect();
    let dir = tree(&spec);
    let mut options = BuildOptions::default();
    options.content_index = Some(ContentIndexStorage::Memory);
    let mut index = build(dir.path(), &options, None);

    for name in &names[..10] {
        write(dir.path(), name, format!("changed {name}\n"));
        refresh_paths(&mut index, [name.as_str()]);
    }

    assert_eq!(search(&index, "changed"), (10, 90));
    assert_eq!(search(&index, "kept"), (90, 10));
    assert_eq!(search(&index, "file09"), (1, 99));
    let built = build(dir.path(), &options, None);
    assert_eq!(
        index.content_index_stats().map(|stats| stats.files()),
        built.content_index_stats().map(|stats| stats.files())
    );
}

#[test]
fn a_cancelled_refresh_of_paths_leaves_the_index_as_it_was() {
    let dir = tree(&[("a.rs", "a")]);
    let mut index = build(dir.path(), &BuildOptions::default(), None);
    write(dir.path(), "b.rs", "b");

    let cancel = CancelToken::new();
    cancel.cancel();
    let refreshed = index.refresh_paths(["b.rs"], None, Some(&cancel));

    assert!(matches!(refreshed, Err(BuildError::Cancelled)));
    assert_eq!(paths(&index), ["a.rs"]);
}

#[test]
fn refreshing_paths_that_change_nothing_still_retires_what_a_stopped_refresh_handed_out() {
    let dir = tree(&[("a.rs", "a"), ("b.rs", "b")]);
    let mut index = build(dir.path(), &BuildOptions::default(), Some(&Ids::default()));
    let cancelling = CancelAtFirstVisit {
        ids: Ids::default(),
        cancel: CancelToken::new(),
    };
    let cancelled = index.refresh(Some(&cancelling), Some(&cancelling.cancel));
    assert!(matches!(cancelled, Err(BuildError::Cancelled)));
    let stopped = cancelling.ids.take();

    let outcome = index
        .refresh_paths(["absent.rs"], None, None)
        .expect("refresh the paths");

    assert_eq!(paths(&index), ["a.rs", "b.rs"]);
    let retired: Vec<ContentId> = outcome.retired().collect();
    assert!(!stopped.is_empty() && stopped.values().all(|id| retired.contains(id)));
    let again = index
        .refresh_paths(["absent.rs"], None, None)
        .expect("refresh the paths");
    assert_eq!(again.retired().count(), 0, "each id retires once");
}

#[test]
fn refreshing_paths_never_hands_out_again_an_id_a_stopped_refresh_handed_its_visitor() {
    let names: Vec<String> = (0..16).map(|at| format!("dir/file{at:02}.rs")).collect();
    let spec: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "x")).collect();
    let dir = tree(&spec);
    let mut index = build(dir.path(), &BuildOptions::default(), Some(&Ids::default()));

    let cancelling = CancelAtFirstVisit {
        ids: Ids::default(),
        cancel: CancelToken::new(),
    };
    let cancelled = index.refresh_paths(["dir"], Some(&cancelling), Some(&cancelling.cancel));
    assert!(matches!(cancelled, Err(BuildError::Cancelled)));
    let panicking = PanicAtFirstVisit(Ids::default());
    let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
        index.refresh_paths(["dir"], Some(&panicking), None)
    }));
    assert!(unwound.is_err(), "the visitor's panic reaches the caller");

    let mut stopped = cancelling.ids.take();
    stopped.extend(
        panicking
            .0
            .take()
            .into_iter()
            .map(|(path, id)| (format!("{path}+"), id)),
    );
    let ids = Ids::default();
    let outcome = index
        .refresh_paths(["dir"], Some(&ids), None)
        .expect("refresh the paths");

    assert_eq!(stopped.len(), 2, "each stopped refresh handed out an id");
    assert!(
        ids.take()
            .values()
            .all(|id| !stopped.values().any(|then| then == id)),
        "no id names two files"
    );
    let retired: Vec<ContentId> = outcome.retired().collect();
    assert!(
        stopped.values().all(|id| retired.contains(id)),
        "the ids the stopped refreshes handed out are retired"
    );
}

#[test]
fn named_files_read_on_threads_never_hand_out_again_an_id_a_stopped_refresh_handed_its_visitor() {
    let names: Vec<String> = (0..16).map(|at| format!("file{at:02}.rs")).collect();
    let spec: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "x")).collect();
    let dir = tree(&spec);
    let mut options = BuildOptions::default();
    options.threads = NonZeroUsize::new(4);
    let mut index = build(dir.path(), &options, Some(&Ids::default()));
    let every_file = || names.iter().map(String::as_str);

    let cancelling = CancelAtFirstVisit {
        ids: Ids::default(),
        cancel: CancelToken::new(),
    };
    let cancelled = index.refresh_paths(every_file(), Some(&cancelling), Some(&cancelling.cancel));
    assert!(matches!(cancelled, Err(BuildError::Cancelled)));
    let panicking = PanicAtFirstVisit(Ids::default());
    let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
        index.refresh_paths(every_file(), Some(&panicking), None)
    }));
    assert!(unwound.is_err(), "the visitor's panic reaches the caller");

    let cancelled_ids = cancelling.ids.take();
    let panicked_ids = panicking.0.take();
    let ids = Ids::default();
    let outcome = index
        .refresh_paths(every_file(), Some(&ids), None)
        .expect("refresh the paths");

    assert!(
        !cancelled_ids.is_empty() && !panicked_ids.is_empty(),
        "each stopped refresh handed out an id"
    );
    let stopped: Vec<&ContentId> = cancelled_ids
        .values()
        .chain(panicked_ids.values())
        .collect();
    assert!(
        ids.take().values().all(|id| !stopped.contains(&id)),
        "no id names two files"
    );
    let retired: Vec<ContentId> = outcome.retired().collect();
    assert!(
        stopped.iter().all(|id| retired.contains(id)),
        "the ids the stopped refreshes handed out are retired"
    );
}
