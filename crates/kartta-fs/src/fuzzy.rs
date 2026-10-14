use std::num::NonZeroUsize;

use kartta_fuzz::{Config, Matcher, Ranked};
use rayon::prelude::*;

use crate::filter::Filter;
use crate::index::Index;

// constraint: the scorer splits segments at `\` too by default, and in an indexed path `\` is a character of a name —
// left in, `main\x.rs` hands its basename bonus to `x.rs`
const RANKING: Config = ranking();
const RANK_CHUNK_LEN: usize = 16 * 1024;

const fn ranking() -> Config {
    let mut ranking = Config::DEFAULT;
    ranking.segment_separators = &['/'];
    ranking
}

/// One ranked entry and the score its path earned.
///
/// The entry is known by position, so a ranking allocates one `Vec` and copies no path; [`Index::paths`] reads the
/// path. The score is a rank key and nothing more: it is comparable against other scores from the same query, and means
/// nothing on its own.
///
/// This is deliberately this crate's own type rather than the scorer's. The ranking engine is an implementation
/// detail, and re-exporting its result type would make swapping it a breaking change for every caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Where the matched entry sits in [`Index::entries`].
    pub position: usize,
    /// Its rank key: higher is better.
    pub score: i32,
}

/// Which paths a fuzzy search ranks, and how many it returns.
///
/// Every bound is absent by default, which ranks every indexed file and returns every match.
///
/// # Examples
///
/// ```
/// use kartta_fs::{Filter, FuzzyOptions};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let best_ten = FuzzyOptions::default().limit(10);
///
/// let sources = Filter::new(["src/**"])?;
/// let in_sources = FuzzyOptions::default().filter(&sources);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct FuzzyOptions<'a> {
    /// The paths to rank. A path the filter does not select is never scored. `None` ranks every indexed file.
    pub filter: Option<&'a Filter>,
    /// How many paths to return at most. `None` returns every match.
    pub limit: Option<NonZeroUsize>,
}

impl<'a> FuzzyOptions<'a> {
    /// Ranks only the paths `filter` selects.
    #[must_use]
    pub const fn filter(mut self, filter: &'a Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Returns at most `limit` paths. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = NonZeroUsize::new(limit);
        self
    }
}

impl Index {
    /// Ranks indexed paths against a fuzzy `query`, best first.
    ///
    /// A path matches when it contains every whitespace-separated term of the query as a subsequence, and ranks by how
    /// well those terms align against path structure — a hit inside a filename beats a hit inside a directory name, so
    /// `main` finds `src/main.rs` before `main/lib.rs`. An empty query matches everything in index order.
    /// [`FuzzyOptions`] narrows what is ranked and caps what is returned.
    ///
    /// Directories are not ranked. A path search answers "which file did you mean", and a directory that matches would
    /// otherwise arrive alongside every file beneath it that matches for the same reason. Symlinks *are* ranked: they
    /// are files as far as the tree is concerned, and are reported without being followed.
    ///
    /// Ranking runs on the index's threads, as many as [`Index::threads`] reports.
    ///
    /// Matching ignores case. That is this crate's choice and not git's — `core.ignoreCase` describes what the
    /// filesystem does with case, which is a different question from what a search should do with it.
    ///
    /// # Where the ranking stops holding
    ///
    /// Two bounds come from the scorer and are stated here because they are visible through this method:
    ///
    /// - Case folding is ASCII-first. A non-ASCII character whose fold lands on ASCII does not match its ASCII
    ///   counterpart, so `k` does not find a path spelled with U+212A KELVIN SIGN.
    /// - A path longer than 1024 characters is not aligned. It still matches, so the file stays findable, but it takes
    ///   a floor score and orders below every path that was aligned, so the filename-over-directory preference does not
    ///   hold for it. A path a term matches *in full* is the exception: that is settled by comparing lengths rather
    ///   than by aligning, so an over-long full match still ranks first.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{BuildOptions, Filter, FuzzyOptions, Index};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src"))?;
    /// std::fs::create_dir_all(dir.path().join("main"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "")?;
    /// std::fs::write(dir.path().join("main/lib.rs"), "")?;
    ///
    /// let index = Index::build(dir.path(), &BuildOptions::default())?.into_index();
    ///
    /// let ranked = index.fuzzy("main", &FuzzyOptions::default());
    /// let mut paths = index.paths();
    /// let ranked_paths: Vec<String> = ranked.iter().map(|found| paths.get(found.position).to_owned()).collect();
    /// assert_eq!(ranked_paths, ["src/main.rs", "main/lib.rs"]);
    ///
    /// assert_eq!(index.fuzzy("main", &FuzzyOptions::default().limit(1)).len(), 1);
    ///
    /// let in_src = Filter::new(["src/**"])?;
    /// let narrowed = index.fuzzy("main", &FuzzyOptions::default().filter(&in_src));
    /// assert_eq!(narrowed.len(), 1);
    /// assert_eq!(paths.get(narrowed[0].position), "src/main.rs");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn fuzzy(&self, query: &str, options: &FuzzyOptions<'_>) -> Vec<FuzzyMatch> {
        let FuzzyOptions { filter, limit } = *options;
        let ranked: Vec<Ranked> = self.on_threads(|| {
            (0..self.len().div_ceil(RANK_CHUNK_LEN))
                .into_par_iter()
                .map(|chunk| {
                    let start = chunk * RANK_CHUNK_LEN;
                    let positions = start..self.len().min(start + RANK_CHUNK_LEN);
                    let mut matcher = Matcher::new(RANKING);
                    let mut ranking = matcher.ranking(query, limit);
                    self.candidates(positions, filter, |at, path| ranking.offer(at, path));
                    ranking.finish()
                })
                .reduce(Vec::new, |left, right| {
                    Matcher::merge_ranked([left, right], query, limit)
                })
        });

        ranked
            .into_iter()
            .map(|found| FuzzyMatch {
                position: found.position,
                score: found.score,
            })
            .collect()
    }
}
