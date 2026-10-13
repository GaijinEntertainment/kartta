//! The glob path search over a built [`Index`] — [`Index::glob_paths`].

use std::num::NonZeroUsize;

use crate::capped::Capped;
use crate::filter::Filter;
use crate::index::Index;

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
    /// use kartta_fs::{BuildOptions, Filter, Index};
    /// # use std::num::NonZeroUsize;
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
}
