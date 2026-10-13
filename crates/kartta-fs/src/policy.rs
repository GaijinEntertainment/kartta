//! The walk policy that turns a filesystem tree into indexable entries.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::{fs, io};

use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit};

use crate::config::IgnoreSources;
use crate::error::BuildError;
use crate::open::{Symlinks, open_regular};
use crate::rules::{DOT_GIT, GITIGNORE, Rules};

/// What the walk records per entry. The path is not here: the tree already carries each name, and the index builds
/// full paths from it in one pass rather than storing a prefix per node.
#[derive(Debug)]
pub(crate) struct Payload {
    pub(crate) kind: EntryKind,
    pub(crate) target: Option<Box<str>>,
}

/// Classifies entries for one build, and collects the failures that cost it information without stopping it.
///
/// [`Policy::enter`] returns a state rather than a `Result`, so a `.gitignore` that exists but will not read has
/// nowhere to report through the walk's own channel. It reports here instead, and the build drains this beside the
/// walk's own error list.
#[derive(Debug)]
pub(crate) struct Indexing {
    sources: IgnoreSources,
    /// The tree's own location, resolved through symlinks, so a symlinked ignore file can be tested for leading back
    /// inside it.
    root: PathBuf,
    errors: Mutex<Vec<BuildError>>,
}

impl Indexing {
    pub(crate) fn new(sources: IgnoreSources, root: &Path) -> Self {
        Self {
            sources,
            root: fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            errors: Mutex::new(Vec::new()),
        }
    }

    /// The failures collected during the walk.
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

    /// The bytes of an ignore file, or a failure naming why no rules could be taken from it.
    ///
    /// Git opens `.gitignore` through a symlink, so this follows one too — but only to a target inside the tree.
    /// Following one outside it would let a repository decide its own index from bytes that do not travel with it,
    /// which is exactly the machine-dependence [`IgnoreSources::InTree`](crate::IgnoreSources::InTree) promises to
    /// keep out.
    ///
    /// Anything that is not a regular file fails rather than reading as no rules at all. Applying an empty rule set
    /// leaves the index holding every entry the file would have hidden, which is the one failure here that makes an
    /// index wrong by addition, so it is reported rather than absorbed.
    ///
    /// What that kind is gets settled on the opened handle rather than on the name. Checking the name and then reading
    /// it are two resolutions of one string, and a fifo renamed onto the name in between would block the second one
    /// forever — which is the hazard the check exists to prevent, so it cannot be left open by the check itself.
    fn read_ignore_file(&self, native_path: &Path) -> io::Result<Vec<u8>> {
        if fs::symlink_metadata(native_path)?.is_symlink() && !self.leads_into_tree(native_path)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "symlink to an ignore file outside the tree",
            ));
        }

        read_all(&open_regular(native_path, Symlinks::Follow)?)
    }

    /// Whether `native_path` resolves to a location inside the indexed tree.
    fn leads_into_tree(&self, native_path: &Path) -> io::Result<bool> {
        Ok(fs::canonicalize(native_path)?.starts_with(&self.root))
    }

    /// Whether the entry's name survives the walk's lossy decoding intact.
    ///
    /// A name that is not valid UTF-8 decodes to replacement characters, which collapses distinct names onto one path:
    /// two entries would share a lookup key, and joining either onto the root would open neither. The index drops such
    /// an entry and reports it, because a reported gap is answerable where a silently aliased entry is not.
    ///
    /// The scan is the cheap half. Only a name that already carries a replacement character pays for the syscall-free
    /// but allocating [`Entry::native_path`], which settles whether the character was in the name or put there by the
    /// decode.
    fn is_representable(&self, entry: &Entry<'_>) -> bool {
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

/// Where a symlink points, as the link itself stores it, or `None` for any other kind and for a target that will not
/// read or is not valid UTF-8.
///
/// A failure here is not recorded. An unresolvable target *is* what the tree holds rather than something the build
/// failed to read — [`Entry::target`](crate::Entry::target) carries that rule.
fn target(entry: &Entry<'_>, kind: EntryKind) -> Option<Box<str>> {
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

/// Reads an already-opened and already-classified handle to the end.
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

/// Whether a listed name is the control name `control`, which is always given in lowercase.
///
/// Case-insensitive on Windows and exact everywhere else, which is what git itself does. A Windows filesystem resolves
/// `.GIT` and `.GITIGNORE` through the lowercase names while preserving the spelling it lists, so an exact comparison
/// there would descend into a repository's own directory and skip an ignore file that governs a whole subtree. On unix
/// `.GIT` is an unrelated directory that git tracks, and folding case would wrongly hide it.
fn names_match(name: &str, control: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(control)
    } else {
        name == control
    }
}

/// The listed spelling of the control name `control`, or `None` when the directory has no such entry.
///
/// Returning the spelling rather than a bool is what lets the caller open the file it actually found: on Windows the
/// listing may say `.GITIGNORE`, and opening the lowercase name would work only by the filesystem's own folding.
fn control_name<'a>(dir: &Dir<'a>, control: &'a str) -> Option<&'a str> {
    if !cfg!(windows) {
        return dir.contains(control).then_some(control);
    }

    dir.names().find(|name| names_match(name, control))
}

/// The layer stack `dir`'s entries inherit.
///
/// A `.git` marks another repository's root, where git's rules stop: the stack restarts there rather than extending its
/// parent's, so the nested repository's own `.gitignore` files govern its contents and the outer ones do not. At the
/// index root this is a no-op, since the stack starts empty there anyway.
fn inherited_across_repository_boundary(dir: &Dir<'_>, parent: &Rules) -> Rules {
    if control_name(dir, DOT_GIT).is_some() {
        Rules::default()
    } else {
        parent.clone()
    }
}

impl Policy for Indexing {
    type State = Rules;
    type Value = Payload;

    fn enter(&self, dir: &Dir<'_>, parent_state: &Rules) -> Rules {
        if self.sources == IgnoreSources::None {
            return Rules::default();
        }

        let mut rules = inherited_across_repository_boundary(dir, parent_state);

        if let Some(name) = control_name(dir, GITIGNORE) {
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

        if !self.is_representable(entry) {
            return Visit::Skip;
        }

        if rules.ignores(entry.path(), kind == EntryKind::Dir) {
            return Visit::Skip;
        }

        Visit::Keep(Payload {
            kind,
            target: target(entry, kind),
        })
    }
}
