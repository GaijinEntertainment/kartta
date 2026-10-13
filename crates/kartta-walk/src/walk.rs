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
use crate::policy::{Dir, Entry, EntryKind, Listing, Policy, Visit};
use crate::tree::{Node, NodeId, Tree};

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
    pub cancel: Option<CancelToken>,
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
/// thread count cannot be started. Every deeper failure is collected into [`WalkOutcome::errors`] instead, leaving a partial
/// tree.
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
///     type Node = Option<u64>;
///
///     fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}
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
/// assert!(outcome.errors().is_empty() && !outcome.cancelled(), "a complete answer");
///
/// let tree = outcome.into_tree();
/// assert_eq!(tree.len(), 1, "the dotfile was skipped");
/// let only = tree.node(tree.roots()[0]);
/// assert_eq!(only.name(), "visible.txt");
/// assert_eq!(*only.value(), Some(5));
/// # Ok(())
/// # }
/// ```
pub fn walk<P: Policy>(
    root: &Path,
    policy: &P,
    initial: &P::State,
    opts: &WalkOptions,
) -> Result<WalkOutcome<P::Node>, WalkError> {
    let listed = list_dir(root).map_err(|source| WalkError::ReadDir {
        path: root.to_path_buf(),
        source,
    })?;

    let ctx = Ctx { policy, opts };

    let level = Level {
        path: root,
        rel: "",
        depth: 0,
        parent: initial,
    };

    let built = match opts.threads {
        Some(threads) => pool(threads)?.install(|| ctx.visit(&level, listed)),
        None => ctx.visit(&level, listed),
    };

    let counted = built.nodes;
    let mut nodes = Vec::with_capacity(counted);
    let mut errors = Vec::new();
    let roots = flatten(built, &mut nodes, &mut errors);
    debug_assert_eq!(
        nodes.len(),
        counted,
        "the walk counted every recorded entry"
    );

    Ok(WalkOutcome {
        tree: Tree { nodes, roots },
        errors,
        cancelled: ctx.cancelled(),
    })
}

/// What one walk produced: the tree, what it could not read, and whether it was cancelled.
///
/// The tree is the part worth keeping; the rest is a report about this particular run, which a consumer surfaces or logs
/// and then drops. [`WalkOutcome::into_tree`] takes the structure and leaves the report behind.
#[derive(Debug)]
pub struct WalkOutcome<N> {
    tree: Tree<N>,
    errors: Vec<WalkError>,
    cancelled: bool,
}

impl<N> WalkOutcome<N> {
    /// The tree the walk discovered.
    #[must_use]
    pub const fn tree(&self) -> &Tree<N> {
        &self.tree
    }

    /// Takes the tree, dropping the report.
    ///
    /// Check [`WalkOutcome::errors`] and [`WalkOutcome::cancelled`] first: a tree taken out of an incomplete walk looks
    /// exactly like a tree taken out of a complete one.
    #[must_use]
    pub fn into_tree(self) -> Tree<N> {
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
    pub const fn cancelled(&self) -> bool {
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
    pub fn into_parts(self) -> (Tree<N>, Vec<WalkError>, bool) {
        (self.tree, self.errors, self.cancelled)
    }
}

/// One directory the walk is about to classify: where it sits and what state its entries inherit.
struct Level<'a, P: Policy> {
    path: &'a Path,
    rel: &'a str,
    depth: usize,
    parent: &'a P::State,
}

/// What is invariant across a whole walk.
struct Ctx<'a, P: Policy> {
    policy: &'a P,
    opts: &'a WalkOptions,
}

impl<P: Policy> Ctx<'_, P> {
    /// Whether the walk has been asked to stop.
    fn cancelled(&self) -> bool {
        self.opts
            .cancel
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
    }

    /// Derives one directory's state, classifies its entries, and walks the surviving subdirectories in parallel.
    fn visit(&self, level: &Level<'_, P>, listed: Listed) -> Built<P::Node> {
        let Listed {
            entries: listing,
            errors,
        } = listed;

        let dir = Dir::new(level.path, level.rel, level.depth, &listing);
        let state = self.policy.enter(&dir, level.parent);

        let (mut built, descend) = self.classify(&dir, &state, errors);

        if descend.is_empty() {
            return built;
        }

        // `map` over an indexed parallel iterator collects in input order, so the tree is identical whatever the
        // thread count — determinism costs nothing here.
        let subtrees: Vec<(usize, Built<P::Node>)> = descend
            .into_par_iter()
            .map(|child| {
                let index = child.index;

                (index, self.subtree(child, level.depth + 1, &state))
            })
            .collect();

        built.nodes += subtrees.iter().map(|(_, sub)| sub.nodes).sum::<usize>();
        built.subtrees = subtrees;

        built
    }

    /// Asks the policy about each of `dir`'s entries, recording what it keeps and collecting the subdirectories it
    /// allows the walk to descend into.
    fn classify(
        &self,
        dir: &Dir<'_>,
        state: &P::State,
        errors: Vec<WalkError>,
    ) -> (Built<P::Node>, Vec<Descent>) {
        let mut built = Built {
            entries: Vec::with_capacity(dir.listing().len()),
            subtrees: Vec::new(),
            errors,
            nodes: 0,
        };
        let mut descend: Vec<Descent> = Vec::new();

        // One buffer for the whole directory: every entry's relative path is this prefix plus its name, so the walk
        // allocates per directory rather than per entry.
        let mut rel = String::with_capacity(dir.rel().len() + 1);
        rel.push_str(dir.rel());
        if !rel.is_empty() {
            rel.push('/');
        }
        let prefix = rel.len();

        for item in dir.listing() {
            if self.cancelled() {
                break;
            }

            let file_type = match item.entry.file_type() {
                Ok(file_type) => file_type,
                Err(source) => {
                    built.errors.push(WalkError::FileType {
                        path: item.entry.path(),
                        source,
                    });
                    continue;
                }
            };

            rel.truncate(prefix);
            rel.push_str(&item.name);

            let entry = Entry::new(item, &rel, dir.depth() + 1, file_type);
            let is_dir = entry.kind() == EntryKind::Dir;
            let (value, descendable) = match self.policy.visit(&entry, state) {
                Visit::Skip => continue,
                Visit::Prune(value) => (value, false),
                Visit::Keep(value) => (value, is_dir),
            };

            if descendable {
                descend.push(Descent {
                    index: built.entries.len(),
                    path: item.entry.path(),
                    rel: rel.clone(),
                });
            }

            built.entries.push(Recorded {
                name: item.name.clone(),
                value,
            });
        }

        built.entries.shrink_to_fit();
        built.nodes = built.entries.len();

        (built, descend)
    }

    /// Walks one child directory. A directory that cannot be listed becomes a recorded error and an empty subtree, so
    /// its parent still reports it.
    fn subtree(&self, child: Descent, depth: usize, parent: &P::State) -> Built<P::Node> {
        match list_dir(&child.path) {
            Ok(listed) => self.visit(
                &Level {
                    path: &child.path,
                    rel: &child.rel,
                    depth,
                    parent,
                },
                listed,
            ),
            Err(source) => Built {
                entries: Vec::new(),
                subtrees: Vec::new(),
                errors: vec![WalkError::ReadDir {
                    path: child.path,
                    source,
                }],
                nodes: 0,
            },
        }
    }
}

/// A directory the walk decided to descend into, and the recorded entry it belongs to.
struct Descent {
    index: usize,
    path: PathBuf,
    rel: String,
}

/// One directory's recorded entries before arena ids exist. A descended directory owns its subtree, so nothing is
/// copied or renumbered until the single flattening pass.
struct Built<N> {
    entries: Vec<Recorded<N>>,
    /// The subtree of each entry the walk descended into, beside that entry's index, in entry order.
    subtrees: Vec<(usize, Self)>,
    errors: Vec<WalkError>,
    /// How many entries this directory's whole subtree records, its own included, so the arena is allocated once at
    /// its final size.
    nodes: usize,
}

/// One kept entry.
struct Recorded<N> {
    name: Box<str>,
    value: N,
}

/// One directory's listing, plus whatever could not be read while listing it.
struct Listed {
    entries: Vec<Listing>,
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

/// Lists `dir` sorted by name. The sort is the walk's whole determinism story: everything downstream preserves this
/// order.
fn list_dir(dir: &Path) -> io::Result<Listed> {
    let mut entries = Vec::new();
    let mut errors = Vec::new();

    for entry in fs::read_dir(dir)? {
        match entry {
            Ok(entry) => entries.push(Listing {
                name: entry
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
                    .into_boxed_str(),
                entry,
            }),
            // The directory opened but an entry could not be read; the rest of the listing still stands.
            Err(source) => errors.push(WalkError::ReadDir {
                path: dir.to_path_buf(),
                source,
            }),
        }
    }

    entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    Ok(Listed { entries, errors })
}

/// Assigns arena ids depth-first — a directory immediately precedes its subtree — and merges the collected errors in
/// that same order.
///
/// shortcut: serial, so it bounds how much the parallel traversal can win. Parallelize it — precompute each subtree's
/// node count to reserve id ranges, then fill them independently — if it shows up as the bottleneck on a large tree.
fn flatten<N>(
    built: Built<N>,
    nodes: &mut Vec<Node<N>>,
    errors: &mut Vec<WalkError>,
) -> Vec<NodeId> {
    let Built {
        entries,
        subtrees,
        errors: mut here,
        nodes: _,
    } = built;
    errors.append(&mut here);

    let mut subtrees = subtrees.into_iter().peekable();
    let mut ids = Vec::with_capacity(entries.len());

    for (index, Recorded { name, value }) in entries.into_iter().enumerate() {
        let id = NodeId(node_id(nodes.len()));
        nodes.push(Node {
            name,
            value,
            children: Vec::new(),
        });
        ids.push(id);

        if let Some((_, sub)) = subtrees.next_if(|&(at, _)| at == index) {
            let children = flatten(sub, nodes, errors);
            nodes[id.index()].children = children;
        }
    }

    ids
}

/// Narrows an arena length to a node id.
#[expect(
    clippy::expect_used,
    reason = "one node per recorded entry, so this needs a tree of over u32::MAX entries — a load this crate is not \
              designed to hold in one arena regardless of id width"
)]
fn node_id(len: usize) -> u32 {
    u32::try_from(len).expect("a walk records fewer than u32::MAX entries")
}
