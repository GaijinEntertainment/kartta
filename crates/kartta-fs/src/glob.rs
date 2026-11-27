use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::Range;

use kartta_walk::EntryKind;
use rayon::prelude::*;

use crate::filter::Filter;
use crate::fuzzy::RANK_CHUNK_LEN;
use crate::index::{Candidate, Index, Paths, Selection};

/// What a glob search leaves out, and how many paths it returns.
///
/// Every bound is absent by default, which returns every path the filter selects.
///
/// # Examples
///
/// ```
/// use kartta_fs::{Filter, GlobOptions};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let first_twenty = GlobOptions::default().limit(20);
///
/// let vendored = Filter::new(["**/vendor"])?;
/// let without_vendored = GlobOptions::default().exclude(&vendored).limit(20);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct GlobOptions<'a> {
    /// How many paths to return at most. `None` returns all of them.
    pub limit: Option<NonZeroUsize>,
    /// The paths to leave out, matched by their root-relative path: a matched file is not returned, and nothing beneath
    /// a matched directory is, as [`ListOptions::exclude`](crate::ListOptions) treats one. [`Capped::excluded`] counts
    /// what this removed. `None` excludes nothing.
    pub exclude: Option<&'a Filter>,
}

impl<'a> GlobOptions<'a> {
    /// Returns at most `limit` paths. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = NonZeroUsize::new(limit);
        self
    }

    /// Leaves out every path `exclude` matches, and everything beneath a matched directory.
    #[must_use]
    pub const fn exclude(mut self, exclude: &'a Filter) -> Self {
        self.exclude = Some(exclude);
        self
    }
}

/// What [`Index::glob`] selected, how many paths it matched in all, and whether the limit turned one away.
///
/// The answer borrows the index that gave it, because it names entries by position, and a position names an entry
/// only in that index as it was when it answered. So it cannot outlive the index:
///
/// ```compile_fail,E0597
/// use kartta_fs::{BuildOptions, Filter, GlobOptions, Index};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// let filter = Filter::new(["*"])?;
/// let found = {
///     let index = Index::build(dir.path(), &BuildOptions::default())?.index;
///     index.glob(&filter, &GlobOptions::default())
/// };
/// # drop(found);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capped<'a> {
    items: Vec<usize>,
    matched: usize,
    excluded: usize,
    index: PhantomData<&'a Index>,
}

impl Capped<'_> {
    /// The positions of the selected paths, in index order.
    #[must_use]
    pub fn items(&self) -> &[usize] {
        &self.items
    }

    /// Whether the limit turned a path away, so more match than the answer shows.
    ///
    /// Settled by [`matched`](Self::matched), never by the answer holding as many paths as the limit, so an answer
    /// holding exactly the limit reports `false`.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.matched > self.items.len()
    }

    /// How many paths the filter selects outside [`GlobOptions::exclude`], those the limit turned away included.
    ///
    /// The search reads on past the limit to count them, matching the filter against every path left on the index's
    /// threads.
    #[must_use]
    pub const fn matched(&self) -> usize {
        self.matched
    }

    /// How many non-directory paths the filter selects that [`GlobOptions::exclude`] removed, those beneath a removed
    /// directory included. A removed directory is no match of its own, so it never counts.
    ///
    /// Counted over the whole index, past the limit included, as [`matched`](Self::matched) is. Counting beneath a
    /// removed directory matches the filter against each path there: that one match is what an exclusion does not save
    /// a glob, where a ranking, which counts nothing, skips the directory's subtree unmatched.
    #[must_use]
    pub const fn excluded(&self) -> usize {
        self.excluded
    }
}

impl Index {
    /// Returns the positions of the paths `filter` selects, in index order; [`Index::paths`] reads each path, cheaply
    /// in this order.
    ///
    /// Nothing here ranks: a glob answers whether a path matches and has no notion of matching it better, so the order
    /// is [`Index::entries`]' own depth-first one and [`GlobOptions::limit`] takes the first paths in it rather than
    /// the best. [`Filter`] carries the dialect the patterns are written in.
    ///
    /// Because nothing ranks, [`Capped::truncated`] and [`Capped::matched`] are what tell a caller whether the limit
    /// turned a path away and how many. A ranked search's order already says the rest scored lower, but ten paths in
    /// index order look identical to the first ten of two hundred. The count costs a match of every path past the
    /// limit, run on the index's threads, as many as [`Index::threads`] reports.
    ///
    /// [`GlobOptions::exclude`] removes paths before the limit sees them, so the limit is spent on what the exclusion
    /// leaves and [`Capped::truncated`] stays exact; [`Capped::excluded`] counts what it removed.
    ///
    /// Directories are not returned. A path search answers "which file did you mean", and a directory that matches
    /// would otherwise arrive alongside every file beneath it that matches for the same reason. Symlinks *are*
    /// returned: they are files as far as the tree is concerned, and are reported without being followed.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{BuildOptions, Filter, GlobOptions, Index};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "")?;
    /// std::fs::write(dir.path().join("Cargo.toml"), "")?;
    /// std::fs::write(dir.path().join("README.md"), "")?;
    ///
    /// let index = Index::build(dir.path(), &BuildOptions::default())?.index;
    ///
    /// let filter = Filter::new(["*.{rs,toml}"])?;
    /// let found = index.glob(&filter, &GlobOptions::default());
    /// let mut paths = index.paths();
    /// let found_paths: Vec<String> = found.items().iter().map(|&at| paths.get(at).to_owned()).collect();
    /// assert_eq!(found_paths, ["Cargo.toml", "src/main.rs"]);
    /// assert!(!found.truncated());
    ///
    /// let capped = index.glob(&filter, &GlobOptions::default().limit(1));
    /// assert_eq!(paths.get(capped.items()[0]), "Cargo.toml");
    /// assert!(capped.truncated(), "a second path matched and was turned away");
    /// assert_eq!(capped.matched(), 2);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn glob(&self, filter: &Filter, options: &GlobOptions<'_>) -> Capped<'_> {
        self.glob_in_chunks(filter, options, RANK_CHUNK_LEN)
    }

    fn glob_in_chunks(
        &self,
        filter: &Filter,
        options: &GlobOptions<'_>,
        chunk_len: usize,
    ) -> Capped<'_> {
        let limit = options.limit.map_or(usize::MAX, NonZeroUsize::get);
        let mut items = Vec::new();
        let mut turned_away = None;
        let mut excluded = 0;
        let mut beneath_paths = self.paths();
        let selection = Selection {
            filter: Some(filter),
            exclude: options.exclude,
        };

        self.candidates(0..self.len(), selection, |candidate| {
            match candidate {
                Candidate::Selected(at, _) if items.len() == limit => {
                    turned_away = Some(at);
                    return false;
                }
                Candidate::Selected(at, _) => items.push(at),
                Candidate::Excluded => excluded += 1,
                Candidate::ExcludedBeneath(beneath) => {
                    excluded += self.selected_in(beneath, selection.filter, &mut beneath_paths);
                }
            }
            true
        });

        let (matched_past, excluded_past) = turned_away.map_or((0, 0), |at| {
            self.tally(at..self.len(), selection, chunk_len)
        });
        Capped {
            matched: items.len() + matched_past,
            items,
            excluded: excluded + excluded_past,
            index: PhantomData,
        }
    }

    /// How many paths in `positions` `selection` selects, and how many its exclusion removed, counted in runs of
    /// `chunk_len` entries on the index's threads.
    fn tally(
        &self,
        positions: Range<usize>,
        selection: Selection<'_>,
        chunk_len: usize,
    ) -> (usize, usize) {
        self.on_threads(|| {
            positions
                .clone()
                .into_par_iter()
                .step_by(chunk_len)
                .map(|start| self.tally_run(start..positions.end.min(start + chunk_len), selection))
                .reduce(
                    || (0, 0),
                    |(left_matched, left_excluded), (right_matched, right_excluded)| {
                        (left_matched + right_matched, left_excluded + right_excluded)
                    },
                )
        })
    }

    fn tally_run(&self, run: Range<usize>, selection: Selection<'_>) -> (usize, usize) {
        let mut paths = self.paths();
        let (mut matched, mut excluded) = (0, 0);
        self.candidates(run, selection, |candidate| {
            match candidate {
                Candidate::Selected(..) => matched += 1,
                Candidate::Excluded => excluded += 1,
                Candidate::ExcludedBeneath(beneath) => {
                    excluded += self.selected_in(beneath, selection.filter, &mut paths);
                }
            }
            true
        });
        (matched, excluded)
    }

    fn selected_in(
        &self,
        positions: Range<usize>,
        filter: Option<&Filter>,
        paths: &mut Paths<'_>,
    ) -> usize {
        positions
            .filter(|&at| {
                self.kind_at(at) != EntryKind::Dir
                    && filter.is_none_or(|filter| filter.matches(paths.get(at)))
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuildOptions;

    #[test]
    fn chunks_counted_apart_count_as_one_chunk_does() {
        // constraint: the count past the limit splits into chunks only past RANK_CHUNK_LEN entries, a tree too large
        // to build here, so the chunks are made small instead
        let dir = tempfile::tempdir().expect("create temp dir");
        for path in [
            "a.rs",
            "b/c.rs",
            "b/d.md",
            "e.rs",
            "vendor/deep/f.rs",
            "vendor/g.rs",
            "z.rs",
        ] {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("create directory");
            std::fs::write(full, "x").expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build")
            .index;
        let sources = Filter::new(["*.rs"]).expect("compile");
        let vendor = Filter::new(["vendor"]).expect("compile");

        for limit in [0, 1, 3, 4] {
            let options = GlobOptions::default().exclude(&vendor).limit(limit);
            let whole = index.glob_in_chunks(&sources, &options, index.len());
            for chunk_len in 1..index.len() {
                assert_eq!(
                    index.glob_in_chunks(&sources, &options, chunk_len),
                    whole,
                    "limit {limit}, chunks of {chunk_len}"
                );
            }
        }

        let cut = index.glob_in_chunks(
            &sources,
            &GlobOptions::default().exclude(&vendor).limit(1),
            1,
        );
        assert_eq!(
            cut.matched(),
            4,
            "a.rs, b/c.rs, e.rs and z.rs, a chunk starting inside vendor counting nothing there"
        );
        assert_eq!(
            cut.excluded(),
            2,
            "vendor/deep/f.rs and vendor/g.rs, past the cut, each counted once whichever chunk it falls in"
        );
        assert!(cut.truncated());
    }
}
