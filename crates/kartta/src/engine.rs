use std::path::Path;
use std::{io, iter};

use kartta_fs::{
    BuildError, BuildOptions, BuildOutcome, CancelToken, ContentVisitor, Index, RefreshOutcome,
};
#[cfg(feature = "watch")]
use kartta_fs::{WatchError, Watcher};
use kartta_graph::{Change, Definition, Graph, Limits, Region, Resolution};
use kartta_lang::{DocComments, Language};

use crate::extracting::{Extracting, FileCounts, claimant};
use crate::following::{Following, Refreshing, Walked};
use crate::resolving::{Graphs, Resolving};
use crate::source::{FileLines, Source, SourceOptions};
use crate::stats::{EngineStats, LanguageFiles};

const ENTRIES_PER_PART: usize = 150;
const FEWEST_PARTS: usize = 50;

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
    /// The engine the build produced. Check [`EngineOutcome::errors`] before keeping it alone: a graph built from an
    /// incomplete walk looks exactly like one built from a complete walk.
    pub engine: Engine,
    /// The failures the build collected rather than returned, as [`kartta_fs::BuildOutcome::errors`] reports them,
    /// each costing only what it names. A source file listed as [`BuildError::Read`] is indexed, and no graph holds it.
    pub errors: Vec<BuildError>,
}

/// What one [`Engine::refresh`] or [`Engine::refresh_paths`] did: what the index's refresh walked and found, and what
/// the graphs took in, dropped and resolved again.
#[derive(Debug)]
pub struct EngineRefresh {
    /// What the index's refresh reports: the parts it walked, the regular files it added and removed, and the ids it
    /// retired.
    pub index: RefreshOutcome,
    pub(crate) errors: Vec<BuildError>,
    /// The files the walk read, new or changed, whose extraction went into a graph.
    pub extracted: usize,
    /// The files the walk carried over unchanged that were read and resolved again, because a question their
    /// resolution asked of the tree has another answer now, or their language's configuration changed.
    pub resolved_again: usize,
    /// The files the graphs held and hold no more: gone, hidden by the ignore rules, no longer read as text, or not
    /// read again where a change reached them. `None` when every graph was [rebuilt](EngineRefresh::rebuilt), which
    /// keeps no list of the files held before; [`RefreshOutcome::removed`] still names the files the tree lost.
    pub dropped: Option<usize>,
    /// The languages whose resolver read another configuration than it read before, a `go.mod` or a `tsconfig.json`,
    /// so that every file of theirs was resolved again, in the order the languages were given to [`Engine::build`].
    pub reconfigured: Vec<String>,
    /// Whether every graph was built again from every file, because the engine was built without
    /// [`BuildOptions::stamps`] and kept nothing to find what a change reaches.
    pub rebuilt: bool,
    /// The paths an [`Engine::refresh_paths`] was named and the limit their parts passed, when it refreshed the whole
    /// tree in their place; `None` from [`Engine::refresh`] and from a call that walked the parts of its paths.
    pub widened: Option<Widened>,
}

/// How an [`Engine::refresh_paths`] came to refresh the whole tree: the paths it was named came to more parts than it
/// walks apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Widened {
    /// The paths the call was named.
    pub named: usize,
    /// The most parts the call would have walked apart, [`Engine::refresh_paths_limit`] when it began.
    pub limit: usize,
}

impl EngineRefresh {
    /// The failures the refresh collected rather than returned: those of the index's walk, then the files a graph
    /// could not read again, each listed as [`BuildError::Read`] and held by no graph.
    pub fn errors(&self) -> impl Iterator<Item = &BuildError> {
        self.index.errors.iter().chain(&self.errors)
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
    /// [name](Language::name), and whose fingerprint is the language's, followed by [`kartta_lang::FINGERPRINT`], which
    /// covers the form they are kept in. A build sees the cache as it was when the build opened it, and publishes what
    /// it extracted once its walk is done, so builds that run at once over a cache that lacks a file each extract that
    /// file.
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
        let root = root.as_ref();
        let (outcome, ()) = Self::build_from_walk(options, languages, cancel, |visitor| {
            Ok((Index::build_with(root, options, Some(visitor), cancel)?, ()))
        })?;
        Ok(outcome)
    }

    /// Starts a [`Watcher`] on `root`, then builds as [`Engine::build`] does, walking the tree as
    /// [`kartta_fs::Index::build_watched`] does, so the watcher holds every change made while the build walked, which
    /// [`Watcher::drain`] then hands over for [`Engine::refresh_paths`].
    ///
    /// # Errors
    ///
    /// As [`Engine::build`]. The watcher's own failures are the second value, as
    /// [`kartta_fs::Index::build_watched`] says.
    #[cfg(feature = "watch")]
    pub fn build_watched(
        root: impl AsRef<Path>,
        options: &BuildOptions,
        languages: Vec<Box<dyn Language>>,
        cancel: Option<&CancelToken>,
    ) -> Result<(EngineOutcome, Result<Watcher, WatchError>), BuildError> {
        let root = root.as_ref();
        Self::build_from_walk(options, languages, cancel, |visitor| {
            Index::build_watched(root, options, Some(visitor), cancel)
        })
    }

    fn build_from_walk<T>(
        options: &BuildOptions,
        languages: Vec<Box<dyn Language>>,
        cancel: Option<&CancelToken>,
        walk: impl FnOnce(&dyn ContentVisitor) -> Result<(BuildOutcome, T), BuildError>,
    ) -> Result<(EngineOutcome, T), BuildError> {
        let extracting = Extracting::new(&languages);
        let (BuildOutcome { index, mut errors }, walked) = walk(&extracting)?;
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

        let engine = Self {
            index,
            languages,
            file_counts,
            families,
            family_of,
            following,
        };
        Ok((EngineOutcome { engine, errors }, walked))
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
        let (refresh, ()) = self
            .refresh_from_walk(|index, visitor| Ok((index.refresh(Some(visitor), cancel)?, ())))?;
        Ok(refresh)
    }

    /// Starts a [`Watcher`] on the tree, then refreshes as [`Engine::refresh`] does, walking the tree as
    /// [`kartta_fs::Index::refresh_watched`] does, so the watcher holds every change made while the refresh walked.
    ///
    /// # Errors
    ///
    /// As [`Engine::refresh`]. The watcher's own failures are the second value, as
    /// [`kartta_fs::Index::build_watched`] says.
    #[cfg(feature = "watch")]
    pub fn refresh_watched(
        &mut self,
        cancel: Option<&CancelToken>,
    ) -> Result<(EngineRefresh, Result<Watcher, WatchError>), BuildError> {
        self.refresh_from_walk(|index, visitor| index.refresh_watched(Some(visitor), cancel))
    }

    /// Walks again only the parts of the tree at `paths`, as [`kartta_fs::Index::refresh_paths`] does, and brings
    /// every family's graph up to them as [`Engine::refresh`] does, stopping the walk when `cancel` is cancelled.
    ///
    /// The paths are what changed, as a [`kartta_fs::Watcher`] or a version control tool names them, and a change no
    /// path names is not seen. The named files are read and extracted one after another, where a whole refresh reads
    /// and extracts on every thread, so a call whose paths come to more parts than [`Engine::refresh_paths_limit`] is
    /// [`Engine::refresh`], and [`EngineRefresh::widened`] says so. The parts are those
    /// [`kartta_fs::Index::parts_of`] answers: the paths inside a directory the index does not hold, such as an
    /// ignored `target/`, are one part, and a path the index skips is none. A call named more paths than the limit
    /// works their parts out first, at the cost [`kartta_fs::Index::parts_of`] states, whether it then walks them or
    /// the whole tree. Without [`BuildOptions::stamps`] at the build, the engine kept nothing to find the files a
    /// change reaches, so this is [`Engine::refresh`] and walks the whole tree, however few the paths.
    ///
    /// # Errors
    ///
    /// As [`kartta_fs::Index::refresh_paths`], and as [`Engine::refresh`] once the walk is done.
    pub fn refresh_paths<'p>(
        &mut self,
        paths: impl IntoIterator<Item = &'p str>,
        cancel: Option<&CancelToken>,
    ) -> Result<EngineRefresh, BuildError> {
        let mut paths = paths.into_iter();
        if self.following.is_none() {
            if let Some(path) = paths.find(|path| leads_outside_tree(path)) {
                return Err(BuildError::OutsideTree {
                    path: path.to_owned(),
                });
            }
            return self.refresh(cancel);
        }
        let limit = self.refresh_paths_limit();
        let first: Vec<&str> = paths.by_ref().take(limit + 1).collect();
        if first.len() <= limit {
            return self.walked_apart(first, cancel);
        }
        let mut named = 0;
        let parts = self
            .index
            .parts_of(first.into_iter().chain(paths).inspect(|_| named += 1))?;
        if parts.len() > limit {
            let mut refresh = self.refresh(cancel)?;
            refresh.widened = Some(Widened { named, limit });
            return Ok(refresh);
        }
        self.walked_apart(parts.iter().map(String::as_str), cancel)
    }

    /// The most parts [`Engine::refresh_paths`] walks apart before it refreshes the whole tree in their place: one for
    /// every 150 entries the index holds, since a whole refresh costs a pass over every entry, and never fewer than 50.
    #[must_use]
    pub fn refresh_paths_limit(&self) -> usize {
        (self.index.len() / ENTRIES_PER_PART).max(FEWEST_PARTS)
    }

    fn walked_apart<'p>(
        &mut self,
        paths: impl IntoIterator<Item = &'p str>,
        cancel: Option<&CancelToken>,
    ) -> Result<EngineRefresh, BuildError> {
        let (refresh, ()) = self.refresh_from_walk(|index, visitor| {
            Ok((index.refresh_paths(paths, Some(visitor), cancel)?, ()))
        })?;
        Ok(refresh)
    }

    fn refresh_from_walk<T>(
        &mut self,
        walk: impl FnOnce(&mut Index, &dyn ContentVisitor) -> Result<(RefreshOutcome, T), BuildError>,
    ) -> Result<(EngineRefresh, T), BuildError> {
        let extracting = Extracting::new(&self.languages);
        let (outcome, walked) = walk(&mut self.index, &extracting)?;
        let (extracted, counts) = extracting.into_extracted();
        let refresh = self.brought_up(Walked {
            outcome,
            extracted,
            counts,
        })?;
        Ok((refresh, walked))
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
            index: outcome,
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

    /// Each language's [name](Language::name) and the name of its family, in the order they were given to
    /// [`Engine::build`].
    pub fn languages(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.languages
            .iter()
            .zip(&self.family_of)
            .map(|(language, &family)| (language.name(), self.families[family].name.as_str()))
    }

    /// Each language family's name and graph, in the order their first languages were given to [`Engine::build`].
    pub fn graphs(&self) -> impl ExactSizeIterator<Item = (&str, &Graph)> {
        self.families
            .iter()
            .map(|family| (family.name.as_str(), &family.graph))
    }

    /// The name and graph of the family whose language claims the file at `path`, or `None` when no language does.
    ///
    /// The graph answers for the file only if the build read it; a claimed path the tree does not hold is in no graph.
    #[must_use]
    pub fn graph_of(&self, path: &str) -> Option<(&str, &Graph)> {
        let family = &self.families[self.family_claiming(path)?];
        Some((&family.name, &family.graph))
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
            .languages()
            .zip(&self.file_counts)
            .map(|((name, family), counts)| LanguageFiles {
                name,
                family,
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
        claimant(&self.languages, path).map_or(DocComments::NONE, |language| {
            self.languages[language].doc_comments()
        })
    }

    fn family_claiming(&self, path: &str) -> Option<usize> {
        claimant(&self.languages, path).map(|language| self.family_of[language])
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
