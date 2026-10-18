use std::marker::PhantomData;
use std::num::{NonZeroU64, NonZeroUsize};
use std::ops::Range;

use kartta_walk::{CancelToken, EntryKind};
use rayon::prelude::*;

use crate::content_index::Narrowed;
use crate::filter::Filter;
use crate::index::Index;
use crate::list::is_below;
use crate::matches::{Failure, Line, LineKind, MatchedFile, Matches, Tally, Total};
use crate::patterns::Patterns;
use crate::scan::{Found, Scanned, Scanner};

const BATCH: usize = 256;
const MIN_FILES_PER_SCANNER: usize = 16;
const PLAN_CHUNK_LEN: usize = 16 * 1024;

/// Which files a content search reads, how much of each match it reports, and when it stops.
///
/// Every bound is absent by default, which searches every regular file the index holds that is not binary and returns
/// every match.
///
/// # Examples
///
/// ```
/// use kartta_fs::{Filter, GrepOptions};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let sources = Filter::new(["*.rs"])?;
/// let vendored = Filter::new(["**/vendor"])?;
/// let options = GrepOptions::default()
///     .filter(&sources)
///     .exclude(&vendored)
///     .context(2)
///     .limit(100);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct GrepOptions<'a> {
    /// The files to search, matched by their root-relative path. `None` searches every regular file.
    pub filter: Option<&'a Filter>,
    /// The files to leave out, matched by their root-relative path: a matched file is not searched, and a matched
    /// directory is not searched together with everything beneath it, as [`ListOptions::exclude`](crate::ListOptions)
    /// treats one. [`Matches::excluded`] counts the files it removed. `None` excludes nothing.
    pub exclude: Option<&'a Filter>,
    /// How many lines before and after each match to report beside it.
    pub context: usize,
    /// How many matching lines to return at most, spent in index order. Context lines do not count. `None` returns
    /// every match.
    pub limit: Option<NonZeroUsize>,
    /// Whether to keep searching once the limit fills, counting every match past it without returning one.
    ///
    /// Off by default, because proving what lies past the limit can cost the whole search over again. On, the answer
    /// carries [`Matches::total_lines`] and [`Matches::total_files`], and [`Matches::truncated`] is exact.
    pub count_all: bool,
    /// How many bytes of a line to report. A longer matching line is cut around its first match and a longer context
    /// line keeps its start; [`Line::cut`] says how much each lost. `None` reports lines whole.
    ///
    /// A cut never splits a character, and never leaves a line empty: a limit narrower than the character the kept
    /// text starts on keeps that one character whole, so a limit under four bytes may be exceeded by up to three. A
    /// matching line cut to nothing would report a match without the text that matched.
    pub max_line_bytes: Option<NonZeroUsize>,
    /// The largest file to search. A larger one is skipped unread and counted in [`Matches::oversized`]. `None`
    /// searches every size.
    ///
    /// Files are streamed, so a file's size costs no memory; a line's does, because the searcher holds a whole line at a
    /// time. A file holding a line longer than 256 MiB is therefore a [`Matches::failures`] entry whatever this is set
    /// to, rather than a buffer grown to the file's size.
    pub max_file_bytes: Option<NonZeroU64>,
    /// An outside stop signal, checked before each file. A search it stops reports what it covered, with
    /// [`Matches::cancelled`] set; which files that is depends on which threads checked the signal first, so two
    /// cancelled searches of one tree may cover different files.
    pub cancel: Option<&'a CancelToken>,
}

impl<'a> GrepOptions<'a> {
    /// Searches only the files `filter` selects.
    #[must_use]
    pub const fn filter(mut self, filter: &'a Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Leaves out every file `exclude` matches, and everything beneath a matched directory.
    #[must_use]
    pub const fn exclude(mut self, exclude: &'a Filter) -> Self {
        self.exclude = Some(exclude);
        self
    }

    /// Reports `context` lines before and after each match.
    #[must_use]
    pub const fn context(mut self, context: usize) -> Self {
        self.context = context;
        self
    }

    /// Returns at most `limit` matching lines. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = NonZeroUsize::new(limit);
        self
    }

    /// Counts every match past the limit.
    #[must_use]
    pub const fn count_all(mut self) -> Self {
        self.count_all = true;
        self
    }

    /// Reports at most `max` bytes of each line. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn max_line_bytes(mut self, max: usize) -> Self {
        self.max_line_bytes = NonZeroUsize::new(max);
        self
    }

    /// Skips files larger than `max` bytes. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn max_file_bytes(mut self, max: u64) -> Self {
        self.max_file_bytes = NonZeroU64::new(max);
        self
    }

    /// Stops the search once `cancel` is cancelled.
    #[must_use]
    pub const fn cancel(mut self, cancel: &'a CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

impl Index {
    /// Searches the content of indexed files for `patterns`, line by line, and answers in index order.
    ///
    /// A line matches when any pattern matches it. The answer lists the matched files in [`Index::entries`] order with
    /// their lines in file order, so it is the same on every run however the search was scheduled, and
    /// [`GrepOptions::limit`] takes the first matches in that order rather than the best. A cancellation is the
    /// exception: which files it turns away depends on which threads checked it first.
    ///
    /// Only regular files are searched. A symlink is reported, never read through, so one the filter selects is
    /// counted in [`Matches::symlinks`] and its target, when it lies inside the tree, is searched under its own path. A
    /// binary file is skipped, by the rule [`Index::is_binary`] applies: an extension in
    /// [`BuildOptions::binary_extensions`](crate::BuildOptions::binary_extensions) skips it unopened, and otherwise a
    /// NUL in its first 64 KiB does.
    ///
    /// [`Matches`] accounts for every candidate the search did not cover. The one thing it does not do by default is go
    /// looking for a match past the limit through files it has not read yet, which can cost the whole search over again;
    /// [`GrepOptions::count_all`] asks for that.
    ///
    /// **This reads the filesystem**: a candidate is opened when it is searched, and what is read is the file as it is
    /// then, not as it was at the walk. The size and the binary verdict each open learns are kept, as [`Index::size`]
    /// and [`Index::is_binary`] keep theirs, so a candidate whose kept verdict is binary, or whose kept size exceeds
    /// [`GrepOptions::max_file_bytes`], is skipped without being opened again, and one whose kept verdict is text is
    /// searched without its first 64 KiB being checked again: each file has the one verdict [`Index::is_binary`]
    /// answers, from the first read that settled it.
    ///
    /// An index built with [`BuildOptions::content_index`](crate::BuildOptions::content_index) opens fewer files: a
    /// candidate missing a trigram every match must contain is counted in [`Matches::ruled_out`] and never read. Which
    /// files hold a match is the same either way, as of the content the build read; a file changed since the build is
    /// ruled in or out by that content.
    ///
    /// The search runs on the index's threads, as many as [`Index::threads`] reports.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{BuildOptions, Case, GrepOptions, Index, Patterns};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "fn main() {\n    run();\n}\n")?;
    /// std::fs::write(dir.path().join("src/run.rs"), "pub fn run() {}\n")?;
    ///
    /// let index = Index::build(dir.path(), &BuildOptions::default())?.index;
    ///
    /// let patterns = Patterns::new([r"\brun\("], Case::Sensitive)?;
    /// let found = index.grep(&patterns, &GrepOptions::default());
    /// let mut paths = index.paths();
    /// let hits: Vec<(String, u64)> = found
    ///     .files()
    ///     .iter()
    ///     .flat_map(|file| {
    ///         let path = paths.get(file.position()).to_owned();
    ///         file.lines().iter().map(move |line| (path.clone(), line.number()))
    ///     })
    ///     .collect();
    /// assert_eq!(hits, [("src/main.rs".to_owned(), 2), ("src/run.rs".to_owned(), 1)]);
    /// assert!(!found.truncated() && found.unsearched() == 0, "a complete answer");
    ///
    /// let capped = index.grep(&patterns, &GrepOptions::default().limit(1));
    /// assert_eq!(capped.lines(), 1);
    /// assert!(capped.truncated(), "the second match was seen and turned away");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn grep(&self, patterns: &Patterns, options: &GrepOptions<'_>) -> Matches<'_> {
        self.on_threads(|| self.grep_on_current_threads(patterns, options))
    }

    fn grep_on_current_threads(
        &self,
        patterns: &Patterns,
        options: &GrepOptions<'_>,
    ) -> Matches<'_> {
        let narrowed = self.narrowed(patterns.requirement());
        let (runs, tally) = self.plan_grep(options, narrowed.as_ref());
        let mut fold = Fold::new(options, tally);
        let mut batches = runs.iter().flat_map(|run| run.chunks(BATCH));

        for batch in batches.by_ref() {
            if fold.settled() {
                fold.leave_unsearched(batch.len());
                break;
            }
            if options.cancel.is_some_and(CancelToken::is_cancelled) {
                fold.cancelled = true;
                fold.leave_unsearched(batch.len());
                break;
            }

            let budget = fold.budget();
            let scanned: Vec<Scanned> = batch
                .par_iter()
                .with_min_len(MIN_FILES_PER_SCANNER)
                .map_init(
                    || Scanner::new(self, patterns.matcher(), options),
                    |scanner, &at| scanner.scan(at as usize, budget),
                )
                .collect();

            for (&at, outcome) in batch.iter().zip(scanned) {
                fold.take(at as usize, outcome);
            }
        }

        let rest: usize = batches.map(<[u32]>::len).sum();
        fold.leave_unsearched(rest);
        fold.finish()
    }

    fn plan_grep(
        &self,
        options: &GrepOptions<'_>,
        narrowed: Option<&Narrowed>,
    ) -> (Vec<Vec<u32>>, Tally) {
        let planned: Vec<(Vec<u32>, Tally)> = (0..self.len())
            .into_par_iter()
            .step_by(PLAN_CHUNK_LEN)
            .map(|start| {
                let positions = start..self.len().min(start + PLAN_CHUNK_LEN);
                self.plan_chunk(positions, options, narrowed)
            })
            .collect();

        let mut tally = Tally::default();
        let runs = planned
            .into_iter()
            .map(|(run, part)| {
                tally.add(&part);
                run
            })
            .collect();

        (runs, tally)
    }

    fn plan_chunk(
        &self,
        positions: Range<usize>,
        options: &GrepOptions<'_>,
        narrowed: Option<&Narrowed>,
    ) -> (Vec<u32>, Tally) {
        let mut paths = self.paths();
        let mut planner = Planner {
            index: self,
            options,
            narrowed,
            excluded_dir: None,
            candidates: Vec::new(),
            tally: Tally::default(),
        };

        if let Some(exclude) = options.exclude
            && !positions.is_empty()
        {
            planner.excluded_dir = excluded_ancestor(paths.get(positions.start), exclude);
        }
        for at in positions {
            planner.plan(at, paths.get(at));
        }

        (planner.candidates, planner.tally)
    }
}

struct Planner<'a, 'o> {
    index: &'a Index,
    options: &'a GrepOptions<'o>,
    narrowed: Option<&'a Narrowed>,
    // constraint: index order is depth-first — once an entry is not beneath this directory, no later one is
    excluded_dir: Option<String>,
    candidates: Vec<u32>,
    tally: Tally,
}

impl Planner<'_, '_> {
    fn plan(&mut self, at: usize, path: &str) {
        let kind = self.index.kind_at(at);
        let beneath = self.beneath_excluded(path);

        match kind {
            EntryKind::Dir if !beneath && self.excludes(path) => {
                self.excluded_dir = Some(path.to_owned());
            }
            EntryKind::File if self.selects(path) => {
                if beneath || self.excludes(path) {
                    self.tally.excluded += 1;
                } else {
                    self.admit(at, path);
                }
            }
            EntryKind::Symlink if self.selects(path) => self.tally.symlinks += 1,
            _ => {}
        }
    }

    fn beneath_excluded(&mut self, path: &str) -> bool {
        let beneath = self
            .excluded_dir
            .as_deref()
            .is_some_and(|dir| is_below(path, dir));
        if !beneath {
            self.excluded_dir = None;
        }

        beneath
    }

    fn selects(&self, path: &str) -> bool {
        self.options
            .filter
            .is_none_or(|filter| filter.matches(path))
    }

    fn excludes(&self, path: &str) -> bool {
        self.options
            .exclude
            .is_some_and(|exclude| exclude.matches(path))
    }

    fn admit(&mut self, at: usize, path: &str) {
        self.tally.candidates += 1;

        let inspection = self.index.inspection();
        if self.index.binary_extensions().matches(path) || inspection.known_binary(at) == Some(true)
        {
            self.tally.binary += 1;
        } else if self.options.max_file_bytes.is_some_and(|max| {
            inspection
                .known_size(at)
                .is_some_and(|size| size > max.get())
        }) {
            self.tally.oversized += 1;
        } else if self.narrowed.is_some_and(|narrowed| !narrowed.admits(at)) {
            self.tally.ruled_out += 1;
        } else {
            self.candidates.push(narrow(at));
        }
    }
}

fn excluded_ancestor(path: &str, exclude: &Filter) -> Option<String> {
    path.match_indices('/')
        .map(|(end, _)| &path[..end])
        .find(|dir| exclude.matches(dir))
        .map(str::to_owned)
}

#[expect(
    clippy::expect_used,
    reason = "positions are the walk's node ids, which it keeps in a u32 and cannot run past"
)]
fn narrow(at: usize) -> u32 {
    u32::try_from(at).expect("a position fits the walk's u32 node ids")
}

struct Fold<'a> {
    limit: usize,
    context: u64,
    files: Vec<MatchedFile<'a>>,
    lines: usize,
    truncated: bool,
    total: Option<Total>,
    cancelled: bool,
    tally: Tally,
    failures: Vec<Failure>,
}

impl<'a> Fold<'a> {
    fn new(options: &GrepOptions<'_>, tally: Tally) -> Self {
        Self {
            limit: options.limit.map_or(usize::MAX, NonZeroUsize::get),
            context: options.context as u64,
            files: Vec::new(),
            lines: 0,
            truncated: false,
            total: options.count_all.then_some(Total::default()),
            cancelled: false,
            tally,
            failures: Vec::new(),
        }
    }

    const fn budget(&self) -> usize {
        self.limit - self.lines
    }

    const fn settled(&self) -> bool {
        self.proven() || (self.total.is_none() && self.lines == self.limit)
    }

    const fn proven(&self) -> bool {
        self.truncated && self.total.is_none()
    }

    const fn leave_unsearched(&mut self, count: usize) {
        self.tally.unsearched += count;
    }

    fn take(&mut self, at: usize, outcome: Scanned) {
        match outcome {
            Scanned::Searched(_) if self.proven() => self.tally.unsearched += 1,
            Scanned::Searched(found) => {
                self.tally.searched += 1;
                self.take_found(at, found);
            }
            Scanned::Binary => self.tally.binary += 1,
            Scanned::Oversized => self.tally.oversized += 1,
            Scanned::Failed(error) => self.failures.push(Failure {
                position: at,
                error,
            }),
            Scanned::Unsearched => {
                self.cancelled = true;
                self.tally.unsearched += 1;
            }
        }
    }

    fn take_found(&mut self, at: usize, mut found: Found) {
        let matched = found.kept + found.past;
        if let Some(total) = &mut self.total {
            total.lines += matched;
            total.files += usize::from(matched > 0);
        }

        let room = self.budget();
        if found.kept > room {
            truncate_to_matches(&mut found.lines, room, self.context);
            found.kept = room;
            self.truncated = true;
        } else if found.past > 0 {
            self.truncated = true;
        }

        self.lines += found.kept;
        if !found.lines.is_empty() {
            self.files.push(MatchedFile {
                position: at,
                lines: found.lines,
                index: PhantomData,
            });
        }
    }

    fn finish(self) -> Matches<'a> {
        Matches {
            files: self.files,
            lines: self.lines,
            truncated: self.truncated,
            total: self.total,
            cancelled: self.cancelled,
            tally: self.tally,
            failures: self.failures,
        }
    }
}

fn truncate_to_matches(lines: &mut Vec<Line>, kept: usize, context: u64) {
    let mut seen = 0;
    let Some(last) = lines.iter().position(|line| {
        seen += usize::from(line.kind == LineKind::Match);
        line.kind == LineKind::Match && seen == kept
    }) else {
        lines.clear();
        return;
    };

    let reach = lines[last].number.saturating_add(context);
    let end = lines[last + 1..]
        .iter()
        .position(|line| line.kind == LineKind::Match || line.number > reach)
        .map_or(lines.len(), |offset| last + 1 + offset);
    lines.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::BuildOptions;

    #[test]
    fn a_run_starting_inside_an_excluded_directory_excludes_from_its_first_entry() {
        // constraint: a planning run starts inside a directory only past PLAN_CHUNK_LEN entries, a tree too large to
        // build here, so the run is planned directly
        let dir = tempfile::tempdir().expect("create temp dir");
        for path in ["vendor/a.rs", "vendor/b.rs", "z.rs"] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("create directory");
            std::fs::write(full, "x").expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build")
            .index;
        let exclude = Filter::new(["vendor"]).expect("compile");
        let options = GrepOptions::default().exclude(&exclude);

        let (candidates, tally) = index.plan_chunk(2..4, &options, None);

        assert_eq!(
            index.paths().get(2),
            "vendor/b.rs",
            "the run starts inside vendor"
        );
        assert_eq!(candidates, [3]);
        assert_eq!(tally.excluded, 1);
    }

    #[test]
    fn the_shallowest_excluded_ancestor_is_the_one_found() {
        let exclude = Filter::new(["a", "a/b"]).expect("compile");

        assert_eq!(
            excluded_ancestor("a/b/c.rs", &exclude).as_deref(),
            Some("a")
        );
        assert_eq!(excluded_ancestor("x/b/c.rs", &exclude), None);
        assert_eq!(
            excluded_ancestor("a", &exclude),
            None,
            "an entry is not its own ancestor"
        );
    }

    #[test]
    fn a_file_scanned_after_the_limit_proved_the_cut_still_reports_what_it_was() {
        let mut fold = Fold::new(&GrepOptions::default().limit(1), Tally::default());
        fold.lines = 1;
        fold.truncated = true;
        let found = Found {
            lines: Vec::new(),
            kept: 1,
            past: 0,
        };

        fold.take(0, Scanned::Searched(found));
        fold.take(1, Scanned::Binary);
        fold.take(
            2,
            Scanned::Failed(std::io::ErrorKind::PermissionDenied.into()),
        );
        fold.take(3, Scanned::Unsearched);

        assert_eq!(
            fold.tally.unsearched, 2,
            "the searched file's lines are not in the answer, and the cancelled one was never read"
        );
        assert_eq!(fold.tally.searched, 0);
        assert_eq!(fold.tally.binary, 1);
        assert_eq!(
            fold.failures.len(),
            1,
            "a file that would not read is reported"
        );
        assert!(fold.cancelled, "the cancellation turned a file away");
    }
}
