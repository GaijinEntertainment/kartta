use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bstr::BStr;
use gix_glob::pattern::Case;
use gix_glob::search::pattern::List;
use gix_ignore::search::{Ignore, pattern_matching_relative_path};

pub(crate) const GITIGNORE: &str = ".gitignore";
pub(crate) const DOT_GIT: &str = ".git";

// constraint: git's default; `core.ignoreCase` records what the filesystem does with case, not what matching should
// do, and reading it would make the index machine-dependent
const CASE: Case = Case::Sensitive;

// constraint: gitoxide's `$`-prefixed precious extension is off, because it makes `$houdini` ignore `houdini` where git
// ignores `$houdini`
const DIALECT: Ignore = Ignore {
    support_precious: false,
};

// constraint: a stack is cloned for every directory the walk descends, so its layers, shallowest first, are shared
// behind `Arc` rather than copied into every descendant
#[derive(Debug, Clone, Default)]
pub(crate) struct Rules {
    layers: Vec<Arc<List<Ignore>>>,
}

impl Rules {
    // constraint: a layer is anchored at a synthetic root-relative source path rather than a filesystem path, so its
    // patterns match the root-relative paths the index stores; gix-glob fails on a `dir` it cannot spell as git bytes
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

    // constraint: git's precedence: the deepest layer with an opinion decides, and inside a layer the last matching
    // pattern wins; a negation re-includes only what the walk reaches, since an ignored directory is not descended
    pub(crate) fn ignores(&self, path: &str, is_dir: bool) -> bool {
        if self.layers.is_empty() {
            return false;
        }

        let git_path = BStr::new(path.as_bytes());
        let basename_start = path.rfind('/').map(|at| at + 1);

        self.layers
            .iter()
            .rev()
            .find_map(|layer| {
                pattern_matching_relative_path(layer, git_path, basename_start, Some(is_dir), CASE)
            })
            .is_some_and(|matched| !matched.pattern.is_negative())
    }
}
