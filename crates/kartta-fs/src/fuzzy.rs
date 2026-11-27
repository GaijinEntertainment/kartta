use std::marker::PhantomData;
use std::num::NonZeroUsize;

use kartta_fuzz::{Ranked, Ranking};
use rayon::prelude::*;

use crate::filter::Filter;
use crate::index::{Candidate, Index, Selection};

pub(crate) const RANK_CHUNK_LEN: usize = 16 * 1024;

/// One ranked entry and the score its path earned.
///
/// The entry is known by position, so a ranking allocates one `Vec` and copies no path; [`Index::paths`] reads the
/// path. The score is a rank key and nothing more: it is comparable against other scores from the same query, and means
/// nothing on its own.
///
/// This is deliberately this crate's own type rather than the scorer's. The ranking engine is an implementation
/// detail, and re-exporting its result type would make swapping it a breaking change for every caller.
///
/// A match borrows the index that ranked it, because a position names an entry only in that index as it was when it
/// answered. So it cannot outlive the index:
///
/// ```compile_fail,E0597
/// use kartta_fs::{BuildOptions, FuzzyOptions, Index};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// let ranked = {
///     let index = Index::build(dir.path(), &BuildOptions::default())?.index;
///     index.fuzzy("main", &FuzzyOptions::default())
/// };
/// # drop(ranked);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyMatch<'a> {
    /// Where the matched entry sits in [`Index::entries`].
    pub position: usize,
    /// Its rank key: higher is better.
    pub score: i32,
    index: PhantomData<&'a Index>,
}

/// What [`Index::fuzzy`] ranked: the best matches, how many paths matched in all, and whether the limit left any out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatches<'a> {
    items: Vec<FuzzyMatch<'a>>,
    matched: Option<usize>,
    truncated: bool,
}

impl<'a> FuzzyMatches<'a> {
    /// The matches, best first, at most [`FuzzyOptions::limit`] of them.
    #[must_use]
    pub fn items(&self) -> &[FuzzyMatch<'a>] {
        &self.items
    }

    /// How many paths matched every term of the query, those the limit left out included.
    ///
    /// Only what the ranking scores counts: never a directory, a path the filter does not select or one the exclusion
    /// removes. The ranking counts as it scores, so this costs no pass of its own. `None` for an empty query, which
    /// matches every path in index order: under a limit it stops there without reading the rest, so counting them would
    /// be the pass the limit spares, and without one every match is already in [`items`](Self::items).
    #[must_use]
    pub const fn matched(&self) -> Option<usize> {
        self.matched
    }

    /// Whether the limit turned a match away.
    ///
    /// An actual rejection: an answer holding exactly the limit is complete and says `false`. An empty query, which
    /// [`matched`](Self::matched) does not count, stops at the first path past the limit, so this holds for it too.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
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
///
/// let vendored = Filter::new(["**/vendor"])?;
/// let without_vendored = FuzzyOptions::default().exclude(&vendored);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct FuzzyOptions<'a> {
    /// The paths to rank. A path the filter does not select is never scored. `None` ranks every indexed file.
    pub filter: Option<&'a Filter>,
    /// The paths to leave out, matched by their root-relative path: a matched file is never scored, and nothing beneath
    /// a matched directory is, as [`ListOptions::exclude`](crate::ListOptions) treats one. `None` excludes nothing.
    pub exclude: Option<&'a Filter>,
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

    /// Leaves out every path `exclude` matches, and everything beneath a matched directory.
    #[must_use]
    pub const fn exclude(mut self, exclude: &'a Filter) -> Self {
        self.exclude = Some(exclude);
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
    /// [`FuzzyOptions`] narrows what is ranked and caps what is returned, [`FuzzyMatches::matched`] counts every match,
    /// those past the cap included, and [`FuzzyMatches::truncated`] says whether the cap left one out.
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
    /// let index = Index::build(dir.path(), &BuildOptions::default())?.index;
    ///
    /// let ranked = index.fuzzy("main", &FuzzyOptions::default());
    /// let mut paths = index.paths();
    /// let ranked_paths: Vec<String> =
    ///     ranked.items().iter().map(|found| paths.get(found.position).to_owned()).collect();
    /// assert_eq!(ranked_paths, ["src/main.rs", "main/lib.rs"]);
    ///
    /// let best = index.fuzzy("main", &FuzzyOptions::default().limit(1));
    /// assert_eq!(best.items().len(), 1);
    /// assert_eq!(best.matched(), Some(2), "one of two matches");
    ///
    /// let in_src = Filter::new(["src/**"])?;
    /// let narrowed = index.fuzzy("main", &FuzzyOptions::default().filter(&in_src));
    /// assert_eq!(narrowed.items().len(), 1);
    /// assert_eq!(paths.get(narrowed.items()[0].position), "src/main.rs");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn fuzzy(&self, query: &str, options: &FuzzyOptions<'_>) -> FuzzyMatches<'_> {
        self.fuzzy_in_chunks(query, options, RANK_CHUNK_LEN)
    }

    fn fuzzy_in_chunks(
        &self,
        query: &str,
        options: &FuzzyOptions<'_>,
        chunk_len: usize,
    ) -> FuzzyMatches<'_> {
        let FuzzyOptions {
            filter,
            exclude,
            limit,
        } = *options;
        let selection = Selection { filter, exclude };
        let (ranked, matched): (Vec<Ranked>, usize) = self.on_threads(|| {
            (0..self.len())
                .into_par_iter()
                .step_by(chunk_len)
                .map(|start| {
                    let positions = start..self.len().min(start + chunk_len);
                    let mut ranking = Ranking::new(query, limit);
                    self.candidates(positions, selection, |found| offer(&mut ranking, &found));
                    let matched = ranking.matched();
                    (ranking.finish(), matched)
                })
                .reduce(
                    || (Vec::new(), 0),
                    |(left, left_matched), (right, right_matched)| {
                        (
                            Ranking::merge([left, right], query, limit),
                            left_matched + right_matched,
                        )
                    },
                )
        });

        FuzzyMatches {
            truncated: matched > ranked.len(),
            items: ranked
                .into_iter()
                .map(|found| FuzzyMatch {
                    position: found.position,
                    score: found.score,
                    index: PhantomData,
                })
                .collect(),
            matched: query.split_whitespace().next().is_some().then_some(matched),
        }
    }
}

fn offer(ranking: &mut Ranking, candidate: &Candidate<'_>) -> bool {
    match *candidate {
        Candidate::Selected(at, path) => ranking.offer(at, path),
        Candidate::Excluded | Candidate::ExcludedBeneath(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuildOptions;

    #[test]
    fn chunks_ranked_apart_answer_and_count_as_one_chunk_does() {
        // constraint: the ranking splits into chunks only past RANK_CHUNK_LEN entries, a tree too large to build here,
        // so the chunks are made small instead
        let dir = tempfile::tempdir().expect("create temp dir");
        for path in [
            "a/main.rs",
            "b/main.rs",
            "docs/main.md",
            "lib.rs",
            "main/lib.rs",
            "src/domain.rs",
            "src/main.rs",
            "vendor/deep/main.c",
            "vendor/main.rs",
        ] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("create directory");
            std::fs::write(full, "x").expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build")
            .index;
        let vendor = Filter::new(["vendor"]).expect("compile");

        for (query, limit) in ["main", "", "zz"]
            .iter()
            .flat_map(|&query| [0, 2].map(|limit| (query, limit)))
        {
            let options = FuzzyOptions::default().exclude(&vendor).limit(limit);
            let whole = index.fuzzy_in_chunks(query, &options, index.len());
            for chunk_len in 1..index.len() {
                assert_eq!(
                    index.fuzzy_in_chunks(query, &options, chunk_len),
                    whole,
                    "query {query:?}, limit {limit}, chunks of {chunk_len}"
                );
            }
        }

        let options = FuzzyOptions::default().exclude(&vendor).limit(2);
        let one_entry_each = index.fuzzy_in_chunks("main", &options, 1);
        assert_eq!(one_entry_each.items().len(), 2);
        assert_eq!(
            one_entry_each.matched(),
            Some(6),
            "every match outside vendor, lib.rs holding no main"
        );
        assert!(one_entry_each.truncated());

        let exact = FuzzyOptions::default().exclude(&vendor).limit(7);
        for chunk_len in 1..index.len() {
            assert!(
                !index.fuzzy_in_chunks("", &exact, chunk_len).truncated(),
                "the seven files outside vendor, in chunks of {chunk_len}"
            );
            let cut = exact.limit(6);
            assert!(
                index.fuzzy_in_chunks("", &cut, chunk_len).truncated(),
                "one of the seven past the limit, in chunks of {chunk_len}"
            );
        }
    }
}
