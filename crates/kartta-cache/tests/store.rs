//! The store's contract: what a published pack answers, what it keeps apart, what it refuses to serve, and what other
//! processes can do to the directory while a store reads it.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::{Path, PathBuf};

use kartta_cache::{ContentHash, ContentHasher, Namespace, PackWriter, Store};
use tempfile::TempDir;

fn trigrams() -> Namespace {
    Namespace::new("trigrams", b"rules 1")
}

fn packs(dir: &Path) -> Vec<PathBuf> {
    let mut packs: Vec<PathBuf> = fs::read_dir(dir)
        .expect("list the store")
        .map(|entry| entry.expect("read an entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "pack")
        })
        .collect();
    packs.sort();
    packs
}

fn files(dir: &Path) -> usize {
    fs::read_dir(dir).expect("list the store").count()
}

type Entry<'a> = (&'a Namespace, &'a [u8], &'a [u8]);

fn published(dir: &TempDir, entries: &[Entry<'_>]) -> Store {
    let writer = Store::open(dir.path()).expect("open").writer();
    for (namespace, content, payload) in entries {
        writer
            .insert(namespace, &ContentHash::of(content), payload)
            .expect("insert");
    }
    writer.publish().expect("publish");

    Store::open(dir.path()).expect("reopen")
}

#[test]
fn a_published_entry_is_found_by_every_store_opened_afterwards() {
    let dir = tempfile::tempdir().expect("temp dir");

    let store = published(
        &dir,
        &[
            (&trigrams(), b"fn main() {}", b"abc"),
            (&trigrams(), b"let x = 1;", b"def"),
        ],
    );

    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"fn main() {}")),
        Some(&b"abc"[..])
    );
    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"let x = 1;")),
        Some(&b"def"[..])
    );
    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"never cached")),
        None
    );
    let stats = store.stats();
    assert_eq!(
        (stats.packs, stats.entries, stats.hits, stats.misses),
        (1, 2, 2, 1)
    );
}

#[test]
fn a_store_keeps_the_packs_it_opened_with() {
    let dir = tempfile::tempdir().expect("temp dir");
    let before = Store::open(dir.path()).expect("open");

    let after = published(&dir, &[(&trigrams(), b"content", b"payload")]);

    assert_eq!(before.get(&trigrams(), &ContentHash::of(b"content")), None);
    assert_eq!(
        after.get(&trigrams(), &ContentHash::of(b"content")),
        Some(&b"payload"[..])
    );
}

#[test]
fn namespaces_keep_entries_for_the_same_content_apart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let cpp = Namespace::new("graph/cpp", b"grammar 7");
    let cpp_next = Namespace::new("graph/cpp", b"grammar 8");

    let store = published(
        &dir,
        &[
            (&trigrams(), b"same bytes", b"trigrams"),
            (&cpp, b"same bytes", b"graph"),
        ],
    );

    let content = ContentHash::of(b"same bytes");
    assert_eq!(store.get(&trigrams(), &content), Some(&b"trigrams"[..]));
    assert_eq!(store.get(&cpp, &content), Some(&b"graph"[..]));
    assert_eq!(
        store.get(&cpp_next, &content),
        None,
        "a new fingerprint is a new namespace, never served what older code computed"
    );
}

#[test]
fn a_namespace_s_kind_and_fingerprint_do_not_run_together() {
    assert_ne!(Namespace::new("ab", b"c"), Namespace::new("a", b"bc"));
    assert_eq!(Namespace::new("trigrams", b"rules 1"), trigrams());
}

#[test]
fn a_key_inserted_twice_keeps_its_first_payload() {
    let dir = tempfile::tempdir().expect("temp dir");

    let store = published(
        &dir,
        &[
            (&trigrams(), b"content", b"first"),
            (&trigrams(), b"content", b"second"),
        ],
    );

    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"content")),
        Some(&b"first"[..])
    );
    assert_eq!(store.stats().entries, 1);
}

#[test]
fn a_writer_that_holds_nothing_publishes_nothing_and_leaves_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path()).expect("open");

    let published = store.writer().publish().expect("publish");
    let writer = store.writer();
    writer
        .insert(&trigrams(), &ContentHash::of(b"content"), b"payload")
        .expect("insert");
    drop(writer);

    assert_eq!(published, None);
    assert_eq!(
        files(dir.path()),
        0,
        "neither writer left a temporary file behind"
    );
}

#[test]
fn identical_batches_publish_one_pack() {
    let dir = tempfile::tempdir().expect("temp dir");

    published(&dir, &[(&trigrams(), b"content", b"payload")]);
    let store = published(&dir, &[(&trigrams(), b"content", b"payload")]);

    assert_eq!(
        packs(dir.path()).len(),
        1,
        "the second publish found its pack already in place"
    );
    assert_eq!(files(dir.path()), 1);
    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"content")),
        Some(&b"payload"[..])
    );
}

#[test]
fn every_pack_answers_for_what_it_holds() {
    let dir = tempfile::tempdir().expect("temp dir");

    published(&dir, &[(&trigrams(), b"one", b"1")]);
    let store = published(&dir, &[(&trigrams(), b"two", b"2")]);

    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"one")),
        Some(&b"1"[..])
    );
    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"two")),
        Some(&b"2"[..])
    );
    assert_eq!((store.stats().packs, store.stats().entries), (2, 2));
}

#[test]
fn a_damaged_payload_is_a_miss_and_is_counted() {
    let dir = tempfile::tempdir().expect("temp dir");
    published(&dir, &[(&trigrams(), b"content", b"payload")]);
    let pack = &packs(dir.path())[0];
    let mut bytes = fs::read(pack).expect("read the pack");
    bytes[0] ^= 0xff;
    fs::remove_file(pack).expect("remove the pack");
    fs::write(pack, &bytes).expect("write the damaged pack");

    let store = Store::open(dir.path()).expect("open");

    assert_eq!(store.get(&trigrams(), &ContentHash::of(b"content")), None);
    assert_eq!((store.stats().damaged, store.stats().misses), (1, 0));
}

#[test]
fn a_file_that_is_not_a_pack_is_rejected_and_the_rest_still_answer() {
    let dir = tempfile::tempdir().expect("temp dir");
    published(&dir, &[(&trigrams(), b"content", b"payload")]);
    fs::write(
        dir.path().join("broken.pack"),
        b"not a pack at all, but long enough to hold a footer",
    )
    .expect("write a foreign file");
    fs::write(dir.path().join("empty.pack"), b"").expect("write an empty file");

    let store = Store::open(dir.path()).expect("open");

    assert_eq!(store.stats().rejected_packs, 2);
    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"content")),
        Some(&b"payload"[..])
    );
}

#[test]
fn a_pack_removed_while_a_store_maps_it_still_answers_that_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = published(&dir, &[(&trigrams(), b"content", b"payload")]);

    let removed = fs::remove_file(&packs(dir.path())[0]);

    assert!(
        removed.is_ok(),
        "another process can remove a pack a store has mapped: {removed:?}"
    );
    assert_eq!(
        store.get(&trigrams(), &ContentHash::of(b"content")),
        Some(&b"payload"[..])
    );
    assert_eq!(Store::open(dir.path()).expect("reopen").stats().packs, 0);
}

fn insert_contents(writer: &PackWriter, contents: &[String]) {
    for content in contents {
        writer
            .insert(
                &trigrams(),
                &ContentHash::of(content.as_bytes()),
                content.as_bytes(),
            )
            .expect("insert");
    }
}

#[test]
fn many_threads_share_one_writer() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path()).expect("open");
    let writer = store.writer();
    let contents: Vec<String> = (0..2000).map(|at| format!("file {at}")).collect();

    std::thread::scope(|scope| {
        for chunk in contents.chunks(250) {
            let writer = &writer;
            scope.spawn(move || insert_contents(writer, chunk));
        }
    });
    writer.publish().expect("publish");

    let store = Store::open(dir.path()).expect("reopen");
    for content in &contents {
        assert_eq!(
            store.get(&trigrams(), &ContentHash::of(content.as_bytes())),
            Some(content.as_bytes())
        );
    }
    assert_eq!(store.get(&trigrams(), &ContentHash::of(b"file 2000")), None);
}

#[test]
fn a_streamed_hash_is_the_hash_of_the_whole_content() {
    let content: Vec<u8> = (0..100_000_u32).flat_map(u32::to_le_bytes).collect();

    for split in [1, 64, 1024, 65_536, 99_999] {
        let mut hasher = ContentHasher::new();
        for chunk in content.chunks(split) {
            hasher.update(chunk);
        }

        assert_eq!(
            hasher.finish(),
            ContentHash::of(&content),
            "split into chunks of {split}"
        );
    }
}
