//! [`Capped`] — an answer a limit may have cut short, and the flag that says whether it did.

/// What a limited query produced: the items it returned, and whether the limit turned another one away.
///
/// A query carries this when its own order cannot explain what is missing. Ranked results need no such flag — asking
/// for the best few and getting them is a complete answer, and the ranking already says the rest scored lower. An
/// answer in index order says nothing of the kind, so a caller given ten paths has no way to tell ten from the first
/// ten of two hundred unless the answer says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capped<T> {
    items: Vec<T>,
    truncated: bool,
}

impl<T> Capped<T> {
    /// Records an answer that a limit did not cut short.
    pub(crate) const fn whole(items: Vec<T>) -> Self {
        Self {
            items,
            truncated: false,
        }
    }

    /// Records an answer a limit cut short, having turned at least one further item away.
    pub(crate) const fn cut(items: Vec<T>) -> Self {
        Self {
            items,
            truncated: true,
        }
    }

    /// What the query returned, in the order the query defines.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Takes the items, dropping the flag.
    ///
    /// Check [`Capped::truncated`] first: an answer that was cut short looks exactly like a whole one.
    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }

    /// How many items the answer holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the answer holds no items.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
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
