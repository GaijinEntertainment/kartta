//! The glob path search over a built [`Index`] — [`Index::glob`].

use std::num::NonZeroUsize;

use crate::capped::Capped;
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

impl Index {
    /// Lists the paths `filter` selects, in index order.
    ///
    /// Nothing here ranks: a glob answers whether a path matches and has no notion of matching it better, so the order
    /// is [`Index::entries`]' own depth-first one and [`GlobOptions::limit`] takes the first paths in it rather than
    /// the best. [`Filter`] carries the dialect the patterns are written in.
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
    /// use kartta_fs::{BuildOptions, Filter, GlobOptions, Index};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "")?;
    /// std::fs::write(dir.path().join("Cargo.toml"), "")?;
    /// std::fs::write(dir.path().join("README.md"), "")?;
    ///
    /// let index = Index::build(dir.path(), &BuildOptions::default())?.into_index();
    ///
    /// let filter = Filter::new(["*.{rs,toml}"])?;
    /// let found = index.glob(&filter, &GlobOptions::default());
    /// assert_eq!(found.items(), ["Cargo.toml", "src/main.rs"]);
    /// assert!(!found.truncated());
    ///
    /// let capped = index.glob(&filter, &GlobOptions::default().limit(1));
    /// assert_eq!(capped.items(), ["Cargo.toml"]);
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
    pub fn glob(&self, filter: &Filter, options: &GlobOptions) -> Capped<&str> {
        let Some(limit) = options.limit else {
            return Capped::whole(self.candidates(0..self.len(), Some(filter)).collect());
        };

        let mut found: Vec<&str> = self
            .candidates(0..self.len(), Some(filter))
            .take(limit.get() + 1)
            .collect();

        if found.len() > limit.get() {
            found.truncate(limit.get());

            return Capped::cut(found);
        }

        Capped::whole(found)
    }
}
