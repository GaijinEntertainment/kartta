use std::fs;

use kartta_fs::{BuildError, CancelToken, Index};
use kartta_lang::{Extraction, Language, RawExtraction, Resolver};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::engine::Family;
use crate::extracting::Extracted;
use crate::repository::IndexRepository;

const RESOLVED_AT_ONCE: usize = 4096;

pub(crate) struct Resolving<'b> {
    pub(crate) index: &'b Index,
    pub(crate) languages: &'b [Box<dyn Language>],
    pub(crate) family_of: &'b [usize],
    pub(crate) cancel: Option<&'b CancelToken>,
}

impl Resolving<'_> {
    pub(crate) fn insert_all(
        &self,
        extracted: Vec<Extracted>,
        families: &mut [Family],
        errors: &mut Vec<BuildError>,
    ) -> Result<Vec<usize>, BuildError> {
        if self.cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(BuildError::Cancelled);
        }
        let mut placed: Vec<(usize, Extracted)> = extracted
            .into_iter()
            .filter_map(|extracted| Some((self.index.content_position(extracted.id)?, extracted)))
            .collect();
        placed.sort_unstable_by_key(|&(position, _)| position);

        let mut claimed = vec![false; self.languages.len()];
        for (_, extracted) in &placed {
            claimed[extracted.language] = true;
        }
        let repository = IndexRepository::new(self.index);
        let resolvers: Vec<Option<Box<dyn Resolver + '_>>> = self
            .languages
            .iter()
            .zip(claimed)
            .map(|(language, claimed)| claimed.then(|| language.resolver(&repository)))
            .collect();
        let mut paths = self.index.paths();
        let mut placed = placed.into_iter();
        let mut held = vec![0; self.languages.len()];

        loop {
            let batch: Vec<(String, Extracted)> = placed
                .by_ref()
                .take(RESOLVED_AT_ONCE)
                .map(|(position, extracted)| (paths.get(position).to_owned(), extracted))
                .collect();
            if batch.is_empty() {
                return Ok(held);
            }
            if self.cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(BuildError::Cancelled);
            }

            for (language, extraction) in self.resolve_batch(batch, &resolvers) {
                held[language] += usize::from(extraction.is_ok());
                match extraction {
                    Ok(extraction) => families[self.family_of[language]].graph.insert(&extraction),
                    Err(error) => errors.push(error),
                }
            }
        }
    }

    fn resolve_batch(
        &self,
        batch: Vec<(String, Extracted)>,
        resolvers: &[Option<Box<dyn Resolver + '_>>],
    ) -> Vec<(usize, Result<Extraction, BuildError>)> {
        self.index.on_threads(|| {
            batch
                .into_par_iter()
                .filter_map(|(path, extracted)| {
                    let language = extracted.language;
                    let resolver = resolvers[language].as_deref()?;
                    Some((language, self.resolve(resolver, &path, &extracted)))
                })
                .collect()
        })
    }

    fn resolve(
        &self,
        resolver: &dyn Resolver,
        path: &str,
        extracted: &Extracted,
    ) -> Result<Extraction, BuildError> {
        if let Some(raw) = RawExtraction::decode(&extracted.encoded) {
            return Ok(resolver.resolve(raw, path));
        }

        let native_path = self.index.root().join(path);
        let bytes = fs::read(&native_path).map_err(|source| BuildError::Read {
            native_path,
            source,
        })?;
        let raw = self.languages[extracted.language].extract(&bytes);
        Ok(resolver.resolve(raw, path))
    }
}
