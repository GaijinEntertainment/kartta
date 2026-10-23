use std::cmp::{self, Reverse};
use std::ops::RangeInclusive;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};

use crate::analyze::{Analysis, ReachBand};
use crate::calls::{Calls, Edge, Limits, Reached, WalkState};
use crate::graph::{Definition, Graph, is_data_kind, keep_first};
use crate::stored::SymbolAt;

// constraint: the docs of `Seed::consumers` and `Seed::consumer_dirs` and the README state this number
const LISTED_CONSUMERS: usize = 30;

// constraint: the docs of `Change::hotspots` and `Change::seeds` and the README state these numbers
const LISTED_HOTSPOTS: usize = 300;
const LISTED_SEEDS: usize = 300;

/// A range of one-based lines a change touches in one file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Region<'a> {
    /// The root-relative path of the file.
    pub path: &'a str,
    /// The lines; `1..=u32::MAX` is the whole file.
    pub lines: RangeInclusive<u32>,
}

impl<'a> Region<'a> {
    /// The lines `lines` of the file at `path`.
    #[must_use]
    pub const fn new(path: &'a str, lines: RangeInclusive<u32>) -> Self {
        Self { path, lines }
    }
}

/// A symbol the callers of two or more changed symbols reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Hotspot<'g> {
    /// The symbol.
    pub symbol: Definition<'g>,
    /// How many changed symbols reach it.
    pub hit_count: usize,
    /// How many calls away from the nearest of them it is.
    pub min_depth: u32,
    /// Whether it is itself a changed symbol.
    pub also_changed: bool,
}

/// One directory's share of a changed symbol's direct callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DirCount<'g> {
    /// The root-relative directory, `.` for the root.
    pub dir: &'g str,
    /// The direct callers whose file sits directly in it.
    pub count: usize,
}

/// A symbol the change touches, and how far a change to it reaches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Seed<'g> {
    /// The symbol, as a changed file defines it.
    pub symbol: Definition<'g>,
    /// The callers a walk over its callers reaches, within the limits of the analysis; from a field, a variable or a
    /// constant the walk takes its users as it takes a function's callers.
    pub reach: usize,
    /// How widely the reach ranks.
    pub band: ReachBand,
    /// Its direct callers that pass the confidence floor.
    pub caller_count: usize,
    /// For a field, a variable or a constant, its direct users that pass the confidence floor: the symbols that call
    /// it or whose code reads or writes it, its callers among them; 0 for a symbol of any other kind.
    pub user_count: usize,
    /// The calls of its name that [`Walk::ambiguous`](crate::Walk::ambiguous) counts for a walk from it: any of them
    /// may call it, and neither `caller_count` nor `reach` counts them.
    pub ambiguous: usize,
    /// Its direct users for a field, a variable or a constant, and its direct callers for any other kind, in path
    /// order and those outside test, generated and entry code first, when 30 or fewer of them pass the floor; empty
    /// otherwise.
    pub consumers: Vec<Reached<'g>>,
    /// The directories of the files that hold those direct users or callers, the most first, when more than 30 of
    /// them pass the floor; empty otherwise.
    pub consumer_dirs: Vec<DirCount<'g>>,
    /// Whether the symbol cap stopped the walk over its callers.
    pub truncated: bool,
    /// The callers the cap turned away at the symbol where it stopped the walk.
    pub turned_away: usize,
    /// The callers reached whose own callers the walk did not follow: those at the depth bound, the symbol itself at a
    /// depth of 0, and those the cap left.
    pub unexpanded: usize,
    /// The callers the confidence floor left out, counted at each symbol the walk expanded that they call.
    pub below_confidence: usize,
}

/// What a change reaches: the symbols it touches, the callers they reach together, and where their reaches meet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Change<'g> {
    /// The symbols the regions overlap, a declaration and the definition it links to counting once.
    pub changed_symbols: usize,
    /// The files whose symbols the regions overlap.
    pub changed_files: usize,
    /// The regions whose file the graph asked does not hold.
    pub unheld_regions: usize,
    /// The regions of held files that overlap no symbol.
    pub regions_outside_symbols: usize,
    /// The symbols the walks over the changed symbols' callers reach, each once.
    pub footprint: usize,
    /// The files that hold them.
    pub footprint_files: usize,
    /// The reaches of the changed symbols, summed: what exceeds `footprint` is where they overlap.
    pub transitive_sum: usize,
    /// The symbols two or more changed symbols reach, at most 300: those outside test, generated and entry code
    /// first, then the most reached, then the nearest.
    pub hotspots: Vec<Hotspot<'g>>,
    /// Every such symbol, before the list was cut.
    pub total_hotspots: usize,
    /// The changed symbols, at most 300 of `changed_symbols`: those outside test, generated and entry code first,
    /// then the widest reach, then the most direct callers.
    pub seeds: Vec<Seed<'g>>,
    /// The changed symbols whose walk the symbol cap stopped.
    pub truncated_seeds: usize,
}

impl Change<'_> {
    /// The changes of several graphs as one, as a change across language families answers: the counts summed, the
    /// hotspots and seeds ranked together and cut again.
    #[must_use]
    pub fn merge(parts: impl IntoIterator<Item = Self>) -> Self {
        let mut merged = Self::default();
        for part in parts {
            merged.changed_symbols += part.changed_symbols;
            merged.changed_files += part.changed_files;
            merged.unheld_regions += part.unheld_regions;
            merged.regions_outside_symbols += part.regions_outside_symbols;
            merged.footprint += part.footprint;
            merged.footprint_files += part.footprint_files;
            merged.transitive_sum += part.transitive_sum;
            merged.total_hotspots += part.total_hotspots;
            merged.truncated_seeds += part.truncated_seeds;
            merged.hotspots.extend(part.hotspots);
            merged.seeds.extend(part.seeds);
        }
        merged.ranked()
    }

    fn ranked(mut self) -> Self {
        self.hotspots.sort_by_key(convergence);
        self.hotspots.truncate(LISTED_HOTSPOTS);
        self.seeds.sort_by_key(fan_out);
        self.seeds.truncate(LISTED_SEEDS);
        self
    }
}

const fn is_reach_artifact(definition: &Definition<'_>) -> bool {
    definition.flags.is_test() || definition.flags.is_generated() || definition.flags.is_entry()
}

fn convergence<'g>(hotspot: &Hotspot<'g>) -> impl Ord + use<'g> {
    let symbol = hotspot.symbol;
    let place = (symbol.qualified_name, symbol.file, symbol.line);
    let rank = (Reverse(hotspot.hit_count), hotspot.min_depth);
    (is_reach_artifact(&symbol), rank, place)
}

fn fan_out<'g>(seed: &Seed<'g>) -> impl Ord + use<'g> {
    let symbol = seed.symbol;
    let place = (symbol.qualified_name, symbol.file, symbol.line);
    let rank = (Reverse(seed.reach), Reverse(seed.caller_count));
    (is_reach_artifact(&symbol), rank, place)
}

impl Graph {
    /// What a change to the lines of `regions` reaches: the symbols they overlap, the callers a walk over each one's
    /// callers reaches within `limits`, the symbols two or more of them reach, and the direct callers of each.
    ///
    /// A declaration and the definition it links to are one changed symbol, answered as the first occurrence the
    /// regions overlap in path order. A field, a variable or a constant reaches first its users, the symbols whose code
    /// reads or writes it, as a function reaches its callers. The direct callers and users take the confidence floor
    /// of `limits`. Calls and uses are resolved when this asks, and each name's sites once for the whole change. A
    /// region whose file this graph does not hold counts as unheld, and one that overlaps no symbol as outside
    /// symbols; [`Change::merge`] combines the answers of several graphs.
    #[must_use]
    pub fn analyze_change(&self, regions: &[Region<'_>], limits: Limits) -> Change<'_> {
        let mut change = Change::default();
        let mut changed: HashMap<SymbolAt, SymbolAt, FixedState> = HashMap::default();
        let mut changed_files: HashSet<u32, FixedState> = HashSet::default();
        let mut analysis = Analysis::new(self, limits);

        for region in regions {
            let Some(slot) = self
                .strings
                .get(region.path)
                .and_then(|path| self.slot_of(path))
            else {
                change.unheld_regions += 1;
                continue;
            };
            let overlapping = self.overlapping(slot, &region.lines);
            if overlapping.is_empty() {
                change.regions_outside_symbols += 1;
                continue;
            }
            changed_files.insert(slot);
            for at in overlapping {
                let held = changed.entry(analysis.calls.canonical(at)).or_insert(at);
                *held = cmp::min_by_key(*held, at, |&symbol| self.order_of(symbol));
            }
        }

        let mut hits: HashMap<SymbolAt, (usize, u32), FixedState> = HashMap::default();
        let by_fan_out = |left: &Seed<'_>, right: &Seed<'_>| fan_out(left).cmp(&fan_out(right));
        for (&canonical, &at) in &changed {
            let seed = analysis.seed(canonical, at, &mut hits);
            change.transitive_sum += seed.reach;
            change.truncated_seeds += usize::from(seed.truncated);
            change.seeds.push(seed);
            if change.seeds.len() >= 2 * LISTED_SEEDS {
                keep_first(&mut change.seeds, LISTED_SEEDS, by_fan_out);
            }
        }

        let footprint_files: HashSet<u32, FixedState> = hits.keys().map(|at| at.file).collect();
        change.changed_symbols = changed.len();
        change.changed_files = changed_files.len();
        change.footprint = hits.len();
        change.footprint_files = footprint_files.len();
        change.hotspots = hits
            .into_iter()
            .filter(|&(_, (hit_count, _))| hit_count >= 2)
            .map(|(at, (hit_count, min_depth))| Hotspot {
                symbol: self.definition_at(at),
                hit_count,
                min_depth,
                also_changed: changed.contains_key(&at),
            })
            .collect();
        change.total_hotspots = change.hotspots.len();
        change.ranked()
    }
}

impl<'g> Analysis<'g> {
    fn seed(
        &mut self,
        canonical: SymbolAt,
        at: SymbolAt,
        hits: &mut HashMap<SymbolAt, (usize, u32), FixedState>,
    ) -> Seed<'g> {
        let mut reach = 0;
        let walk = WalkState::new(canonical, self.limits);
        let left_out = self.calls.reach(walk, Calls::dependents_of, |edge, depth| {
            reach += 1;
            let hit = hits.entry(edge.far).or_insert((0, depth));
            hit.0 += 1;
            hit.1 = hit.1.min(depth);
        });
        let (graph, floor) = (self.graph, self.limits.min_confidence);
        let passes = |edge: &&Edge| edge.confidence >= floor;
        let data_callers = is_data_kind(graph.sym(at).kind).then(|| {
            self.calls
                .caller_edges(canonical)
                .iter()
                .filter(passes)
                .count()
        });
        let direct = self.calls.dependents_of(canonical);
        let direct_count = direct.iter().filter(passes).count();
        let (caller_count, user_count) =
            data_callers.map_or((direct_count, 0), |callers| (callers, direct_count));
        let (consumers, consumer_dirs) = if direct_count > LISTED_CONSUMERS {
            (Vec::new(), consumer_dirs(graph, &direct, floor))
        } else {
            (consumers(graph, &direct, floor), Vec::new())
        };
        let symbol = graph.sym(at);

        Seed {
            symbol: graph.definition_at(at),
            reach,
            band: ReachBand::of(reach, symbol.kind, symbol.exported),
            caller_count,
            user_count,
            ambiguous: self.calls.ambiguous_calls(canonical),
            consumers,
            consumer_dirs,
            truncated: left_out.truncated,
            turned_away: left_out.turned_away,
            unexpanded: left_out.unexpanded,
            below_confidence: left_out.below_confidence,
        }
    }
}

fn consumers<'g>(graph: &'g Graph, direct: &[Edge], floor: u8) -> Vec<Reached<'g>> {
    let (mut consumers, artifacts): (Vec<Reached<'g>>, Vec<Reached<'g>>) = direct
        .iter()
        .filter(|edge| edge.confidence >= floor)
        .map(|edge| edge.reached(graph, 1))
        .partition(|reached| !is_reach_artifact(&reached.symbol));
    consumers.extend(artifacts);
    consumers
}

fn consumer_dirs<'g>(graph: &'g Graph, direct: &[Edge], floor: u8) -> Vec<DirCount<'g>> {
    let mut counts: HashMap<&'g str, usize, FixedState> = HashMap::default();
    for edge in direct.iter().filter(|edge| edge.confidence >= floor) {
        let dir = graph.strings.resolve(graph.file(edge.far.file).dir);
        *counts.entry(dir).or_insert(0) += 1;
    }
    let mut dirs: Vec<DirCount<'g>> = counts
        .into_iter()
        .map(|(dir, count)| DirCount { dir, count })
        .collect();
    dirs.sort_unstable_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.dir.cmp(right.dir))
    });
    dirs
}
