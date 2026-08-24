//! Queries over a built [`Index`].

use std::num::NonZeroUsize;

use kartta_fuzz::{Config as Scoring, Matcher};
use kartta_walk::EntryKind;

use crate::entry::Entry;
use crate::index::Index;

/// The ranking weights, with the scorer's separator set narrowed to this crate's path grammar.
///
/// The scorer treats `\` as a segment separator too, which is right for a general matcher that may be handed a native
/// Windows path. An indexed path is never one: every path here is forward-slash form, on every platform, so `/` is the
/// only separator a candidate can carry and a `\` inside one is an ordinary character of a name. Left in, it would
/// split `main\x.rs` into two segments and hand the basename bonus to the wrong half of a single filename, which is
/// the ordering this crate exists to get right.
const SCORING: Scoring = scoring();

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
    /// Ranks indexed paths against a fuzzy `query`, best first.
    ///
    /// A path matches when it contains every whitespace-separated term of the query as a subsequence, and ranks by how
    /// well those terms align against path structure — a hit inside a filename beats a hit inside a directory name, so
    /// `main` finds `src/main.rs` before `main/lib.rs`. An empty query matches everything in index order. `limit` caps
    /// the returned list; `None` returns every match.
    ///
    /// Directories are not ranked. A path query answers "which file did you mean", and a directory that matches would
    /// otherwise arrive alongside every file beneath it that matches for the same reason. Symlinks *are* ranked: they
    /// are files as far as the tree is concerned, and are reported without being followed.
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
    /// use kartta_fs::{Config, Index};
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
    /// let ranked = index.fuzzy_paths("main", None);
    /// let paths: Vec<&str> = ranked.iter().map(|found| found.path).collect();
    /// assert_eq!(paths, ["src/main.rs", "main/lib.rs"]);
    ///
    /// assert_eq!(index.fuzzy_paths("main", NonZeroUsize::new(1)).len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn fuzzy_paths(&self, query: &str, limit: Option<NonZeroUsize>) -> Vec<PathMatch<'_>> {
        let mut matcher = Matcher::new(SCORING);
        let candidates = self
            .entries()
            .iter()
            .filter(|entry| entry.kind() != EntryKind::Dir)
            .map(Entry::path);

        matcher
            .match_list(candidates, query, limit)
            .into_iter()
            .map(|found| PathMatch {
                path: found.candidate,
                score: found.score,
            })
            .collect()
    }
}
