//! The glob path search over a built [`Index`] — [`Index::glob`].

use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::filter::Filter;
use crate::index::Index;

/// How many paths a glob search returns.
///
/// Every bound is absent by default, which returns every path the filter selects.
///
/// # Examples
///
/// ```
/// use kartta_fs::GlobOptions;
///
/// let first_twenty = GlobOptions::default().limit(20);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct GlobOptions {
    /// How many paths to return at most. `None` returns all of them.
    pub limit: Option<NonZeroUsize>,
}

impl GlobOptions {
    /// Returns at most `limit` paths. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = NonZeroUsize::new(limit);
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
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "every query takes its options by reference, as list and fuzzy do; GlobOptions is one word only until \
                  it grows a second bound"
    )]
    pub fn glob(&self, filter: &Filter, options: &GlobOptions) -> Capped<'_> {
        let limit = options.limit.map_or(usize::MAX, NonZeroUsize::get);
        let mut items = Vec::new();
        let mut truncated = false;

        self.candidates(0..self.len(), Some(filter), |at, _| {
            if items.len() == limit {
                truncated = true;
                return false;
            }
            items.push(at);
            true
        });

        Capped {
            items,
            truncated,
            index: PhantomData,
        }
    }
}
