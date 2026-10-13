//! The fuzzy path search over a built [`Index`] — [`Index::fuzzy`].

use std::num::NonZeroUsize;

use kartta_fuzz::{Config, Match, Matcher};
use rayon::prelude::*;

use crate::filter::Filter;
use crate::index::Index;

/// The scorer's configuration for indexed paths: its defaults, with the separator set narrowed to this crate's path
/// grammar.
///
/// The scorer treats `\` as a segment separator too, which is right for a general matcher that may be handed a native
/// Windows path. An indexed path is never one: every path here is forward-slash form, on every platform, so `/` is the
/// only separator a candidate can carry and a `\` inside one is an ordinary character of a name. Left in, it would
/// split `main\x.rs` into two segments and hand the basename bonus to the wrong half of a single filename, which is
/// the ordering this crate exists to get right.
const RANKING: Config = ranking();

/// How many entries one parallel ranking task scores.
const RANK_CHUNK_LEN: usize = 16 * 1024;

const fn ranking() -> Config {
    let mut ranking = Config::DEFAULT;
    ranking.segment_separators = &['/'];
    ranking
}

/// One ranked path and the score it earned.
///
/// The path is borrowed from the index, so a ranked list allocates one `Vec` and copies no path. The score is a rank
/// key and nothing more: it is comparable against other scores from the same query, and means nothing on its own.
///
/// This is deliberately this crate's own type rather than the scorer's. The ranking engine is an implementation
/// detail, and re-exporting its result type would make swapping it a breaking change for every caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyMatch<'a> {
    /// The matched path, root-relative and forward-slash separated, borrowed from the index.
    pub path: &'a str,
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
    /// Directories are not ranked. A path query answers "which file did you mean", and a directory that matches would
    /// otherwise arrive alongside every file beneath it that matches for the same reason. Symlinks *are* ranked: they
    /// are files as far as the tree is concerned, and are reported without being followed.
    ///
    /// Ranking runs on the ambient rayon thread pool; call this inside `ThreadPool::install` to bound the threads it
    /// takes.
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
    /// let paths: Vec<&str> = ranked.iter().map(|found| found.path).collect();
    /// assert_eq!(paths, ["src/main.rs", "main/lib.rs"]);
    ///
    /// assert_eq!(index.fuzzy("main", &FuzzyOptions::default().limit(1)).len(), 1);
    ///
    /// let in_src = Filter::new(["src/**"])?;
    /// let narrowed = index.fuzzy("main", &FuzzyOptions::default().filter(&in_src));
    /// assert_eq!(narrowed.len(), 1);
    /// assert_eq!(narrowed[0].path, "src/main.rs");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn fuzzy(&self, query: &str, options: &FuzzyOptions<'_>) -> Vec<FuzzyMatch<'_>> {
        let FuzzyOptions { filter, limit } = *options;
        let rankings: Vec<Vec<Match<'_>>> = (0..self.len().div_ceil(RANK_CHUNK_LEN))
            .into_par_iter()
            .map(|chunk| {
                let start = chunk * RANK_CHUNK_LEN;
                let positions = start..self.len().min(start + RANK_CHUNK_LEN);
                Matcher::new(RANKING).rank(self.candidates(positions, filter), query, limit)
            })
            .collect();

        Matcher::new(RANKING)
            .merge(rankings, query, limit)
            .into_iter()
            .map(|found| FuzzyMatch {
                path: found.candidate,
                score: found.score,
            })
            .collect()
    }
}
