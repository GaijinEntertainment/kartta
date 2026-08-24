//! [`Index`] — the snapshot one walk produces, and everything queries answer from.

use std::path::{Path, PathBuf};

use kartta_walk::{NodeId, Tree, WalkOptions, walk};

use crate::config::Config;
use crate::entry::Entry;
use crate::error::BuildError;
use crate::policy::{Indexing, Payload};
use crate::rules::Rules;

/// A snapshot of a directory tree: every entry the ignore rules kept, in the order a depth-first descent finds them.
///
/// Build one with [`Index::build`] and query it as often as you like — nothing here re-reads the filesystem, so an
/// index is a fixed answer about the moment it was built rather than a live view. Rebuild it to see later changes.
///
/// # Order
///
/// [`Index::entries`] is in depth-first order, which means a directory is immediately followed by everything beneath
/// it, and entries within one directory are name-sorted. That is the canonical order: a subtree is a contiguous run of
/// the list, which a bytewise path sort would not give (`a.txt` sorts between `a` and `a/b`).
///
/// # Examples
///
/// ```
/// use kartta_fs::{Config, Index};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// std::fs::write(dir.path().join("main.rs"), "fn main() {}")?;
/// std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n")?;
/// std::fs::write(dir.path().join("ignored.txt"), "x")?;
///
/// let outcome = Index::build(dir.path(), &Config::default())?;
/// assert!(outcome.errors().is_empty(), "a complete answer");
///
/// let index = outcome.into_index();
/// assert_eq!(index.entry("main.rs").and_then(|e| e.size()), Some(12));
/// assert!(index.entry("ignored.txt").is_none(), "the .gitignore applied");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Index {
    root: PathBuf,
    entries: Vec<Entry>,
    /// Positions into `entries`, ordered by path, so a lookup is a binary search and no path is stored twice.
    by_path: Vec<usize>,
}

impl Index {
    /// Walks `root` and builds the index.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError::Walk`] if `root` itself cannot be listed, or if the thread count
    /// [`Config::threads`] asks for cannot be started. Every deeper failure is collected into
    /// [`BuildOutcome::errors`] instead, leaving a partial index — check it before trusting the answer, because a
    /// partial index looks exactly like a complete one.
    pub fn build(root: impl AsRef<Path>, config: &Config) -> Result<BuildOutcome, BuildError> {
        let root = root.as_ref();
        let policy = Indexing::new(config.ignore, root);
        let options = WalkOptions {
            threads: config.threads,
            ..WalkOptions::default()
        };

        let (tree, walk_errors, _cancelled) =
            walk(root, &policy, &Rules::default(), &options)?.into_parts();

        let mut errors = policy.into_errors();
        errors.extend(walk_errors.into_iter().map(BuildError::Walk));
        order_report(&mut errors);

        Ok(BuildOutcome {
            index: Self::from_tree(root.to_path_buf(), &tree),
            errors,
        })
    }

    fn from_tree(root: PathBuf, tree: &Tree<Payload>) -> Self {
        let entries = flatten(tree);

        let mut by_path: Vec<usize> = (0..entries.len()).collect();
        by_path.sort_unstable_by(|&left, &right| entries[left].path.cmp(&entries[right].path));

        Self {
            root,
            entries,
            by_path,
        }
    }

    /// The directory the index was built from, as it was given to [`Index::build`].
    ///
    /// Join an entry's path onto this to address it on disk — see [`Entry::path`] for when that is also safe to open.
    /// This is the one path on the type that is the platform's own spelling rather than forward-slash form.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every indexed entry, in depth-first order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry at this root-relative forward-slash path, or `None` if nothing was indexed there.
    ///
    /// `None` does not mean the path does not exist: an ignored file, a file below an unreadable directory and a file
    /// created after the walk all answer the same way.
    #[must_use]
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        let found = self
            .by_path
            .binary_search_by(|&at| self.entries[at].path.as_ref().cmp(path))
            .ok()?;

        self.entries.get(self.by_path[found])
    }

    /// How many entries the index holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index holds no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Puts the report in an order two builds of one tree agree on.
///
/// The walk collects failures as its threads reach them, so the order they arrive in is the scheduler's. An index that
/// is identical across builds is worth little beside a report that is not.
fn order_report(errors: &mut [BuildError]) {
    errors.sort_by(|left, right| {
        left.path()
            .cmp(&right.path())
            .then_with(|| left.kind_order().cmp(&right.kind_order()))
    });
}

/// One directory being expanded during the depth-first pass: the children left to visit, and how much of the shared
/// path buffer this directory's own path occupies.
struct Frame<'a> {
    children: &'a [NodeId],
    next: usize,
    prefix: usize,
}

/// Turns the walk's tree into the flat depth-first entry list, building each full path once.
///
/// One buffer carries the path of the entry being visited, truncated back to the parent's length on the way down a
/// sibling: every node under one directory shares that directory's prefix, so the bytes below the truncation point are
/// always already correct. The stack holds one frame per level, not one per pending node.
fn flatten(tree: &Tree<Payload>) -> Vec<Entry> {
    let mut entries = Vec::with_capacity(tree.len());
    let mut path = String::new();
    let mut stack = vec![Frame {
        children: tree.roots(),
        next: 0,
        prefix: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        let Some(&id) = frame.children.get(frame.next) else {
            stack.pop();
            continue;
        };
        frame.next += 1;
        let prefix = frame.prefix;

        let node = tree.node(id);
        path.truncate(prefix);
        if prefix != 0 {
            path.push('/');
        }
        path.push_str(node.name());

        entries.push(Entry {
            path: path.as_str().into(),
            kind: node.value().kind,
            size: node.value().size,
        });

        if !node.children().is_empty() {
            stack.push(Frame {
                children: node.children(),
                next: 0,
                prefix: path.len(),
            });
        }
    }

    entries
}

/// What one build produced: the index, and what the build could not read.
///
/// The index is the part worth keeping; the errors are a report about this particular build, which a consumer surfaces
/// or logs and then drops. [`BuildOutcome::into_index`] takes the snapshot and leaves the report behind.
#[derive(Debug)]
pub struct BuildOutcome {
    index: Index,
    errors: Vec<BuildError>,
}

impl BuildOutcome {
    /// The index the build produced.
    #[must_use]
    pub const fn index(&self) -> &Index {
        &self.index
    }

    /// Takes the index, dropping the report.
    ///
    /// Check [`BuildOutcome::errors`] first: an index built from an incomplete walk looks exactly like one built from
    /// a complete walk.
    #[must_use]
    pub fn into_index(self) -> Index {
        self.index
    }

    /// What the build could not read, ordered by path so two builds of one tree report identically.
    ///
    /// A non-empty list means the index is wrong in a way it cannot detect, and each kind is wrong differently:
    ///
    /// - [`BuildError::Walk`] — the entry or the whole subtree it names is missing.
    /// - [`BuildError::Ignore`] — the rules went unread, so the index *holds* whatever that file would have hidden.
    ///   The only kind that makes an index wrong by addition, and the only one no later check can catch.
    /// - [`BuildError::Metadata`] — the entry is indexed, without a size.
    /// - [`BuildError::Name`] — the entry is missing, and a directory takes its subtree with it.
    #[must_use]
    pub fn errors(&self) -> &[BuildError] {
        &self.errors
    }
}
