//! The shared cache contract: what builds that name one directory share, and what a failure there costs.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::Path;

use kartta_cache::{ContentHash, Namespace, Store};
use kartta_fs::{
    BuildError, BuildOptions, BuildOutcome, Case, ContentIndexStorage, GrepOptions, Index, Patterns,
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
