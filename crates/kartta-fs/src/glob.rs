use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::Range;

use kartta_walk::EntryKind;

use crate::filter::Filter;
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

/// What [`Index::glob`] selected, and whether the limit turned another path away.
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
    truncated: bool,
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
    /// Settled by an actual rejection rather than inferred from a count, so an answer holding exactly the limit
    /// reports `false`. Inferring would get this wrong in the one case where a caller most needs it right.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// How many non-directory paths the filter selects that [`GlobOptions::exclude`] removed, those beneath a removed
    /// directory included. A removed directory is no match of its own, so it never counts.
    ///
    /// Counted up to where the answer ended, so an answer the limit cut short says nothing about exclusions past the
    /// cut. Counting beneath a removed directory matches the filter against each path there: that one match is what an
    /// exclusion does not save a glob, where a ranking, which counts nothing, skips the directory's subtree unmatched.
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
    /// Because nothing ranks, [`Capped::truncated`] is what tells a caller whether the limit turned a path away. A
    /// ranked search needs no such flag — its order already says the rest scored lower — but ten paths in index order
    /// look identical to the first ten of two hundred. It costs one extra match: the search pulls one path past the
    /// limit and reports whether it was there.
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
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn glob(&self, filter: &Filter, options: &GlobOptions<'_>) -> Capped<'_> {
        let limit = options.limit.map_or(usize::MAX, NonZeroUsize::get);
        let mut items = Vec::new();
        let mut truncated = false;
        let mut excluded = 0;
        let mut beneath_paths = self.paths();
        let selection = Selection {
            filter: Some(filter),
            exclude: options.exclude,
        };

        self.candidates(0..self.len(), selection, |candidate| {
            match candidate {
                Candidate::Selected(..) if items.len() == limit => {
                    truncated = true;
                    return false;
                }
                Candidate::Selected(at, _) => items.push(at),
                Candidate::Excluded => excluded += 1,
                Candidate::ExcludedBeneath(beneath) => {
                    excluded += self.selected_in(beneath, filter, &mut beneath_paths);
                }
            }
            true
        });

        Capped {
            items,
            truncated,
            excluded,
            index: PhantomData,
        }
    }

    fn selected_in(
        &self,
        positions: Range<usize>,
        filter: &Filter,
        paths: &mut Paths<'_>,
    ) -> usize {
        positions
            .filter(|&at| self.kind_at(at) != EntryKind::Dir && filter.matches(paths.get(at)))
            .count()
    }
}
