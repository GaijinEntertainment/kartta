use foldhash::fast::FixedState;
use hashbrown::HashSet;

use crate::graph::{Graph, keep_first};

/// A file that includes another, directly or through files that include it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Dependent<'g> {
    /// The root-relative path of the including file.
    pub file: &'g str,
    /// How many includes away from the file asked about it is: 1 includes it directly.
    pub depth: u32,
}

/// The files that include one file, transitively, and what the walk over them left out.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Dependents<'g> {
    /// The files that include it, each once, nearest first, then in path order.
    pub found: Vec<Dependent<'g>>,
    /// Whether the file cap stopped the walk.
    pub truncated: bool,
    /// The includers the cap turned away, at the depth where it stopped the walk.
    pub turned_away: usize,
    /// The files reached whose own includers the walk did not look at: those at the depth bound, the file asked about
    /// itself at a depth of 0, and those the cap stopped the walk at.
    pub unexpanded: usize,
}

impl Graph {
    /// The files that include the file at `path`, transitively up to `depth` includes away, at most `max_files` of
    /// them, 0 setting no cap, or `None` when the graph does not hold the file.
    ///
    /// The walk goes a depth at a time and takes a depth's includers in path order, so the cap keeps the nearest
    /// files. A file a language resolves an import to every file of a package for, as Go does, counts every importer
    /// of that package among its includers.
    #[must_use]
    pub fn dependents(&self, path: &str, depth: u32, max_files: usize) -> Option<Dependents<'_>> {
        let start = self.slot_of(self.strings.get(path)?)?;
        let mut dependents = Dependents {
            found: Vec::new(),
            truncated: false,
            turned_away: 0,
            unexpanded: 0,
        };
        let mut visited: HashSet<u32, FixedState> = HashSet::with_hasher(FixedState::default());
        visited.insert(start);
        let mut level = vec![start];

        for at_depth in 1..=depth {
            if level.is_empty() {
                break;
            }
            let mut next: Vec<u32> = level
                .iter()
                .flat_map(|&slot| self.includers_of(slot))
                .filter(|&slot| visited.insert(slot))
                .collect();
            let reached = next.len();
            let room = match max_files {
                0 => usize::MAX,
                cap => cap - dependents.found.len(),
            };
            let by_path = |&slot: &u32| self.path_rank(slot);
            keep_first(&mut next, room, |left, right| {
                by_path(left).cmp(&by_path(right))
            });
            next.sort_unstable_by_key(by_path);
            dependents.found.extend(next.iter().map(|&slot| Dependent {
                file: self.strings.resolve(self.file(slot).path),
                depth: at_depth,
            }));
            if next.len() < reached {
                dependents.truncated = true;
                dependents.turned_away = reached - next.len();
                dependents.unexpanded = next.len();
                return Some(dependents);
            }
            level = next;
        }
        dependents.unexpanded = level.len();
        Some(dependents)
    }
}
