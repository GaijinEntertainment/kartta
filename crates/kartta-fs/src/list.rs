use std::num::NonZeroUsize;

use kartta_walk::EntryKind;

use crate::entry::Entry;
use crate::filter::Filter;
use crate::index::{Index, Inspected};
use crate::policy::pruned;

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
    unentered_subtree: Option<NonZeroUsize>,
}

impl<'a> Listed<'a> {
    /// The entry itself, its path decoded.
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
    /// and a directory created after the walk all answer this way, exactly as they do through [`Index::entry`]. A path
    /// through a name the build prunes answers [`ListError::Pruned`] instead.
    #[error("nothing is indexed at {path:?}")]
    NotFound {
        /// The path that was asked for.
        path: String,
    },
    /// Nothing is indexed at the path, because one of its names is one the build prunes: `.git`, or a name in
    /// [`BuildOptions::pruned_names`](crate::BuildOptions::pruned_names). Whether anything is there on disk is not
    /// known.
    #[error("nothing is indexed at {path:?}: the build prunes every entry named {name:?}")]
    Pruned {
        /// The path that was asked for.
        path: String,
        /// The first name of the path that the build prunes.
        name: String,
    },
    /// The path is indexed, as something the index holds no children for.
    #[error("{path:?} is indexed as {}, which has no listing", described(*kind))]
    NotADirectory {
        /// The path that was asked for.
        path: String,
        /// What is indexed there instead. A symlink to a directory lands here as [`EntryKind::Symlink`]: the walk
        /// never descended it, so the index holds nothing beneath it to list.
        kind: EntryKind,
    },
}

/// What an entry of `kind` is, in the words of a sentence.
const fn described(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Dir => "a directory",
        EntryKind::File => "a file",
        EntryKind::Symlink => "a symlink",
        EntryKind::Other => "a special file",
    }
}

impl ListError {
    /// The word for the failure in an answer: `not_found`, for a path the index holds nothing at, pruned or not, or
    /// `not_a_directory`.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::NotFound { .. } | Self::Pruned { .. } => "not_found",
            Self::NotADirectory { .. } => "not_a_directory",
        }
    }
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
    /// Returns [`ListError::Pruned`] when `dir` names no indexed entry and one of its names is one the build prunes,
    /// [`ListError::NotFound`] when it names no indexed entry otherwise, and [`ListError::NotADirectory`] when it names
    /// one that is not a directory.
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
    /// let index = Index::build(dir.path(), &BuildOptions::default())?.index;
    ///
    /// let listed = index.list("src", &ListOptions::default())?;
    /// let listed_paths: Vec<String> = listed.items().iter().map(|found| found.entry().path().to_owned()).collect();
    /// assert_eq!(listed_paths, ["src/main.rs", "src/net", "src/net/tcp.rs"]);
    /// assert!(!listed.truncated());
    ///
    /// // Each item carries its position, so a size beside a row costs no second lookup.
    /// assert_eq!(listed.items()[0].size(), Some(0));
    ///
    /// let shallow = index.list("src", &ListOptions::default().depth(1))?;
    /// let shallow_paths: Vec<String> = shallow.items().iter().map(|found| found.entry().path().to_owned()).collect();
    /// assert_eq!(shallow_paths, ["src/main.rs", "src/net"], "src/net/tcp.rs is a level too deep");
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

        let whole_subtree = depth.is_none() && collapse.is_none() && exclude.is_none();
        let mut items = Vec::with_capacity(if whole_subtree {
            (end - start).min(limit)
        } else {
            0
        });
        let mut truncated = false;
        let mut excluded = 0;
        let mut at = start;
        let mut paths = self.paths();

        while at < end {
            let here = at;

            if exclude.is_some_and(|exclude| exclude.matches(paths.get(here))) {
                at = self.past_subtree(here + 1, paths.get(here));
                excluded += at - here;
                continue;
            }

            if items.len() == limit {
                truncated = true;
                break;
            }

            let unentered = self.kind_at(here) == EntryKind::Dir && {
                let path = paths.get(here);
                depth.is_some_and(|depth| is_at_depth_bound(&path[below_from..], depth))
                    || collapse.is_some_and(|collapse| collapse.matches(path))
            };
            at = if unentered {
                self.past_subtree(here + 1, paths.get(here))
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

    fn subtree_of(&self, dir: &str) -> Result<(usize, usize), ListError> {
        if dir.is_empty() {
            return Ok((0, self.len()));
        }

        let at = self.position(dir).ok_or_else(|| self.not_found(dir))?;
        let kind = self.kind_at(at);

        if kind != EntryKind::Dir {
            return Err(ListError::NotADirectory {
                path: dir.to_owned(),
                kind,
            });
        }

        Ok((at + 1, self.past_subtree(at + 1, dir)))
    }

    fn not_found(&self, dir: &str) -> ListError {
        let path = dir.to_owned();
        match dir
            .split('/')
            .find(|name| pruned(name, self.pruned_names()))
        {
            Some(name) => ListError::Pruned {
                path,
                name: name.to_owned(),
            },
            None => ListError::NotFound { path },
        }
    }

    pub(crate) fn past_subtree(&self, after_dir: usize, dir: &str) -> usize {
        let below = dir.len() + 1;
        if after_dir == self.len() || !self.starts_inside(after_dir, dir) {
            return after_dir;
        }

        let first_block = self.block_of(after_dir) + 1;
        let (mut low, mut high) = (first_block, self.block_count());
        while low < high {
            let middle = low + (high - low) / 2;
            if is_below(self.suffix_in(middle, self.block_first(middle)), dir) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }

        let scan_from = if low > first_block {
            self.block_first(low - 1) + 1
        } else {
            after_dir + 1
        };
        let scan_end = self.block_end(low - 1);
        (scan_from..scan_end)
            .find(|&at| self.shared_at(at) < below)
            .unwrap_or(scan_end)
    }

    fn starts_inside(&self, after_dir: usize, dir: &str) -> bool {
        let block = self.block_of(after_dir);
        let suffix = self.suffix_in(block, after_dir);
        if self.starts_block(after_dir) {
            return is_below(suffix, dir);
        }
        self.shared_at(after_dir) == dir.len() && suffix.starts_with('/')
    }
}

pub(crate) fn is_below(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|below| below.starts_with('/'))
}

pub(crate) fn excluded_ancestor<'p>(path: &'p str, exclude: &Filter) -> Option<&'p str> {
    path.match_indices('/')
        .map(|(end, _)| &path[..end])
        .find(|dir| exclude.matches(dir))
}

fn is_at_depth_bound(below: &str, depth: NonZeroUsize) -> bool {
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

    #[test]
    fn the_shallowest_excluded_ancestor_is_the_one_found() {
        let exclude = Filter::new(["a", "a/b"]).expect("compile");

        assert_eq!(excluded_ancestor("a/b/c.rs", &exclude), Some("a"));
        assert_eq!(excluded_ancestor("x/b/c.rs", &exclude), None);
        assert_eq!(
            excluded_ancestor("a", &exclude),
            None,
            "an entry is not its own ancestor"
        );
    }
}
