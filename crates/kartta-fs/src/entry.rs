pub use kartta_walk::EntryKind;

/// One entry of the index: where it is, what it is, and where a symlink points.
///
/// The index keeps paths front-coded, so an entry owns the path rebuilt for it; a symlink's target is borrowed from its
/// [`Index`](crate::Index). A pass that only reads paths reads them through [`Index::paths`](crate::Index::paths)
/// rather than building an entry per path.
///
/// The path is root-relative and forward-slash separated on every platform, and it is the entry's identity —
/// [`Index::entry`](crate::Index::entry) looks one up by exactly this string. To *open* the entry, join it onto
/// [`Index::root`](crate::Index::root); the join is where the platform's own separator comes back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entry<'a> {
    pub(crate) path: Box<str>,
    pub(crate) kind: EntryKind,
    pub(crate) target: Option<&'a str>,
}

impl<'a> Entry<'a> {
    /// The entry's forward-slash path relative to the index root.
    ///
    /// It addresses this directory entry and no other: joining it onto [`Index::root`](crate::Index::root) names the
    /// entry the walk recorded, and a name that could not survive that round trip is never indexed at all — it is
    /// reported through [`BuildError::Name`](crate::BuildError::Name) instead.
    ///
    /// *Addressing* an entry is not *opening* it. An ordinary open follows symlinks, so for
    /// [`EntryKind::Symlink`] the joined path opens the link's target — possibly outside the tree, possibly a second
    /// copy of a file already indexed under its own path. Only a [`EntryKind::File`] is safe to open by joining;
    /// [`kind`](Entry::kind) is what tells the two apart, and a caller that must open a symlink as itself needs a
    /// non-following open of its own.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The entry's own name — everything after the last `/`, or the whole path for a root-level entry.
    #[must_use]
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// What the entry is, read without following symlinks.
    ///
    /// A symlink reports [`EntryKind::Symlink`] whatever it points at, so a link to a directory is not a directory
    /// here and has no indexed children.
    #[must_use]
    pub const fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Where a symlink points, exactly as the link stores it.
    ///
    /// This is a read *of* the link, not *through* it: the string is the target the filesystem holds, and nothing here
    /// resolves it. It is not an index path and does not follow this crate's path conventions — a relative target is
    /// relative to the link's own directory, it keeps whatever separators the link was created with, and it may name a
    /// place outside the tree or nothing at all. Resolving one is the caller's to do, against
    /// [`Index::root`](crate::Index::root) and the link's own directory.
    ///
    /// `None` for every kind but [`EntryKind::Symlink`], and for a symlink whose target could not be read or is not
    /// valid UTF-8; [`kind`](Entry::kind) tells the two apart. A symlink is a view onto somewhere else, so a view that
    /// resolves to nothing is still the entry the tree holds rather than a failure to index one, and no build reports
    /// it.
    ///
    /// Unlike [`Index::size`](crate::Index::size), this is walk-time data: it is read once, with the structure, and
    /// belongs to the same moment as the rest of the tree.
    #[must_use]
    pub const fn target(&self) -> Option<&'a str> {
        self.target
    }
}
