//! [`Policy`] — what a walk records and where it descends.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One listed directory entry. The name is kept beside the entry so a directory's listing sorts and searches by name
/// without touching the filesystem again.
#[derive(Debug)]
pub(crate) struct Listing {
    pub(crate) name: Box<str>,
    pub(crate) entry: fs::DirEntry,
}

/// Decides what a walk records and where it descends.
///
/// A policy is shared across the walker threads, so it must be [`Sync`]. Per-directory bookkeeping belongs in
/// [`Policy::State`], which the walk threads down the tree for you; a policy needing shared mutable state holds it
/// behind its own synchronization.
///
/// # Examples
///
/// A policy that keeps everything but refuses to descend into a directory named `vendor`:
///
/// ```
/// use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit};
///
/// struct SkipVendor;
///
/// impl Policy for SkipVendor {
///     type State = ();
///     type Node = EntryKind;
///
///     fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}
///
///     fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<EntryKind> {
///         let kind = entry.kind();
///
///         if kind == EntryKind::Dir && entry.name() == "vendor" {
///             // Listed, but opaque: `vendor` appears in the tree with no children.
///             return Visit::Prune(kind);
///         }
///
///         Visit::Keep(kind)
///     }
/// }
/// ```
pub trait Policy: Sync {
    /// State that applies to one directory's entries, derived per directory and inherited by its subdirectories.
    ///
    /// This is where an accumulating rule set lives — a `.gitignore` stack, an allow-list, a budget. One directory's
    /// state is shared by the threads walking each of its subtrees, hence [`Sync`].
    type State: Send + Sync;

    /// What the walk records for a kept entry, stored as [`Node::value`](crate::Node::value).
    type Node: Send;

    /// Derives the state that applies to `dir`'s entries, from the state its parent directory produced.
    ///
    /// Called once per directory, before any of its entries is classified, and given the directory's whole listing —
    /// so a policy can spot a control file such as `.gitignore` or `.git` without a syscall of its own. The walk root
    /// receives the initial state passed to [`walk`](crate::walk).
    fn enter(&self, dir: &Dir<'_>, parent: &Self::State) -> Self::State;

    /// Classifies one entry of a directory.
    fn visit(&self, entry: &Entry<'_>, state: &Self::State) -> Visit<Self::Node>;
}

/// What an entry is, read without following symlinks.
///
/// Exhaustive on purpose, so a `match` cannot silently miss a case the way [`std::fs::FileType`]'s separate predicates
/// can: all three of its `is_dir`, `is_file` and `is_symlink` answer `false` for a socket or a device node, which makes
/// "not a directory and not a file" a wrong way to spell "symlink".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EntryKind {
    /// A directory, and therefore the only kind a walk descends into.
    Dir,
    /// A regular file.
    File,
    /// A symlink, reported as itself whatever it points at — the walk neither follows nor descends one.
    Symlink,
    /// None of the above: a socket, fifo or device node on unix. [`Entry::file_type`] says which.
    Other,
}

impl EntryKind {
    /// Classifies a non-following file type. `Other` absorbs everything the three named kinds do not cover.
    fn of(file_type: fs::FileType) -> Self {
        if file_type.is_dir() {
            Self::Dir
        } else if file_type.is_file() {
            Self::File
        } else if file_type.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }
}

/// What a [`Policy`] decides for one entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visit<N> {
    /// Record the entry, and descend into it when it is a directory.
    Keep(N),
    /// Record the entry but never descend into it: the entry appears in the tree, its contents do not.
    Prune(N),
    /// Drop the entry. Nothing is recorded and nothing is descended.
    Skip,
}

/// The directory a [`Policy::enter`] call is deriving state for.
#[derive(Debug)]
pub struct Dir<'a> {
    path: &'a Path,
    rel: &'a str,
    depth: usize,
    listing: &'a [Listing],
}

impl<'a> Dir<'a> {
    pub(crate) const fn new(
        path: &'a Path,
        rel: &'a str,
        depth: usize,
        listing: &'a [Listing],
    ) -> Self {
        Dir {
            path,
            rel,
            depth,
            listing,
        }
    }

    /// The directory's own listing, for the walk's classification pass.
    pub(crate) const fn listing(&self) -> &'a [Listing] {
        self.listing
    }

    /// The directory's path as the walk reached it — the walk root joined with [`Dir::rel`].
    #[must_use]
    pub const fn path(&self) -> &'a Path {
        self.path
    }

    /// The directory's forward-slash path relative to the walk root. Empty for the root itself.
    #[must_use]
    pub const fn rel(&self) -> &'a str {
        self.rel
    }

    /// How far below the walk root this directory sits. `0` for the root itself.
    #[must_use]
    pub const fn depth(&self) -> usize {
        self.depth
    }

    /// The directory's entry names, sorted. Free to read: the walk has already listed the directory.
    pub fn names(&self) -> impl Iterator<Item = &'a str> {
        self.listing.iter().map(|item| item.name.as_ref())
    }

    /// Whether the directory holds an entry with this name. Binary search over the sorted listing — no syscall.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.listing
            .binary_search_by(|item| item.name.as_ref().cmp(name))
            .is_ok()
    }
}

/// One entry a [`Policy::visit`] call is classifying.
#[derive(Debug)]
pub struct Entry<'a> {
    listing: &'a Listing,
    rel: &'a str,
    depth: usize,
    file_type: fs::FileType,
}

impl<'a> Entry<'a> {
    pub(crate) const fn new(
        listing: &'a Listing,
        rel: &'a str,
        depth: usize,
        file_type: fs::FileType,
    ) -> Self {
        Entry {
            listing,
            rel,
            depth,
            file_type,
        }
    }

    /// The entry's own name, without any directory prefix.
    ///
    /// Lossily decoded, like [`Entry::rel`]. Use [`Entry::path`] to open the entry.
    #[must_use]
    pub fn name(&self) -> &'a str {
        self.listing.name.as_ref()
    }

    /// The entry's forward-slash path relative to the walk root.
    ///
    /// Lossily decoded: a name that is not valid UTF-8 renders replacement characters here. The path is a borrow of
    /// the walk's per-directory buffer, so a policy that keeps it must copy it.
    #[must_use]
    pub const fn rel(&self) -> &'a str {
        self.rel
    }

    /// The entry's path on disk, faithful even when [`Entry::name`] is lossy.
    ///
    /// Allocates. The walk does not need it, so it is built only when a policy asks.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.listing.entry.path()
    }

    /// How far below the walk root this entry sits. `1` for an entry of the root directory.
    #[must_use]
    pub const fn depth(&self) -> usize {
        self.depth
    }

    /// What the entry is, read without following symlinks.
    ///
    /// A symlink reports [`EntryKind::Symlink`] whatever it points at, so a link to a directory is not a directory here
    /// and is never descended.
    #[must_use]
    pub fn kind(&self) -> EntryKind {
        EntryKind::of(self.file_type)
    }

    /// The raw non-following file type, for a platform question [`EntryKind`] does not answer — telling a socket from a
    /// fifo through [`std::os::unix::fs::FileTypeExt`], say.
    #[must_use]
    pub const fn file_type(&self) -> fs::FileType {
        self.file_type
    }

    /// The entry's metadata, read without following symlinks — a symlink reports on itself, not on its target.
    ///
    /// This is the syscall a policy pays for a file's size or timestamps, and it is the policy's choice: the walk
    /// itself never asks.
    ///
    /// # Errors
    ///
    /// Propagates the underlying failure to stat the entry.
    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        self.listing.entry.metadata()
    }
}
