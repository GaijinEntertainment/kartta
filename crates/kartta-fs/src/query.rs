//! Queries over a built [`Index`].

use std::num::NonZeroUsize;
use std::ops::Range;

use kartta_fuzz::{Config as Scoring, Match, Matcher};
use kartta_walk::EntryKind;
use rayon::prelude::*;

use crate::capped::Capped;
use crate::filter::Filter;
use crate::index::Index;

/// The ranking weights, with the scorer's separator set narrowed to this crate's path grammar.
///
/// The scorer treats `\` as a segment separator too, which is right for a general matcher that may be handed a native
/// Windows path. An indexed path is never one: every path here is forward-slash form, on every platform, so `/` is the
/// only separator a candidate can carry and a `\` inside one is an ordinary character of a name. Left in, it would
/// split `main\x.rs` into two segments and hand the basename bonus to the wrong half of a single filename, which is
/// the ordering this crate exists to get right.
const SCORING: Scoring = scoring();

/// How many entries one parallel ranking task scores.
const RANKED_CHUNK: usize = 16 * 1024;

const fn scoring() -> Scoring {
    let mut scoring = Scoring::DEFAULT;
    scoring.segment_separators = &['/'];
    scoring
}

/// One ranked path and the score it earned.
///
/// The path is borrowed from the index, so a ranked list allocates one `Vec` and copies no path. The score is a rank
/// key and nothing more: it is comparable against other scores from the same query, and means nothing on its own.
///
/// This is deliberately this crate's own type rather than the scorer's. The ranking engine is an implementation
/// detail, and re-exporting its result type would make swapping it a breaking change for every caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathMatch<'a> {
    /// The matched path, root-relative and forward-slash separated, borrowed from the index.
    pub path: &'a str,
    /// Its rank key: higher is better.
    pub score: i32,
}

impl Index {
    /// Lists the paths `filter` selects, in index order.
    ///
    /// Nothing here ranks: a glob answers whether a path matches and has no notion of matching it better, so the order
    /// is [`Index::entries`]' own depth-first one and a `limit` takes the first paths in it rather than the best.
    /// `None` returns every path the filter selects. [`Filter`] carries the dialect the patterns are written in.
    ///
    /// Because nothing ranks, [`Capped::truncated`] is what tells a caller whether the limit turned a path away. A
    /// ranked query needs no such flag — its order already says the rest scored lower — but ten paths in index order
    /// look identical to the first ten of two hundred. It costs one extra match: the query pulls one path past the
    /// limit and reports whether it was there.
    ///
    /// Directories are not listed. A path query answers "which file did you mean", and a directory that matches would
    /// otherwise arrive alongside every file beneath it that matches for the same reason. Symlinks *are* listed: they
    /// are files as far as the tree is concerned, and are reported without being followed.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{Config, Filter, Index};
    /// # use std::num::NonZeroUsize;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "")?;
    /// std::fs::write(dir.path().join("Cargo.toml"), "")?;
    /// std::fs::write(dir.path().join("README.md"), "")?;
    ///
    /// let index = Index::build(dir.path(), &Config::default())?.into_index();
    ///
    /// let filter = Filter::new(["*.{rs,toml}"])?;
    /// let found = index.glob_paths(&filter, None);
    /// assert_eq!(found.items(), ["Cargo.toml", "src/main.rs"]);
    /// assert!(!found.truncated());
    ///
    /// let capped = index.glob_paths(&filter, NonZeroUsize::new(1));
    /// assert_eq!(capped.items(), ["Cargo.toml"]);
    /// assert!(capped.truncated(), "a second path matched and was turned away");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn glob_paths(&self, filter: &Filter, limit: Option<NonZeroUsize>) -> Capped<&str> {
        let Some(limit) = limit else {
            return Capped::whole(self.files(0..self.len(), Some(filter)).collect());
        };

        let mut found: Vec<&str> = self
            .files(0..self.len(), Some(filter))
            .take(limit.get() + 1)
            .collect();

        if found.len() > limit.get() {
            found.truncate(limit.get());

            return Capped::cut(found);
        }

        Capped::whole(found)
    }

    /// Ranks indexed paths against a fuzzy `query`, best first.
    ///
    /// A path matches when it contains every whitespace-separated term of the query as a subsequence, and ranks by how
    /// well those terms align against path structure — a hit inside a filename beats a hit inside a directory name, so
    /// `main` finds `src/main.rs` before `main/lib.rs`. An empty query matches everything in index order. `filter`
    /// narrows what is ranked before a path is scored, and `None` ranks every indexed file. `limit` caps the returned
    /// list; `None` returns every match.
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
    ///   than by aligning, so an over-long exact match still ranks first.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{Config, Filter, Index};
    /// # use std::num::NonZeroUsize;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src"))?;
    /// std::fs::create_dir_all(dir.path().join("main"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "")?;
    /// std::fs::write(dir.path().join("main/lib.rs"), "")?;
    ///
    /// let index = Index::build(dir.path(), &Config::default())?.into_index();
    ///
    /// let ranked = index.fuzzy_paths("main", None, None);
    /// let paths: Vec<&str> = ranked.iter().map(|found| found.path).collect();
    /// assert_eq!(paths, ["src/main.rs", "main/lib.rs"]);
    ///
    /// assert_eq!(index.fuzzy_paths("main", None, NonZeroUsize::new(1)).len(), 1);
    ///
    /// let in_src = Filter::new(["src/**"])?;
    /// let narrowed = index.fuzzy_paths("main", Some(&in_src), None);
    /// assert_eq!(narrowed.len(), 1);
    /// assert_eq!(narrowed[0].path, "src/main.rs");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn fuzzy_paths(
        &self,
        query: &str,
        filter: Option<&Filter>,
        limit: Option<NonZeroUsize>,
    ) -> Vec<PathMatch<'_>> {
        let rankings: Vec<Vec<Match<'_>>> = (0..self.len().div_ceil(RANKED_CHUNK))
            .into_par_iter()
            .map(|chunk| {
                let start = chunk * RANKED_CHUNK;
                let slice = start..self.len().min(start + RANKED_CHUNK);
                Matcher::new(SCORING).rank(self.files(slice, filter), query, limit)
            })
            .collect();

        Matcher::new(SCORING)
            .merge(rankings, query, limit)
            .into_iter()
            .map(|found| PathMatch {
                path: found.candidate,
                score: found.score,
            })
            .collect()
    }

    /// The paths a query answers from: every entry in `positions` that is not a directory, narrowed by `filter`.
    fn files(
        &self,
        positions: Range<usize>,
        filter: Option<&Filter>,
    ) -> impl Iterator<Item = &str> {
        self.kinded_paths(positions)
            .filter(|&(kind, _)| kind != EntryKind::Dir)
            .map(|(_, path)| path)
            .filter(move |path| filter.is_none_or(|filter| filter.matches(path)))
    }
}
