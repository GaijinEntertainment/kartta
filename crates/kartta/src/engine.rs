use std::path::Path;
use std::{fs, io, iter};

use kartta_fs::{BuildError, BuildOptions, CancelToken, ContentVisitor, Index};
use kartta_graph::{Change, Definition, Graph, Limits, Region, Resolution};
use kartta_lang::{DocComments, Extraction, Language, RawExtraction, Resolver};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::extracting::{Extracted, Extracting, FileCounts};
use crate::repository::IndexRepository;
use crate::source::{FileLines, Source, SourceOptions};
use crate::stats::{EngineStats, LanguageFiles};

const RESOLVED_AT_ONCE: usize = 4096;

/// A tree's file index and the code graph of its languages, built from one walk that reads each source file once.
///
/// [`Engine::build`] walks the tree with [`kartta_fs`], and every file a language [claims](Language::claims) is read
/// during that walk and extracted on the walk's threads. Once the walk is done, each extraction is resolved against
/// the index and goes into the graph of its language's [family](Language::family), so files of languages that call
/// each other share one graph.
#[derive(Debug)]
pub struct Engine {
    index: Index,
    languages: Vec<Box<dyn Language>>,
    file_counts: Vec<FileCounts>,
    families: Vec<Family>,
    family_of: Vec<usize>,
}

#[derive(Debug)]
struct Family {
    name: String,
    graph: Graph,
}

/// What one [`Engine::build`] produced: the engine, and the failures the build collected.
#[derive(Debug)]
pub struct EngineOutcome {
    engine: Engine,
    errors: Vec<BuildError>,
}

impl EngineOutcome {
    /// The engine the build produced.
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Takes the engine, dropping the report.
    ///
    /// Check [`EngineOutcome::errors`] first: a graph built from an incomplete walk looks exactly like one built from a
    /// complete walk.
    #[must_use]
    pub fn into_engine(self) -> Engine {
        self.engine
    }

    /// The failures the build collected rather than returned, as [`kartta_fs::BuildOutcome::errors`] reports them,
    /// each costing only what it names. A source file listed as [`BuildError::Read`] is indexed, and no graph holds it.
    #[must_use]
    pub fn errors(&self) -> &[BuildError] {
        &self.errors
    }
}

impl Engine {
    /// Walks `root`, indexes it as [`kartta_fs::Index::build_with`] does under `options`, and builds the graph of each
    /// language family from the files `languages` claim, stopping when `cancel` is cancelled.
    ///
    /// A file goes to the first of `languages` that claims it, and each family's graph resolves calls under the
    /// [conventions](Language::conventions) of the first of its languages. A claimed file goes into a graph only when
    /// the walk reads it for a [`kartta_fs::ContentVisitor`]: a regular file the ignore rules keep and that is text by
    /// the rule [`kartta_fs::Index::is_binary`] applies. The walk reads each such file once and extracts it there; a
    /// file a [resolver](Language::resolver) asks the [`Repository`](kartta_lang::Repository) for, such as a
    /// `go.mod`, is read when the resolver asks.
    ///
    /// With [`BuildOptions::content_cache`], an extraction is taken from the cache when an earlier build of the same
    /// bytes by the same [fingerprint](Language::fingerprint) put it there, and is put there otherwise. The cache keeps
    /// a language's extractions under the namespace whose kind is `graph/` followed by the language's
    /// [name](Language::name), and whose fingerprint is the language's, followed by [`kartta_lang::FINGERPRINT`] and by
    /// [`RawExtraction::CODEC`], the form they are kept in. A build sees the cache as it was when the build opened it,
    /// and publishes what it extracted once its walk is done, so builds that run at once over a cache that lacks a file
    /// each extract that file.
    ///
    /// Until the walk is done, the build holds each extraction in that form. The extractions are then resolved on the
    /// index's [threads](kartta_fs::Index::on_threads) in batches, each batch inserted in path order before the next is
    /// resolved, so only one batch is held resolved at a time. A language is asked for its
    /// [resolver](Language::resolver) only when it claimed a file the build read, so one with nothing to resolve reads
    /// no configuration.
    ///
    /// On Linux with glibc, the index build and this build both end with the trim [`kartta_fs::Index::build`]
    /// describes: the index build once its walk is done, and this build once the graphs hold every extraction, since
    /// resolving frees each extraction and its decode.
    ///
    /// # Errors
    ///
    /// As [`kartta_fs::Index::build_with`], and [`BuildError::Cancelled`] when `cancel` stopped the build before it
    /// resolved every extraction. Every deeper failure is collected into [`EngineOutcome::errors`]. An extraction the
    /// cache holds in a form this build cannot read is extracted again from a second read of the file, and that read
    /// failing is collected as [`BuildError::Read`].
    pub fn build(
        root: impl AsRef<Path>,
        options: &BuildOptions,
        languages: Vec<Box<dyn Language>>,
        cancel: Option<&CancelToken>,
    ) -> Result<EngineOutcome, BuildError> {
        let extracting = Extracting::new(&languages);
        let visitor: &dyn ContentVisitor = &extracting;
        let (index, mut errors) =
            Index::build_with(root, options, Some(visitor), cancel)?.into_parts();
        let (extracted, mut file_counts) = extracting.into_extracted();

        let (mut families, family_of) = families(&languages);
        let resolving = Resolving {
            index: &index,
            languages: &languages,
            family_of: &family_of,
            cancel,
        };
        let held = resolving.insert_all(extracted, &mut families, &mut errors)?;
        for (counts, held) in file_counts.iter_mut().zip(held) {
            counts.held = held;
        }
        release_freed_memory();

        Ok(EngineOutcome {
            engine: Self {
                index,
                languages,
                file_counts,
                families,
                family_of,
            },
            errors,
        })
    }

    /// The tree's file index.
    #[must_use]
    pub const fn index(&self) -> &Index {
        &self.index
    }

    /// Each language family's name and graph, in the order their first languages were given to [`Engine::build`].
    pub fn graphs(&self) -> impl ExactSizeIterator<Item = (&str, &Graph)> {
        self.families
            .iter()
            .map(|family| (family.name.as_str(), &family.graph))
    }

    /// The graph of the family whose language claims the file at `path`, or `None` when no language does.
    ///
    /// The graph answers for the file only if the build read it; a claimed path the tree does not hold is in no graph.
    #[must_use]
    pub fn graph_of(&self, path: &str) -> Option<&Graph> {
        Some(&self.families[self.family_claiming(path)?].graph)
    }

    /// What a change to the lines of `regions` reaches, across the families whose languages claim their files: each
    /// family's graph analyzes the regions of its files as [`Graph::analyze_change`] does, within `limits`, and
    /// [`Change::merge`] ranks the answers together. A region whose file no language claims counts as unheld.
    #[must_use]
    pub fn analyze_change(&self, regions: &[Region<'_>], limits: Limits) -> Change<'_> {
        let mut by_family: Vec<Vec<Region<'_>>> = vec![Vec::new(); self.families.len()];
        let mut unclaimed = 0;
        for region in regions {
            match self.family_claiming(region.path) {
                Some(family) => by_family[family].push(region.clone()),
                None => unclaimed += 1,
            }
        }
        let mut change = Change::merge(
            self.families
                .iter()
                .zip(&by_family)
                .filter(|(_, regions)| !regions.is_empty())
                .map(|(family, regions)| family.graph.analyze_change(regions, limits)),
        );
        change.unheld_regions += unclaimed;
        change
    }

    /// What the build made of each language's files, and what each family's graph holds, as [`Graph::stats`] counts
    /// it. The files are counted as the build met them, so this costs only the graphs' counts.
    #[must_use]
    pub fn stats(&self) -> EngineStats<'_> {
        let languages = self
            .languages
            .iter()
            .zip(&self.file_counts)
            .zip(&self.family_of)
            .map(|((language, counts), &family)| LanguageFiles {
                name: language.name(),
                family: &self.families[family].name,
                claimed: counts.claimed,
                read: counts.read,
                cached: counts.cached,
                held: counts.held,
            })
            .collect();
        EngineStats {
            languages,
            families: self
                .graphs()
                .map(|(name, graph)| (name, graph.stats()))
                .collect(),
        }
    }

    /// How every call site of each family's graph binds, resolved as [`Graph::resolution`] does on the index's
    /// [threads](kartta_fs::Index::on_threads), at most `limit` of each family's unbound sites listed. Each family
    /// resolves after the one before it, in the order of [`Engine::graphs`].
    #[must_use]
    pub fn resolution(&self, limit: usize) -> Vec<(&str, Resolution<'_>)> {
        self.index.on_threads(|| {
            self.graphs()
                .map(|(name, graph)| (name, graph.resolution(limit)))
                .collect()
        })
    }

    /// The text of each of `definitions`, in their order, read from its file now: from the doc comment that the file's
    /// language writes directly above the definition through the definition's last line, as much of it as `options`
    /// lets through. A file is read once for all of its definitions, and only as far as the last line one of them ends
    /// on.
    ///
    /// The graph holds the lines the build found a definition on, so a file edited since the build answers its new
    /// text at those lines, and [`Source::resized`] says when its size changed. An element is the error of opening or
    /// reading the file where that failed, its message naming the file: a file deleted since the build answers
    /// [`io::ErrorKind::NotFound`].
    #[must_use]
    pub fn sources(
        &self,
        definitions: &[Definition<'_>],
        options: SourceOptions,
    ) -> Vec<io::Result<Source>> {
        let mut order: Vec<usize> = (0..definitions.len()).collect();
        order.sort_by_key(|&at| definitions[at].file);
        let mut sources: Vec<Option<io::Result<Source>>> =
            iter::repeat_with(|| None).take(definitions.len()).collect();
        for run in order.chunk_by(|&left, &right| definitions[left].file == definitions[right].file)
        {
            let path = definitions[run[0]].file;
            let through = run
                .iter()
                .map(|&at| options.last_line_answered(&definitions[at]))
                .max();
            let file = self.file_lines(path, through.unwrap_or(0));
            let reading = file
                .as_ref()
                .map(|file| file.reading(self.doc_comments_of(path), options));
            for &at in run {
                sources[at] = Some(match &reading {
                    Ok(reading) => Ok(reading.source(&definitions[at])),
                    Err(error) => Err(io::Error::new(error.kind(), format!("{path}: {error}"))),
                });
            }
        }
        sources.into_iter().flatten().collect()
    }

    fn file_lines(&self, path: &str, through: u32) -> io::Result<FileLines> {
        FileLines::read(
            &self.index.root().join(path),
            self.index.size(path),
            through,
        )
    }

    fn doc_comments_of(&self, path: &str) -> DocComments {
        self.languages
            .iter()
            .find(|language| language.claims(path))
            .map_or(DocComments::NONE, |language| language.doc_comments())
    }

    fn family_claiming(&self, path: &str) -> Option<usize> {
        let language = self
            .languages
            .iter()
            .position(|language| language.claims(path))?;
        Some(self.family_of[language])
    }
}

fn families(languages: &[Box<dyn Language>]) -> (Vec<Family>, Vec<usize>) {
    let mut families: Vec<Family> = Vec::new();
    let mut family_of = Vec::with_capacity(languages.len());

    for language in languages {
        let name = language.family();
        let at = families
            .iter()
            .position(|family| family.name == name)
            .unwrap_or_else(|| {
                families.push(Family {
                    name: name.to_owned(),
                    graph: Graph::new(language.conventions()),
                });
                families.len() - 1
            });
        family_of.push(at);
    }

    (families, family_of)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_freed_memory() {
    // SAFETY: `malloc_trim` takes no pointer: it returns the pages of glibc's own free chunks to the system, each arena
    // under its own lock, so it is sound from any thread at any time.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
const fn release_freed_memory() {}

struct Resolving<'b> {
    index: &'b Index,
    languages: &'b [Box<dyn Language>],
    family_of: &'b [usize],
    cancel: Option<&'b CancelToken>,
}

impl Resolving<'_> {
    fn insert_all(
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
