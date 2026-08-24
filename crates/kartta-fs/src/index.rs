//! [`Index`] — the snapshot one walk produces, and everything queries answer from.

use std::path::{Path, PathBuf};

use kartta_walk::{EntryKind, NodeId, Tree, WalkOptions, walk};

use crate::config::Config;
use crate::entry::Entry;
use crate::error::BuildError;
use crate::inspection::Inspection;
use crate::policy::{Indexing, Payload};
use crate::rules::Rules;

/// A snapshot of a directory tree: every entry the ignore rules kept, in the order a depth-first descent finds them.
///
/// Build one with [`Index::build`] and query it as often as you like. The structure is fixed at the moment of the
/// walk — which entries exist, what each one is, where a symlink points — and rebuilding is the only way to see a
/// later change to it.
///
/// A file's own contents are not part of that snapshot. [`Index::size`] reads the filesystem on the first ask for an
/// entry and keeps the answer, because stat'ing a whole tree during the walk costs about half a build and most of
/// those sizes are never looked at. So structure and contents belong to different moments, and only the structure is
/// coherent across entries.
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
/// assert_eq!(index.size("main.rs"), Some(12));
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
    /// What the walk did not read, filled by the queries that need it.
    inspection: Inspection,
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
            inspection: Inspection::new(entries.len()),
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
        self.entries.get(self.position(path)?)
    }

    /// Where the entry at this root-relative forward-slash path sits in [`Index::entries`], or `None` if nothing was
    /// indexed there.
    ///
    /// The position is what a query needs to reach an entry's neighbours: [`Index::entries`] is depth-first, so a
    /// directory's subtree is the run that starts immediately after it.
    pub(crate) fn position(&self, path: &str) -> Option<usize> {
        let found = self
            .by_path
            .binary_search_by(|&at| self.entries[at].path.as_ref().cmp(path))
            .ok()?;

        self.by_path.get(found).copied()
    }

    /// The size in bytes of the regular file at this root-relative forward-slash path.
    ///
    /// `None` for a path nothing is indexed at, for every kind but a regular file, where a size would not mean
    /// anything, and for a file that would not stat. `Some(0)` is a genuinely empty file and is
    /// never a stand-in for an unknown size.
    ///
    /// **This reads the filesystem**, on the first ask for a given entry, and keeps the answer — so a second ask
    /// costs nothing and a listing that is drawn twice is stat'd once. The walk does not collect sizes, because
    /// stat'ing every file of a large tree is roughly half of what a build costs and most of those sizes are never
    /// looked at.
    ///
    /// What that buys is paid for in coherence. A size belongs to the moment it was first asked for rather than to
    /// the walk, so two sizes from one index may describe two different moments, and neither is refreshed once taken.
    /// A symlink that replaced the file since the walk is measured as the link, never followed.
    #[must_use]
    pub fn size(&self, path: &str) -> Option<u64> {
        let at = self.position(path)?;

        if self.entries.get(at)?.kind() != EntryKind::File {
            return None;
        }

        self.inspection.size(at, &self.root.join(path))
    }

    /// Whether the regular file at this root-relative forward-slash path holds binary content.
    ///
    /// `None` for a path nothing is indexed at, for every kind but a regular file, and for a file whose name settles
    /// nothing and which then would not open or would not read.
    ///
    /// Two rules settle it, in order. An extension naming a format that is binary by construction answers `Some(true)`
    /// with no file opened at all — which is most of the weight on a real tree, because those are the archives, media,
    /// disk images and model weights. That answer is about the *name*, so it stands even for a file since deleted or
    /// replaced: opening one to confirm would spend on every such entry the very syscall the rule exists to save.
    ///
    /// A name that settles nothing is answered by whether a NUL appears in the file's first 64 KiB, the rule git and
    /// ripgrep both apply. That verdict is about a prefix rather than about a whole file: a file that is text for
    /// 64 KiB and binary after reports text here.
    ///
    /// **This reads the filesystem**, on the first ask for a given entry, and keeps the answer — see [`Index::size`]
    /// for what taking an answer at ask time rather than at walk time costs in coherence.
    #[must_use]
    pub fn is_binary(&self, path: &str) -> Option<bool> {
        let at = self.position(path)?;

        if self.entries.get(at)?.kind() != EntryKind::File {
            return None;
        }

        self.inspection.is_binary(at, path, &self.root.join(path))
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
            target: node.value().target.clone(),
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
    /// - [`BuildError::Name`] — the entry is missing, and a directory takes its subtree with it.
    #[must_use]
    pub fn errors(&self) -> &[BuildError] {
        &self.errors
    }
}
