use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use kartta_cache::{ContentHash, DEFAULT_TTL, Merged, Namespace, PackWriter, Store};

use crate::build::BuildError;
use crate::trigram::{self, Trigrams};
use crate::{into_inner, lock};

#[derive(Debug)]
pub(crate) struct SharedCache {
    store: Store,
    writer: PackWriter,
    ttl: Duration,
    trigrams: Namespace,
    failure: Mutex<Option<io::Error>>,
}

impl SharedCache {
    pub(crate) fn open(dir: &Path, ttl: Duration) -> Result<Self, BuildError> {
        let store = Store::open(dir).map_err(|source| cache_error(dir, source))?;

        Ok(Self {
            writer: store.writer(),
            store,
            ttl: if ttl.is_zero() { DEFAULT_TTL } else { ttl },
            trigrams: trigram_namespace(),
            failure: Mutex::new(None),
        })
    }

    pub(crate) fn get(&self, namespace: &Namespace, content: &ContentHash) -> Option<&[u8]> {
        self.store.get(namespace, content)
    }

    pub(crate) fn insert(&self, namespace: &Namespace, content: &ContentHash, payload: &[u8]) {
        if let Err(error) = self.writer.insert(namespace, content, payload) {
            lock(&self.failure).get_or_insert(error);
        }
    }

    pub(crate) fn fill_trigrams(
        &self,
        content: &ContentHash,
        bytes: &[u8],
        extractor: &mut Trigrams,
    ) {
        if !self.take_trigrams(content, extractor) {
            extractor.feed(bytes);
            self.share_trigrams(content, extractor);
        }
    }

    pub(crate) fn take_trigrams(&self, content: &ContentHash, extractor: &mut Trigrams) -> bool {
        self.get(&self.trigrams, content)
            .is_some_and(|encoded| extractor.decode(encoded))
    }

    pub(crate) fn share_trigrams(&self, content: &ContentHash, extractor: &mut Trigrams) {
        let mut encoded = Vec::new();
        extractor.encode(&mut encoded);
        self.insert(&self.trigrams, content, &encoded);
    }

    pub(crate) fn publish(self) -> Option<BuildError> {
        let Self {
            store,
            writer,
            ttl,
            failure,
            ..
        } = self;
        let dir = store.dir().to_path_buf();
        drop(store);
        if let Some(source) = into_inner(failure) {
            return Some(cache_error(&dir, source));
        }

        writer
            .publish()
            .and_then(|_| Store::open(&dir)?.merge(ttl))
            .map_or_else(Some, |merged| unremoved(&merged))
            .map(|source| cache_error(&dir, source))
    }
}

fn unremoved(merged: &Merged) -> Option<io::Error> {
    let left: Vec<String> = [
        (merged.left_in_place, "merged pack"),
        (merged.orphans_left, "temporary file"),
        (merged.unreadable_left, "unmapped pack"),
    ]
    .into_iter()
    .filter(|&(count, _)| count > 0)
    .map(|(count, what)| format!("{count} {what}{}", if count == 1 { "" } else { "s" }))
    .collect();

    (!left.is_empty()).then(|| {
        io::Error::other(format!(
            "the merge could not remove {}, which stay in the directory",
            left.join(", ")
        ))
    })
}

fn trigram_namespace() -> Namespace {
    Namespace::new("trigrams", &trigram::fingerprint())
}

fn cache_error(dir: &Path, source: io::Error) -> BuildError {
    BuildError::Cache {
        native_path: dir.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildOptions, Case, ContentIndexStorage, GrepOptions, Index, Patterns};

    fn plant_trigrams(cache: &Path, content: &[u8], of_text: &[u8]) {
        let mut planted = Trigrams::new();
        planted.feed(of_text);
        let mut encoded = Vec::new();
        planted.encode(&mut encoded);

        let writer = Store::open(cache).expect("open the store").writer();
        writer
            .insert(&trigram_namespace(), &ContentHash::of(content), &encoded)
            .expect("insert");
        writer.publish().expect("publish");
    }

    #[test]
    fn the_index_takes_a_file_s_trigrams_from_the_entry_its_content_has() {
        let root = tempfile::tempdir().expect("tree dir");
        let cache = tempfile::tempdir().expect("cache dir");
        let content = b"needle\n";
        std::fs::write(root.path().join("file.txt"), content).expect("write");
        plant_trigrams(cache.path(), content, b"haystack");
        let options = BuildOptions {
            content_index: Some(ContentIndexStorage::Memory),
            content_cache: Some(cache.path().to_path_buf()),
            ..BuildOptions::default()
        };

        let index = Index::build(root.path(), &options).expect("build").index;
        let needle = Patterns::new(["needle"], Case::Sensitive).expect("compile");
        let haystack = Patterns::new(["haystack"], Case::Sensitive).expect("compile");

        assert_eq!(
            index.grep(&needle, &GrepOptions::default()).ruled_out(),
            1,
            "the planted entry lacks the needle's trigrams"
        );
        assert_eq!(
            index.grep(&haystack, &GrepOptions::default()).searched(),
            1,
            "the planted entry holds the haystack's, so the file is read and nothing found"
        );
    }

    #[test]
    fn a_merge_that_left_files_behind_is_a_failure_naming_what_it_left() {
        let mut merged = Merged::default();
        merged.left_in_place = 2;
        merged.orphans_left = 1;
        merged.unreadable_left = 3;

        let error = unremoved(&merged).expect("the merge left files behind");

        assert_eq!(
            error.to_string(),
            "the merge could not remove 2 merged packs, 1 temporary file, 3 unmapped packs, which stay in the directory"
        );
    }

    #[test]
    fn a_merge_that_removed_all_it_meant_to_is_no_failure_whatever_it_expired() {
        let mut merged = Merged::default();
        merged.expired = 5;
        merged.expired_kept = 3;
        merged.orphans = 1;
        merged.unreadable = 1;

        assert!(unremoved(&merged).is_none());
    }
}
