use std::collections::HashMap;
use std::ops::Range;
use std::panic::resume_unwind;
use std::sync::atomic::AtomicU32;
use std::time::{Duration, Instant};
use std::{fs, io, thread};

use kartta_walk::{CancelToken, EntryKind, Tree, WalkError, WalkOptions, WalkOutcome, walk};
use rayon::prelude::*;

use super::splice::{Fresh, Pending, Replacement};
use super::{
    Asked, ChangedFiles, Finished, Held, Index, Previous, RefreshOutcome, depth_first_below,
    indexing, node_sized, order_errors, tree_order, unfinished_error,
};
use crate::build::{BuildError, IgnoreSources, PathFault};
use crate::content::{ContentId, ContentVisitor};
use crate::content_index::{ContentIndexes, IdPositions, Target};
use crate::inspection::Inspection;
use crate::list::is_below;
use crate::policy::{
    DirRules, DirState, Indexing, Learned, Numbering, Payload, Report, names_match,
};
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
    /// The parts are taken in the index's tree order, and a part below another is dropped, as [`Index::parts_of`]
    /// answers them.
    ///
    /// A path inside a `.git` directory names nothing the index holds and is skipped, as is a `.git` at the root, a
    /// `.git` anywhere when no ignore rules apply, and a path through a name in
    /// [`BuildOptions::pruned_names`](crate::BuildOptions::pruned_names). While ignore rules apply, a part below a
    /// directory that is a linked worktree of the root's repository, as [`Index::pruned_worktrees`] says, is walked as
    /// gone, at a read of that directory's `.git` and of the `commondir` it leads to, and the worktree is named again.
    ///
    /// A path names the entry the disk holds. On macOS and Windows, where a volume can find an entry by a name in
    /// another letter case, or on macOS in another Unicode normalization form, than the one it holds, a part the index
    /// does not hold takes the name the filesystem answers for it, so `dir/f.rs` refreshes an indexed `Dir/f.rs`; and a
    /// part the disk holds under another name, as after a rename that changed only its case, is gone, as is a part no
    /// longer on disk. Asking costs a part found on disk one more call to the filesystem, and a part the index does not
    /// hold another, once for each run of paths [`Index::parts_of`] meets it in however many paths come to it.
    /// Elsewhere a path is taken as spelled.
    ///
    /// The parts are stat'd one after another on the calling thread until one stat takes longer than a local disk
    /// answers, as on a network mount, and the rest side by side on up to 32 threads of the call's own, which wait for
    /// the filesystem rather than compute; [`Index::parts_of`] asks the filesystem the same way while it works the
    /// parts out. While ignore rules apply, the rules for a part walked again come from the `.gitignore` files of the
    /// directories above it, read again once a call for each such directory, at two stats a directory and a third with
    /// the read of a `.gitignore` it holds, once every part has been stat'd: one directory after another on the calling
    /// thread until one takes longer than a local disk answers, and the rest side by side on those threads. The
    /// visitor is not handed those files, which lie outside the parts. Content is read, carried over and numbered as
    /// [`Index::refresh`] does, on the index's [threads](Index::on_threads): the parts that are files are read there
    /// once every part has been looked at, except that a call with one such part reads it on the calling thread. What
    /// it reads is indexed in a segment of the content index of its own. Unlike [`Index::refresh`], this changes the
    /// index in place: every entry outside the walked parts moves once, so a call costs one pass over the index's
    /// per-entry arrays whatever the number of paths, and the old and new arrays are held at once while it runs. A call
    /// whose parts are neither indexed nor on disk, such as ignored paths and files already gone again, changes nothing
    /// and makes no such pass.
    ///
    /// # Errors
    ///
    /// [`BuildError::Misspelled`] for a path with a [`PathFault`], before anything is walked, and otherwise as
    /// [`Index::refresh`]. A returned error leaves the index answering as it did. Like [`Index::refresh`], the call
    /// stops for `cancel` only while it has something left to walk or read, or a content index to finish: it looks at
    /// `cancel` before it takes each part, reads the rules of each directory above the parts and reads each file, so a
    /// cancel that comes once the last files are being read, with no content index to finish, stops nothing, and the
    /// call completes.
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
                ChangedFiles::since(self),
            ));
        }
        if scopes[0].is_empty() {
            return self.refresh_walking(asked);
        }

        let Planned {
            pending,
            errors,
            changed,
            worktrees,
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
        self.note_worktrees(&scopes, worktrees);

        Ok(RefreshOutcome::new(errors, retired, scopes, changed))
    }

    /// The parts of the tree [`Index::refresh_paths`] walks again for these paths, as [`RefreshOutcome::walked`] lists
    /// them, worked out without walking any: each path widened as that refresh widens it, the paths it skips left out,
    /// and none below another. So every path inside a directory the index does not hold, such as an ignored `target/`,
    /// comes to that one directory, and the empty path alone says the paths come to the whole tree.
    ///
    /// No ignore rule is read: a part the rules hide is still a part. A call stats a directory the index holds when a
    /// path's widening first meets it, to see that it is still a directory, and on macOS and Windows asks the
    /// filesystem for the name of each part the index does not hold, so a path in another case comes to the part the
    /// disk holds, as [`Index::refresh_paths`] says. It takes the paths one after another on the calling thread until
    /// one of these asks takes longer than a local disk answers, as on a network mount, and the rest side by side on up
    /// to 32 threads of its own, one run of paths each. A run asks about each directory and part once, so a call on the
    /// calling thread alone asks once, and a directory that paths in several runs share is stat'd once by each.
    ///
    /// # Errors
    ///
    /// [`BuildError::Misspelled`] for a path with a [`PathFault`].
    pub fn parts_of<'p>(
        &self,
        paths: impl IntoIterator<Item = &'p str>,
    ) -> Result<Vec<String>, BuildError> {
        self.parts_within(paths, ROUND_TRIP)
    }

    fn parts_within<'p>(
        &self,
        paths: impl IntoIterator<Item = &'p str>,
        round_trip: Duration,
    ) -> Result<Vec<String>, BuildError> {
        let mut lookups = Lookups::within(round_trip);
        let mut scopes = Vec::new();
        let mut paths = paths.into_iter();
        for path in paths.by_ref() {
            scopes.extend(self.scope_of(path, &mut lookups, 0)?);
            if lookups.slow {
                break;
            }
        }
        let left: Vec<&str> = paths.collect();
        for scope in side_by_side(&left, |paths| {
            let mut lookups = lookups.clone();
            paths
                .iter()
                .map(|path| self.scope_of(path, &mut lookups, 0))
                .collect()
        }) {
            scopes.extend(scope?);
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
        if let Some(fault) = PathFault::of(path) {
            return Err(BuildError::Misspelled {
                path: path.to_owned(),
                fault,
            });
        }
        let mut parts: Vec<&str> = path
            .split('/')
            .filter(|part| !matches!(*part, "" | "."))
            .collect();

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
            let holds = if let Some(&holds) = lookups.held.get(parent) {
                holds
            } else {
                let holds = self.holds_dir(parent, lookups);
                lookups.held.insert(parent.to_owned(), holds);
                holds
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
        let Some(name) = self.respelling(&scope, lookups) else {
            return Ok(Some(scope));
        };
        parts[at] = &name;
        self.scope_of(&parts.join("/"), lookups, at + 1)
    }

    fn respelling(&self, scope: &str, lookups: &mut Lookups) -> Option<String> {
        if let Some(respelling) = lookups.respellings.get(scope) {
            return respelling.clone();
        }
        let respelling = if self.position(scope).is_some() {
            None
        } else {
            lookups
                .timed(|| spelling::on_disk(&self.root.join(scope)))
                .filter(|name| name != name_of(scope))
        };
        lookups
            .respellings
            .insert(scope.to_owned(), respelling.clone());
        respelling
    }

    fn holds_dir(&self, dir: &str, lookups: &mut Lookups) -> bool {
        dir.is_empty()
            || (self
                .position(dir)
                .is_some_and(|at| self.kind_at(at) == EntryKind::Dir)
                && lookups.timed(|| {
                    fs::symlink_metadata(self.root.join(dir))
                        .is_ok_and(|metadata| metadata.is_dir())
                }))
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
            targets,
            worktrees,
        } = policy.into_report();
        if ran_out_of_ids {
            return Err(BuildError::ContentIdsExhausted);
        }
        errors.extend(read_errors);
        order_errors(&mut errors);
        if changes_nothing(&ranges, &found) && self.stopped_refresh_ids().is_empty() {
            return Ok(Planned {
                pending: None,
                errors,
                changed: ChangedFiles::since(self),
                worktrees,
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
            targets: &targets,
        };
        let replacements = facts.replacements(scopes, ranges, &found);
        let mut changed = ChangedFiles::since(self);
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
            worktrees,
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

#[derive(Clone)]
struct Lookups {
    held: HashMap<String, bool>,
    respellings: HashMap<String, Option<String>>,
    round_trip: Duration,
    slow: bool,
}

impl Lookups {
    fn within(round_trip: Duration) -> Self {
        Self {
            held: HashMap::new(),
            respellings: HashMap::new(),
            round_trip,
            slow: false,
        }
    }

    fn timed<R>(&mut self, ask: impl FnOnce() -> R) -> R {
        let started = Instant::now();
        let answer = ask();
        self.slow |= started.elapsed() >= self.round_trip;
        answer
    }
}

struct Planned {
    pending: Option<(Pending, Option<ContentIndexes>)>,
    errors: Vec<BuildError>,
    changed: ChangedFiles,
    worktrees: Vec<Box<str>>,
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
}

impl Rewalk<'_, '_> {
    fn rules_above(&self, parts: &[Option<&str>]) -> Result<Vec<Option<Rules>>, BuildError> {
        if self.index.ignore_sources == IgnoreSources::None {
            return Ok(parts
                .iter()
                .map(|part| part.map(|_| Rules::default()))
                .collect());
        }

        let DirsAbove { dirs, holding } = dirs_above(parts);
        let added = self
            .read_rules(&dirs, ROUND_TRIP)
            .ok_or(BuildError::Cancelled)?;
        let none = Rules::default();
        let mut rules: Vec<Option<Rules>> = Vec::with_capacity(dirs.len());
        for (added, dir) in added.into_iter().zip(&dirs) {
            let inherited = dir.parent.map_or(Some(&none), |at| rules[at].as_ref());
            rules.push(inherited.and_then(|inherited| added.below(inherited)));
        }

        Ok(holding
            .into_iter()
            .map(|at| at.and_then(|at| rules[at].clone()))
            .collect())
    }

    fn read_rules(&self, dirs: &[DirAbove<'_>], round_trip: Duration) -> Option<Vec<DirRules>> {
        in_turn_then_side_by_side(dirs, round_trip, |dir| self.dir_rules(dir.path))
            .into_iter()
            .collect()
    }

    fn dir_rules(&self, dir: &str) -> Option<DirRules> {
        (!self.cancelled()).then(|| self.policy.rules_in(&self.index.root.join(dir), dir))
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(CancelToken::is_cancelled)
    }

    fn found(&mut self, scopes: &[String]) -> Result<Vec<Found>, BuildError> {
        let on_disk = self.on_disk(scopes, ROUND_TRIP)?;
        let present: Vec<Option<&str>> = scopes
            .iter()
            .zip(&on_disk)
            .map(|(scope, metadata)| metadata.as_ref().map(|_| scope.as_str()))
            .collect();
        let rules = self.rules_above(&present)?;
        let mut parts = scopes
            .iter()
            .zip(on_disk)
            .zip(rules)
            .map(|((scope, metadata), rules)| match metadata.zip(rules) {
                Some((metadata, rules)) => self.scope(scope, &metadata, rules),
                None => Ok(Part::Found(Found::GONE)),
            })
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
        (!self.cancelled()).then(|| *part = Part::Found(self.file(path, seen)))
    }

    fn file(&self, path: &str, seen: Seen) -> Found {
        Found {
            scope: Some(self.policy.file(path, || self.index.root.join(path), seen)),
            below: None,
        }
    }

    fn on_disk(
        &mut self,
        scopes: &[String],
        round_trip: Duration,
    ) -> Result<Vec<Option<fs::Metadata>>, BuildError> {
        in_turn_then_side_by_side(scopes, round_trip, |scope| self.stat(scope))
            .into_iter()
            .map(|stat| match stat {
                Err(BuildError::Cancelled) => Err(BuildError::Cancelled),
                Err(error) => {
                    self.errors.push(error);
                    Ok(None)
                }
                metadata => metadata,
            })
            .collect()
    }

    fn stat(&self, scope: &str) -> Result<Option<fs::Metadata>, BuildError> {
        if self.cancelled() {
            return Err(BuildError::Cancelled);
        }
        let native_path = self.index.root.join(scope);
        let metadata = match fs::symlink_metadata(&native_path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(BuildError::Walk(WalkError::FileType {
                    native_path,
                    source,
                }));
            }
        };
        if spelling::on_disk(&native_path).is_some_and(|name| name != name_of(scope)) {
            return Ok(None);
        }

        Ok(Some(metadata))
    }

    fn scope(
        &mut self,
        scope: &str,
        metadata: &fs::Metadata,
        rules: Rules,
    ) -> Result<Part, BuildError> {
        if self.cancelled() {
            return Err(BuildError::Cancelled);
        }
        let native_path = self.index.root.join(scope);
        let is_dir = metadata.is_dir();
        if rules.ignores(scope, is_dir) {
            return Ok(Part::Found(Found::GONE));
        }
        if self.policy.reads()
            && let Some(seen) = Seen::of(metadata)
        {
            return Ok(Part::File(seen));
        }

        let entry = self.policy.single(scope, &native_path, metadata);
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
        let mut options = WalkOptions::default();
        options.cancel_token = self.cancel.cloned();
        options.subtree = Some(dir.to_owned());
        options.listed_stats = self.policy.may_carry();
        let walked = self
            .index
            .on_threads(|| walk(&self.index.root, &self.policy, state, &options));

        match walked {
            Ok(WalkOutcome {
                tree,
                errors,
                cancelled,
                ..
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
    targets: &'a [Box<str>],
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
        let mut fresh = Fresh::new(path.to_owned(), payload.kind());
        if let Payload::Symlink(Some(target)) = *payload {
            fresh.target = Some(self.targets[target.at()].clone());
        }
        if let Payload::File(Some(id)) = *payload
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

const ROUND_TRIP: Duration = Duration::from_micros(200);
const WAITING_THREADS: usize = 32;

// constraint: a local disk answers faster than a thread starts, a network mount in a round trip
fn in_turn_then_side_by_side<T: Sync, R: Send>(
    items: &[T],
    round_trip: Duration,
    ask: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let mut answers = Vec::with_capacity(items.len());
    for item in items {
        let started = Instant::now();
        answers.push(ask(item));
        if started.elapsed() >= round_trip {
            break;
        }
    }
    answers.extend(side_by_side(&items[answers.len()..], |items| {
        items.iter().map(&ask).collect()
    }));

    answers
}

// constraint: a network mount overlaps as many round trips as there are threads waiting for one
fn side_by_side<T: Sync, R: Send>(items: &[T], ask: impl Fn(&[T]) -> Vec<R> + Sync) -> Vec<R> {
    match items.len() {
        0 => return Vec::new(),
        1 => return ask(items),
        _ => {}
    }
    let ask = &ask;
    thread::scope(|scope| {
        let waiting: Vec<_> = items
            .chunks(items.len().div_ceil(WAITING_THREADS))
            .map(|chunk| {
                let asked = thread::Builder::new().spawn_scoped(scope, move || ask(chunk));
                (chunk, asked)
            })
            .collect();
        let mut answers = Vec::with_capacity(items.len());
        for (chunk, asked) in waiting {
            answers.extend(asked.map_or_else(
                |_| ask(chunk),
                |thread| thread.join().unwrap_or_else(|panic| resume_unwind(panic)),
            ));
        }
        answers
    })
}

struct DirAbove<'p> {
    path: &'p str,
    parent: Option<usize>,
}

struct DirsAbove<'p> {
    dirs: Vec<DirAbove<'p>>,
    holding: Vec<Option<usize>>,
}

fn dirs_above<'p>(parts: &[Option<&'p str>]) -> DirsAbove<'p> {
    let mut dirs: Vec<DirAbove<'p>> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut holding = Vec::with_capacity(parts.len());
    for part in parts {
        let Some(part) = part else {
            holding.push(None);
            continue;
        };
        let dir = parent_of(part);
        while stack.last().is_some_and(|&at| !holds(dirs[at].path, dir)) {
            stack.pop();
        }
        if stack.is_empty() {
            stack.push(dirs.len());
            dirs.push(DirAbove {
                path: "",
                parent: None,
            });
        }
        while let Some(&held) = stack.last()
            && dirs[held].path.len() < dir.len()
        {
            let held_path = dirs[held].path;
            let from = if held_path.is_empty() {
                0
            } else {
                held_path.len() + 1
            };
            let end = dir[from..].find('/').map_or(dir.len(), |at| from + at);
            stack.push(dirs.len());
            dirs.push(DirAbove {
                path: &dir[..end],
                parent: Some(held),
            });
        }
        holding.push(stack.last().copied());
    }

    DirsAbove { dirs, holding }
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |at| &path[..at])
}

fn name_of(path: &str) -> &str {
    path.rfind('/').map_or(path, |at| &path[at + 1..])
}

fn changes_nothing(ranges: &[Range<usize>], found: &[Found]) -> bool {
    ranges
        .iter()
        .zip(found)
        .all(|(range, found)| range.is_empty() && found.scope.is_none())
}

pub(super) fn holds(dir: &str, path: &str) -> bool {
    dir.is_empty() || path == dir || is_below(path, dir)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::build::BuildOptions;

    fn rewalk<'i>(index: &'i Index, options: &'i BuildOptions) -> Rewalk<'i, 'i> {
        let numbering = Numbering {
            first_id: index.next_content_id,
            handed: None,
            stamps: None,
            previous: None,
        };
        Rewalk {
            index,
            policy: indexing(&index.root, options, None, numbering).expect("a policy"),
            cancel: None,
            errors: Vec::new(),
        }
    }

    #[test]
    fn the_asks_left_after_a_slow_one_wait_for_their_answers_at_once() {
        let waiting = AtomicU32::new(0);
        let items = [0, 1, 2, 3];

        let met_the_others = in_turn_then_side_by_side(&items, Duration::ZERO, |&item| {
            item == 0 || all_wait(&waiting, 3)
        });

        assert_eq!(met_the_others, [true; 4]);
    }

    #[test]
    fn asks_side_by_side_wait_on_at_most_the_threads_a_call_starts() {
        let asking = AtomicU32::new(0);
        let most = AtomicU32::new(0);
        let items: Vec<usize> = (0..100).collect();

        let answers = side_by_side(&items, |items| {
            let now = asking.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(20));
            asking.fetch_sub(1, Ordering::SeqCst);
            items.to_vec()
        });

        assert_eq!(answers, items);
        assert!(most.into_inner() as usize <= WAITING_THREADS);
    }

    fn all_wait(waiting: &AtomicU32, count: u32) -> bool {
        waiting.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(10);
        while waiting.load(Ordering::SeqCst) < count {
            if Instant::now() > deadline {
                return false;
            }
            thread::yield_now();
        }
        true
    }

    #[test]
    fn parts_and_their_stats_asked_side_by_side_are_those_asked_one_after_another() {
        let dir = tempfile::tempdir().expect("create temp dir");
        for path in [
            "a/b/c.rs",
            "a/n/.git/HEAD",
            "d/e.txt",
            "f.rs",
            "g/h.rs",
            "i/j.rs",
        ] {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().expect("a parent")).expect("create directory");
            fs::write(full, "").expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build")
            .index;
        fs::remove_file(dir.path().join("d/e.txt")).expect("remove a file");
        fs::remove_dir_all(dir.path().join("g")).expect("remove a directory");
        fs::write(dir.path().join("g"), "").expect("write a file over a directory");
        for path in ["a/b/new.rs", "new/i.rs"] {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().expect("a parent")).expect("create directory");
            fs::write(full, "").expect("write file");
        }
        let paths = [
            "a/b/c.rs",
            "d/e.txt",
            "g/h.rs",
            "a/b/new.rs",
            "new/i.rs",
            "a/n/.git",
            "i/.gitignore",
            "gone/k.rs",
            "A/B/C.rs",
            "f.rs",
        ];

        let in_turn = index.parts_within(paths, Duration::MAX).expect("parts");
        let side_by_side = index.parts_within(paths, Duration::ZERO).expect("parts");
        let options = index.rebuild_options();
        let mut rewalk = rewalk(&index, &options);
        let stats_in_turn = rewalk.on_disk(&in_turn, Duration::MAX).expect("stats");
        let stats_side_by_side = rewalk.on_disk(&in_turn, Duration::ZERO).expect("stats");
        let outside = ["a/b/c.rs", "f.rs", "../l.rs", "a/../../m.rs"];

        assert_eq!(in_turn, side_by_side);
        for part in [
            "a/b/c.rs",
            "a/b/new.rs",
            "a/n",
            "d/e.txt",
            "f.rs",
            "g",
            "gone",
            "i",
            "new",
        ] {
            assert!(
                in_turn.iter().any(|scope| scope == part),
                "{part} in {in_turn:?}"
            );
        }
        assert_eq!(
            format!("{stats_in_turn:?}"),
            format!("{stats_side_by_side:?}")
        );
        assert_eq!(
            format!("{:?}", index.parts_within(outside, Duration::MAX)),
            format!("{:?}", index.parts_within(outside, Duration::ZERO)),
        );
    }

    #[test]
    fn rules_read_on_the_threads_are_those_read_one_directory_after_another() {
        let dir = tempfile::tempdir().expect("create temp dir");
        for (path, text) in [
            (".gitignore", "*.log\n"),
            ("a/.gitignore", "*.tmp\n"),
            ("a/b/c.rs", ""),
            ("a/n/.git/HEAD", ""),
            ("a/n/.gitignore", "*.rs\n"),
            ("a/n/d/e.txt", ""),
            ("f/g.rs", ""),
        ] {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().expect("a parent")).expect("create directory");
            fs::write(full, text).expect("write file");
        }
        let index = Index::build(dir.path(), &BuildOptions::default())
            .expect("build")
            .index;
        let options = index.rebuild_options();
        let rewalk = rewalk(&index, &options);
        let DirsAbove { dirs, .. } =
            dirs_above(&[Some("a/b/c.rs"), Some("a/n/d/e.txt"), Some("f/g.rs")]);

        let in_turn = rewalk.read_rules(&dirs, Duration::MAX);
        let on_threads = rewalk.read_rules(&dirs, Duration::ZERO);

        assert_eq!(dirs.len(), 6);
        assert_eq!(format!("{in_turn:?}"), format!("{on_threads:?}"));
    }
}
