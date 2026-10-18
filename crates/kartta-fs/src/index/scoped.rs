use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::{fs, io};

use kartta_walk::{CancelToken, EntryKind, Tree, WalkError, WalkOptions, WalkOutcome, walk};

use super::splice::{Fresh, Pending, Replacement};
use super::{
    ChangedFiles, Held, Index, Previous, RefreshOutcome, depth_first_below, indexing, node_sized,
    order_errors, tree_order, unfinished_error,
};
use crate::build::{BuildError, IgnoreSources};
use crate::content::{ContentId, ContentVisitor};
use crate::content_index::{
    ContentIndex, ContentIndexStorage, ContentIndexes, IdPositions, Target,
};
use crate::inspection::Inspection;
use crate::policy::{
    Collected, DirState, Indexing, Learned, Numbering, Payload, Report, names_match,
};
use crate::rules::{DOT_GIT, GITIGNORE, Rules};
use crate::stamp::{Stamp, Stamping, UNTRUSTED};

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
    /// While ignore rules apply, the rules for a part walked again come from the `.gitignore` files of the directories
    /// above it, read again once a call for each such directory, at two stats a directory and a third with the read of
    /// a `.gitignore` it holds; the visitor is not handed those files, which lie outside the parts. Content is read,
    /// carried over and numbered as [`Index::refresh`] does, and what it reads is indexed in a segment of the content
    /// index of its own. Unlike [`Index::refresh`], this changes the index in place: every entry outside the walked
    /// parts moves once, so a call costs one pass over the index's per-entry arrays whatever the number of paths, and
    /// the old and new arrays are held at once while it runs. A call whose parts are neither indexed nor on disk, such
    /// as ignored paths and files already gone again, changes nothing and makes no such pass.
    ///
    /// # Errors
    ///
    /// [`BuildError::OutsideTree`] for a path with a `..` component, before anything is walked, and otherwise as
    /// [`Index::refresh`]. A returned error leaves the index answering as it did.
    pub fn refresh_paths<'p>(
        &mut self,
        paths: impl IntoIterator<Item = &'p str>,
        visitor: Option<&dyn ContentVisitor>,
        cancel: Option<&CancelToken>,
    ) -> Result<RefreshOutcome, BuildError> {
        let scopes = self.scopes(paths)?;
        if scopes.is_empty() {
            return Ok(RefreshOutcome::new(
                Vec::new(),
                Vec::new(),
                scopes,
                ChangedFiles::default(),
            ));
        }
        if scopes[0].is_empty() {
            return self.refresh(visitor, cancel);
        }

        let asked = Asked { visitor, cancel };
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

    fn scopes<'p>(
        &self,
        paths: impl IntoIterator<Item = &'p str>,
    ) -> Result<Vec<String>, BuildError> {
        let mut scopes = Vec::new();
        let mut held = HashMap::new();
        for path in paths {
            if let Some(scope) = self.scope_of(path, &mut held)? {
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
        held: &mut HashMap<String, bool>,
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
            let holds = match held.get(parent) {
                Some(&holds) => holds,
                None => *held
                    .entry(parent.to_owned())
                    .or_insert_with(|| self.holds_dir(parent)),
            };
            if holds {
                break;
            }
            scope.truncate(parent.len());
        }
        Ok(Some(scope))
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
            policy: indexing(&self.root, &options, asked.visitor, numbering)?,
            cancel: asked.cancel,
            errors: Vec::new(),
            rules: Vec::new(),
        };
        let found = scopes
            .iter()
            .map(|scope| rewalk.scope(scope))
            .collect::<Result<Vec<Found>, BuildError>>()?;

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
        if changes_nothing && !self.holds_unreported_ids() {
            return Ok(Planned {
                pending: None,
                errors,
                changed: ChangedFiles::default(),
            });
        }

        let (finished, mut unindexed) = match content {
            Some(collected) => {
                let (finished, unindexed) =
                    self.finished(collected, node_sized(learned.len()), asked.cancel)?;
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

    fn holds_unreported_ids(&self) -> bool {
        self.first_content_id + node_sized(self.content_positions.len()) < self.next_content_id
    }

    fn finished(
        &self,
        collected: Collected,
        reads: u32,
        cancel: Option<&CancelToken>,
    ) -> Result<(Finished, Vec<Box<str>>), BuildError> {
        let Collected {
            collector,
            pairs,
            unindexed,
        } = collected;
        let storage = collector.storage().clone();
        let segment = self
            .on_threads(|| collector.finish(pairs, reads, cancel))
            .map_err(|unfinished| unfinished_error(&storage, unfinished))?;
        let finished = Finished {
            segment: segment.with_first_id(self.next_content_id),
            storage,
        };

        Ok((finished, unindexed))
    }

    fn content_after(
        &self,
        finished: Finished,
        pending: &mut Pending,
        cancel: Option<&CancelToken>,
    ) -> Result<ContentIndexes, BuildError> {
        let Finished { segment, storage } = finished;
        let read = segment.files();
        let files = self.content.files().saturating_sub(pending.retired_text()) + read;
        let mut segments = self.content.segments.clone();
        if segments.is_empty() || read > 0 {
            segments.push(Arc::new(segment));
        }
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
            ContentIndexes::new(segments, unindexed, files).compacted_when_due(
                ids,
                pending.inspection(),
                target,
            )
        })
        .map_err(|unfinished| unfinished_error(&storage, unfinished))
    }
}

#[derive(Clone, Copy)]
struct Asked<'a> {
    visitor: Option<&'a dyn ContentVisitor>,
    cancel: Option<&'a CancelToken>,
}

struct Planned {
    pending: Option<(Pending, Option<ContentIndexes>)>,
    errors: Vec<BuildError>,
    changed: ChangedFiles,
}

struct Finished {
    segment: ContentIndex,
    storage: ContentIndexStorage,
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

    fn scope(&mut self, scope: &str) -> Result<Found, BuildError> {
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
                return Ok(Found::GONE);
            }
        };
        let rules = self.rules_for(parent_of(scope));
        let is_dir = metadata.is_dir();
        if rules.ignores(scope, is_dir) {
            return Ok(Found::GONE);
        }

        let entry = self.policy.single(scope, &native_path, &metadata);
        let below = if is_dir {
            self.below(scope, &DirState::below(rules))?
        } else {
            None
        };
        Ok(Found {
            scope: Some(entry),
            below,
        })
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

fn holds(dir: &str, path: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}
