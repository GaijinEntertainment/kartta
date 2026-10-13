//! The directory listing over a built [`Index`] — [`Index::list`].

use std::num::NonZeroUsize;

use kartta_walk::EntryKind;

use crate::entry::Entry;
use crate::filter::Filter;
use crate::index::{Index, Inspected};

/// How far a listing descends, which directories it does not enter, what it leaves out, and how many entries it
/// returns.
///
/// Every bound is absent by default, which lists the whole subtree. Set them by name — `depth` and `limit` are the same
/// type, so a call that passed them positionally would swap silently and change both how deep the listing went and how
/// much of it came back.
///
/// # Examples
///
/// ```
/// use kartta_fs::{Filter, ListOptions};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let immediate_children = ListOptions::default().depth(1);
/// let first_twenty = ListOptions::default().limit(20);
///
/// let dependencies = Filter::new(["**/node_modules", "**/vendor"])?;
/// let without_dependencies = ListOptions::default().collapse(&dependencies);
///
/// let dotfiles = Filter::new(["**/.*"])?;
/// let without_dotfiles = ListOptions::default().exclude(&dotfiles);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct ListOptions<'a> {
    /// How many levels below the listed directory to descend: `1` is its immediate children, `2` adds theirs.
    ///
    /// `None` takes the whole subtree.
    pub depth: Option<NonZeroUsize>,
    /// How many entries to return at most. `None` returns all of them.
    pub limit: Option<NonZeroUsize>,
    /// The directories to list without entering, matched by their root-relative path. A matched directory is listed,
    /// and what lies beneath it is counted in [`Listed::unlisted`] rather than listed.
    ///
    /// `None` collapses nothing. The listed directory itself is never collapsed, so naming one by path shows what is
    /// inside it.
    pub collapse: Option<&'a Filter>,
    /// The entries to leave out, matched by their root-relative path: a matched file is not listed, and a matched
    /// directory is not listed together with everything beneath it. [`Listing::excluded`] counts what this removed,
    /// and [`Listing::total`] still includes it, being the index's count rather than the listing's.
    ///
    /// `None` excludes nothing. Exclusion wins over [`ListOptions::collapse`] where both match, and the listed
    /// directory itself is never excluded.
    pub exclude: Option<&'a Filter>,
}

impl<'a> ListOptions<'a> {
    /// Descends `depth` levels below the listed directory. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn depth(mut self, depth: usize) -> Self {
        self.depth = NonZeroUsize::new(depth);
        self
    }

    /// Returns at most `limit` entries. `0` lifts the bound, as `None` does.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = NonZeroUsize::new(limit);
        self
    }

    /// Lists every directory `collapse` matches without entering it.
    #[must_use]
    pub const fn collapse(mut self, collapse: &'a Filter) -> Self {
        self.collapse = Some(collapse);
        self
    }

    /// Leaves out every entry `exclude` matches, and everything beneath a matched directory.
    #[must_use]
    pub const fn exclude(mut self, exclude: &'a Filter) -> Self {
        self.exclude = Some(exclude);
        self
    }
}

/// What [`Index::list`] found under a directory, and how much of it the answer left out.
#[derive(Debug, Clone)]
pub struct Listing<'a> {
    items: Vec<Listed<'a>>,
    truncated: bool,
    excluded: usize,
    total: usize,
}

impl<'a> Listing<'a> {
    /// The listed entries, in [`Index::entries`] order.
    #[must_use]
    pub fn items(&self) -> &[Listed<'a>] {
        &self.items
    }

    /// Takes the entries, dropping what the listing says about the rest.
    ///
    /// Check [`Listing::truncated`] first: a listing the limit cut short looks exactly like a whole one.
    #[must_use]
    pub fn into_items(self) -> Vec<Listed<'a>> {
        self.items
    }

    /// How many entries the listing holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the listing holds no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether the limit turned an entry away, so more exist than the listing shows.
    ///
    /// Settled by an actual rejection rather than inferred from a count, so a listing holding exactly the limit reports
    /// `false`.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// How many indexed entries [`ListOptions::exclude`] removed, counting everything beneath a removed directory.
    ///
    /// Counted up to where the listing ended, so a listing the limit cut short says nothing about exclusions past the
    /// cut.
    #[must_use]
    pub const fn excluded(&self) -> usize {
        self.excluded
    }

    /// How many entries the index holds under the listed directory, at every depth.
    ///
    /// The listing with no bound at all would hold this many, so it bounds what the depth and the limit left out.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }
}

/// One entry of a [`Listing`], with the late reads reachable without looking the entry up again.
#[derive(Debug, Clone, Copy)]
pub struct Listed<'a> {
    inspected: Inspected<'a>,
    /// How many entries the directory's subtree holds, the directory included, when the listing did not enter it.
    unentered_subtree: Option<NonZeroUsize>,
}

impl<'a> Listed<'a> {
    /// The entry itself.
    #[must_use]
    pub fn entry(&self) -> Entry<'a> {
        self.inspected.entry()
    }

    /// The entry's size, on the terms [`Index::size`] states.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.inspected.size()
    }

    /// Whether the entry holds binary content, on the terms [`Index::is_binary`] states.
    #[must_use]
    pub fn is_binary(&self) -> Option<bool> {
        self.inspected.is_binary()
    }

    /// How many indexed entries lie beneath this directory that the listing did not enter.
    ///
    /// `Some` for a directory at the depth bound or one the listing collapsed, `Some(0)` included, so a directory the
    /// listing did not enter never reads as an empty one. `None` for a directory the listing entered, and for anything
    /// that is not a directory. An entered directory's contents can still be cut short by the limit, which
    /// [`Listing::truncated`] reports.
    #[must_use]
    pub const fn unlisted(&self) -> Option<usize> {
        match self.unentered_subtree {
            Some(subtree) => Some(subtree.get() - 1),
            None => None,
        }
    }
}

/// A path [`Index::list`] cannot list.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ListError {
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
    /// Directories are listed. That is what parts this from [`Index::glob`] and [`Index::fuzzy`], which
    /// answer "which file did you mean" and drop them: a listing answers what is in a place, and the directories are
    /// half of that answer.
    ///
    /// [`ListOptions`] carries the bounds: how far below `dir` to descend, which directories not to enter, what to
    /// leave out, and how many entries to return. The answer says how much each bound left out: [`Listing::truncated`]
    /// whether the limit turned anything away, [`Listed::unlisted`] how many entries lie beneath a directory the
    /// listing did not enter, [`Listing::excluded`] how many entries the exclusion removed, and [`Listing::total`] how
    /// many the index holds under `dir` altogether. Neither a collapsed directory's contents nor an excluded entry
    /// counts toward the limit.
    ///
    /// A directory with nothing indexed under it lists nothing and is not an error.
    ///
    /// Listing itself reads nothing: kinds and symlink targets are the ones the walk recorded. Each item carries the
    /// position it was found at, so [`Listed::size`] and [`Listed::is_binary`] reach an entry's late-read facts
    /// without looking the path up again — those *do* read the filesystem on first ask, and may describe a later
    /// moment than the listing that names them.
    ///
    /// # Errors
    ///
    /// Returns [`ListError::NotFound`] when `dir` names no indexed entry, and [`ListError::NotADirectory`] when
    /// it names one that is not a directory.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{BuildOptions, Index, ListOptions};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let dir = tempfile::tempdir()?;
    /// std::fs::create_dir_all(dir.path().join("src/net"))?;
    /// std::fs::write(dir.path().join("src/main.rs"), "")?;
    /// std::fs::write(dir.path().join("src/net/tcp.rs"), "")?;
    ///
    /// let index = Index::build(dir.path(), &BuildOptions::default())?.into_index();
    ///
    /// let listed = index.list("src", &ListOptions::default())?;
    /// let paths: Vec<&str> = listed.items().iter().map(|found| found.entry().path()).collect();
    /// assert_eq!(paths, ["src/main.rs", "src/net", "src/net/tcp.rs"]);
    /// assert!(!listed.truncated());
    ///
    /// // Each item carries its position, so a size beside a row costs no second lookup.
    /// assert_eq!(listed.items()[0].size(), Some(0));
    ///
    /// let shallow = index.list("src", &ListOptions::default().depth(1))?;
    /// let paths: Vec<&str> = shallow.items().iter().map(|found| found.entry().path()).collect();
    /// assert_eq!(paths, ["src/main.rs", "src/net"], "src/net/tcp.rs is a level too deep");
    /// assert_eq!(shallow.items()[1].unlisted(), Some(1), "and the listing says one entry sits beneath src/net");
    /// assert_eq!(shallow.total(), 3);
    ///
    /// let capped = index.list("src", &ListOptions::default().limit(1))?;
    /// assert_eq!(capped.len(), 1);
    /// assert!(capped.truncated(), "the directory holds more than the cap allowed");
    /// # Ok(())
    /// # }
    /// ```
    pub fn list(&self, dir: &str, options: &ListOptions<'_>) -> Result<Listing<'_>, ListError> {
        let (start, end) = self.subtree_of(dir)?;
        let below_from = if dir.is_empty() { 0 } else { dir.len() + 1 };
        let limit = options.limit.map_or(usize::MAX, NonZeroUsize::get);
        let ListOptions {
            depth,
            collapse,
            exclude,
            ..
        } = *options;

        let mut items = Vec::new();
        let mut truncated = false;
        let mut excluded = 0;
        let mut at = start;

        while at < end {
            let here = at;

            if exclude.is_some_and(|exclude| exclude.matches(self.path_at(here))) {
                at = self.past_subtree(here + 1, self.path_at(here));
                excluded += at - here;
                continue;
            }

            if items.len() == limit {
                truncated = true;
                break;
            }

            let unentered = self.kind_at(here) == EntryKind::Dir && {
                let path = self.path_at(here);
                depth.is_some_and(|depth| bottoms_out(&path[below_from..], depth))
                    || collapse.is_some_and(|collapse| collapse.matches(path))
            };
            at = if unentered {
                self.past_subtree(here + 1, self.path_at(here))
            } else {
                here + 1
            };

            items.push(Listed {
                inspected: self.inspected_at(here),
                unentered_subtree: if unentered {
                    NonZeroUsize::new(at - here)
                } else {
                    None
                },
            });
        }

        Ok(Listing {
            items,
            truncated,
            excluded,
            total: end - start,
        })
    }

    /// The run of [`Index::entries`] that `dir`'s subtree occupies: from one past the directory to the first position
    /// that has left it.
    ///
    /// [`Index::entries`] is depth-first, so the subtree is contiguous and the end is one search away.
    fn subtree_of(&self, dir: &str) -> Result<(usize, usize), ListError> {
        if dir.is_empty() {
            return Ok((0, self.len()));
        }

        let at = self.position(dir).ok_or_else(|| ListError::NotFound {
            path: dir.to_owned(),
        })?;
        let kind = self.kind_at(at);

        if kind != EntryKind::Dir {
            return Err(ListError::NotADirectory {
                path: dir.to_owned(),
                kind,
            });
        }

        Ok((at + 1, self.past_subtree(at + 1, dir)))
    }

    /// The first position at or after `from` that has left `dir`'s subtree.
    ///
    /// [`Index::entries`] is depth-first, so `dir`'s descendants are the run at the front of what remains and the
    /// predicate holds over a prefix of it. A binary search over that prefix is what makes a depth-limited listing cost
    /// its own size rather than the size of everything it declined to descend into.
    ///
    /// The entry immediately after `dir` is checked first, because it settles the two commonest cases outright: an
    /// empty directory and a leaf directory both have the next sibling sitting there. Searching for that would spend a
    /// logarithmic probe per directory, which on a listing full of empty ones is the cost the search was meant to
    /// avoid.
    fn past_subtree(&self, from: usize, dir: &str) -> usize {
        let inside = |at: usize| {
            self.path_at(at)
                .strip_prefix(dir)
                .is_some_and(|below| below.starts_with('/'))
        };

        if from == self.len() || !inside(from) {
            return from;
        }

        let (mut low, mut high) = (from + 1, self.len());
        while low < high {
            let middle = low + (high - low) / 2;
            if inside(middle) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }
}

/// Whether a directory `below` the listed one sits at the deepest level `depth` allows, so its own children are one
/// level too deep to list.
fn bottoms_out(below: &str, depth: NonZeroUsize) -> bool {
    below.matches('/').count() + 1 >= depth.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_entry_stays_three_words() {
        assert_eq!(
            size_of::<Listed<'_>>(),
            3 * size_of::<usize>(),
            "a listing holds one of these per entry, so every word is paid per listed entry"
        );
    }
}
