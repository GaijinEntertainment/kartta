use std::fs;

use kartta_fs::{BuildError, CancelToken, ContentId, Index};
use kartta_lang::{Extraction, Language, RawExtraction, Resolver};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::engine::Family;
use crate::extracting::Extracted;
use crate::following::Followed;
use crate::repository::{Configuration, IndexRepository, asked_while, configured_while};

const RESOLVED_AT_ONCE: usize = 4096;

pub(crate) struct Resolving<'b> {
    pub(crate) index: &'b Index,
    pub(crate) languages: &'b [Box<dyn Language>],
    pub(crate) family_of: &'b [usize],
    pub(crate) cancel: Option<&'b CancelToken>,
    pub(crate) recording: bool,
}

/// One file to resolve and insert, at its position in the index: its extraction as the walk encoded it, or `None` to
/// read and extract it again.
pub(crate) struct ToResolve {
    pub(crate) id: ContentId,
    pub(crate) position: usize,
    pub(crate) language: usize,
    pub(crate) cached: bool,
    pub(crate) encoded: Option<Box<[u8]>>,
}

struct Batched {
    file: ToResolve,
    path: Box<str>,
}

pub(crate) struct Resolvers<'r> {
    resolvers: Vec<Option<Box<dyn Resolver + 'r>>>,
    pub(crate) configurations: Vec<Option<Configuration>>,
}

type Resolved = Result<(Extraction, Box<[u32]>), BuildError>;

pub(crate) struct Graphs {
    pub(crate) families: Vec<Family>,
    pub(crate) inserted: Inserted,
    pub(crate) configurations: Vec<Option<Configuration>>,
}

pub(crate) struct Inserted {
    pub(crate) held: Vec<usize>,
    pub(crate) followed: Vec<(ContentId, Followed)>,
    pub(crate) errors: Vec<BuildError>,
}

impl<'b> Resolving<'b> {
    /// Each family's graph built from `extracted`, what went into it, and what each language's resolver asked as it
    /// was built when this resolution records.
    pub(crate) fn graphs(
        &self,
        mut families: Vec<Family>,
        extracted: Vec<Extracted>,
    ) -> Result<Graphs, BuildError> {
        if self.cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(BuildError::Cancelled);
        }
        let work = self.placed(extracted);
        let mut wanted = vec![false; self.languages.len()];
        for file in &work {
            wanted[file.language] = true;
        }
        let repository = if self.recording {
            IndexRepository::recording(self.index)
        } else {
            IndexRepository::new(self.index)
        };
        let resolvers = self.resolvers(&repository, &wanted);
        let inserted = self.insert_all(work, &resolvers, &mut families)?;
        Ok(Graphs {
            families,
            inserted,
            configurations: resolvers.configurations,
        })
    }

    pub(crate) fn placed(&self, extracted: Vec<Extracted>) -> Vec<ToResolve> {
        extracted
            .into_iter()
            .filter_map(|extracted| {
                Some(ToResolve {
                    id: extracted.id,
                    position: self.index.content_position(extracted.id)?,
                    language: extracted.language,
                    cached: extracted.cached,
                    encoded: Some(extracted.encoded),
                })
            })
            .collect()
    }

    /// The resolver of each language `wanted` names, built over `repository`, and what each asked of it when this
    /// resolution records.
    pub(crate) fn resolvers<'r>(
        &self,
        repository: &'r IndexRepository<'_>,
        wanted: &[bool],
    ) -> Resolvers<'r>
    where
        'b: 'r,
    {
        let mut resolvers = Resolvers {
            resolvers: Vec::with_capacity(wanted.len()),
            configurations: Vec::with_capacity(wanted.len()),
        };
        for (language, &wanted) in self.languages.iter().zip(wanted) {
            let (resolver, configuration) = match (wanted, self.recording) {
                (false, _) => (None, None),
                (true, false) => (Some(language.resolver(repository)), None),
                (true, true) => {
                    let (resolver, configuration) =
                        configured_while(|| language.resolver(repository));
                    (Some(resolver), Some(configuration))
                }
            };
            resolvers.resolvers.push(resolver);
            resolvers.configurations.push(configuration);
        }
        resolvers
    }

    /// Resolves `work` in batches of the index's order, each batch on the index's threads, and inserts each extraction
    /// into its family's graph, answering what each file asked when this resolution records.
    pub(crate) fn insert_all(
        &self,
        mut work: Vec<ToResolve>,
        resolvers: &Resolvers<'_>,
        families: &mut [Family],
    ) -> Result<Inserted, BuildError> {
        work.sort_unstable_by_key(|file| file.position);
        let mut inserted = Inserted {
            held: vec![0; self.languages.len()],
            followed: Vec::new(),
            errors: Vec::new(),
        };
        let mut paths = self.index.paths();
        let mut work = work.into_iter();
        loop {
            let batch: Vec<Batched> = work
                .by_ref()
                .take(RESOLVED_AT_ONCE)
                .map(|file| Batched {
                    path: paths.get(file.position).into(),
                    file,
                })
                .collect();
            if batch.is_empty() {
                return Ok(inserted);
            }
            if self.cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(BuildError::Cancelled);
            }

            for resolved in self.resolve_batch(batch, resolvers) {
                self.insert(resolved, families, &mut inserted);
            }
        }
    }

    fn insert(
        &self,
        (batched, extraction): (Batched, Resolved),
        families: &mut [Family],
        inserted: &mut Inserted,
    ) {
        let (extraction, asked) = match extraction {
            Ok(resolved) => resolved,
            Err(error) => return inserted.errors.push(error),
        };
        let language = batched.file.language;
        families[self.family_of[language]].graph.insert(&extraction);
        inserted.held[language] += 1;
        if self.recording {
            inserted.followed.push(batched.followed(asked));
        }
    }

    fn resolve_batch(
        &self,
        batch: Vec<Batched>,
        resolvers: &Resolvers<'_>,
    ) -> Vec<(Batched, Resolved)> {
        self.index.on_threads(|| {
            batch
                .into_par_iter()
                .filter_map(|mut batched| {
                    let resolver = resolvers.resolvers[batched.file.language].as_deref()?;
                    let encoded = batched.file.encoded.take();
                    let outcome = self.recorded(resolver, &batched, encoded.as_deref());
                    Some((batched, outcome))
                })
                .collect()
        })
    }

    fn recorded(
        &self,
        resolver: &dyn Resolver,
        batched: &Batched,
        encoded: Option<&[u8]>,
    ) -> Resolved {
        if !self.recording {
            return self
                .resolve(resolver, batched, encoded)
                .map(|extraction| (extraction, Box::default()));
        }
        let (extraction, asked) = asked_while(|| self.resolve(resolver, batched, encoded));
        extraction.map(|extraction| (extraction, asked))
    }

    fn resolve(
        &self,
        resolver: &dyn Resolver,
        batched: &Batched,
        encoded: Option<&[u8]>,
    ) -> Result<Extraction, BuildError> {
        if let Some(raw) = encoded.and_then(RawExtraction::decode) {
            return Ok(resolver.resolve(raw, &batched.path));
        }

        let native_path = self.index.root().join(&*batched.path);
        let bytes = fs::read(&native_path).map_err(|source| BuildError::Read {
            native_path,
            source,
        })?;
        let raw = self.languages[batched.file.language].extract(&bytes);
        Ok(resolver.resolve(raw, &batched.path))
    }
}

impl Batched {
    fn followed(self, asked: Box<[u32]>) -> (ContentId, Followed) {
        let Self { file, path } = self;
        let followed = Followed {
            path,
            language: file.language,
            cached: file.cached,
            asked,
        };
        (file.id, followed)
    }
}
