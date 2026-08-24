//! The directory listing over a built [`Index`] — [`Index::list_dir`].

use std::num::NonZeroUsize;

use kartta_walk::EntryKind;

use crate::capped::Capped;
use crate::entry::Entry;
use crate::index::Index;

/// A path [`Index::list_dir`] cannot list.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ListDirError {
    /// Nothing is indexed at the path.
    ///
    /// Which is not the same as nothing being there: an ignored directory, a directory below one that would not read,
    /// and a directory created after the walk all answer this way, exactly as they do through [`Index::entry`].
    #[error("nothing is indexed at {path:?}")]
    NotFound {
        /// The path that was asked for.
        path: String,
    },
    /// The path is indexed, as something the index holds no children for.
    #[error("{path:?} is indexed as {kind:?}, which has no listing")]
    NotADirectory {
        /// The path that was asked for.
        path: String,
        /// What is indexed there instead. A symlink to a directory lands here as [`EntryKind::Symlink`]: the walk
        /// never descended it, so the index holds nothing beneath it to list.
        kind: EntryKind,
    },
}

impl Index {
    /// Lists what is indexed under `dir`, depth-first.
    ///
    /// `dir` is a root-relative forward-slash path, and `""` is the index root. The listing holds the directory's
    /// descendants and not the directory itself, in [`Index::entries`] order — so each directory is immediately
    /// followed by its own subtree, and one directory's children are name-sorted.
    ///
    /// Directories are listed. That is what parts this from [`Index::glob_paths`] and [`Index::fuzzy_paths`], which
    /// answer "which file did you mean" and drop them: a listing answers what is in a place, and the directories are
    /// half of that answer.
    ///
    /// `depth` counts levels below `dir`: `1` is its immediate children, `2` adds theirs, and `None` takes the whole
    /// subtree. `limit` caps the entries returned and `None` returns all of them;
    /// [`Capped::truncated`] reports whether the cap turned anything away.
    ///
    /// A directory with nothing indexed under it lists nothing and is not an error. Nothing here reads the
    /// filesystem: kinds and symlink targets are the ones the walk recorded. A size or a binary verdict beside a
    /// listing is not — [`Index::size`] and [`Index::is_binary`] read those when first asked, so they may describe a
    /// later moment than the listing that names them.
    ///
    /// # Errors
    ///
    /// Returns [`ListDirError::NotFound`] when `dir` names no indexed entry, and [`ListDirError::NotADirectory`] when
    /// it names one that is not a directory.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{Config, Entry, Index};
    /// # use std::num::NonZeroUsize;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src/net"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "")?;
    /// std::fs::write(dir.path().join("src/net/tcp.rs"), "")?;
    ///
    /// let index = Index::build(dir.path(), &Config::default())?.into_index();
    ///
    /// let listed = index.list_dir("src", None, None)?;
    /// let paths: Vec<&str> = listed.items().iter().map(|entry| entry.path()).collect();
    /// assert_eq!(paths, ["src/main.rs", "src/net", "src/net/tcp.rs"]);
    /// assert!(!listed.truncated());
    ///
    /// let shallow = index.list_dir("src", NonZeroUsize::new(1), None)?;
    /// let paths: Vec<&str> = shallow.items().iter().map(|entry| entry.path()).collect();
    /// assert_eq!(paths, ["src/main.rs", "src/net"], "src/net/tcp.rs is a level too deep");
    ///
    /// let capped = index.list_dir("src", None, NonZeroUsize::new(1))?;
    /// assert_eq!(capped.len(), 1);
    /// assert!(capped.truncated(), "the directory holds more than the cap allowed");
    /// # Ok(())
    /// # }
    /// ```
    pub fn list_dir(
        &self,
        dir: &str,
        depth: Option<NonZeroUsize>,
        limit: Option<NonZeroUsize>,
    ) -> Result<Capped<&Entry>, ListDirError> {
        let (start, prefix) = self.subtree_of(dir)?;
        let cap = limit.map_or(usize::MAX, NonZeroUsize::get);
        let all = self.entries();

        let mut entries = Vec::new();
        let mut truncated = false;
        let mut at = start;

        while let Some(entry) = all.get(at) {
            let Some(below) = entry.path().strip_prefix(prefix.as_str()) else {
                break;
            };

            if entries.len() == cap {
                truncated = true;
                break;
            }

            entries.push(entry);
            at += 1;

            if entry.kind() == EntryKind::Dir && bottoms_out(below, depth) {
                at = past_subtree(all, at, entry.path());
            }
        }

        Ok(if truncated {
            Capped::cut(entries)
        } else {
            Capped::whole(entries)
        })
    }

    /// Where `dir`'s subtree starts in [`Index::entries`], and the prefix every path in it carries.
    ///
    /// The subtree needs no search of its own. [`Index::entries`] is depth-first, so it is the run beginning one past
    /// the directory and ending at the first path that has left the prefix behind.
    fn subtree_of(&self, dir: &str) -> Result<(usize, String), ListDirError> {
        if dir.is_empty() {
            return Ok((0, String::new()));
        }

        let at = self.position(dir).ok_or_else(|| ListDirError::NotFound {
            path: dir.to_owned(),
        })?;
        let kind = self.entries()[at].kind();

        if kind != EntryKind::Dir {
            return Err(ListDirError::NotADirectory {
                path: dir.to_owned(),
                kind,
            });
        }

        Ok((at + 1, format!("{dir}/")))
    }
}

/// Whether a directory `below` the listed one sits at the deepest level `depth` allows, so its own children are one
/// level too deep to list.
fn bottoms_out(below: &str, depth: Option<NonZeroUsize>) -> bool {
    depth.is_some_and(|depth| below.matches('/').count() + 1 >= depth.get())
}

/// The first position at or after `from` that has left `dir`'s subtree.
///
/// A binary search rather than a walk. [`Index::entries`] is depth-first, so `dir`'s descendants are the run at the
/// front of what remains and the predicate holds over a prefix of it — which is what makes a depth-limited listing
/// cost its own size rather than the size of everything it declined to descend into.
fn past_subtree(entries: &[Entry], from: usize, dir: &str) -> usize {
    from + entries[from..].partition_point(|entry| {
        entry
            .path()
            .strip_prefix(dir)
            .is_some_and(|below| below.starts_with('/'))
    })
}
