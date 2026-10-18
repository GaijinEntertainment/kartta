use std::path::Path;
use std::{io, iter};

use kartta_fs::{
    BuildError, BuildOptions, BuildOutcome, CancelToken, ContentVisitor, Index, RefreshOutcome,
};
use kartta_graph::{Change, Definition, Graph, Limits, Region, Resolution};
use kartta_lang::{DocComments, Language};

use crate::extracting::{Extracting, FileCounts};
use crate::following::{Following, Refreshing, Walked};
use crate::resolving::{Graphs, Resolving};
use crate::source::{FileLines, Source, SourceOptions};
use crate::stats::{EngineStats, LanguageFiles};

const ENTRIES_PER_NAMED_PATH: usize = 150;
const FEWEST_NAMED_PATHS: usize = 50;

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
    following: Option<Following>,
}

#[derive(Debug)]
pub(crate) struct Family {
    name: String,
    pub(crate) graph: Graph,
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

/// What one [`Engine::refresh`] or [`Engine::refresh_paths`] did: what the index's refresh walked and found, and what
/// the graphs took in, dropped and resolved again.
#[derive(Debug)]
pub struct EngineRefresh {
    pub(crate) outcome: RefreshOutcome,
    pub(crate) errors: Vec<BuildError>,
    pub(crate) extracted: usize,
    pub(crate) resolved_again: usize,
    pub(crate) dropped: Option<usize>,
    pub(crate) reconfigured: Vec<String>,
    pub(crate) rebuilt: bool,
    pub(crate) widened: Option<Widened>,
}

/// How an [`Engine::refresh_paths`] came to refresh the whole tree: it was named more paths than it walks apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Widened {
    /// The paths the call was named.
    pub named: usize,
    /// The most paths the call would have walked apart, [`Engine::refresh_paths_limit`] when it began.
    pub limit: usize,
}

impl EngineRefresh {
    /// What the index's refresh reports: the parts it walked, the regular files it added and removed, and the ids it
    /// retired.
    #[must_use]
    pub const fn index(&self) -> &RefreshOutcome {
        &self.outcome
    }

    /// The failures the refresh collected rather than returned: those of the index's walk, then the files a graph
    /// could not read again, each listed as [`BuildError::Read`] and held by no graph.
    pub fn errors(&self) -> impl Iterator<Item = &BuildError> {
        self.outcome.errors.iter().chain(&self.errors)
    }

    /// The files the walk read, new or changed, whose extraction went into a graph.
    #[must_use]
    pub const fn extracted(&self) -> usize {
        self.extracted
    }

    /// The files the walk carried over unchanged that were read and resolved again, because a question their
    /// resolution asked of the tree has another answer now, or their language's configuration changed.
    #[must_use]
    pub const fn resolved_again(&self) -> usize {
        self.resolved_again
    }

    /// The files the graphs held and hold no more: gone, hidden by the ignore rules, no longer read as text, or not
    /// read again where a change reached them. `None` when every graph was [rebuilt](EngineRefresh::rebuilt), which
    /// keeps no list of the files held before; [`RefreshOutcome::removed`] still names the files the tree lost.
    #[must_use]
    pub const fn dropped(&self) -> Option<usize> {
        self.dropped
    }

    /// The languages whose resolver read another configuration than it read before, a `go.mod` or a `tsconfig.json`,
    /// so that every file of theirs was resolved again, in the order the languages were given to [`Engine::build`].
    #[must_use]
    pub fn reconfigured(&self) -> &[String] {
        &self.reconfigured
    }

    /// Whether every graph was built again from every file, because the engine was built without
    /// [`BuildOptions::stamps`] and kept nothing to find what a change reaches.
    #[must_use]
    pub const fn rebuilt(&self) -> bool {
        self.rebuilt
    }

    /// The paths an [`Engine::refresh_paths`] was named and the limit they passed, when it refreshed the whole tree
    /// in their place; `None` from [`Engine::refresh`] and from a call that walked the paths it was named.
    #[must_use]
    pub const fn widened(&self) -> Option<Widened> {
        self.widened
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
    /// [`kartta_lang::RawExtraction::CODEC`], the form they are kept in. A build sees the cache as it was when the
    /// build opened it, and publishes what it extracted once its walk is done, so builds that run at once over a cache
    /// that lacks a file each extract that file.
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
        let BuildOutcome { index, mut errors } =
            Index::build_with(root, options, Some(visitor), cancel)?;
        let (extracted, mut file_counts) = extracting.into_extracted();

        let (families, family_of) = families(&languages);
        let resolving = Resolving {
            index: &index,
            languages: &languages,
            family_of: &family_of,
            cancel,
            recording: options.stamps,
        };
        let Graphs {
            families,
            inserted,
            configurations,
        } = resolving.graphs(families, extracted)?;
        for (counts, held) in file_counts.iter_mut().zip(inserted.held) {
            counts.held = held;
        }
        errors.extend(inserted.errors);
        let following = options
            .stamps
            .then(|| Following::new(inserted.followed, configurations));
        release_freed_memory();

        Ok(EngineOutcome {
            engine: Self {
                index,
                languages,
                file_counts,
                families,
                family_of,
                following,
            },
            errors,
        })
    }

    /// Walks the tree again as [`kartta_fs::Index::refresh`] does and brings every family's graph up to it, in place,
    /// so each answers as a build of the tree would now, stopping the walk when `cancel` is cancelled.
    ///
    /// What the refresh costs depends on [`BuildOptions::stamps`] at the build. Without stamps the engine kept nothing
    /// for a refresh, every file is read again, and each graph is built again from every file. With stamps the
    /// engine kept, for each file its graphs hold, the questions its language's [resolver](Language::resolver) asked
    /// of the tree while it resolved that file: a file read again is extracted again, through the content cache when
    /// it holds the new bytes, a file gone or no longer read as text leaves its graph, and a file whose questions the
    /// refresh answers differently, such as an include that now finds a file or a Go package that gained one, is read
    /// again and resolved again. When the configuration a resolver read as it was built changed, every file of its
    /// language is read and resolved again. [`EngineRefresh`] counts each.
    ///
    /// The engine attributes a question to the file whose resolution asked it on the asking thread, so a
    /// [`Resolver`](kartta_lang::Resolver) that asks from another thread, or answers one file from what it asked for
    /// another, leaves a change to that answer unseen.
    ///
    /// # Errors
    ///
    /// As [`kartta_fs::Index::refresh`]: a returned error, a cancelled refresh included, leaves the engine answering
    /// as it did. Once the index is replaced, the graphs are brought up to it whatever `cancel` says.
    pub fn refresh(&mut self, cancel: Option<&CancelToken>) -> Result<EngineRefresh, BuildError> {
        let extracting = Extracting::new(&self.languages);
        let outcome = self.index.refresh(Some(&extracting), cancel)?;
        let (extracted, counts) = extracting.into_extracted();
        self.brought_up(Walked {
            outcome,
            extracted,
            counts,
        })
    }

    /// Walks again only the parts of the tree at `paths`, as [`kartta_fs::Index::refresh_paths`] does, and brings
    /// every family's graph up to them as [`Engine::refresh`] does, stopping the walk when `cancel` is cancelled.
    ///
    /// The paths are what changed, as a [`kartta_fs::Watcher`] or a version control tool names them, and a change no
    /// path names is not seen. The named files are read and extracted one after another, where a whole refresh reads
    /// and extracts on every thread, so a call named more paths than [`Engine::refresh_paths_limit`] is
    /// [`Engine::refresh`], and [`EngineRefresh::widened`] says so. Without [`BuildOptions::stamps`] at the build,
    /// the engine kept nothing to find the files a change reaches, so this is [`Engine::refresh`] and walks the whole
    /// tree, however few the paths.
    ///
    /// # Errors
    ///
    /// As [`kartta_fs::Index::refresh_paths`], and as [`Engine::refresh`] once the walk is done.
    pub fn refresh_paths<'p>(
        &mut self,
        paths: impl IntoIterator<Item = &'p str>,
        cancel: Option<&CancelToken>,
    ) -> Result<EngineRefresh, BuildError> {
        let walks_apart = self.following.is_some();
        let limit = self.refresh_paths_limit();
        let mut walked_apart: Vec<&str> = Vec::new();
        let mut named = 0;
        for path in paths {
            if leads_outside_tree(path) {
                return Err(BuildError::OutsideTree {
                    path: path.to_owned(),
                });
            }
            if walks_apart && named < limit {
                walked_apart.push(path);
            }
            named += 1;
        }
        if !walks_apart {
            return self.refresh(cancel);
        }
        if named > limit {
            let mut refresh = self.refresh(cancel)?;
            refresh.widened = Some(Widened { named, limit });
            return Ok(refresh);
        }
        let extracting = Extracting::new(&self.languages);
        let outcome = self
            .index
            .refresh_paths(walked_apart, Some(&extracting), cancel)?;
        let (extracted, counts) = extracting.into_extracted();
        self.brought_up(Walked {
            outcome,
            extracted,
            counts,
        })
    }

    /// The most paths [`Engine::refresh_paths`] walks apart before it refreshes the whole tree in their place: one for
    /// every 150 entries the index holds, since a whole refresh costs a pass over every entry, and never fewer than 50.
    #[must_use]
    pub fn refresh_paths_limit(&self) -> usize {
        (self.index.len() / ENTRIES_PER_NAMED_PATH).max(FEWEST_NAMED_PATHS)
    }

    fn brought_up(&mut self, walked: Walked) -> Result<EngineRefresh, BuildError> {
        let Some(following) = self.following.as_mut() else {
            return self.rebuilt(walked);
        };
        following.refresh(
            Refreshing {
                index: &self.index,
                languages: &self.languages,
                family_of: &self.family_of,
                families: &mut self.families,
                file_counts: &mut self.file_counts,
            },
            walked,
        )
    }

    fn rebuilt(&mut self, walked: Walked) -> Result<EngineRefresh, BuildError> {
        let Walked {
            outcome,
            extracted,
            mut counts,
        } = walked;
        let resolving = Resolving {
            index: &self.index,
            languages: &self.languages,
            family_of: &self.family_of,
            cancel: None,
            recording: false,
        };
        let Graphs {
            families, inserted, ..
        } = resolving.graphs(families(&self.languages).0, extracted)?;
        for (counts, held) in counts.iter_mut().zip(inserted.held) {
            counts.held = held;
        }
        let extracted = counts.iter().map(|counts| counts.held).sum();
        self.families = families;
        self.file_counts = counts;
        release_freed_memory();

        Ok(EngineRefresh {
            outcome,
            errors: inserted.errors,
            extracted,
            resolved_again: 0,
            dropped: None,
            reconfigured: Vec::new(),
            rebuilt: true,
            widened: None,
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

pub(crate) fn families(languages: &[Box<dyn Language>]) -> (Vec<Family>, Vec<usize>) {
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

// constraint: kartta_fs::Index::refresh_paths refuses a path by this rule, and a call that never reaches it must too
fn leads_outside_tree(path: &str) -> bool {
    path.split('/').any(|part| part == "..")
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
