//! [`Capped`] — an answer a limit may have cut short, and the flag that says whether it did.

use std::marker::PhantomData;

use crate::index::Index;

/// What a limited query produced: the items it returned, and whether the limit turned another one away.
///
/// A query carries this when its own order cannot explain what is missing. Ranked results need no such flag — asking
/// for the best few and getting them is a complete answer, and the ranking already says the rest scored lower. An
/// answer in index order says nothing of the kind, so a caller given ten paths has no way to tell ten from the first
/// ten of two hundred unless the answer says so.
///
/// The answer borrows the index that gave it, because its items name entries by position, and a position names an
/// entry only in that index as it was when it answered. So it cannot outlive the index:
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
pub struct Capped<'a, T> {
    items: Vec<T>,
    truncated: bool,
    index: PhantomData<&'a Index>,
}

impl<T> Capped<'_, T> {
    /// Records an answer that a limit did not cut short.
    pub(crate) const fn whole(items: Vec<T>) -> Self {
        Self {
            items,
            truncated: false,
            index: PhantomData,
        }
    }

    /// Records an answer a limit cut short, having turned at least one further item away.
    pub(crate) const fn cut(items: Vec<T>) -> Self {
        Self {
            items,
            truncated: true,
            index: PhantomData,
        }
    }

    /// What the query returned, in the order the query defines.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Whether the limit turned an item away, so more exist than the answer shows.
    ///
    /// Settled by an actual rejection rather than inferred from a count, so an answer holding exactly the limit
    /// reports `false`. Inferring would get this wrong in the one case where a caller most needs it right.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}
