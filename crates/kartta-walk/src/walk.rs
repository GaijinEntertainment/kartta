//! The traversal: read a directory, ask the policy, walk the surviving subdirectories in parallel.
//!
//! Each directory is read once and its entries sorted by name, so the policy sees a stable listing and subtree results
//! collect in that same order — the parallelism never reaches the result. Subtrees stay nested while the walk runs and
//! are flattened into the arena once at the end, so no node is ever copied or renumbered twice.

use std::fs;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::cancel::CancelToken;
use crate::error::WalkError;
use crate::policy::{Dir, Entry, EntryKind, ListedEntry, Policy, Visit};
use crate::tree::{Slot, Tree};

/// Settings for one walk. Both default to `None`; construct with functional-update syntax
/// (`WalkOptions { threads, ..Default::default() }`) — further fields may be added over time.
///
/// There is deliberately no cap on entries recorded. Truncating a walk hands back an arbitrary subset — which entries
/// survive depends on scheduling — that a consumer cannot tell from a complete tree without asking, so a guard against
/// large trees becomes a source of quietly wrong answers. A bound worth having would *fail* the walk instead.
/// Cancellation is safe only because the caller who cancelled is the one discarding the result.
#[derive(Clone, Debug, Default)]
pub struct WalkOptions {
    /// An outside stop signal, checked before each directory and before each entry. `None` never cancels.
    pub cancel_token: Option<CancelToken>,
    /// How many threads may walk the tree, for a caller that does not own the machine — a container with a CPU quota, a
    /// process under a thread limit. `None` uses the ambient rayon pool.
    ///
    /// The count is exact rather than a ceiling: the walk gets its own pool of this size and nothing else, so `Some(1)`
    /// walks sequentially. [`NonZeroUsize`] keeps a stray zero from meaning "use the default", which is how rayon reads
    /// `num_threads(0)`.
    ///
    /// Each walk builds and drops its own pool, so a caller making many small walks should install one pool itself and
    /// leave this `None`. A pool that cannot be started is [`WalkError::ThreadPool`], not a silent fall back to some
    /// other parallelism.
    pub threads: Option<NonZeroUsize>,
}

/// Walks the tree rooted at `root`, recording whatever `policy` keeps.
///
/// `initial` is the state the root directory inherits — the seed [`Policy::enter`] extends for the root's entries.
/// Parallelism comes from [`WalkOptions::threads`], or from the ambient rayon pool when that is `None`.
///
/// # Errors
///
/// Returns [`WalkError::ReadDir`] if `root` itself cannot be listed, or [`WalkError::ThreadPool`] if the requested
/// thread count cannot be started. Every deeper failure is collected into [`WalkOutcome::errors`] instead, leaving a
/// partial tree.
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
    let listing = list_dir(root).map_err(|source| WalkError::ReadDir {
        native_path: root.to_path_buf(),
        source,
    })?;

    let walker = Walker { policy, options };

    let frame = Frame {
        native_path: root,
        path: "",
        depth: 0,
        parent_state: initial,
    };

    let subtree = match options.threads {
        Some(threads) => pool(threads)?.install(|| walker.walk_dir(&frame, listing)),
        None => walker.walk_dir(&frame, listing),
    };

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
        cancelled: walker.is_cancelled(),
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

    /// What the walk could not read. A non-empty list means the tree is missing those entries or subtrees.
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

/// One directory the walk is about to classify: where it sits and what state its entries inherit.
struct Frame<'a, P: Policy> {
    native_path: &'a Path,
    path: &'a str,
    depth: usize,
    parent_state: &'a P::State,
}

/// What is invariant across a whole walk.
struct Walker<'a, P: Policy> {
    policy: &'a P,
    options: &'a WalkOptions,
}

impl<P: Policy> Walker<'_, P> {
    /// Whether the walk has been asked to stop.
    fn is_cancelled(&self) -> bool {
        self.options
            .cancel_token
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
    }

    /// Derives one directory's state, classifies its entries, and walks the surviving subdirectories in parallel.
    fn walk_dir(&self, frame: &Frame<'_, P>, listing: Listing) -> Subtree<P::Value> {
        let Listing { entries, errors } = listing;

        let dir = Dir::new(frame.native_path, frame.path, frame.depth, &entries);
        let state = self.policy.enter(&dir, frame.parent_state);

        let (mut subtree, descents) = self.classify(&dir, &state, errors);

        if descents.is_empty() {
            return subtree;
        }

        // `map` over an indexed parallel iterator collects in input order, so the tree is identical whatever the
        // thread count — determinism costs nothing here.
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

    /// Asks the policy about each of `dir`'s entries, recording what it keeps and collecting the subdirectories it
    /// allows the walk to descend into.
    fn classify(
        &self,
        dir: &Dir<'_>,
        state: &P::State,
        errors: Vec<WalkError>,
    ) -> (Subtree<P::Value>, Vec<Descent>) {
        let mut subtree = Subtree {
            entries: Vec::with_capacity(dir.listing().len()),
            subtrees: Vec::new(),
            errors,
            node_count: 0,
            name_bytes: 0,
        };
        let mut descents: Vec<Descent> = Vec::new();

        // One buffer for the whole directory: every entry's path is this prefix plus its name, so the walk allocates
        // per directory rather than per entry.
        let mut path = String::with_capacity(dir.path().len() + 1);
        path.push_str(dir.path());
        if !path.is_empty() {
            path.push('/');
        }
        let prefix_len = path.len();

        for item in dir.listing() {
            if self.is_cancelled() {
                break;
            }

            let file_type = match item.dir_entry.file_type() {
                Ok(file_type) => file_type,
                Err(source) => {
                    subtree.errors.push(WalkError::FileType {
                        native_path: item.dir_entry.path(),
                        source,
                    });
                    continue;
                }
            };

            path.truncate(prefix_len);
            path.push_str(&item.name);

            let entry = Entry::new(item, &path, dir.depth() + 1, file_type);
            let is_dir = entry.kind() == EntryKind::Dir;
            let (value, descendable) = match self.policy.visit(&entry, state) {
                Visit::Skip => continue,
                Visit::Prune(value) => (value, false),
                Visit::Keep(value) => (value, is_dir),
            };

            if descendable {
                descents.push(Descent {
                    index: subtree.entries.len(),
                    native_path: item.dir_entry.path(),
                    path: path.clone(),
                });
            }

            subtree.name_bytes += item.name.len();
            subtree.entries.push(Recorded {
                name: item.name.clone(),
                value,
            });
        }

        subtree.entries.shrink_to_fit();
        subtree.node_count = subtree.entries.len();

        (subtree, descents)
    }

    /// Walks one child directory. A directory that cannot be listed becomes a recorded error and an empty subtree, so
    /// its parent still reports it.
    fn walk_child(
        &self,
        descent: Descent,
        depth: usize,
        parent_state: &P::State,
    ) -> Subtree<P::Value> {
        match list_dir(&descent.native_path) {
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

/// A directory the walk decided to descend into, and the recorded entry it belongs to.
struct Descent {
    index: usize,
    native_path: PathBuf,
    path: String,
}

/// One directory's recorded entries before arena ids exist. A descended directory owns its subtree, so nothing is
/// copied or renumbered until the single flattening pass.
struct Subtree<V> {
    entries: Vec<Recorded<V>>,
    /// The subtree of each entry the walk descended into, beside that entry's index, in entry order.
    subtrees: Vec<(usize, Self)>,
    errors: Vec<WalkError>,
    /// How many entries this directory's whole subtree records, its own included, so the arena is allocated once at
    /// its final size.
    node_count: usize,
    /// How many bytes the names of those entries take, so the name buffer is allocated exactly too.
    name_bytes: usize,
}

/// One kept entry.
struct Recorded<V> {
    name: Box<str>,
    value: V,
}

/// One directory's listing, plus whatever could not be read while listing it.
struct Listing {
    entries: Vec<ListedEntry>,
    errors: Vec<WalkError>,
}

/// Builds the walk's own thread pool, so the traversal cannot outgrow what the caller allowed.
fn pool(threads: NonZeroUsize) -> Result<rayon::ThreadPool, WalkError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads.get())
        .thread_name(|index| format!("kartta-walk-{index}"))
        .build()
        .map_err(|source| WalkError::ThreadPool {
            threads,
            source: Box::new(source),
        })
}

/// Lists the directory at `native_path` sorted by name. The sort is the walk's whole determinism story: everything
/// downstream preserves this order.
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
                dir_entry,
            }),
            Err(source) => errors.push(WalkError::ReadEntry {
                native_path: native_path.to_path_buf(),
                source,
            }),
        }
    }

    entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    Ok(Listing { entries, errors })
}

/// Lays the arena out depth-first — a directory immediately precedes its subtree — and merges the collected errors in
/// that same order.
///
/// shortcut: serial, so it bounds how much the parallel traversal can win. Parallelize it — precompute each subtree's
/// node count to reserve id ranges, then fill them independently — if it shows up as the bottleneck on a large tree.
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

/// Narrows a node id or a name offset to the width the arena stores.
#[expect(
    clippy::expect_used,
    reason = "this needs a tree of over u32::MAX entries or name bytes — a load this crate is not designed to hold in \
              one arena regardless of offset width"
)]
fn arena_offset(len: usize) -> u32 {
    u32::try_from(len).expect("a walk records fewer than u32::MAX entries and name bytes")
}
