//! The path arguments of the file and symbol tools, read as the index spells a path: from the root, with `/` between
//! names. A path the index does not hold is a wrong call, answered with what is wrong with its spelling and the paths
//! the index holds nearest to it.

use kartta_fs::{EntryKind, Index, ListOptions, PathFault};

use crate::llmxml::List;

/// How many near paths the error for a path the index does not hold names.
const NEAR: usize = 5;

/// The characters the query glob dialect reads as a wildcard, a class or an alternation, so a glob that holds none of
/// them, nor the `\` escape, names one path.
pub(crate) const GLOB_CHARACTERS: [char; 6] = ['*', '?', '[', ']', '{', '}'];

/// What a path argument names: a file, a directory, or the start of the paths it selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wanted {
    File,
    Dir,
    Prefix,
}

/// `path` without its leading `./`: the index spells a path from the root, where `./a` names what `a` names.
pub(crate) fn relative(path: &str) -> &str {
    path.trim_start_matches("./")
}

/// The file argument `path` without its leading `./`, or the tool error for a path at which the index holds no file.
pub(crate) fn held_file<'p>(index: &Index, path: &'p str) -> Result<&'p str, String> {
    let path = relative(path);
    match index.entry(path).map(|entry| entry.kind()) {
        Some(EntryKind::Dir) => Err(format!(
            "{path:?} is a directory - pass one of its files, which list_dir lists"
        )),
        Some(_) => Ok(path),
        None => Err(unheld(index, path, Wanted::File)),
    }
}

/// The path prefix `prefix` without its leading `./`, or the tool error for a prefix no path the index holds starts
/// with.
pub(crate) fn held_prefix<'p>(index: &Index, prefix: &'p str) -> Result<&'p str, String> {
    let prefix = relative(prefix);
    let named = prefix.strip_suffix('/').unwrap_or(prefix);
    let held = !named.is_empty()
        && index
            .entry(named)
            .is_some_and(|entry| named.len() == prefix.len() || entry.kind() == EntryKind::Dir);
    if prefix.is_empty() || held {
        return Ok(prefix);
    }
    let (dir, start) = match prefix.rsplit_once('/') {
        Some(("", _)) => return Err(unheld(index, prefix, Wanted::Prefix)),
        Some(split) => split,
        None => ("", prefix),
    };
    let started = index
        .list(dir, &ListOptions::default().depth(1))
        .is_ok_and(|listing| {
            listing
                .items()
                .iter()
                .any(|listed| listed.entry().name().starts_with(start))
        });
    if started {
        Ok(prefix)
    } else {
        Err(unheld(index, prefix, Wanted::Prefix))
    }
}

/// The tool error for `path`, which the index does not hold as `wanted` asks: what is wrong with its spelling, or that
/// nothing is indexed there, and the paths of that kind the index holds nearest to it.
pub(crate) fn unheld(index: &Index, path: &str, wanted: Wanted) -> String {
    let fault = PathFault::of(path).map_or_else(
        || {
            let missed = if wanted == Wanted::Prefix {
                format!("no indexed path starts with {path:?}")
            } else {
                format!("nothing is indexed at {path:?}")
            };
            format!("{missed} - the index holds no ignored path, nor one made since it last read the tree")
        },
        |fault| {
            format!(
                "{path:?} {fault} - pass the path from the repository root, with / between names, as find_files \
                 prints it"
            )
        },
    );
    let near = near(index, path, wanted);
    if near.len() == 0 {
        return format!("{fault}; no indexed path is near it");
    }
    format!(
        "{fault}; the nearest the index holds:\n{}",
        near.into_text()
    )
}

/// Up to [`NEAR`] paths the index holds nearest to `path`, best first. The names of `path`, its drive, `.` and `..`
/// names dropped and `\` read as `/`, lead to the deepest directory the index holds: that directory unless a file is
/// asked for, then its entries whose names start as the next name does, longest start first, which takes in the path
/// the names spell when the index holds it. Last come the entries anywhere in the tree named as the last name is, of
/// the kind `wanted` asks, so a misplaced file is found too.
fn near(index: &Index, path: &str, wanted: Wanted) -> List {
    let names: Vec<&str> = path
        .split(['/', '\\'])
        .enumerate()
        .filter(|&(at, name)| !(matches!(name, "" | "." | "..") || at == 0 && is_drive(name)))
        .map(|(_, name)| name)
        .collect();
    let mut near = Near::default();
    let Some(&last) = names.last() else {
        return near.list;
    };
    let spelled = names.join("/");
    let ends: Vec<usize> = spelled.match_indices('/').map(|(at, _)| at).collect();
    let held = (1..names.len())
        .rev()
        .find(|&at| {
            index
                .entry(&spelled[..ends[at - 1]])
                .is_some_and(|entry| entry.kind() == EntryKind::Dir)
        })
        .unwrap_or(0);
    let dir = held.checked_sub(1).map_or("", |at| &spelled[..ends[at]]);
    if wanted != Wanted::File {
        near.push(dir);
    }
    let next = names[held];
    let started = next.len().min(STARTED);
    if let Ok(listing) = index.list(dir, &ListOptions::default().depth(1)) {
        let mut children: Vec<(usize, String)> = listing
            .items()
            .iter()
            .map(|listed| {
                let entry = listed.entry();
                (common_start(entry.name(), next), entry.path().to_owned())
            })
            .filter(|&(common, _)| common >= started)
            .collect();
        children.sort_by_key(|&(common, _)| std::cmp::Reverse(common));
        for (_, child) in &children {
            near.push(child);
        }
    }
    let mut paths = index.paths();
    for at in 0..index.len() {
        if near.full() {
            break;
        }
        let path = paths.get(at);
        let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
        if name.eq_ignore_ascii_case(last) {
            let path = path.to_owned();
            if index
                .entry(&path)
                .is_some_and(|entry| wanted.takes(entry.kind()))
            {
                near.push(&path);
            }
        }
    }
    near.list
}

/// How many leading bytes an entry's name shares with the asked name for the entry to be named near it.
const STARTED: usize = 3;

/// The paths a miss names, each once, at most [`NEAR`] of them.
#[derive(Debug, Default)]
struct Near {
    list: List,
    listed: Vec<String>,
}

impl Near {
    fn push(&mut self, path: &str) {
        if !self.full() && !path.is_empty() && !self.listed.iter().any(|listed| listed == path) {
            self.listed.push(path.to_owned());
            self.list.push(path);
        }
    }

    const fn full(&self) -> bool {
        self.list.len() >= NEAR
    }
}

impl Wanted {
    /// Whether an entry of `kind` is what a path of this kind names.
    fn takes(self, kind: EntryKind) -> bool {
        match self {
            Self::File => kind != EntryKind::Dir,
            Self::Dir => kind == EntryKind::Dir,
            Self::Prefix => true,
        }
    }
}

/// How many leading bytes `name` and `asked` share, counted in whole characters.
fn common_start(name: &str, asked: &str) -> usize {
    name.char_indices()
        .zip(asked.chars())
        .find(|&((_, left), right)| left != right)
        .map_or_else(|| name.len().min(asked.len()), |((at, _), _)| at)
}

/// Whether `name` is a drive such as `C:`.
const fn is_drive(name: &str) -> bool {
    matches!(name.as_bytes(), [drive, b':'] if drive.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_dot_slash_names_what_the_path_after_it_names() {
        assert_eq!(relative("./pkg/a.go"), "pkg/a.go");
        assert_eq!(relative("././pkg/"), "pkg/");
        assert_eq!(relative("pkg/./a.go"), "pkg/./a.go");
        assert_eq!(relative(".hidden/a"), ".hidden/a");
        assert_eq!(relative("."), ".");
    }
}
