use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::AtomicU32;
use std::{fs, io};

use kartta_walk::{CancelToken, EntryKind, Tree, WalkError, WalkOptions, WalkOutcome, walk};
use rayon::prelude::*;

use super::splice::{Fresh, Pending, Replacement};
use super::{
    Asked, ChangedFiles, Finished, Held, Index, Previous, RefreshOutcome, depth_first_below,
    indexing, node_sized, order_errors, tree_order, unfinished_error,
};
use crate::build::{BuildError, IgnoreSources};
use crate::content::{ContentId, ContentVisitor};
use crate::content_index::{ContentIndexes, IdPositions, Target};
use crate::inspection::Inspection;
use crate::list::is_below;
use crate::policy::{DirState, Indexing, Learned, Numbering, Payload, Report, names_match};
use crate::rules::{DOT_GIT, GITIGNORE, Rules};
use crate::spelling;
use crate::stamp::{Seen, Stamp, Stamping, UNTRUSTED};

impl Index {
    /// Walks again only the parts of the tree at these root-relative forward-slash paths and makes this index what
    /// [`Index::refresh`] would make of them, handing `visitor` each wanted text file it reads in those parts, as a
    /// build does.
    ///
    /// Each path names something that changed: a file or directory added, removed, modified or replaced by another
    /// kind, as a filesystem watcher or a version control tool reports it. A directory is walked again with everything
    /// below it. The rest of the index is kept as it was, so a change no path names is not seen.
    ///
    /// A path that cannot be walked again alone is widened, and [`RefreshOutcome::walked`] reports what was walked:
    ///
    /// - to its parent, while that parent is not an indexed directory that is still a directory on disk, so a file in
    ///   a new directory walks the new directory;
    /// - while ignore rules apply, to the directory that holds it, for a `.gitignore` and for a `.git` below the root:
    ///   their rules govern everything below that directory;
    /// - to the whole tree, when it widens to the root, which makes this [`Index::refresh`].
    ///
    /// A path inside a `.git` directory names nothing the index holds and is skipped, as is a `.git` at the root, a
    /// `.git` anywhere when no ignore rules apply, and a path through a name in
    /// [`BuildOptions::pruned_names`](crate::BuildOptions::pruned_names).
    ///
    /// A path names the entry the disk holds. On macOS and Windows, where a volume can find an entry by a name in
    /// another letter case, or on macOS in another Unicode normalization form, than the one it holds, a part the index
    /// does not hold takes the name the filesystem answers for it, so `dir/f.rs` refreshes an indexed `Dir/f.rs`; and a
    /// part the disk holds under another name, as after a rename that changed only its case, is gone, as is a part no
    /// longer on disk. Asking costs a part found on disk one more call to the filesystem, and a part the index does not
    /// hold another, once a call however many paths come to it. Elsewhere a path is taken as spelled.
    ///
    /// While ignore rules apply, the rules for a part walked again come from the `.gitignore` files of the directories
    /// above it, read again once a call for each such directory, at two stats a directory and a third with the read of
    /// a `.gitignore` it holds; the visitor is not handed those files, which lie outside the parts. Content is read,
    /// carried over and numbered as [`Index::refresh`] does, on the index's [threads](Index::on_threads): the parts
    /// that are files are read there once every part has been looked at, except that a call with one such part reads
    /// it on the calling thread. What it reads is indexed in a segment of the content index of its own. Unlike
    /// [`Index::refresh`], this changes the index in place: every entry outside the walked parts moves once, so a call
    /// costs one pass over the index's per-entry arrays whatever the number of paths, and the old and new arrays are
    /// held at once while it runs. A call whose parts are neither indexed nor on disk, such as ignored paths and files
    /// already gone again, changes nothing and makes no such pass.
    ///
    /// # Errors
    ///
    /// [`BuildError::OutsideTree`] for a path with a `..` component, before anything is walked, and otherwise as
    /// [`Index::refresh`]. A returned error leaves the index answering as it did. Like [`Index::refresh`], the call
    /// stops for `cancel` only while it has something left to walk or read, or a content index to finish: it looks at
    /// `cancel` before it takes each part and reads each file, so a cancel that comes once the last files are being
    /// read, with no content index to finish, stops nothing, and the call completes.
    pub fn refresh_paths<'p>(
        &mut self,
        paths: impl IntoIterator<Item = &'p str>,
        visitor: Option<&dyn ContentVisitor>,
        cancel: Option<&CancelToken>,
    ) -> Result<RefreshOutcome, BuildError> {
        self.refresh_paths_walking(paths, Asked::new(visitor, cancel))
    }

    pub(crate) fn refresh_paths_walking<'p>(
        &mut self,
        paths: impl IntoIterator<Item = &'p str>,
        asked: Asked<'_>,
    ) -> Result<RefreshOutcome, BuildError> {
        let scopes = self.parts_of(paths)?;
        if scopes.is_empty() {
            return Ok(RefreshOutcome::new(
                Vec::new(),
                Vec::new(),
                scopes,
                ChangedFiles::default(),
            ));
        }
        if scopes[0].is_empty() {
            return self.refresh_walking(asked);
        }

        let Planned {
            pending,
            errors,
            changed,
        } = self.counting_handed(|index, handed| index.plan(&scopes, asked, handed))?;
        let retired = match pending {
            Some((pending, content)) => {
                if let Some(content) = content {
                    self.content = content;
                }
                self.install(pending).retired
            }
            None => Vec::new(),
        };

        Ok(RefreshOutcome::new(errors, retired, scopes, changed))
    }

    /// The parts of the tree [`Index::refresh_paths`] walks again for these paths, as [`RefreshOutcome::walked`] lists
    /// them, worked out without walking any: each path widened as that refresh widens it, the paths it skips left out,
    /// and none below another. So every path inside a directory the index does not hold, such as an ignored `target/`,
    /// comes to that one directory, and the empty path alone says the paths come to the whole tree.
    ///
    /// No ignore rule is read: a part the rules hide is still a part. A call stats each directory the index holds
    /// once, when a path's widening first meets it, to see that it is still a directory, and on macOS and Windows asks
    /// the filesystem for the name of each part the index does not hold, so a path in another case comes to the part
    /// the disk holds, as [`Index::refresh_paths`] says.
    ///
    /// # Errors
    ///
    /// [`BuildError::OutsideTree`] for a path with a `..` component.
    pub fn parts_of<'p>(
        &self,
        paths: impl IntoIterator<Item = &'p str>,
    ) -> Result<Vec<String>, BuildError> {
        let mut scopes = Vec::new();
        let mut lookups = Lookups::default();
        for path in paths {
            if let Some(scope) = self.scope_of(path, &mut lookups, 0)? {
                scopes.push(scope);
            }
        }
        scopes.sort_unstable_by(|left, right| tree_order(left, right));
        scopes.dedup_by(|later, kept| holds(kept, later));

        Ok(scopes)
    }

    fn scope_of(
        &self,
        path: &str,
        lookups: &mut Lookups,
        names_on_disk: usize,
    ) -> Result<Option<String>, BuildError> {
        let mut parts = Vec::new();
        for part in path.split('/').filter(|part| !matches!(*part, "" | ".")) {
            if part == ".." {
                return Err(BuildError::OutsideTree {
                    path: path.to_owned(),
                });
            }
            parts.push(part);
        }

        let rules_apply = self.ignore_sources != IgnoreSources::None;
        if let Some(at) = parts.iter().position(|part| names_match(part, DOT_GIT)) {
            let cuts_inherited_rules = rules_apply && at > 0 && at + 1 == parts.len();
            if !cuts_inherited_rules {
                return Ok(None);
            }
            parts.pop();
        } else if rules_apply
            && parts
                .last()
                .is_some_and(|part| names_match(part, GITIGNORE))
        {
            parts.pop();
        }
        if parts.iter().any(|part| self.pruned_names.matches(part)) {
            return Ok(None);
        }

        let mut scope = parts.join("/");
        while !scope.is_empty() {
            let parent = parent_of(&scope);
            let holds = match lookups.held.get(parent) {
                Some(&holds) => holds,
                None => *lookups
                    .held
                    .entry(parent.to_owned())
                    .or_insert_with(|| self.holds_dir(parent)),
            };
            if holds {
                break;
            }
            scope.truncate(parent.len());
        }

        let at = scope.bytes().filter(|&byte| byte == b'/').count();
        if scope.is_empty() || at < names_on_disk {
            return Ok(Some(scope));
        }
        let Some(name) = self.respelling(&scope, &mut lookups.respellings) else {
            return Ok(Some(scope));
        };
        parts[at] = &name;
        self.scope_of(&parts.join("/"), lookups, at + 1)
    }

    fn respelling(
        &self,
        scope: &str,
        respellings: &mut HashMap<String, Option<String>>,
    ) -> Option<String> {
        if let Some(respelling) = respellings.get(scope) {
            return respelling.clone();
        }
        let respelling = if self.position(scope).is_some() {
            None
        } else {
            spelling::on_disk(&self.root.join(scope)).filter(|name| name != name_of(scope))
        };
        respellings.insert(scope.to_owned(), respelling.clone());
        respelling
    }

    fn holds_dir(&self, dir: &str) -> bool {
        dir.is_empty()
            || (self
                .position(dir)
                .is_some_and(|at| self.kind_at(at) == EntryKind::Dir)
                && fs::symlink_metadata(self.root.join(dir))
                    .is_ok_and(|metadata| metadata.is_dir()))
    }

    fn range_of(&self, scope: &str) -> Range<usize> {
        match self.search(scope) {
            Ok(at) if self.kind_at(at) == EntryKind::Dir => at..self.past_subtree(at + 1, scope),
            Ok(at) => at..at + 1,
            Err(at) => at..at,
        }
    }

    fn plan(
        &self,
        scopes: &[String],
        asked: Asked<'_>,
        handed: &AtomicU32,
    ) -> Result<Planned, BuildError> {
        let ranges: Vec<Range<usize>> = scopes.iter().map(|scope| self.range_of(scope)).collect();
        let can_carry = asked.visitor.is_some() || self.content_storage.is_some();
        let previous = can_carry.then(|| Previous::within(self, &ranges)).flatten();
        let options = self.rebuild_options();
        let numbering = Numbering {
            first_id: self.next_content_id,
            handed: Some(handed),
            stamps: options.stamps.then(Stamping::starting_now),
            previous: previous.as_ref(),
        };
        let mut rewalk = Rewalk {
            index: self,
            policy: indexing(&self.root, &options, asked.visitor, numbering)?
                .descending(asked.descending),
            cancel: asked.cancel,
            errors: Vec::new(),
            rules: Vec::new(),
        };
        let found = rewalk.found(scopes)?;

        let Rewalk {
            policy, mut errors, ..
        } = rewalk;
        let Report {
            errors: read_errors,
            learned,
            stamps,
            ran_out_of_ids,
            content,
        } = policy.into_report();
        if ran_out_of_ids {
            return Err(BuildError::ContentIdsExhausted);
        }
        errors.extend(read_errors);
        order_errors(&mut errors);
        let changes_nothing = ranges
            .iter()
            .zip(&found)
            .all(|(range, found)| range.is_empty() && found.scope.is_none());
        if changes_nothing && self.stopped_refresh_ids().is_empty() {
            return Ok(Planned {
                pending: None,
                errors,
                changed: ChangedFiles::default(),
            });
        }

        let reads = self.next_content_id..self.next_content_id + node_sized(learned.len());
        let (finished, mut unindexed) = match content {
            Some(collected) => {
                let (finished, unindexed) = self.finished(collected, reads, asked.cancel)?;
                (Some(finished), unindexed)
            }
            None => (None, Vec::new()),
        };
        unindexed.sort_unstable();
        let facts = Facts {
            first_read: self.next_content_id,
            learned: &learned,
            stamps: stamps.as_deref(),
            previous: previous.as_ref(),
            unindexed: &unindexed,
        };
        let replacements = facts.replacements(scopes, ranges, &found);
        let mut changed = ChangedFiles::default();
        for replacement in &replacements {
            changed.add_between(
                Held::within(self, replacement.range.clone()),
                replacement.entries.as_slice(),
            );
        }
        let mut pending = self.spliced(&replacements, node_sized(learned.len()));
        let content = finished
            .map(|finished| self.content_after(finished, &mut pending, asked.cancel))
            .transpose()?;

        Ok(Planned {
            pending: Some((pending, content)),
            errors,
            changed,
        })
    }

    fn content_after(
        &self,
        finished: Finished,
        pending: &mut Pending,
        cancel: Option<&CancelToken>,
    ) -> Result<ContentIndexes, BuildError> {
        let Finished { segment, storage } = finished;
        let held_files = self.content.files().saturating_sub(pending.retired_text());
        let unindexed = std::mem::take(&mut pending.unindexed);
        let ids = IdPositions {
            first: self.first_content_id,
            positions: pending.positions(),
        };
        let target = Target {
            storage: &storage,
            cancel,
        };

        self.on_threads(|| {
            let held = self.content.segments.clone();
            ContentIndexes::joined(held, held_files, segment, unindexed).compacted_when_due(
                ids,
                pending.inspection(),
                target,
            )
        })
        .map_err(|unfinished| unfinished_error(&storage, unfinished))
    }
}

#[derive(Default)]
struct Lookups {
    held: HashMap<String, bool>,
    respellings: HashMap<String, Option<String>>,
}

struct Planned {
    pending: Option<(Pending, Option<ContentIndexes>)>,
    errors: Vec<BuildError>,
    changed: ChangedFiles,
}

struct Found {
    scope: Option<Payload>,
    below: Option<Tree<Payload>>,
}

impl Found {
    const GONE: Self = Self {
        scope: None,
        below: None,
    };
}

enum Part {
    Found(Found),
    File(Seen),
}

struct Rewalk<'i, 'a> {
    index: &'i Index,
    policy: Indexing<'a>,
    cancel: Option<&'a CancelToken>,
    errors: Vec<BuildError>,
    rules: Vec<(String, Rules)>,
}

impl Rewalk<'_, '_> {
    fn rules_for(&mut self, dir: &str) -> Rules {
        while self.rules.last().is_some_and(|(held, _)| !holds(held, dir)) {
            self.rules.pop();
        }
        if self.rules.is_empty() {
            let rules = self
                .policy
                .rules_in(&self.index.root, "", &Rules::default());
            self.rules.push((String::new(), rules));
        }

        while let Some((held, inherited)) = self.rules.last()
            && held.len() < dir.len()
        {
            let from = if held.is_empty() { 0 } else { held.len() + 1 };
            let end = dir[from..].find('/').map_or(dir.len(), |at| from + at);
            let next = &dir[..end];
            let rules = self
                .policy
                .rules_in(&self.index.root.join(next), next, inherited);
            self.rules.push((next.to_owned(), rules));
        }
        self.rules
            .last()
            .map(|(_, rules)| rules.clone())
            .unwrap_or_default()
    }

    fn found(&mut self, scopes: &[String]) -> Result<Vec<Found>, BuildError> {
        let mut parts = scopes
            .iter()
            .map(|scope| self.scope(scope))
            .collect::<Result<Vec<Part>, BuildError>>()?;
        let mut files = parts.iter().filter(|part| matches!(part, Part::File(_)));
        if files.nth(1).is_some() {
            self.read_on_threads(scopes, &mut parts)?;
        }

        Ok(parts
            .into_iter()
            .zip(scopes)
            .map(|(part, scope)| match part {
                Part::Found(found) => found,
                Part::File(seen) => self.file(scope, seen),
            })
            .collect())
    }

    fn read_on_threads(&self, scopes: &[String], parts: &mut [Part]) -> Result<(), BuildError> {
        self.index
            .on_threads(|| {
                parts
                    .par_iter_mut()
                    .zip(scopes)
                    .try_for_each(|(part, scope)| self.read(part, scope))
            })
            .ok_or(BuildError::Cancelled)
    }

    fn read(&self, part: &mut Part, path: &str) -> Option<()> {
        let Part::File(seen) = *part else {
            return Some(());
        };
        let cancelled = self.cancel.is_some_and(CancelToken::is_cancelled);
        (!cancelled).then(|| *part = Part::Found(self.file(path, seen)))
    }

    fn file(&self, path: &str, seen: Seen) -> Found {
        Found {
            scope: Some(self.policy.file(path, || self.index.root.join(path), seen)),
            below: None,
        }
    }

    fn scope(&mut self, scope: &str) -> Result<Part, BuildError> {
        if self.cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(BuildError::Cancelled);
        }
        let root = &self.index.root;
        let native_path = root.join(scope);
        let metadata = match fs::symlink_metadata(&native_path) {
            Ok(metadata) => metadata,
            Err(source) => {
                if source.kind() != io::ErrorKind::NotFound {
                    self.errors.push(BuildError::Walk(WalkError::FileType {
                        native_path,
                        source,
                    }));
                }
                return Ok(Part::Found(Found::GONE));
            }
        };
        if spelling::on_disk(&native_path).is_some_and(|name| name != name_of(scope)) {
            return Ok(Part::Found(Found::GONE));
        }
        let rules = self.rules_for(parent_of(scope));
        let is_dir = metadata.is_dir();
        if rules.ignores(scope, is_dir) {
            return Ok(Part::Found(Found::GONE));
        }
        if self.policy.reads()
            && let Some(seen) = Seen::of(&metadata)
        {
            return Ok(Part::File(seen));
        }

        let entry = self.policy.single(scope, &native_path, &metadata);
        let below = if is_dir {
            self.policy.descend(&native_path);
            self.below(scope, &DirState::below(rules))?
        } else {
            None
        };
        Ok(Part::Found(Found {
            scope: Some(entry),
            below,
        }))
    }

    fn below(&mut self, dir: &str, state: &DirState) -> Result<Option<Tree<Payload>>, BuildError> {
        let options = WalkOptions {
            cancel_token: self.cancel.cloned(),
            subtree: Some(dir.to_owned()),
            listed_stats: self.policy.may_carry(),
        };
        let walked = self
            .index
            .on_threads(|| walk(&self.index.root, &self.policy, state, &options));

        match walked {
            Ok(WalkOutcome {
                tree,
                errors,
                cancelled,
            }) => {
                if cancelled {
                    return Err(BuildError::Cancelled);
                }
                self.errors.extend(errors.into_iter().map(BuildError::Walk));
                Ok(Some(tree))
            }
            Err(error @ WalkError::ReadDir { .. }) => {
                self.errors.push(BuildError::Walk(error));
                Ok(None)
            }
            Err(error) => Err(BuildError::Walk(error)),
        }
    }
}

struct Facts<'a> {
    first_read: u32,
    learned: &'a [Learned],
    stamps: Option<&'a [Stamp]>,
    previous: Option<&'a Previous<'a>>,
    unindexed: &'a [Box<str>],
}

impl Facts<'_> {
    fn replacements(
        &self,
        scopes: &[String],
        ranges: Vec<Range<usize>>,
        found: &[Found],
    ) -> Vec<Replacement> {
        let mut replacements: Vec<Replacement> = Vec::with_capacity(scopes.len());
        for ((scope, range), found) in scopes.iter().zip(ranges).zip(found) {
            let entries = self.entries(scope, found);
            match replacements.last_mut() {
                Some(last) if last.range.end == range.start => {
                    last.range.end = range.end;
                    last.entries.extend(entries);
                }
                _ => replacements.push(Replacement { range, entries }),
            }
        }
        replacements
    }

    fn entries(&self, scope: &str, found: &Found) -> Vec<Fresh> {
        let mut entries = Vec::with_capacity(found.below.as_ref().map_or(0, Tree::len) + 1);
        if let Some(payload) = &found.scope {
            entries.push(self.fresh(scope, payload));
        }
        if let Some(tree) = &found.below {
            depth_first_below(tree, scope, |payload, path, _| {
                entries.push(self.fresh(path, payload));
            });
        }
        entries
    }

    fn fresh(&self, path: &str, payload: &Payload) -> Fresh {
        let mut fresh = Fresh::new(path.to_owned(), payload.kind);
        fresh.target.clone_from(&payload.target);
        if let Some(id) = payload.content
            && let Some((facts, stamp)) = self.of(id)
        {
            fresh.facts = facts;
            fresh.content = Some((id, stamp));
        }
        fresh.unindexed = self
            .unindexed
            .binary_search_by(|unindexed| (**unindexed).cmp(path))
            .is_ok();
        fresh
    }

    fn of(&self, id: ContentId) -> Option<((u64, u8), Stamp)> {
        let Some(read) = id.slot().checked_sub(self.first_read as usize) else {
            return self.previous?.carried(id);
        };
        let Learned { size, binary } = self.learned[read];
        let stamp = self.stamps.map_or(UNTRUSTED, |stamps| stamps[read]);

        Some((Inspection::raw_of(size, binary), stamp))
    }
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |at| &path[..at])
}

fn name_of(path: &str) -> &str {
    path.rfind('/').map_or(path, |at| &path[at + 1..])
}

fn holds(dir: &str, path: &str) -> bool {
    dir.is_empty() || path == dir || is_below(path, dir)
}
