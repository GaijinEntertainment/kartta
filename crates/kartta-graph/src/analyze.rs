use std::cmp::Reverse;
use std::ops::RangeInclusive;

use foldhash::fast::FixedState;
use hashbrown::HashSet;
use kartta_lang::SymbolKind;

use crate::calls::{Calls, Edge, Limits, Reached, WalkState};
use crate::graph::{Definition, Graph, narrow};
use crate::stored::SymbolAt;

// constraint: the docs of `DirectCalls::listed` and the README state this number
const LISTED_CALLS: usize = 50;

/// How widely a change to a symbol reaches, from the callers a walk over them reaches.
///
/// An interface escalates sooner, because each implementor answers for its contract, and so does a symbol visible
/// outside its module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ReachBand {
    /// Fewer than 10 callers, and not exported with 20 or more.
    Low,
    /// 10 to 49 callers, and not exported with 20 or more.
    Medium,
    /// 50 or more callers, or 20 or more for an exported symbol.
    High,
    /// 200 or more callers, or 50 or more for an interface.
    Critical,
}

impl ReachBand {
    /// The band of a symbol of `kind`, visible outside its module when `exported`, whose change reaches `reach`
    /// callers: the band [`Blast::band`] grades, for a caller that counted the callers of a walk of its own, as over
    /// [`Graph::callers`].
    ///
    /// ```
    /// use kartta_graph::ReachBand;
    /// use kartta_lang::SymbolKind;
    ///
    /// assert_eq!(ReachBand::of(19, SymbolKind::Function, true), ReachBand::Medium);
    /// assert_eq!(ReachBand::of(20, SymbolKind::Function, true), ReachBand::High);
    /// assert_eq!(ReachBand::of(50, SymbolKind::Interface, false).label(), "critical");
    /// ```
    #[must_use]
    pub fn of(reach: usize, kind: SymbolKind, exported: bool) -> Self {
        if reach >= 200 || (kind == SymbolKind::Interface && reach >= 50) {
            Self::Critical
        } else if reach >= 50 || (exported && reach >= 20) {
            Self::High
        } else if reach >= 10 {
            Self::Medium
        } else {
            Self::Low
        }
    }

    /// The band's name, as answers print it: `low`, `medium`, `high` or `critical`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// The callers a change to a symbol reaches within a walk's limits, and the files they and the file's includers sit
/// in.
///
/// From a field, a variable or a constant the walk takes its users, the symbols that read or write it among them, as
/// it takes a function's callers, so each count here counts them too.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Blast {
    /// The callers reached, the symbol itself not among them.
    pub reach: usize,
    /// The callers reached at each depth, the direct callers first.
    pub by_depth: Vec<usize>,
    /// The files other than the symbol's own that hold a caller reached, or include its file within the walk's depth.
    pub affected_files: usize,
    /// How widely the reach ranks.
    pub band: ReachBand,
    /// Whether the symbol cap stopped the walk.
    pub truncated: bool,
    /// The callers the cap turned away at the symbol where it stopped the walk.
    pub turned_away: usize,
    /// The callers reached whose own callers the walk did not follow: those at the depth bound, the symbol itself at a
    /// depth of 0, and those the cap left.
    pub unexpanded: usize,
    /// The callers the confidence floor left out, counted at each symbol the walk expanded that they call.
    pub below_confidence: usize,
}

/// One side of a symbol's direct calls: its callers, its callees, or a field's, a variable's or a constant's users.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DirectCalls<'g> {
    /// At most 50 of them, in path order, each with its most confident call or use.
    pub listed: Vec<Reached<'g>>,
    /// Every one that passes the confidence floor, before the list was cut.
    pub total: usize,
    /// Those the confidence floor left out.
    pub below_confidence: usize,
}

/// One symbol a segment holds, with its direct calls and the reach of a change to it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SegmentSymbol<'g> {
    /// The symbol, as the file defines it.
    pub definition: Definition<'g>,
    /// The symbols that call it.
    pub callers: DirectCalls<'g>,
    /// For a field, a variable or a constant, the symbols that call it or whose code reads or writes it, each with its
    /// most confident site; none for a symbol of any other kind. A change to one reaches them as a change to a
    /// function reaches its callers.
    pub users: DirectCalls<'g>,
    /// The calls of its name that [`Walk::ambiguous`](crate::Walk::ambiguous) counts for a walk from it: any of them
    /// may call it, and neither `callers` nor `blast` holds them.
    pub ambiguous: usize,
    /// The symbols it calls.
    pub callees: DirectCalls<'g>,
    /// The calls of its body that bound no definition the graph holds: calls into code outside it, and calls the
    /// cascade could not tell apart. A construction of a class outside the graph is one such call, whatever the
    /// language records for its constructor.
    pub unresolved: usize,
    /// The reach of a change to it.
    pub blast: Blast,
}

/// The symbols a range of lines in one file overlaps, and the files that file includes and is included by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Segment<'g> {
    /// The one-based lines asked about. A range that ends at `u32::MAX` ends here at the last line a symbol of the file
    /// reaches, and never before its start.
    pub lines: RangeInclusive<u32>,
    /// The symbols whose lines overlap them, ordered by the line each starts on, one that encloses others before them.
    pub symbols: Vec<SegmentSymbol<'g>>,
    /// The files of the graph the file includes directly, in path order.
    pub includes: Vec<&'g str>,
    /// The files of the graph that include the file directly, in path order.
    pub included_by: Vec<&'g str>,
}

impl Graph {
    /// The symbols of the file at `path` whose lines overlap `lines`, each with its direct callers and callees and the
    /// callers a change to it reaches within `limits`, or `None` when the graph does not hold the file. `1..=u32::MAX`
    /// asks about the whole file, and a range that starts past its end overlaps no symbol.
    ///
    /// A declaration answers for the definition it links to. A field, a variable or a constant also answers its direct
    /// users, the symbols whose code reads or writes it, and the walk from it starts from them. The direct calls take
    /// the confidence floor of `limits`; its depth and symbol cap bound the walk over each symbol's callers, and its
    /// depth the files that include the file. Calls and uses are resolved when this asks, and each name's sites once
    /// for the whole segment.
    #[must_use]
    pub fn analyze_segment(
        &self,
        path: &str,
        lines: RangeInclusive<u32>,
        limits: Limits,
    ) -> Option<Segment<'_>> {
        let slot = self.slot_of(self.strings.get(path)?)?;
        let file = self.file(slot);

        let overlapping = self.overlapping(slot, &lines);
        let symbols = if overlapping.is_empty() {
            Vec::new()
        } else {
            let includers = self.includers_within(slot, limits.depth);
            let mut analysis = Analysis::new(self, limits);
            overlapping
                .into_iter()
                .map(|at| analysis.segment_symbol(at, &includers))
                .collect()
        };

        let mut targets: Vec<u32> = file
            .includes
            .iter()
            .filter_map(|&target| self.slot_of(target))
            .collect();
        let mut sources: Vec<u32> = self.includers_of(slot).collect();
        let (start, mut end) = lines.into_inner();
        if end == u32::MAX {
            let reached = file
                .symbols
                .iter()
                .map(|symbol| symbol.end_line.max(symbol.line));
            end = reached.max().unwrap_or(start).max(start);
        }
        Some(Segment {
            lines: start..=end,
            symbols,
            includes: self.paths_in_order(&mut targets),
            included_by: self.paths_in_order(&mut sources),
        })
    }

    pub(crate) fn overlapping(&self, slot: u32, lines: &RangeInclusive<u32>) -> Vec<SymbolAt> {
        if lines.is_empty() {
            return Vec::new();
        }
        let file = self.file(slot);
        let (start, end) = (*lines.start(), *lines.end());
        let mut overlapping: Vec<SymbolAt> = (0..narrow(file.symbols.len()))
            .filter(|&index| {
                let symbol = &file.symbols[index as usize];
                symbol.line <= end && symbol.end_line.max(symbol.line) >= start
            })
            .map(|index| SymbolAt::new(slot, index))
            .collect();
        overlapping.sort_by_key(|at| {
            let symbol = &file.symbols[at.index as usize];
            (
                symbol.line,
                Reverse(symbol.end_line.max(symbol.line)),
                at.index,
            )
        });
        overlapping
    }

    fn includers_within(&self, slot: u32, depth: u32) -> HashSet<u32, FixedState> {
        let mut found: HashSet<u32, FixedState> = HashSet::default();
        let mut level = vec![slot];
        for _ in 0..depth {
            level = level
                .iter()
                .flat_map(|&file| self.includers_of(file))
                .filter(|&includer| includer != slot && found.insert(includer))
                .collect();
            if level.is_empty() {
                break;
            }
        }
        found
    }

    fn paths_in_order(&self, slots: &mut Vec<u32>) -> Vec<&str> {
        slots.sort_unstable_by_key(|&slot| self.path_rank(slot));
        slots.dedup();
        slots
            .iter()
            .map(|&slot| self.strings.resolve(self.file(slot).path))
            .collect()
    }
}

pub(crate) struct Analysis<'g> {
    pub(crate) graph: &'g Graph,
    pub(crate) calls: Calls<'g>,
    pub(crate) limits: Limits,
}

impl<'g> Analysis<'g> {
    pub(crate) fn new(graph: &'g Graph, limits: Limits) -> Self {
        Self {
            graph,
            calls: Calls::new(graph),
            limits,
        }
    }

    fn segment_symbol(
        &mut self,
        at: SymbolAt,
        includers: &HashSet<u32, FixedState>,
    ) -> SegmentSymbol<'g> {
        let root = self.calls.canonical(at);
        let floor = self.limits.min_confidence;
        let callers = DirectCalls::of(self.graph, &self.calls.callers_of(root), floor);
        let users = DirectCalls::of(self.graph, &self.calls.user_edges(root), floor);
        let unresolved_before = self.calls.unresolved();
        let outgoing = self.calls.callees_of(root);
        let unresolved = self.calls.unresolved() - unresolved_before;

        SegmentSymbol {
            definition: self.graph.definition_at(at),
            callers,
            users,
            ambiguous: self.calls.ambiguous_calls(root),
            callees: DirectCalls::of(self.graph, &outgoing, floor),
            unresolved,
            blast: self.blast(at, root, includers),
        }
    }

    fn blast(
        &mut self,
        at: SymbolAt,
        root: SymbolAt,
        includers: &HashSet<u32, FixedState>,
    ) -> Blast {
        let mut by_depth: Vec<usize> = Vec::new();
        let mut caller_files: HashSet<u32, FixedState> = HashSet::default();
        let walk = WalkState::new(root, self.limits);
        let left_out = self.calls.reach(walk, Calls::dependents_of, |edge, depth| {
            let index = depth as usize - 1;
            if by_depth.len() <= index {
                by_depth.resize(index + 1, 0);
            }
            by_depth[index] += 1;
            if edge.far.file != at.file && !includers.contains(&edge.far.file) {
                caller_files.insert(edge.far.file);
            }
        });
        let reach = by_depth.iter().sum();
        let symbol = self.graph.sym(at);

        Blast {
            reach,
            by_depth,
            affected_files: includers.len() + caller_files.len(),
            band: ReachBand::of(reach, symbol.kind, symbol.exported()),
            truncated: left_out.truncated,
            turned_away: left_out.turned_away,
            unexpanded: left_out.unexpanded,
            below_confidence: left_out.below_confidence,
        }
    }
}

impl<'g> DirectCalls<'g> {
    fn of(graph: &'g Graph, edges: &[Edge], floor: u8) -> Self {
        let mut passing = edges.iter().filter(|edge| edge.confidence >= floor);
        let listed: Vec<Reached<'g>> = passing
            .by_ref()
            .take(LISTED_CALLS)
            .map(|edge| edge.reached(graph, 1))
            .collect();
        let total = listed.len() + passing.count();
        Self {
            listed,
            total,
            below_confidence: edges.len() - total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_and_an_exported_symbol_reach_a_band_sooner() {
        let plain = |reach| ReachBand::of(reach, SymbolKind::Function, false);

        assert_eq!(plain(9), ReachBand::Low);
        assert_eq!(plain(10), ReachBand::Medium);
        assert_eq!(plain(50), ReachBand::High);
        assert_eq!(plain(200), ReachBand::Critical);
        assert_eq!(
            ReachBand::of(20, SymbolKind::Function, true),
            ReachBand::High
        );
        assert_eq!(
            ReachBand::of(19, SymbolKind::Function, true),
            ReachBand::Medium
        );
        assert_eq!(
            ReachBand::of(50, SymbolKind::Interface, false),
            ReachBand::Critical
        );
        assert_eq!(
            ReachBand::of(49, SymbolKind::Interface, true),
            ReachBand::High
        );
    }
}
