//! Repository file index: one filesystem walk builds a snapshot, and every query answers from it.
//!
//! [`Index::build`] reads a directory tree once, applying git's ignore rules on the way down, and keeps what survives.
//! Every question about *structure* is answered from that snapshot — a lookup, a listing and both path searches never
//! re-read the tree. Questions about a file's own bytes are answered by reading it, on the first ask for that entry,
//! and the answer is kept. Construction takes an OS path and reads the real filesystem; there is no
//! virtual-filesystem abstraction to mock.
//!
//! ```
//! use kartta_fs::{BuildOptions, FuzzyOptions, Index};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let dir = tempfile::tempdir()?;
//! std::fs::create_dir_all(dir.path().join("src"))?;
//! std::fs::write(dir.path().join("src/main.rs"), "fn main() {}")?;
//! std::fs::write(dir.path().join(".gitignore"), "*.log\n")?;
//! std::fs::write(dir.path().join("build.log"), "noise")?;
//!
//! let outcome = Index::build(dir.path(), &BuildOptions::default())?;
//! assert!(outcome.errors.is_empty(), "a complete answer");
//! let index = outcome.index;
//!
//! let ranked = index.fuzzy("main", &FuzzyOptions::default());
//! assert_eq!(index.paths().get(ranked.items()[0].position), "src/main.rs");
//! assert!(index.entry("build.log").is_none(), "the .gitignore applied");
//! # Ok(())
//! # }
//! ```
//!
//! # Conventions
//!
//! - Paths are forward-slash and root-relative on every platform. Windows is a first-class target, so no path that
//!   *identifies* an entry carries an OS-native separator. Two surfaces are native instead, because both point at a
//!   place on disk rather than naming an index entry: [`Index::root`], which an entry's path is joined onto to open it,
//!   and [`BuildError::native_path`], which tells an operator where to go and look. The rule is one form for identity,
//!   the other for access. [`Entry::target`] is in neither form, because it names nothing this crate addresses: it is
//!   the string a symlink holds, reproduced rather than interpreted.
//! - An entry whose name is not valid UTF-8 is not indexed, and is reported through [`BuildError::Name`]. It has no
//!   faithful path — the walk decodes lossily, and two such names collapse onto one — so indexing it would hand two
//!   entries a single lookup key and a path that opens neither.
//! - Structure is one snapshot; a file's own bytes are not. The walk records what exists and what each entry is, so
//!   those answers are coherent across the whole tree. [`Index::size`] and [`Index::is_binary`] instead read the
//!   filesystem when first asked and keep what they read, because doing either for a whole tree during the walk costs
//!   far more than the answers are worth to a caller who looks at a handful of files. Two such answers from one index
//!   may therefore describe two different moments, and none is refreshed once taken. A consumer that reads content
//!   anyway builds with [`Index::build_with`] and a [`ContentVisitor`], so each file it wants is read once, during the
//!   walk, and that read's answers are kept.
//! - `Some(0)` is a genuinely empty file and `None` is a size that could not be read; the two are never conflated.
//! - A build that could not read everything says so. [`BuildOutcome::errors`] is not a log — an index missing a
//!   subtree, or holding files an unread `.gitignore` would have hidden, is indistinguishable from a correct one
//!   without it.
//! - Gitignore matching belongs to [`gix-ignore`](https://crates.io/crates/gix-ignore) and path ranking to
//!   `kartta-fuzz`. Git's dialect is a compatibility surface, so it is delegated rather than reimplemented.
//! - Import resolution is not this crate's concern: a specifier-to-file mapping is language semantics. What lives here
//!   are the mechanical primitives a resolver queries.
//!
//! # Ignore rules
//!
//! `.git` is never indexed and never descended, whatever the configuration — it holds the repository's bookkeeping
//! rather than the tree the repository describes. A `.git` found deeper in the tree marks another repository's root,
//! and the inherited rule stack resets there, exactly as git's own rules stop at a repository boundary. Its contents
//! are still indexed: this crate indexes a directory tree, not a git index.
//!
//! One such directory is held empty instead: a linked worktree of the repository the root belongs to, a second checkout
//! of that repository that would put every file in the index twice. It is known by what git itself writes, never by
//! its shape: its `.git` is a gitfile, `gitdir: <path>`, whose git directory holds a `commondir` that comes to the
//! common git directory of the root's own `.git`. [`Index::pruned_worktrees`] names each. A submodule, a clone and a
//! worktree of another repository are indexed under their own rules like any other directory, and at
//! [`IgnoreSources::None`] no worktree is held empty.
//!
//! That reset applies to a nested repository the walk reaches. An outer rule that hides the directory itself hides
//! everything under it, nested repository or not, because an ignored directory is never descended — which is git's own
//! answer, as `git status` on a tree whose `.gitignore` names the directory will confirm.
//!
//! [`BuildOptions::pruned_names`] names what else is never indexed and never descended: the clutter an editor or the
//! operating system leaves in a tree — `.idea`, `.vscode`, `.fleet` and `.DS_Store` unless the caller composes a
//! [`PrunedNames`] of its own. An entry so named is pruned at any depth before any rule is consulted, so no negation
//! brings it back and no choice of ignore sources indexes it.
//!
//! Which ignore files apply is [`BuildOptions::ignore_sources`], and the default is the reproducible one — in-tree
//! `.gitignore` files only, which is the level at which the same tree indexes identically wherever it sits. Sources
//! outside the tree are machine-local, so honouring one gives that up; [`IgnoreSources`] carries the full reasoning.
//!
//! What is modelled is git's **ignore rules**, not git's index. A file git already tracks stays tracked even once a
//! rule would hide it, and such a file is not indexed here — `git check-ignore --no-index` is the command that agrees
//! with this crate, where `git ls-files` is the one that does not. Reading the git index to re-include those files
//! would mean a real git repository and a large dependency tree for a case that only arises when a rule is added after
//! a file, so a build reports what the rules say and nothing else.
//!
//! # Symlinks
//!
//! One rule on every surface: a symlink is **reported, never read through**. Its content is either already indexed
//! under the target's own path (an in-tree target) or is not the repository's at all (an out-of-tree target), so
//! following one either duplicates a file under a second path or reads outside the tree.
//!
//! What the link itself holds is reported. [`Entry::target`] gives the target string the filesystem stores, which is a
//! read *of* the link rather than *through* it, and nothing resolves where it leads. A link into the tree, a link out
//! of it and a link to nothing therefore all answer alike — a symlink is a view onto somewhere else, and a view onto
//! nothing is still the entry the tree holds rather than a failure to index one.
//!
//! A symlink is therefore a leaf. It is never descended even when it points at a directory, so no cycle detection is
//! needed and the walk cannot leave the tree it was given. It is also never stat'd through: a dangling link and a live
//! one are indistinguishable to every query here, since neither is read, so following one to tell them apart would buy
//! nothing for a syscall per link.
//!
//! Indexed is thus not the same as readable, by rule rather than by accident.
//!
//! The rule binds what the index *reports*, not what a concurrent writer can do to the tree underneath it. An entry
//! classified as a directory and then replaced by a symlink before the walk lists it is descended through, and the
//! target's names enter the index under the original prefix. Closing that needs descent through a directory handle
//! rather than a path, which is per-platform work; until then the race is accepted and stated, as it is on Windows
//! for a regular file swapped for a symlink between the walk and a later read. Both cost a writer mutating the tree
//! mid-build what putting the same content at the same path costs them anyway.
//!
//! # Binary content
//!
//! [`Index::is_binary`] settles a file two ways, in order. An extension in [`BuildOptions::binary_extensions`] answers
//! with nothing opened — which carries most of the weight on a real tree, because the default set,
//! [`BinaryExtensions::builtin`], covers the archives, media, disk images and model weights. Everything else is answered by whether a
//! NUL appears in the file's first 64 KiB, which is git's and ripgrep's rule and the size ripgrep reads at a time.
//!
//! Both halves are deliberately one-sided about what a *name* may claim. The extension set says "binary, do not
//! open" and never says text, so a file it does not name is read rather than assumed. And because the probe is
//! bounded, the verdict describes a prefix: a file that is text for 64 KiB and binary after reports text.
//!
//! # Content search
//!
//! [`Index::grep`] searches the content of regular files with [`Patterns`], line by line, through ripgrep's own
//! matcher and searcher, so a pattern written for `rg` means the same here. It reads each candidate as it is when
//! searched, applies the binary rule above before it searches a byte, and answers in index order — and its [`Matches`]
//! account for every candidate the answer does not cover, because an agent cannot see what it was not given.
//!
//! A build asked for a content index, through [`BuildOptions::content_index`], reads every text file during the walk
//! and records which files hold which trigrams. It reads each file once, except that with
//! [`BuildOptions::content_cache`] a file of 64 KiB or more that no [`ContentVisitor`] wants is read once to hash it
//! and, when the store lacks it, again to scan it. A search on that index opens only the candidates holding every
//! trigram a match needs, and counts the rest in [`Matches::ruled_out`]. The index is a snapshot of content as the
//! build read it, as the structure is a snapshot of the tree.
//!
//! # Keeping an index current
//!
//! An index that nobody refreshes is a snapshot and costs nothing more. [`Index::refresh`] walks the tree again and
//! makes the index what a build would make now; [`Index::refresh_paths`] walks again only the paths a caller names. With
//! the `watch` feature, a `Watcher` follows the tree through the operating system's change notifications and hands
//! over the paths to name.
//!
//! # What is here so far
//!
//! The walk, the ignore rules, the snapshot, path lookup, the directory listing, sizes, binary classification, the two
//! path searches — glob and fuzzy — both narrowed by a [`Filter`], content search, the content index that narrows it,
//! and refreshing all of these as the tree changes. Not yet: the ignore sources outside the tree — [`IgnoreSources`]
//! names only the levels that are implemented.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod binary;
mod build;
mod cache;
mod content;
mod content_index;
mod entry;
mod filter;
mod fuzzy;
mod glob;
mod grep;
mod index;
mod inspection;
mod list;
mod matches;
mod open;
mod patterns;
mod policy;
mod postings;
mod pruned;
mod requirement;
mod rules;
mod scan;
mod spelling;
mod stamp;
mod trigram;
#[cfg(feature = "watch")]
mod watch;
mod worktree;

use std::sync::{Mutex, MutexGuard, PoisonError};

pub use kartta_cache::{ContentHash, Namespace};
pub use kartta_walk::CancelToken;

pub use crate::binary::{BinaryExtensions, BinaryExtensionsError};
pub use crate::build::{BuildError, BuildOptions, IgnoreSources, PathFault};
pub use crate::content::{Content, ContentId, ContentVisitor};
pub use crate::content_index::{ContentIndexStats, ContentIndexStorage};
pub use crate::entry::{Entry, EntryKind};
pub use crate::filter::{Filter, FilterError};
pub use crate::fuzzy::{FuzzyMatch, FuzzyMatches, FuzzyOptions};
pub use crate::glob::{Capped, GlobOptions};
pub use crate::grep::GrepOptions;
pub use crate::index::{BuildOutcome, Index, Paths, RefreshOutcome, Stopped};
pub use crate::list::{ListError, ListOptions, Listed, Listing};
pub use crate::matches::{Cut, Failure, Line, LineKind, MatchedFile, Matches};
pub use crate::patterns::{Case, PatternError, Patterns};
pub use crate::pruned::{PrunedNames, PrunedNamesError};
pub use crate::stamp::FileStamp;
#[cfg(feature = "watch")]
pub use crate::watch::{WatchError, Watcher};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn into_inner<T>(mutex: Mutex<T>) -> T {
    mutex.into_inner().unwrap_or_else(PoisonError::into_inner)
}
