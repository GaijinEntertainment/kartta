//! The `.gitignore` layers in force at one point in the tree.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bstr::BStr;
use gix_glob::pattern::Case;
use gix_glob::search::pattern::List;
use gix_ignore::search::{Ignore, pattern_matching_relative_path};

/// The name of the per-directory ignore file, and of the repository directory that bounds a stack of them.
pub(crate) const GITIGNORE: &str = ".gitignore";
pub(crate) const DOT_GIT: &str = ".git";

/// How pattern and path characters compare.
///
/// Always case-sensitive, which is git's own default. `core.ignoreCase` is not consulted: it records what the
/// *filesystem* does with case, not what the caller wants matching to do, and reading it would make the index
/// machine-dependent for a reason unrelated to the ignore sources that field is meant to control.
const CASE: Case = Case::Sensitive;

/// Git's own dialect, with gitoxide's `$`-prefixed "precious" extension off.
///
/// The extension changes what a pattern starting with `$` means, so leaving it on would ignore `houdini` where git
/// ignores `$houdini`. The index matches what git does; nothing here ever deletes a file, so the distinction precious
/// files exist to draw has nothing to act on anyway.
const DIALECT: Ignore = Ignore {
    support_precious: false,
};

/// The ignore rules that apply to one directory's entries: one layer per `.gitignore` on the way down, shallowest
/// first.
///
/// Layers are shared rather than copied. A stack is cloned once per directory the walk descends into, and on a large
/// tree the same ancestor patterns would otherwise be duplicated once per descendant directory; behind [`Arc`] a clone
/// is a refcount bump per layer, and layers are never mutated after parsing.
#[derive(Debug, Clone, Default)]
pub(crate) struct Rules {
    layers: Vec<Arc<List<Ignore>>>,
}

impl Rules {
    /// Adds the layer parsed from the `.gitignore` of the directory at `dir`, which is a forward-slash path relative to
    /// the index root and empty for the root itself.
    ///
    /// No filesystem path is involved: the layer is anchored by a synthetic root-relative source path, which is what
    /// makes the patterns match the same root-relative paths the index stores.
    ///
    /// Fails when `gix-glob` cannot represent `dir` as git path bytes.
    pub(crate) fn push(&mut self, bytes: &[u8], dir: &str) -> io::Result<()> {
        let source = if dir.is_empty() {
            PathBuf::from(GITIGNORE)
        } else {
            PathBuf::from(format!("{dir}/{GITIGNORE}"))
        };

        let layer = List::from_bytes(bytes, source, Some(Path::new("")), DIALECT)?;
        self.layers.push(Arc::new(layer));
        Ok(())
    }

    /// Whether `rel` is ignored, where `rel` is a forward-slash path relative to the index root.
    ///
    /// The deepest layer with an opinion decides, and inside one layer the last matching pattern wins — git's two
    /// precedence rules, in that order. A negated pattern therefore re-includes a path a shallower layer excluded, but
    /// only for a path the walk still reaches: a directory that is ignored is never descended, so nothing inside it can
    /// be negated back in, which is git's rule rather than an approximation of it.
    pub(crate) fn ignores(&self, rel: &str, is_dir: bool) -> bool {
        if self.layers.is_empty() {
            return false;
        }

        let path = BStr::new(rel.as_bytes());
        let basename_pos = rel.rfind('/').map(|at| at + 1);

        self.layers
            .iter()
            .rev()
            .find_map(|layer| {
                pattern_matching_relative_path(layer, path, basename_pos, Some(is_dir), CASE)
            })
            .is_some_and(|matched| !matched.pattern.is_negative())
    }
}
