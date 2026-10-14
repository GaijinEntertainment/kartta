use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::{fs, io};

use std::num::NonZeroU32;

use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit};

use crate::binary::BinaryExtensions;
use crate::build::{BuildError, IgnoreSources};
use crate::content::{Content, ContentId, ContentVisitor};
use crate::inspection::PROBE;
use crate::open::{Symlinks, open_regular};
use crate::rules::{DOT_GIT, GITIGNORE, Rules};

#[derive(Debug)]
pub(crate) struct Payload {
    pub(crate) kind: EntryKind,
    pub(crate) target: Option<Box<str>>,
    pub(crate) content: Option<ContentId>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Learned {
    pub(crate) size: u64,
    pub(crate) binary: Option<bool>,
}

pub(crate) struct Indexing<'a> {
    ignore_sources: IgnoreSources,
    canonical_root: PathBuf,
    // constraint: `Policy::enter` and `Policy::visit` return no `Result`, so a failure that costs the build information
    // collects here
    errors: Mutex<Vec<BuildError>>,
    reading: Option<Reading<'a>>,
}

struct Reading<'a> {
    visitor: &'a dyn ContentVisitor,
    binary_extensions: &'a BinaryExtensions,
    learned: Mutex<Vec<Learned>>,
}

impl<'a> Indexing<'a> {
    pub(crate) fn new(
        ignore_sources: IgnoreSources,
        root: &Path,
        reading: Option<(&'a dyn ContentVisitor, &'a BinaryExtensions)>,
    ) -> Self {
        Self {
            ignore_sources,
            canonical_root: fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()),
            errors: Mutex::new(Vec::new()),
            reading: reading.map(|(visitor, binary_extensions)| Reading {
                visitor,
                binary_extensions,
                learned: Mutex::new(Vec::new()),
            }),
        }
    }

    pub(crate) fn into_report(self) -> (Vec<BuildError>, Vec<Learned>) {
        let learned = self.reading.map_or_else(Vec::new, |reading| {
            reading
                .learned
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner)
        });
        let errors = self
            .errors
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);

        (errors, learned)
    }

    fn record(&self, error: BuildError) {
        self.errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error);
    }

    fn read_ignore_file(&self, native_path: &Path) -> io::Result<IgnoreFile> {
        let via_symlink = fs::symlink_metadata(native_path)?.is_symlink();
        if via_symlink && !self.leads_into_tree(native_path)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "symlink to an ignore file outside the tree",
            ));
        }

        // constraint: a name the check found regular is opened refusing a link, so a link renamed onto it after the check
        // fails the read rather than escaping the tree
        let symlinks = if via_symlink {
            Symlinks::Follow
        } else {
            Symlinks::Refuse
        };
        let (file, size) = open_regular(native_path, symlinks)?;

        Ok(IgnoreFile {
            bytes: read_all(&file)?,
            size,
            via_symlink,
        })
    }

    fn apply_ignore_file(&self, dir: &Dir<'_>, name: &str, rules: &mut Rules) -> IgnoreFileRead {
        let native_path = dir.native_path().join(name);
        let read = match self.read_ignore_file(&native_path) {
            Ok(read) => read,
            Err(source) => {
                self.record(BuildError::Ignore {
                    native_path,
                    source,
                });
                return IgnoreFileRead::Unread;
            }
        };

        if let Err(source) = rules.push(&read.bytes, dir.path()) {
            self.record(BuildError::Ignore {
                native_path,
                source,
            });
        }

        if self.reading.is_none() {
            return IgnoreFileRead::Read(None);
        }
        let path = child_path(dir.path(), name);
        if read.via_symlink || rules.ignores(&path, false) {
            return IgnoreFileRead::Read(None);
        }

        IgnoreFileRead::Read(self.hand_over_ignore_file(&path, &read))
    }

    fn hand_over_ignore_file(&self, path: &str, read: &IgnoreFile) -> Option<ContentId> {
        let reading = self.wanted(path)?;
        let bytes = &read.bytes;
        let binary = bytes[..bytes.len().min(PROBE)].contains(&0);
        let id = reading.keep(Learned {
            size: read.size,
            binary: Some(binary),
        });
        if !binary {
            reading.visitor.visit(Content { id, path, bytes });
        }

        Some(id)
    }

    fn wanted(&self, path: &str) -> Option<&Reading<'a>> {
        self.reading.as_ref().filter(|reading| {
            reading.visitor.wants(path) && !reading.binary_extensions.matches(path)
        })
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

    fn read_content(&self, entry: &Entry<'_>) -> Option<ContentId> {
        let path = entry.path();
        let reading = self.wanted(path)?;

        let native_path = entry.native_path();
        let (learned, readout) = match read_text(&native_path) {
            Ok(read) => read,
            Err(source) => {
                self.record(BuildError::Read {
                    native_path,
                    source,
                });
                return None;
            }
        };

        let id = reading.keep(learned);
        match readout {
            Readout::Text(bytes) => reading.visitor.visit(Content {
                id,
                path,
                bytes: &bytes,
            }),
            Readout::Binary => {}
            Readout::Failed(source) => self.record(BuildError::Read {
                native_path,
                source,
            }),
        }

        Some(id)
    }
}

struct IgnoreFile {
    bytes: Vec<u8>,
    size: u64,
    via_symlink: bool,
}

enum Readout {
    Text(Vec<u8>),
    Binary,
    Failed(io::Error),
}

impl Reading<'_> {
    #[expect(
        clippy::expect_used,
        reason = "ids are node-sized, and a walk records fewer than u32::MAX nodes, of which reads are a part"
    )]
    fn keep(&self, learned: Learned) -> ContentId {
        let count = {
            let mut kept = self.learned.lock().unwrap_or_else(PoisonError::into_inner);
            kept.push(learned);
            kept.len()
        };
        let count = u32::try_from(count).expect("fewer reads than a u32 counts");

        ContentId(NonZeroU32::MIN.saturating_add(count - 1))
    }
}

#[expect(
    clippy::verbose_file_reads,
    reason = "the point is to read the handle whose identity was checked, which fs::read cannot do -- it reopens the \
              name and reopening is what the check exists to avoid"
)]
fn read_text(native_path: &Path) -> io::Result<(Learned, Readout)> {
    let (mut file, size) = open_regular(native_path, Symlinks::Refuse)?;
    let whole = usize::try_from(size).unwrap_or(usize::MAX);
    let mut learned = Learned { size, binary: None };

    let mut bytes = Vec::with_capacity(whole.min(PROBE));
    if let Err(error) = Read::by_ref(&mut file)
        .take(PROBE as u64)
        .read_to_end(&mut bytes)
    {
        return Ok((learned, Readout::Failed(error)));
    }

    let binary = bytes.contains(&0);
    learned.binary = Some(binary);
    if binary {
        return Ok((learned, Readout::Binary));
    }
    let ended_within_probe = bytes.len() < PROBE;
    if ended_within_probe {
        return Ok((learned, Readout::Text(bytes)));
    }

    let rest = bytes
        .try_reserve_exact(whole.saturating_sub(bytes.len()))
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))
        .and_then(|()| file.read_to_end(&mut bytes));

    match rest {
        Ok(_) => Ok((learned, Readout::Text(bytes))),
        Err(error) => Ok((learned, Readout::Failed(error))),
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

#[derive(Debug, Clone, Default)]
pub(crate) struct DirState {
    rules: Rules,
    ignore_file: IgnoreFileRead,
}

#[derive(Debug, Clone, Copy, Default)]
enum IgnoreFileRead {
    #[default]
    Unread,
    Read(Option<ContentId>),
}

impl Policy for Indexing<'_> {
    type State = DirState;
    type Value = Payload;

    fn enter(&self, dir: &Dir<'_>, parent_state: &DirState) -> DirState {
        if self.ignore_sources == IgnoreSources::None {
            return DirState::default();
        }

        let mut state = DirState {
            rules: inherited_across_repository_boundary(dir, &parent_state.rules),
            ignore_file: IgnoreFileRead::Unread,
        };

        if let Some(name) = listed_control_name(dir, GITIGNORE) {
            state.ignore_file = self.apply_ignore_file(dir, name, &mut state.rules);
        }

        state
    }

    fn visit(&self, entry: &Entry<'_>, state: &DirState) -> Visit<Payload> {
        if names_match(entry.name(), DOT_GIT) {
            return Visit::Skip;
        }

        let kind = entry.kind();

        if !self.admits_name(entry) {
            return Visit::Skip;
        }

        if state.rules.ignores(entry.path(), kind == EntryKind::Dir) {
            return Visit::Skip;
        }

        let content = match state.ignore_file {
            _ if kind != EntryKind::File => None,
            IgnoreFileRead::Read(content) if names_match(entry.name(), GITIGNORE) => content,
            _ => self.read_content(entry),
        };

        Visit::Keep(Payload {
            kind,
            target: read_target(entry, kind),
            content,
        })
    }
}

fn child_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_carries_its_content_id_in_padding() {
        assert_eq!(
            size_of::<Payload>(),
            size_of::<(EntryKind, Option<Box<str>>)>(),
            "every walk node holds one of these until the index is built, so the id must cost no bytes"
        );
    }
}
