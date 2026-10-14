use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::{fs, io};

use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit};

use crate::build::{BuildError, IgnoreSources};
use crate::open::{Symlinks, open_regular};
use crate::rules::{DOT_GIT, GITIGNORE, Rules};

#[derive(Debug)]
pub(crate) struct Payload {
    pub(crate) kind: EntryKind,
    pub(crate) target: Option<Box<str>>,
}

#[derive(Debug)]
pub(crate) struct Indexing {
    ignore_sources: IgnoreSources,
    canonical_root: PathBuf,
    // constraint: `Policy::enter` and `Policy::visit` return no `Result`, so a failure that costs the build information
    // collects here
    errors: Mutex<Vec<BuildError>>,
}

impl Indexing {
    pub(crate) fn new(ignore_sources: IgnoreSources, root: &Path) -> Self {
        Self {
            ignore_sources,
            canonical_root: fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            errors: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn into_errors(self) -> Vec<BuildError> {
        self.errors
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, error: BuildError) {
        self.errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error);
    }

    fn read_ignore_file(&self, native_path: &Path) -> io::Result<Vec<u8>> {
        if fs::symlink_metadata(native_path)?.is_symlink() && !self.leads_into_tree(native_path)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "symlink to an ignore file outside the tree",
            ));
        }

        read_all(&open_regular(native_path, Symlinks::Follow)?.0)
    }

    fn leads_into_tree(&self, native_path: &Path) -> io::Result<bool> {
        Ok(fs::canonicalize(native_path)?.starts_with(&self.canonical_root))
    }

    fn admits_name(&self, entry: &Entry<'_>) -> bool {
        if !entry.name().contains(char::REPLACEMENT_CHARACTER) {
            return true;
        }

        let native_path = entry.native_path();
        if native_path
            .file_name()
            .is_some_and(|name| name.to_str().is_some())
        {
            return true;
        }

        self.record(BuildError::Name { native_path });
        false
    }
}

fn read_target(entry: &Entry<'_>, kind: EntryKind) -> Option<Box<str>> {
    if kind != EntryKind::Symlink {
        return None;
    }

    let target = fs::read_link(entry.native_path()).ok()?;

    target
        .into_os_string()
        .into_string()
        .ok()
        .map(String::into_boxed_str)
}

#[expect(
    clippy::verbose_file_reads,
    reason = "the point is to read the handle whose identity was checked, which fs::read cannot do -- it reopens the \
              name and reopening is what the check exists to avoid"
)]
fn read_all(mut file: &fs::File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    Ok(bytes)
}

// constraint: git folds the case of a control name only on Windows, whose filesystem resolves `.GIT` and `.GITIGNORE`
// to the lowercase names; on unix `.GIT` is an ordinary directory git tracks
fn names_match(name: &str, control: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(control)
    } else {
        name == control
    }
}

fn listed_control_name<'a>(dir: &Dir<'a>, control: &'a str) -> Option<&'a str> {
    if !cfg!(windows) {
        return dir.contains(control).then_some(control);
    }

    dir.names().find(|name| names_match(name, control))
}

fn inherited_across_repository_boundary(dir: &Dir<'_>, parent_state: &Rules) -> Rules {
    if listed_control_name(dir, DOT_GIT).is_some() {
        Rules::default()
    } else {
        parent_state.clone()
    }
}

impl Policy for Indexing {
    type State = Rules;
    type Value = Payload;

    fn enter(&self, dir: &Dir<'_>, parent_state: &Rules) -> Rules {
        if self.ignore_sources == IgnoreSources::None {
            return Rules::default();
        }

        let mut rules = inherited_across_repository_boundary(dir, parent_state);

        if let Some(name) = listed_control_name(dir, GITIGNORE) {
            let native_path = dir.native_path().join(name);
            let pushed = self
                .read_ignore_file(&native_path)
                .and_then(|bytes| rules.push(&bytes, dir.path()));
            if let Err(source) = pushed {
                self.record(BuildError::Ignore {
                    native_path,
                    source,
                });
            }
        }

        rules
    }

    fn visit(&self, entry: &Entry<'_>, rules: &Rules) -> Visit<Payload> {
        if names_match(entry.name(), DOT_GIT) {
            return Visit::Skip;
        }

        let kind = entry.kind();

        if !self.admits_name(entry) {
            return Visit::Skip;
        }

        if rules.ignores(entry.path(), kind == EntryKind::Dir) {
            return Visit::Skip;
        }

        Visit::Keep(Payload {
            kind,
            target: read_target(entry, kind),
        })
    }
}
