use std::collections::{HashMap, HashSet};
use std::path::Path;

use foldhash::fast::FixedState;
use kartta_fs::{ContentId, Index, RefreshOutcome};
use kartta_lang::Language;

use crate::engine::{EngineRefresh, Family};
use crate::extracting::{Extracted, FileCounts, claimant};
use crate::repository::{
    Configuration, FileTables, IndexRepository, Query, asked, reached_by_presence,
};
use crate::resolving::{Resolvers, Resolving, ToResolve};

/// What an engine built with stamps keeps to follow its tree: for each file its graphs hold, the questions its
/// resolution put to the repository, for each family, its resolver and what that resolver asked while it was built,
/// and the tables the last refresh's repository built to find files in.
#[derive(Debug, Default)]
pub(crate) struct Following {
    files: HashMap<ContentId, Kept, FixedState>,
    asked: Asked,
    resolvers: Resolvers,
    tables: FileTables,
}

/// A file a resolution put into a graph, and the keys of the questions it asked.
#[derive(Debug)]
pub(crate) struct Followed {
    pub(crate) path: Box<str>,
    pub(crate) language: usize,
    pub(crate) cached: bool,
    pub(crate) asked: Box<[u32]>,
}

/// A followed file as the engine keeps it, its keys a run of [`Asked`], or [`NO_RUN`] when it asked nothing.
#[derive(Debug)]
struct Kept {
    path: Box<str>,
    language: usize,
    cached: bool,
    run: u32,
}

/// The run of a file that asked nothing: no refresh can reach it through its keys, so it takes no run.
const NO_RUN: u32 = u32::MAX;

/// The keys of every kept file in one buffer, a run a file in the order the files came, so that a refresh reads them
/// in the order of memory. Every run holds a key, and a run whose file left stays until such runs hold a quarter of
/// the keys.
#[derive(Debug, Default)]
struct Asked {
    keys: Vec<u32>,
    runs: Vec<Run>,
    retired: usize,
}

/// Where a run of [`Asked::keys`] ends, and the file it holds the keys of until that file leaves.
#[derive(Debug, Clone, Copy)]
struct Run {
    owner: Option<ContentId>,
    end: u32,
}

/// The parts of an engine a refresh brings up to its refreshed index.
pub(crate) struct Refreshing<'e> {
    pub(crate) index: &'e Index,
    pub(crate) languages: &'e [Box<dyn Language>],
    pub(crate) family_of: &'e [usize],
    pub(crate) families: &'e mut [Family],
    pub(crate) file_counts: &'e mut [FileCounts],
}

/// What the index's refresh did, and what its walk handed the engine's visitor.
pub(crate) struct Walked {
    pub(crate) outcome: RefreshOutcome,
    pub(crate) extracted: Vec<Extracted>,
    pub(crate) counts: Vec<FileCounts>,
}

type Keys = HashSet<u32, FixedState>;

/// The keys a refresh reached, behind a bit set at the low bits of each, so that most keys a file asked are told apart
/// from them without a probe of the set.
struct Reached {
    keys: Keys,
    bits: Box<[u64]>,
}

const BITS_PER_KEY: usize = 32;

struct Plan {
    stale: Vec<Kept>,
    reread: Vec<Kept>,
    reread_ids: Vec<ContentId>,
    work: Vec<ToResolve>,
    reconfigured: Vec<usize>,
}

impl Following {
    pub(crate) fn new(followed: Vec<(ContentId, Followed)>, resolvers: Resolvers) -> Self {
        let mut following = Self {
            resolvers,
            ..Self::default()
        };
        following.keep(followed);
        following
    }

    /// Takes in the files a resolution put into the graphs, their keys appended to [`Asked`].
    fn keep(&mut self, followed: Vec<(ContentId, Followed)>) {
        self.files.reserve(followed.len());
        self.asked.reserve(&followed);
        for (id, followed) in followed {
            let run = self.asked.push(id, &followed.asked);
            let Followed {
                path,
                language,
                cached,
                ..
            } = followed;
            self.files.insert(
                id,
                Kept {
                    path,
                    language,
                    cached,
                    run,
                },
            );
        }
        if self.asked.retired > self.asked.keys.len() / 4 {
            self.asked.compact(&mut self.files);
        }
    }

    /// Takes `id` out of the kept files, its run retired.
    fn forget(&mut self, id: ContentId) -> Option<Kept> {
        let kept = self.files.remove(&id)?;
        self.asked.retire(kept.run);
        Some(kept)
    }

    /// Brings `engine`'s graphs up to its refreshed index: drops the files the refresh retired, inserts what its walk
    /// handed over, and resolves again from a second read every file whose questions a change answers differently,
    /// every file of a family whose resolver now derives another configuration among them. A family's kept resolver
    /// resolves them unless the refresh reached what it read as it was built.
    pub(crate) fn refresh(&mut self, engine: Refreshing<'_>, walked: Walked) -> EngineRefresh {
        let Walked {
            outcome,
            extracted,
            counts,
        } = walked;
        let Refreshing {
            index,
            languages,
            family_of,
            families,
            file_counts,
        } = engine;
        let resolving = Resolving {
            index,
            languages,
            family_of,
            cancel: None,
            recording: true,
        };
        let tables = std::mem::take(&mut self.tables).refreshed(index, &outcome);
        let repository = IndexRepository::recording(index, tables);
        let mut stale: Vec<Kept> = outcome.retired().filter_map(|id| self.forget(id)).collect();
        let handed = resolving.placed(extracted);
        let reached = reached(index, &outcome, &handed);
        let plan = self.plan(&resolving, &repository, (&outcome, &reached, handed));
        stale.extend(plan.stale);
        for followed in stale.iter().chain(&plan.reread) {
            families[family_of[followed.language]]
                .graph
                .remove(&followed.path);
            file_counts[followed.language].held -= 1;
        }
        let inserted = resolving.insert_all(plan.work, (&self.resolvers, &repository), families);
        self.tables = repository.into_tables();
        let reread: HashSet<ContentId, FixedState> = plan.reread_ids.iter().copied().collect();
        let resolved_again = inserted
            .followed
            .iter()
            .filter(|(id, _)| reread.contains(id))
            .count();
        let extracted = inserted.followed.len() - resolved_again;
        let held: HashSet<&str, FixedState> = inserted
            .followed
            .iter()
            .map(|(_, followed)| &*followed.path)
            .collect();
        let dropped = stale
            .iter()
            .chain(&plan.reread)
            .filter(|followed| !held.contains(&*followed.path))
            .count();
        self.keep(inserted.followed);
        count(
            languages,
            file_counts,
            (&outcome, &counts, &stale),
            &inserted.held,
        );

        EngineRefresh {
            index: outcome,
            errors: inserted.errors,
            extracted,
            resolved_again,
            dropped,
            reconfigured: plan
                .reconfigured
                .iter()
                .map(|&family| families[family].name.clone())
                .collect(),
            widened: None,
        }
    }

    /// The work of a refresh, with the resolver of each family it reaches built again: of a family whose kept resolver
    /// read what the refresh reached, and of a family with a file to resolve and no kept resolver.
    fn plan(
        &mut self,
        resolving: &Resolving<'_>,
        repository: &IndexRepository<'_>,
        (outcome, reached, handed): (&RefreshOutcome, &Reached, Vec<ToResolve>),
    ) -> Plan {
        // constraint: kartta_lang::Resolver reads no file, so only a file added or removed reaches a file's keys
        let mut dependents: Vec<ContentId> =
            if outcome.added.is_empty() && outcome.removed.is_empty() {
                Vec::new()
            } else {
                self.asked.reaching(reached).collect()
            };
        let family_of = resolving.family_of;
        let mut wanted = vec![false; resolving.families()];
        for file in &handed {
            wanted[family_of[file.language]] = true;
        }
        for id in &dependents {
            wanted[family_of[self.files[id].language]] = true;
        }
        for (family, wanted) in wanted.iter_mut().enumerate() {
            if let Some(Some(configuration)) = self.resolvers.configurations.get(family) {
                *wanted = reaches(configuration, (reached, outcome), resolving.index.root());
            }
        }

        let rebuilt = resolving.resolvers(repository, &wanted);
        let reconfigured = self.reconfigure(rebuilt);
        dependents.extend(
            self.files
                .iter()
                .filter(|(_, followed)| reconfigured.contains(&family_of[followed.language]))
                .map(|(&id, _)| id),
        );
        dependents.sort_unstable();
        dependents.dedup();

        let mut plan = Plan {
            stale: Vec::new(),
            reread: Vec::new(),
            reread_ids: Vec::new(),
            work: handed,
            reconfigured,
        };
        for id in dependents {
            let Some(followed) = self.forget(id) else {
                continue;
            };
            match resolving.index.content_position(id) {
                Some(position) => {
                    plan.work.push(ToResolve {
                        id,
                        position,
                        language: followed.language,
                        cached: followed.cached,
                        encoded: None,
                    });
                    plan.reread_ids.push(id);
                    plan.reread.push(followed);
                }
                None => plan.stale.push(followed),
            }
        }
        plan
    }

    /// Keeps each resolver of `rebuilt` in place of its family's, and answers the families whose configuration it
    /// read changed in what it derived: whose answers differ, and whose rebuilt resolver does not resolve as the kept.
    fn reconfigure(&mut self, rebuilt: Resolvers) -> Vec<usize> {
        let families = rebuilt.of_family.len();
        let kept = &mut self.resolvers;
        if kept.of_family.len() < families {
            kept.of_family.resize_with(families, || None);
            kept.configurations.resize_with(families, || None);
        }
        let mut reconfigured = Vec::new();
        let built = rebuilt.of_family.into_iter().zip(rebuilt.configurations);
        for (family, built) in built.enumerate() {
            let (Some(resolver), Some(configuration)) = built else {
                continue;
            };
            if kept.configurations[family]
                .as_ref()
                .is_some_and(|kept| kept.answers != configuration.answers)
                && !kept.of_family[family]
                    .as_deref()
                    .is_some_and(|kept| resolver.resolves_as(kept))
            {
                reconfigured.push(family);
            }
            kept.of_family[family] = Some(resolver);
            kept.configurations[family] = Some(configuration);
        }
        reconfigured
    }
}

impl Asked {
    /// Makes room for the keys of `followed`, by at least an eighth of what is held when a buffer grows, so that the
    /// few files of a refresh do not double it.
    fn reserve(&mut self, followed: &[(ContentId, Followed)]) {
        let (keys, runs) = followed
            .iter()
            .map(|(_, followed)| followed.asked.len())
            .fold((0, 0), |(keys, runs), asked| {
                (keys + asked, runs + usize::from(asked > 0))
            });
        grow(&mut self.keys, keys);
        grow(&mut self.runs, runs);
    }

    fn push(&mut self, owner: ContentId, keys: &[u32]) -> u32 {
        if keys.is_empty() {
            return NO_RUN;
        }
        self.keys.extend_from_slice(keys);
        self.runs.push(Run {
            owner: Some(owner),
            end: narrow(self.keys.len()),
        });
        narrow(self.runs.len() - 1)
    }

    fn retire(&mut self, run: u32) {
        if run == NO_RUN {
            return;
        }
        let run = run as usize;
        let start = run.checked_sub(1).map_or(0, |before| self.runs[before].end);
        self.retired += (self.runs[run].end - start) as usize;
        self.runs[run].owner = None;
    }

    /// The files whose run holds a key `reached` holds.
    fn reaching<'a>(&'a self, reached: &'a Reached) -> impl Iterator<Item = ContentId> + 'a {
        let mut start = 0;
        self.runs.iter().filter_map(move |run| {
            let keys = &self.keys[start..run.end as usize];
            start = run.end as usize;
            run.owner
                .filter(|_| keys.iter().any(|&key| reached.holds(key)))
        })
    }

    /// Moves the runs of the files still kept to the front, in their order, and points each file at its run.
    fn compact(&mut self, files: &mut HashMap<ContentId, Kept, FixedState>) {
        let (mut start, mut end, mut held) = (0, 0, 0);
        for at in 0..self.runs.len() {
            let Run { owner, end: until } = self.runs[at];
            let (from, until) = (start, until as usize);
            start = until;
            let Some(owner) = owner else {
                continue;
            };
            self.keys.copy_within(from..until, end);
            end += until - from;
            self.runs[held] = Run {
                owner: Some(owner),
                end: narrow(end),
            };
            if let Some(kept) = files.get_mut(&owner) {
                kept.run = narrow(held);
            }
            held += 1;
        }
        self.keys.truncate(end);
        self.runs.truncate(held);
        self.retired = 0;
    }
}

fn grow<T>(vec: &mut Vec<T>, adding: usize) {
    if vec.capacity() - vec.len() < adding {
        vec.reserve_exact(adding.max(vec.len() / 8));
    }
}

#[expect(
    clippy::expect_used,
    reason = "an engine keeps fewer than 4G keys and files: 2.35M files asked 41.7M on a tree of 2.9M entries"
)]
fn narrow(len: usize) -> u32 {
    u32::try_from(len).expect("fewer than 4G keys and files")
}

fn count(
    languages: &[Box<dyn Language>],
    file_counts: &mut [FileCounts],
    (outcome, counts, stale): (&RefreshOutcome, &[FileCounts], &[Kept]),
    inserted: &[usize],
) {
    for path in &outcome.added {
        if let Some(language) = claimant(languages, path) {
            file_counts[language].claimed += 1;
        }
    }
    for path in &outcome.removed {
        if let Some(language) = claimant(languages, path) {
            file_counts[language].claimed = file_counts[language].claimed.saturating_sub(1);
        }
    }
    for ((held, walked), inserted) in file_counts.iter_mut().zip(counts).zip(inserted) {
        held.read += walked.read;
        held.cached += walked.cached;
        held.held += inserted;
    }
    for followed in stale {
        let counts = &mut file_counts[followed.language];
        counts.read = counts.read.saturating_sub(1);
        counts.cached = counts.cached.saturating_sub(usize::from(followed.cached));
    }
}

/// The keys of every question a refresh may answer differently: whether each file it added or removed is there, and
/// what each file it read again holds.
fn reached(index: &Index, outcome: &RefreshOutcome, handed: &[ToResolve]) -> Reached {
    let mut reached = HashSet::with_hasher(FixedState::default());
    for path in outcome.added.iter().chain(&outcome.removed) {
        reached.extend(reached_by_presence(path));
    }
    let mut paths = index.paths();
    for file in handed {
        reached.insert(asked(Query::Read, paths.get(file.position)));
    }
    Reached::new(reached)
}

impl Reached {
    fn new(keys: Keys) -> Self {
        let words = (keys.len() * BITS_PER_KEY).next_power_of_two().div_ceil(64);
        let mut bits = vec![0; words].into_boxed_slice();
        for &key in &keys {
            let (word, bit) = bit_of(key, words);
            bits[word] |= bit;
        }
        Self { keys, bits }
    }

    fn holds(&self, key: u32) -> bool {
        let (word, bit) = bit_of(key, self.bits.len());
        self.bits[word] & bit != 0 && self.keys.contains(&key)
    }
}

const fn bit_of(key: u32, words: usize) -> (usize, u64) {
    let at = key as usize & (words * 64 - 1);
    (at / 64, 1 << (at % 64))
}

/// Whether a refresh may have changed what a resolver read while it was built: an answer to one of its questions, or a
/// file it read inside a part the refresh walked again whose stamp under `root` does not match the one it read under.
fn reaches(
    configuration: &Configuration,
    (reached, outcome): (&Reached, &RefreshOutcome),
    root: &Path,
) -> bool {
    configuration.asked.iter().any(|&key| reached.holds(key))
        || configuration
            .read
            .iter()
            .any(|read| outcome.walked_over(&read.path) && !read.unchanged(root))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use kartta_fs::{BuildOptions, Content, ContentVisitor};

    use super::*;

    fn spread(range: std::ops::Range<u32>) -> impl Iterator<Item = u32> {
        range.map(|at| at.wrapping_mul(2_654_435_761))
    }

    struct Ids(Mutex<Vec<ContentId>>);

    impl ContentVisitor for Ids {
        fn wants(&self, _: &str) -> bool {
            true
        }

        fn visit(&self, content: Content<'_>) {
            self.0.lock().expect("the ids").push(content.id());
        }
    }

    /// The ids of `count` files an index read, sorted.
    fn ids(count: usize) -> Vec<ContentId> {
        let tree = tempfile::tempdir().expect("a temporary directory");
        for at in 0..count {
            fs::write(tree.path().join(format!("{at}.txt")), "text").expect("write a file");
        }
        let ids = Ids(Mutex::new(Vec::new()));
        Index::build_with(tree.path(), &BuildOptions::default(), Some(&ids), None)
            .expect("build the index");
        let mut ids = ids.0.into_inner().expect("the ids");
        ids.sort_unstable();
        ids
    }

    fn followed(id: ContentId, asked: &[u32]) -> (ContentId, Followed) {
        let followed = Followed {
            path: format!("{id:?}").into(),
            language: 0,
            cached: false,
            asked: asked.into(),
        };
        (id, followed)
    }

    fn reaching(following: &Following, key: u32) -> Vec<ContentId> {
        let reached = Reached::new(std::iter::once(key).collect());
        let mut found: Vec<ContentId> = following.asked.reaching(&reached).collect();
        found.sort_unstable();
        found
    }

    #[test]
    fn files_that_asked_nothing_leave_no_run_however_often_they_come_back() {
        let ids = ids(4);
        let mut following = Following::new(
            vec![
                followed(ids[0], &[]),
                followed(ids[1], &[]),
                followed(ids[2], &[7, 9]),
                followed(ids[3], &[9]),
            ],
            Resolvers::default(),
        );

        for _ in 0..100 {
            for &id in &ids[..2] {
                following.forget(id).expect("a kept file");
                following.keep(vec![followed(id, &[])]);
            }
        }

        assert_eq!(
            (following.asked.runs.len(), following.asked.keys.len()),
            (2, 3)
        );
        assert_eq!(reaching(&following, 9), ids[2..]);
    }

    #[test]
    fn the_files_a_key_reaches_are_found_after_their_runs_moved() {
        let ids = ids(6);
        let more = |at: u32, extra: u32| 1_000 + 10 * at + extra;
        let file = |at: u32, extras: u32| {
            let asked: Vec<u32> = [at, 100]
                .into_iter()
                .chain((0..extras).map(|extra| more(at, extra)))
                .collect();
            followed(ids[at as usize], &asked)
        };
        let mut following = Following::new(
            (0..6).map(|at| file(at, at)).collect(),
            Resolvers::default(),
        );

        for round in 0..60 {
            let at = round % 3;
            following.forget(ids[at as usize]).expect("a kept file");
            following.keep(vec![file(at, round % 5)]);
        }

        let extras = |at: u32| if at < 3 { (57 + at) % 5 } else { at };
        let held: u32 = (0..6).map(|at| 2 + extras(at)).sum();
        assert!(following.asked.keys.len() <= held as usize * 4 / 3);
        assert_eq!(reaching(&following, 100), ids);
        for at in 0..6 {
            let id = [ids[at as usize]];
            assert_eq!(reaching(&following, at), id);
            for extra in 0..extras(at) {
                assert_eq!(reaching(&following, more(at, extra)), id);
            }
            assert_eq!(reaching(&following, more(at, extras(at))), []);
        }
    }

    #[test]
    fn the_reached_keys_hold_every_key_of_their_set_and_no_other() {
        let keys: Keys = spread(0..1_000).collect();
        let reached = Reached::new(keys.clone());

        assert!(keys.iter().all(|&key| reached.holds(key)));
        assert_eq!(
            spread(1_000..200_000)
                .filter(|&key| reached.holds(key))
                .count(),
            0
        );
    }

    #[test]
    fn no_reached_key_holds_nothing() {
        let reached = Reached::new(Keys::default());

        assert_eq!(
            spread(0..10_000).filter(|&key| reached.holds(key)).count(),
            0
        );
    }
}
