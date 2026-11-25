//! What git itself states about the repository a checkout belongs to: the gitfile a linked worktree's `.git` is, and the
//! `commondir` its git directory holds (`gitrepository-layout(5)`).

use std::fs;
use std::path::{Path, PathBuf};

const GITFILE_PREFIX: &str = "gitdir: ";
const COMMONDIR: &str = "commondir";

/// The common git directory of the repository whose checkout's `.git` is `dot_git`, canonical: the git directory itself
/// or the one its gitfile names, unless that holds a `commondir`, whose directory it is then. `None` where `dot_git`
/// is neither a directory nor a gitfile, or names a path that does not resolve.
pub(crate) fn common_dir(dot_git: &Path) -> Option<PathBuf> {
    let git_dir = match gitfile_target(dot_git) {
        Some(git_dir) => git_dir,
        None if fs::symlink_metadata(dot_git).is_ok_and(|metadata| metadata.is_dir()) => {
            dot_git.to_path_buf()
        }
        None => return None,
    };
    let common = trimmed_text(&git_dir.join(COMMONDIR))
        .map_or_else(|| git_dir.clone(), |common| git_dir.join(common));

    fs::canonicalize(common).ok()
}

/// The common git directory, canonical, of the linked worktree whose `.git` is `dot_git`: `None` unless `dot_git` is a
/// gitfile and the git directory it names holds a `commondir` that resolves.
pub(crate) fn linked_common_dir(dot_git: &Path) -> Option<PathBuf> {
    let git_dir = gitfile_target(dot_git)?;
    let common = trimmed_text(&git_dir.join(COMMONDIR))?;

    fs::canonicalize(git_dir.join(common)).ok()
}

// constraint: git reads a gitfile whole, takes what follows `gitdir: ` up to the line ends it strips, and resolves a
// relative path against the directory holding the file (git v2.56.1 `setup.c:975`, `:1045-1055`)
fn gitfile_target(dot_git: &Path) -> Option<PathBuf> {
    let text = trimmed_text(dot_git)?;
    let target = text.strip_prefix(GITFILE_PREFIX)?;

    Some(dot_git.parent()?.join(target))
}

// constraint: git strips every trailing `\n` and `\r` of a gitfile and of a `commondir`, and refuses one left empty
// (git v2.56.1 `setup.c:331-333`, `:1049-1053`)
fn trimmed_text(path: &Path) -> Option<String> {
    let mut text = fs::read_to_string(path).ok()?;
    let kept = text.trim_end_matches(['\n', '\r']).len();
    text.truncate(kept);

    (!text.is_empty()).then_some(text)
}
