use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use kartta_fs::{Content, ContentId, ContentVisitor, Namespace};
use kartta_lang::{Language, RawExtraction};

pub(crate) struct Extracted {
    pub(crate) id: ContentId,
    pub(crate) language: usize,
    pub(crate) encoded: Box<[u8]>,
}

#[derive(Debug)]
pub(crate) struct FileCounts {
    pub(crate) claimed: usize,
    pub(crate) read: usize,
    pub(crate) cached: usize,
    pub(crate) held: usize,
}

#[derive(Default)]
struct Counters {
    claimed: AtomicUsize,
    read: AtomicUsize,
    cached: AtomicUsize,
}

pub(crate) struct Extracting<'l> {
    languages: &'l [Box<dyn Language>],
    namespaces: Box<[Namespace]>,
    counters: Box<[Counters]>,
    extracted: Mutex<Vec<Extracted>>,
    encoding_buffers: Mutex<Vec<Vec<u8>>>,
}

impl<'l> Extracting<'l> {
    pub(crate) fn new(languages: &'l [Box<dyn Language>]) -> Self {
        let namespaces = languages
            .iter()
            .map(|language| {
                let fingerprint = [
                    language.fingerprint(),
                    &kartta_lang::FINGERPRINT,
                    RawExtraction::CODEC,
                ]
                .concat();
                Namespace::new(&format!("graph/{}", language.name()), &fingerprint)
            })
            .collect();

        Self {
            languages,
            namespaces,
            counters: languages.iter().map(|_| Counters::default()).collect(),
            extracted: Mutex::new(Vec::new()),
            encoding_buffers: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn into_extracted(self) -> (Vec<Extracted>, Vec<FileCounts>) {
        let counts = self
            .counters
            .iter()
            .map(|counters| FileCounts {
                claimed: counters.claimed.load(Ordering::Relaxed),
                read: counters.read.load(Ordering::Relaxed),
                cached: counters.cached.load(Ordering::Relaxed),
                held: 0,
            })
            .collect();
        let extracted = self
            .extracted
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        (extracted, counts)
    }

    fn claimant(&self, path: &str) -> Option<usize> {
        self.languages
            .iter()
            .position(|language| language.claims(path))
    }

    // constraint: on glibc, encodings grown in place and shrunk to fit raised a large tree's resident peak by a quarter
    // (ADR 1.18), so each is written into a buffer the walk's threads reuse and kept as an exact-size copy
    fn extract(&self, language: usize, content: &Content<'_>) -> Box<[u8]> {
        let mut buffer = self
            .encoding_buffers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_default();
        buffer.clear();
        self.languages[language]
            .extract(content.bytes())
            .encode(&mut buffer);
        content.cache(&self.namespaces[language], &buffer);
        let encoded = Box::from(buffer.as_slice());
        self.encoding_buffers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(buffer);
        encoded
    }
}

impl ContentVisitor for Extracting<'_> {
    fn wants(&self, path: &str) -> bool {
        let Some(language) = self.claimant(path) else {
            return false;
        };
        self.counters[language]
            .claimed
            .fetch_add(1, Ordering::Relaxed);
        true
    }

    fn visit(&self, content: Content<'_>) {
        let Some(language) = self.claimant(content.path()) else {
            return;
        };
        let counters = &self.counters[language];
        counters.read.fetch_add(1, Ordering::Relaxed);
        let encoded = content.cached(&self.namespaces[language]).map_or_else(
            || self.extract(language, &content),
            |cached| {
                counters.cached.fetch_add(1, Ordering::Relaxed);
                Box::from(cached)
            },
        );

        self.extracted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Extracted {
                id: content.id(),
                language,
                encoded,
            });
    }
}
