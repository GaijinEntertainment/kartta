//! The shared cache contract: what builds that name one directory share, and what a failure there costs.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use kartta_cache::Store;
use kartta_fs::{
    BuildError, BuildOptions, BuildOutcome, Case, Content, ContentHash, ContentIndexStorage,
    ContentVisitor, GrepOptions, Index, Namespace, Patterns,
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

fn cached(cache: &Path) -> BuildOptions {
    let mut options = BuildOptions::default();
    options.content_index = Some(ContentIndexStorage::Memory);
    options.content_cache = Some(cache.to_path_buf());
    options
}

fn build(root: &Path, options: &BuildOptions) -> BuildOutcome {
    let outcome = Index::build(root, options).expect("build the index");
    assert_eq!(outcome.errors().len(), 0, "{:?}", outcome.errors());
    outcome
}

fn packs_and_entries(cache: &Path) -> (u64, u64) {
    let stats = Store::open(cache).expect("open the store").stats();
    (stats.packs, stats.entries)
}

fn found(index: &Index, pattern: &str) -> Vec<String> {
    let patterns = Patterns::new([pattern], Case::Sensitive).expect("compile");
    let matches = index.grep(&patterns, &GrepOptions::default());
    let mut paths = index.paths();

    matches
        .files()
        .iter()
        .map(|file| paths.get(file.position()).to_owned())
        .collect()
}

#[test]
fn a_build_adds_only_the_contents_the_store_lacks_wherever_they_sit() {
    let cache = tempfile::tempdir().expect("cache dir");
    let first = tree(&[
        ("src/run.rs", b"pub fn run() {}\n"),
        ("notes.md", b"run it\n"),
    ]);
    let second = tree(&[
        ("lib/run.rs", b"pub fn run() {}\n"),
        ("docs/notes.md", b"run it\n"),
        ("new.txt", b"gamma\n"),
    ]);

    build(first.path(), &cached(cache.path()));
    let index = build(second.path(), &cached(cache.path())).into_index();

    assert_eq!(
        packs_and_entries(cache.path()),
        (2, 3),
        "the second pack holds the one content the first lacks"
    );
    assert_eq!(found(&index, "pub fn run"), ["lib/run.rs"]);
    assert_eq!(found(&index, "gamma"), ["new.txt"]);
}

#[test]
fn a_file_past_one_read_chunk_is_shared_as_a_small_one_is() {
    let cache = tempfile::tempdir().expect("cache dir");
    let large = "needle in a haystack\n".repeat(10_000);
    let first = tree(&[("large.txt", large.as_bytes())]);
    let second = tree(&[
        ("moved/large.txt", large.as_bytes()),
        ("new.txt", b"gamma\n"),
    ]);

    build(first.path(), &cached(cache.path()));
    let index = build(second.path(), &cached(cache.path())).into_index();

    assert!(large.len() > 64 * 1024, "{} bytes", large.len());
    assert_eq!(
        packs_and_entries(cache.path()),
        (2, 2),
        "the second pack holds only the small file"
    );
    assert_eq!(found(&index, "needle"), ["moved/large.txt"]);
    assert_eq!(found(&index, "absent"), Vec::<String>::new());
}

#[test]
fn the_build_that_publishes_a_ninth_pack_merges_the_store() {
    let cache = tempfile::tempdir().expect("cache dir");
    let builds = 9;

    for at in 0..builds {
        let content = format!("content of build {at}\n");
        let root = tree(&[("file.txt", content.as_bytes())]);
        build(root.path(), &cached(cache.path()));
    }

    assert_eq!(packs_and_entries(cache.path()), (1, builds));
}

#[test]
fn a_build_that_publishes_nothing_still_merges_a_store_past_eight_packs() {
    let cache = tempfile::tempdir().expect("cache dir");
    let packs = 9;
    let namespace = Namespace::new("elsewhere", b"1");
    for at in 0..packs {
        let writer = Store::open(cache.path()).expect("open").writer();
        let content = format!("published elsewhere {at}");
        writer
            .insert(
                &namespace,
                &ContentHash::of(content.as_bytes()),
                content.as_bytes(),
            )
            .expect("insert");
        writer.publish().expect("publish");
    }
    let empty = tree(&[]);

    build(empty.path(), &cached(cache.path()));

    assert_eq!(packs_and_entries(cache.path()), (1, packs));
}

#[test]
fn a_refresh_shares_the_files_it_reads_through_the_same_store() {
    let cache = tempfile::tempdir().expect("cache dir");
    let root = tree(&[("a.txt", b"alpha\n"), ("b.txt", b"beta\n")]);
    let mut options = cached(cache.path());
    options.stamps = true;
    let mut index = build(root.path(), &options).into_index();
    fs::write(root.path().join("b.txt"), "gamma, longer\n").expect("rewrite");

    let refreshed = index.refresh(None, None).expect("refresh");

    assert_eq!(refreshed.errors().len(), 0, "{:?}", refreshed.errors());
    assert_eq!(packs_and_entries(cache.path()), (2, 3));
    assert_eq!(found(&index, "gamma"), ["b.txt"]);
}

#[test]
fn a_cache_that_cannot_be_opened_costs_only_the_sharing() {
    let root = tree(&[("a.txt", b"alpha\n"), ("b.txt", b"beta\n")]);
    let not_a_directory = tempfile::NamedTempFile::new().expect("temp file");
    let options = cached(not_a_directory.path());

    let outcome = Index::build(root.path(), &options).expect("build the index");

    assert!(
        matches!(outcome.errors(), [BuildError::Cache { native_path, .. }] if native_path == not_a_directory.path()),
        "{:?}",
        outcome.errors()
    );
    let index = outcome.into_index();
    assert_eq!(
        index.content_index_stats().map(|stats| stats.files()),
        Some(2)
    );
    assert_eq!(found(&index, "alpha"), ["a.txt"]);
}

#[derive(Debug, PartialEq)]
struct Visited {
    path: String,
    cached: Option<Vec<u8>>,
    hash: Option<ContentHash>,
}

struct Lengths {
    namespace: Namespace,
    visited: Mutex<Vec<Visited>>,
}

impl Lengths {
    fn new() -> Self {
        Self {
            namespace: Namespace::new("lengths", b"byte length, little-endian usize, 1"),
            visited: Mutex::new(Vec::new()),
        }
    }

    fn visited(self) -> Vec<Visited> {
        let mut visited = self.visited.into_inner().expect("lock");
        visited.sort_by(|left, right| left.path.cmp(&right.path));
        visited
    }
}

impl ContentVisitor for Lengths {
    fn wants(&self, _path: &str) -> bool {
        true
    }

    fn visit(&self, content: Content<'_>) {
        let cached = content.cached(&self.namespace).map(<[u8]>::to_vec);
        if cached.is_none() {
            content.cache(&self.namespace, &content.bytes().len().to_le_bytes());
        }
        self.visited.lock().expect("lock").push(Visited {
            path: content.path().to_owned(),
            cached,
            hash: content.hash(),
        });
    }
}

fn build_visited(root: &Path, options: &BuildOptions, visitor: &Lengths) {
    let outcome = Index::build_with(root, options, Some(visitor), None).expect("build the index");
    assert_eq!(outcome.errors().len(), 0, "{:?}", outcome.errors());
}

#[test]
fn a_visitor_finds_what_a_build_of_another_tree_cached_for_the_same_content() {
    let cache = tempfile::tempdir().expect("cache dir");
    let content = b"alpha\n";
    let first = tree(&[("a.txt", content)]);
    let moved = tree(&[("moved/a.txt", content)]);
    let before = Lengths::new();
    let after = Lengths::new();

    build_visited(first.path(), &cached(cache.path()), &before);
    build_visited(moved.path(), &cached(cache.path()), &after);

    let hash = Some(ContentHash::of(content));
    assert_eq!(
        before.visited(),
        [Visited {
            path: "a.txt".to_owned(),
            cached: None,
            hash,
        }]
    );
    assert_eq!(
        after.visited(),
        [Visited {
            path: "moved/a.txt".to_owned(),
            cached: Some(content.len().to_le_bytes().to_vec()),
            hash,
        }]
    );
    assert_eq!(
        packs_and_entries(cache.path()),
        (1, 2),
        "the trigrams and the visitor's entry share the first build's pack"
    );
}

#[test]
fn a_visitor_without_a_content_index_still_shares_through_the_cache() {
    let cache = tempfile::tempdir().expect("cache dir");
    let root = tree(&[("a.txt", b"alpha\n"), ("b.txt", b"beta\n")]);
    let mut options = BuildOptions::default();
    options.content_cache = Some(cache.path().to_path_buf());

    build_visited(root.path(), &options, &Lengths::new());

    assert_eq!(packs_and_entries(cache.path()), (1, 2));
}

#[test]
fn a_build_without_a_cache_hands_the_visitor_no_hash_and_no_entry() {
    let root = tree(&[("a.txt", b"alpha\n")]);
    let lengths = Lengths::new();

    build_visited(root.path(), &BuildOptions::default(), &lengths);

    assert_eq!(
        lengths.visited(),
        [Visited {
            path: "a.txt".to_owned(),
            cached: None,
            hash: None,
        }]
    );
}
