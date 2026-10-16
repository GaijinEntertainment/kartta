use std::io;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use kartta_cache::{ContentHash, Namespace, PackWriter, Store};

use crate::build::BuildError;
use crate::trigram::{self, Trigrams};

#[derive(Debug)]
pub(crate) struct SharedCache {
    store: Store,
    writer: PackWriter,
    trigrams: Namespace,
    failure: Mutex<Option<io::Error>>,
}

impl SharedCache {
    pub(crate) fn open(dir: &Path) -> Result<Self, BuildError> {
        let store = Store::open(dir).map_err(|source| cache_error(dir, source))?;

        Ok(Self {
            writer: store.writer(),
            store,
            trigrams: trigram_namespace(),
            failure: Mutex::new(None),
        })
    }

    pub(crate) fn get(&self, namespace: &Namespace, content: &ContentHash) -> Option<&[u8]> {
        self.store.get(namespace, content)
    }

    pub(crate) fn insert(&self, namespace: &Namespace, content: &ContentHash, payload: &[u8]) {
        if let Err(error) = self.writer.insert(namespace, content, payload) {
            self.failure
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert(error);
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
            failure,
            ..
        } = self;
        let dir = store.dir().to_path_buf();
        drop(store);
        if let Some(source) = failure.into_inner().unwrap_or_else(PoisonError::into_inner) {
            return Some(cache_error(&dir, source));
        }

        writer
            .publish()
            .and_then(|_| Store::open(&dir)?.merge())
            .err()
            .map(|source| cache_error(&dir, source))
    }
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

        let index = Index::build(root.path(), &options)
            .expect("build")
            .into_index();
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
}
