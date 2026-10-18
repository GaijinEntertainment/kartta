use std::collections::{HashMap, HashSet};

use foldhash::fast::FixedState;
use kartta_fs::{BuildError, ContentId, Index, RefreshOutcome};
use kartta_lang::Language;

use crate::engine::{EngineRefresh, Family};
use crate::extracting::{Extracted, FileCounts, claimant};
use crate::repository::{Configuration, IndexRepository, Query, asked, reached_by_presence};
use crate::resolving::{Resolvers, Resolving, ToResolve};

/// What an engine built with stamps keeps to follow its tree: for each file its graphs hold, the questions its
/// resolution put to the repository, and for each language, what its resolver asked while it was built.
#[derive(Debug, Default)]
pub(crate) struct Following {
    files: HashMap<ContentId, Followed, FixedState>,
    configurations: Vec<Option<Configuration>>,
}

#[derive(Debug)]
pub(crate) struct Followed {
    pub(crate) path: Box<str>,
    pub(crate) language: usize,
    pub(crate) cached: bool,
    pub(crate) asked: Box<[u32]>,
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

struct Plan {
    stale: Vec<Followed>,
    reread: Vec<Followed>,
    reread_ids: Vec<ContentId>,
    work: Vec<ToResolve>,
    reconfigured: Vec<usize>,
}

impl Following {
    pub(crate) fn new(
        followed: Vec<(ContentId, Followed)>,
        configurations: Vec<Option<Configuration>>,
    ) -> Self {
        Self {
            files: followed.into_iter().collect(),
            configurations,
        }
    }

    /// Brings `engine`'s graphs up to its refreshed index: drops the files the refresh retired, inserts what its walk
    /// handed over, and resolves again from a second read every file whose questions a change answers differently,
    /// every file of a language whose resolver now reads another configuration among them.
    pub(crate) fn refresh(
        &mut self,
        engine: Refreshing<'_>,
        walked: Walked,
    ) -> Result<EngineRefresh, BuildError> {
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
        let repository = IndexRepository::recording(index);
        let mut stale: Vec<Followed> = outcome
            .retired()
            .filter_map(|id| self.files.remove(&id))
            .collect();
        let handed = resolving.placed(extracted);
        let reached = reached(index, &outcome, &handed);
        let (resolvers, plan) = self.plan(&resolving, &repository, (&outcome, &reached, handed));
        stale.extend(plan.stale);
        for followed in stale.iter().chain(&plan.reread) {
            families[family_of[followed.language]]
                .graph
                .remove(&followed.path);
            file_counts[followed.language].held -= 1;
        }
        let inserted = resolving.insert_all(plan.work, &resolvers, families)?;
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
        self.files.extend(inserted.followed);
        count(
            languages,
            file_counts,
            (&outcome, &counts, &stale),
            &inserted.held,
        );

        Ok(EngineRefresh {
            index: outcome,
            errors: inserted.errors,
            extracted,
            resolved_again,
            dropped: Some(dropped),
            reconfigured: plan
                .reconfigured
                .iter()
                .map(|&language| languages[language].name().to_owned())
                .collect(),
            rebuilt: false,
            widened: None,
        })
    }

    fn plan<'r>(
        &mut self,
        resolving: &Resolving<'r>,
        repository: &'r IndexRepository<'_>,
        (outcome, reached, handed): (&RefreshOutcome, &Keys, Vec<ToResolve>),
    ) -> (Resolvers<'r>, Plan) {
        let mut dependents: Vec<ContentId> = if reached.is_empty() {
            Vec::new()
        } else {
            self.files
                .iter()
                .filter(|(_, followed)| followed.asked.iter().any(|key| reached.contains(key)))
                .map(|(&id, _)| id)
                .collect()
        };
        let mut wanted = vec![false; resolving.languages.len()];
        for file in &handed {
            wanted[file.language] = true;
        }
        for id in &dependents {
            wanted[self.files[id].language] = true;
        }
        for (language, configuration) in self.configurations.iter().enumerate() {
            wanted[language] |= configuration
                .as_ref()
                .is_some_and(|configuration| reaches(configuration, reached, &outcome.walked));
        }

        let mut resolvers = resolving.resolvers(repository, &wanted);
        let reconfigured = self.reconfigure(&mut resolvers);
        dependents.extend(
            self.files
                .iter()
                .filter(|(_, followed)| reconfigured.contains(&followed.language))
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
            let Some(followed) = self.files.remove(&id) else {
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
        (resolvers, plan)
    }

    fn reconfigure(&mut self, resolvers: &mut Resolvers<'_>) -> Vec<usize> {
        if self.configurations.len() < resolvers.configurations.len() {
            self.configurations
                .resize_with(resolvers.configurations.len(), || None);
        }
        let mut reconfigured = Vec::new();
        for (language, built) in resolvers.configurations.iter_mut().enumerate() {
            let Some(built) = built.take() else {
                continue;
            };
            let kept = &mut self.configurations[language];
            if kept
                .as_ref()
                .is_some_and(|kept| kept.answers != built.answers)
            {
                reconfigured.push(language);
            }
            *kept = Some(built);
        }
        reconfigured
    }
}

fn count(
    languages: &[Box<dyn Language>],
    file_counts: &mut [FileCounts],
    (outcome, counts, stale): (&RefreshOutcome, &[FileCounts], &[Followed]),
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
fn reached(index: &Index, outcome: &RefreshOutcome, handed: &[ToResolve]) -> Keys {
    let mut reached = HashSet::with_hasher(FixedState::default());
    for path in outcome.added.iter().chain(&outcome.removed) {
        reached.extend(reached_by_presence(path));
    }
    let mut paths = index.paths();
    for file in handed {
        reached.insert(asked(Query::Read, paths.get(file.position)));
    }
    reached
}

/// Whether a refresh may have changed what a resolver read while it was built: an answer to one of its questions, or a
/// file it read inside a part the refresh walked again.
fn reaches(configuration: &Configuration, reached: &Keys, walked: &[String]) -> bool {
    configuration.asked.iter().any(|key| reached.contains(key))
        || configuration
            .read
            .iter()
            .any(|path| walked.iter().any(|part| holds(part, path)))
}

fn holds(part: &str, path: &str) -> bool {
    part.is_empty()
        || path
            .strip_prefix(part)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}
