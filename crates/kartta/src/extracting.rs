use std::sync::{Mutex, PoisonError};

use kartta_fs::{Content, ContentId, ContentVisitor, Namespace};
use kartta_lang::{Language, RawExtraction};

pub(crate) struct Extracted {
    pub(crate) id: ContentId,
    pub(crate) language: usize,
    pub(crate) encoded: Box<[u8]>,
}

pub(crate) struct Extracting<'l> {
    languages: &'l [Box<dyn Language>],
    namespaces: Box<[Namespace]>,
    extracted: Mutex<Vec<Extracted>>,
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
            extracted: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn into_extracted(self) -> Vec<Extracted> {
        self.extracted
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn claimant(&self, path: &str) -> Option<usize> {
        self.languages
            .iter()
            .position(|language| language.claims(path))
    }
}

impl ContentVisitor for Extracting<'_> {
    fn wants(&self, path: &str) -> bool {
        self.claimant(path).is_some()
    }

    fn visit(&self, content: Content<'_>) {
        let Some(language) = self.claimant(content.path()) else {
            return;
        };
        let namespace = &self.namespaces[language];
        let encoded = content.cached(namespace).map_or_else(
            || {
                let mut encoded = Vec::new();
                self.languages[language]
                    .extract(content.bytes())
                    .encode(&mut encoded);
                content.cache(namespace, &encoded);
                encoded.into_boxed_slice()
            },
            Box::from,
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
