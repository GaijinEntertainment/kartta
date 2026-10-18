use std::borrow::Cow;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::cancel::CancelToken;
use crate::error::WalkError;
use crate::policy::{Dir, Entry, EntryKind, ListedEntry, Origin, Policy, Visit};
use crate::tree::{Slot, Tree};

/// Settings for one walk. Every field defaults to `None` or `false`; construct with functional-update syntax
/// (`WalkOptions { listed_stats, ..Default::default() }`) — further fields may be added over time.
///
/// There is deliberately no cap on entries recorded. Truncating a walk hands back an arbitrary subset — which entries
/// survive depends on scheduling — that a consumer cannot tell from a complete tree without asking, so a guard against
/// large trees becomes a source of quietly wrong answers. A bound worth having would *fail* the walk instead.
/// Cancellation is safe only because the caller who cancelled is the one discarding the result.
#[derive(Clone, Debug, Default)]
pub struct WalkOptions {
    /// An outside stop signal, checked before each directory and before each entry. `None` never cancels.
    pub cancel_token: Option<CancelToken>,
    /// The forward-slash path, relative to the walk root, of the one directory to walk. `None`, or an empty path, walks
    /// the whole root. Empty and `.` components are dropped, so `/dir//./nested/` walks `dir/nested`; a `..` component
    /// fails the walk with [`WalkError::Subtree`], since what it names may lie outside the root.
    ///
    /// The walk lists only that directory and what lies beneath it, and reports every path and depth — [`Dir::path`],
    /// [`Entry::path`] and their `depth` — as a walk of the whole root would, so a policy sees the part exactly as it
    /// sees it in the whole. The tree holds the directory's contents, not the directory itself. A caller walking part
    /// of a tree again hands the state the directory's parent would have handed it as `initial`.
    pub subtree: Option<String>,
    /// Whether each directory is listed with its regular files' lengths and inode change times, which a policy then
    /// reads from [`Entry::listed_stat`] with no stat call of its own.
    ///
    /// Only macOS lists them with the names, through `getattrlistbulk`; elsewhere the listing carries none and
    /// [`Entry::listed_stat`] is `None`. Such a listing costs more than one without them, so a walk whose policy reads
    /// no file's stat leaves this `false`. A directory the kernel cannot list with them is listed without, so the tree
    /// is the one a walk without them records.
    pub listed_stats: bool,
}

/// Walks the tree rooted at `root`, recording every entry `policy` does not skip.
///
/// `initial` is the state the root directory inherits — the seed [`Policy::enter`] extends for the root's entries.
/// The walk runs on the rayon pool it is called from, so a caller that does not own the machine bounds its threads by
/// installing a pool of the size it may use around the call.
///
/// # Errors
///
/// Returns [`WalkError::ReadDir`] if `root` itself cannot be listed — or the directory [`WalkOptions::subtree`] names,
/// when it names one — or [`WalkError::Subtree`] if that path has a `..` component. Every deeper failure is collected
/// into [`WalkOutcome::errors`] instead, leaving a partial tree.
///
/// # Examples
///
/// ```
/// use kartta_walk::{Dir, Entry, Policy, Visit, WalkOptions, walk};
///
/// /// Records each entry's size, skipping anything hidden.
/// struct Sizes;
///
/// impl Policy for Sizes {
///     type State = ();
///     type Value = Option<u64>;
///
///     fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}
///
///     fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<Option<u64>> {
///         if entry.name().starts_with('.') {
///             return Visit::Skip;
///         }
///
///         Visit::Keep(entry.metadata().ok().map(|meta| meta.len()))
///     }
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// std::fs::write(dir.path().join("visible.txt"), "12345")?;
/// std::fs::write(dir.path().join(".hidden"), "x")?;
///
/// let outcome = walk(dir.path(), &Sizes, &(), &WalkOptions::default())?;
/// assert!(outcome.errors().is_empty() && !outcome.is_cancelled(), "a complete answer");
///
/// let tree = outcome.into_tree();
/// assert_eq!(tree.len(), 1, "the dotfile was skipped");
/// let only = tree.children().next().ok_or("the walk root holds one entry")?;
/// assert_eq!(only.name(), "visible.txt");
/// assert_eq!(*only.value(), Some(5));
/// # Ok(())
/// # }
/// ```
pub fn walk<P: Policy>(
    root: &Path,
    policy: &P,
    initial: &P::State,
    options: &WalkOptions,
) -> Result<WalkOutcome<P::Value>, WalkError> {
    let path = options
        .subtree
        .as_deref()
        .map(subtree_path)
        .transpose()?
        .unwrap_or_default();
    let (native_path, depth) = if path.is_empty() {
        (Cow::Borrowed(root), 0)
    } else {
        (Cow::Owned(root.join(&path)), path.split('/').count())
    };
    let list = lister(options);
    let listing = list(&native_path).map_err(|source| WalkError::ReadDir {
        native_path: native_path.to_path_buf(),
        source,
    })?;

    let walker = Walker {
        policy,
        options,
        list,
        stopped: AtomicBool::new(false),
    };

    let frame = Frame {
        native_path: &native_path,
        path: &path,
        depth,
        parent_state: initial,
    };

    let subtree = walker.walk_dir(&frame, listing);

    let counted = (subtree.node_count, subtree.name_bytes);
    let mut tree = Tree {
        names: String::with_capacity(subtree.name_bytes),
        slots: Vec::with_capacity(subtree.node_count),
    };
    let mut errors = Vec::new();
    flatten(subtree, &mut tree, &mut errors);
    debug_assert_eq!(
        (tree.slots.len(), tree.names.len()),
        counted,
        "the walk counted every recorded entry and name byte"
    );

    Ok(WalkOutcome {
        tree,
        errors,
        cancelled: walker.stopped.load(Ordering::Relaxed),
    })
}

/// What one walk produced: the tree, what it could not read, and whether it was cancelled.
///
/// The tree is the part worth keeping; the rest is a report about this particular run, which a consumer surfaces or
/// logs and then drops. [`WalkOutcome::into_tree`] takes the structure and leaves the report behind.
#[derive(Debug)]
pub struct WalkOutcome<V> {
    tree: Tree<V>,
    errors: Vec<WalkError>,
    cancelled: bool,
}

impl<V> WalkOutcome<V> {
    /// The tree the walk discovered.
    #[must_use]
    pub const fn tree(&self) -> &Tree<V> {
        &self.tree
    }

    /// Takes the tree, dropping the report.
    ///
    /// Check [`WalkOutcome::errors`] and [`WalkOutcome::is_cancelled`] first: a tree taken out of an incomplete walk
    /// looks exactly like a tree taken out of a complete one.
    #[must_use]
    pub fn into_tree(self) -> Tree<V> {
        self.tree
    }

    /// What the walk could not read, in the depth-first order of where it failed. A non-empty list means the tree is
    /// missing those entries or subtrees.
    #[must_use]
    pub fn errors(&self) -> &[WalkError] {
        &self.errors
    }

    /// Whether the walk was cancelled before it saw every entry the policy allowed.
    ///
    /// A cancelled tree holds whichever entries the threads had reached, which depends on scheduling — so it is
    /// something to report or discard, never to build on. No errors and no cancellation is the only complete answer.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Takes the tree *and* the report: the discovered tree, the collected failures, and whether the walk was
    /// cancelled.
    ///
    /// [`WalkOutcome::into_tree`] is the common case, for a consumer that surfaces the report and drops it. This is for
    /// one that has to keep it — an index that stores its build failures beside itself outlives the outcome, and a
    /// [`WalkError`] wraps [`std::io::Error`], so it can be moved but not copied out through
    /// [`errors`](WalkOutcome::errors).
    #[must_use]
    pub fn into_parts(self) -> (Tree<V>, Vec<WalkError>, bool) {
        (self.tree, self.errors, self.cancelled)
    }
}

struct Frame<'a, P: Policy> {
    native_path: &'a Path,
    path: &'a str,
    depth: usize,
    parent_state: &'a P::State,
}

struct Walker<'a, P: Policy> {
    policy: &'a P,
    options: &'a WalkOptions,
    list: fn(&Path) -> io::Result<Listing>,
    stopped: AtomicBool,
}

impl<P: Policy> Walker<'_, P> {
    fn stops_here(&self) -> bool {
        let cancelled = self
            .options
            .cancel_token
            .as_ref()
            .is_some_and(CancelToken::is_cancelled);
        if cancelled {
            self.stopped.store(true, Ordering::Relaxed);
        }

        cancelled
    }

    fn walk_dir(&self, frame: &Frame<'_, P>, listing: Listing) -> Subtree<P::Value> {
        let Listing { entries, errors } = listing;

        let dir = Dir::new(frame.native_path, frame.path, frame.depth, &entries);
        let state = self.policy.enter(&dir, frame.parent_state);

        let (mut subtree, descents) = self.classify(&dir, &state, errors);

        if descents.is_empty() {
            return subtree;
        }

        // constraint: rayon collects an indexed parallel iterator in input order, which keeps the tree identical
        // whatever the thread count and `subtrees` in the entry order `flatten` reads them in
        let subtrees: Vec<(usize, Subtree<P::Value>)> = descents
            .into_par_iter()
            .map(|descent| {
                let index = descent.index;

                (index, self.walk_child(descent, frame.depth + 1, &state))
            })
            .collect();

        for (_, child) in &subtrees {
            subtree.node_count += child.node_count;
            subtree.name_bytes += child.name_bytes;
        }
        subtree.subtrees = subtrees;

        subtree
    }

    fn classify(
        &self,
        dir: &Dir<'_>,
        state: &P::State,
        errors: Vec<WalkError>,
    ) -> (Subtree<P::Value>, Vec<Descent>) {
        let mut classified = Classified {
            subtree: Subtree {
                entries: Vec::with_capacity(dir.listing().len()),
                subtrees: Vec::new(),
                errors,
                node_count: 0,
                name_bytes: 0,
            },
            descents: Vec::new(),
        };

        let mut path = path_prefix(dir.path());
        let at = Classifying {
            dir,
            state,
            prefix_len: path.len(),
        };

        if dir.listing().len() >= PARALLEL_CLASSIFY {
            self.classify_in_parallel(&at, &path, &mut classified);
        } else {
            let decided = dir
                .listing()
                .iter()
                .map(|item| (item, self.decide(&at, &mut path, item)))
                .take_while(|(_, decided)| !matches!(decided, Decided::Stopped));
            for (item, decided) in decided {
                classified.record(item, decided);
            }
        }

        let Classified {
            mut subtree,
            descents,
        } = classified;
        subtree.entries.shrink_to_fit();
        subtree.node_count = subtree.entries.len();

        (subtree, descents)
    }

    fn classify_in_parallel(
        &self,
        at: &Classifying<'_, '_, P>,
        prefix: &str,
        classified: &mut Classified<P::Value>,
    ) {
        for run in at.dir.listing().chunks(CLASSIFY_RUN) {
            let decided: Vec<Decided<P::Value>> = run
                .par_iter()
                .map_init(
                    || prefix.to_owned(),
                    |path, item| self.decide(at, path, item),
                )
                .collect();
            let mut stopped = false;
            for (item, decided) in run.iter().zip(decided) {
                stopped |= matches!(decided, Decided::Stopped);
                classified.record(item, decided);
            }
            if stopped {
                return;
            }
        }
    }

    fn decide(
        &self,
        at: &Classifying<'_, '_, P>,
        prefixed_path: &mut String,
        item: &ListedEntry,
    ) -> Decided<P::Value> {
        if self.stops_here() {
            return Decided::Stopped;
        }

        let kind = match item.kind() {
            Ok(kind) => kind,
            Err(error) => return Decided::Failed(error),
        };

        prefixed_path.truncate(at.prefix_len);
        prefixed_path.push_str(&item.name);

        let entry = Entry::new(item, at.dir, prefixed_path, kind);
        let (value, descendable) = match self.policy.visit(&entry, at.state) {
            Visit::Skip => return Decided::Skipped,
            Visit::Prune(value) => (value, false),
            Visit::Keep(value) => (value, kind == EntryKind::Dir),
        };

        Decided::Recorded {
            value,
            descent: descendable.then(|| (entry.native_path(), prefixed_path.clone())),
        }
    }

    fn walk_child(
        &self,
        descent: Descent,
        depth: usize,
        parent_state: &P::State,
    ) -> Subtree<P::Value> {
        match (self.list)(&descent.native_path) {
            Ok(listing) => self.walk_dir(
                &Frame {
                    native_path: &descent.native_path,
                    path: &descent.path,
                    depth,
                    parent_state,
                },
                listing,
            ),
            Err(source) => Subtree {
                entries: Vec::new(),
                subtrees: Vec::new(),
                errors: vec![WalkError::ReadDir {
                    native_path: descent.native_path,
                    source,
                }],
                node_count: 0,
                name_bytes: 0,
            },
        }
    }
}

const PARALLEL_CLASSIFY: usize = 64;
const CLASSIFY_RUN: usize = 4096;

struct Classifying<'d, 's, P: Policy> {
    dir: &'d Dir<'d>,
    state: &'s P::State,
    prefix_len: usize,
}

struct Classified<V> {
    subtree: Subtree<V>,
    descents: Vec<Descent>,
}

enum Decided<V> {
    Stopped,
    Failed(WalkError),
    Skipped,
    Recorded {
        value: V,
        descent: Option<(PathBuf, String)>,
    },
}

impl<V> Classified<V> {
    fn record(&mut self, item: &ListedEntry, decided: Decided<V>) {
        match decided {
            Decided::Stopped | Decided::Skipped => {}
            Decided::Failed(error) => self.subtree.errors.push(error),
            Decided::Recorded { value, descent } => {
                if let Some((native_path, path)) = descent {
                    self.descents.push(Descent {
                        index: self.subtree.entries.len(),
                        native_path,
                        path,
                    });
                }
                self.subtree.name_bytes += item.name.len();
                self.subtree.entries.push(Recorded {
                    name: item.name.clone(),
                    value,
                });
            }
        }
    }
}

fn subtree_path(subtree: &str) -> Result<String, WalkError> {
    let mut path = String::with_capacity(subtree.len());
    for part in subtree.split('/').filter(|part| !matches!(*part, "" | ".")) {
        if part == ".." {
            return Err(WalkError::Subtree {
                subtree: subtree.to_owned(),
            });
        }
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
    }

    Ok(path)
}

fn path_prefix(path: &str) -> String {
    let mut prefix = String::with_capacity(path.len() + 1);
    prefix.push_str(path);
    if !prefix.is_empty() {
        prefix.push('/');
    }

    prefix
}

struct Descent {
    index: usize,
    native_path: PathBuf,
    path: String,
}

struct Subtree<V> {
    entries: Vec<Recorded<V>>,
    subtrees: Vec<(usize, Self)>,
    errors: Vec<WalkError>,
    node_count: usize,
    name_bytes: usize,
}

struct Recorded<V> {
    name: Box<str>,
    value: V,
}

struct Listing {
    entries: Vec<ListedEntry>,
    errors: Vec<WalkError>,
}

#[cfg(target_os = "macos")]
const fn lister(options: &WalkOptions) -> fn(&Path) -> io::Result<Listing> {
    if options.listed_stats {
        list_dir_with_stats
    } else {
        list_dir
    }
}

#[cfg(not(target_os = "macos"))]
const fn lister(_options: &WalkOptions) -> fn(&Path) -> io::Result<Listing> {
    list_dir
}

#[cfg(target_os = "macos")]
fn list_dir_with_stats(native_path: &Path) -> io::Result<Listing> {
    crate::bulk::list(native_path)?.map_or_else(
        || list_dir(native_path),
        |entries| Ok(Listing::sorted(entries, Vec::new())),
    )
}

fn list_dir(native_path: &Path) -> io::Result<Listing> {
    let mut entries = Vec::new();
    let mut errors = Vec::new();

    for entry in fs::read_dir(native_path)? {
        match entry {
            Ok(dir_entry) => entries.push(ListedEntry {
                name: dir_entry
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
                    .into_boxed_str(),
                origin: Origin::ReadDir(dir_entry),
            }),
            Err(source) => errors.push(WalkError::ReadEntry {
                native_path: native_path.to_path_buf(),
                source,
            }),
        }
    }

    Ok(Listing::sorted(entries, errors))
}

impl Listing {
    fn sorted(mut entries: Vec<ListedEntry>, errors: Vec<WalkError>) -> Self {
        // constraint: the sort is bytewise by name, and `kartta-fs` binary-searches the depth-first order it gives
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));

        Self { entries, errors }
    }
}

// shortcut: serial, so it bounds how much the parallel traversal can win — reserve each subtree's id range from its
// node count and fill the ranges in parallel once this shows up as the bottleneck on a large tree
fn flatten<V>(subtree: Subtree<V>, tree: &mut Tree<V>, errors: &mut Vec<WalkError>) {
    let Subtree {
        entries,
        subtrees,
        errors: mut own_errors,
        node_count: _,
        name_bytes: _,
    } = subtree;
    errors.append(&mut own_errors);

    let mut subtrees = subtrees.into_iter().peekable();

    for (index, Recorded { name, value }) in entries.into_iter().enumerate() {
        let child = subtrees
            .next_if(|&(at, _)| at == index)
            .map(|(_, child)| child);
        let descendant_count = child.as_ref().map_or(0, |child| child.node_count);
        tree.slots.push(Slot {
            name_start: arena_offset(tree.names.len()),
            subtree_end: arena_offset(tree.slots.len() + 1 + descendant_count),
            value,
        });
        tree.names.push_str(&name);

        if let Some(child) = child {
            flatten(child, tree, errors);
        }
    }
}

#[expect(
    clippy::expect_used,
    reason = "this needs a tree of over u32::MAX entries or name bytes — a load this crate is not designed to hold in \
              one arena regardless of offset width"
)]
fn arena_offset(len: usize) -> u32 {
    u32::try_from(len).expect("a walk records fewer than u32::MAX entries and name bytes")
}
