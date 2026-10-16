use std::cmp::Reverse;
use std::collections::VecDeque;
use std::hash::Hash;

use foldhash::fast::FixedState;
use hashbrown::HashMap;

use crate::graph::{Definition, Graph, QUALIFIER_SEPARATORS, narrow};
use crate::resolve::Cascade;
use crate::stored::{Entry, Site, SymbolAt};

/// How far a walk over calls goes, and which calls it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Limits {
    /// How many calls away from the symbol the walk goes: 1 takes the direct calls alone.
    pub depth: u32,
    /// The least [`confidence`](crate::confidence) a call needs to be taken, 0 to 100; 0 takes every call.
    pub min_confidence: u8,
    /// How many symbols the walk reaches before it stops, the symbol it starts from included; 0 sets no cap.
    pub max_symbols: usize,
}

impl Limits {
    /// A walk `depth` calls deep that takes every call and reaches as many symbols as there are.
    #[must_use]
    pub const fn new(depth: u32) -> Self {
        Self {
            depth,
            min_confidence: 0,
            max_symbols: 0,
        }
    }

    /// These limits with the confidence floor set to `min_confidence`.
    #[must_use]
    pub const fn with_min_confidence(self, min_confidence: u8) -> Self {
        Self {
            min_confidence,
            ..self
        }
    }

    /// These limits with the symbol cap set to `max_symbols`.
    #[must_use]
    pub const fn with_max_symbols(self, max_symbols: usize) -> Self {
        Self {
            max_symbols,
            ..self
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(1)
    }
}

/// A symbol a walk over calls reached, and the call it reached it through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Reached<'g> {
    /// The symbol: a caller in a walk over callers, a callee in a walk over callees.
    pub symbol: Definition<'g>,
    /// How many calls away from the start it is.
    pub depth: u32,
    /// How sure the resolution of the call is, 15 to 95: the [`confidence`](crate::confidence) of the tier of the
    /// cascade that bound it.
    pub confidence: u8,
    /// The one-based line of the call, in the caller's file. Where a caller calls a callee from several lines, the
    /// first line among the calls the most confident tier bound.
    pub line: u32,
}

/// What a walk over calls found, and what it left out.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Walk<'g> {
    /// The symbol the walk starts from: the callable the name selects, a declaration's linked definition.
    pub root: Definition<'g>,
    /// The other callables the name selects, which the walk did not start from.
    pub passed_over: usize,
    /// The symbols reached, each once, nearest first.
    pub reached: Vec<Reached<'g>>,
    /// Whether the symbol cap stopped the walk.
    pub truncated: bool,
    /// The symbols the cap turned away at the symbol where it stopped the walk.
    pub turned_away: usize,
    /// The symbols reached whose own calls the walk did not follow: those at the depth bound, the root itself at a
    /// depth of 0, and those the cap left.
    pub unexpanded: usize,
    /// The symbols the confidence floor left out, counted at each symbol the walk expanded that calls them or that they
    /// call: a symbol that calls another from several lines counts once there, at its most confident call.
    pub below_confidence: usize,
    /// The calls at the symbols a walk over callees expanded that bound no definition the graph holds: calls into
    /// code outside it, and calls the cascade could not tell apart. A walk over callers counts none.
    pub unresolved: usize,
}

#[derive(Debug, Clone, Copy)]
struct Edge {
    far: SymbolAt,
    confidence: u8,
    line: u32,
}

fn keep_stronger<K: Eq + Hash>(strongest: &mut HashMap<K, Edge, FixedState>, key: K, edge: Edge) {
    let held = strongest.entry(key).or_insert(edge);
    if (edge.confidence, Reverse(edge.line)) > (held.confidence, Reverse(held.line)) {
        *held = edge;
    }
}

impl Graph {
    /// The callees of the symbol `name` selects, transitively within `limits`, or `None` when it selects none.
    ///
    /// A name selects as [`Graph::definitions`] does, and one symbol among them, a callable before any symbol that is
    /// not one: the symbol whose qualified name it is, else the first definition of a bare name in path order, else the
    /// one callable a qualified name ends with at a separator. Where several share the qualified name, as free
    /// functions of one name do, the first in path order is selected. A declaration and the definition it links to are
    /// one callable, and a call of either is a call of that definition.
    ///
    /// Calls are resolved when this asks, from the call sites the graph keeps.
    #[must_use]
    pub fn callees(&self, name: &str, limits: Limits) -> Option<Walk<'_>> {
        let mut calls = Calls::new(self);
        let root = calls.select(name)?;
        let mut walk = calls.walk(root, limits, Calls::callees_of);
        walk.passed_over = calls.passed_over(name, root);
        Some(walk)
    }
}

struct Calls<'g> {
    graph: &'g Graph,
    cascade: Cascade<'g>,
    unresolved: usize,
}

impl<'g> Calls<'g> {
    fn new(graph: &'g Graph) -> Self {
        Self {
            graph,
            cascade: Cascade::new(graph),
            unresolved: 0,
        }
    }

    fn select(&self, name: &str) -> Option<SymbolAt> {
        let key = self.graph.strings.get(name);
        if let Some(key) = key {
            let holders = self.targets_first(self.cascade.qualified_as(key).collect());
            if let Some(&first) = holders.first() {
                return Some(self.cascade.canonical(first));
            }
        }
        let named = key.map_or_else(Vec::new, |key| self.graph.named(key).to_vec());
        if let Some(&first) = self.targets_first(named).first() {
            return Some(self.cascade.canonical(first));
        }
        if !name.contains(QUALIFIER_SEPARATORS) {
            return None;
        }
        let mut pool = self
            .targets_first(self.graph.selected(name).collect())
            .into_iter()
            .map(|at| self.cascade.canonical(at));
        let first = pool.next()?;
        pool.all(|at| at == first).then_some(first)
    }

    fn targets_first(&self, mut candidates: Vec<SymbolAt>) -> Vec<SymbolAt> {
        let is_target = |at: SymbolAt| self.cascade.is_call_target(self.graph.sym(at).kind);
        if candidates.iter().any(|&at| is_target(at)) {
            candidates.retain(|&at| is_target(at));
        }
        candidates
    }

    fn passed_over(&self, name: &str, chosen: SymbolAt) -> usize {
        let mut others: Vec<SymbolAt> = self
            .targets_first(self.graph.selected(name).collect())
            .into_iter()
            .map(|at| self.cascade.canonical(at))
            .filter(|&at| at != chosen)
            .collect();
        others.sort_unstable();
        others.dedup();
        others.len()
    }

    fn callees_of(&mut self, body: SymbolAt) -> Vec<Edge> {
        let file = self.graph.file(body.file);
        let sites = file
            .calls_by(body.index)
            .map(|index| Site::new(body.file, Entry::Call(narrow(index))))
            .chain(
                file.local_calls_by(body.index)
                    .map(|index| Site::new(body.file, Entry::LocalCall(narrow(index)))),
            );

        let mut strongest: HashMap<SymbolAt, Edge, FixedState> =
            HashMap::with_hasher(FixedState::default());
        for site in sites {
            let call = self.cascade.call(site);
            let Some((callee, confidence)) = self.cascade.resolve_call(&call) else {
                self.unresolved += 1;
                continue;
            };
            let callable = self.cascade.canonical(callee);
            let edge = Edge {
                far: callable,
                confidence,
                line: call.line,
            };
            keep_stronger(&mut strongest, callable, edge);
        }
        let mut edges: Vec<Edge> = strongest.into_values().collect();
        self.sort(&mut edges);
        edges
    }

    fn sort(&self, edges: &mut [Edge]) {
        edges.sort_unstable_by_key(|edge| (self.graph.path_rank(edge.far.file), edge.far.index));
    }

    fn walk(
        &mut self,
        root: SymbolAt,
        limits: Limits,
        edges_of: fn(&mut Self, SymbolAt) -> Vec<Edge>,
    ) -> Walk<'g> {
        let mut state = WalkState {
            graph: self.graph,
            limits,
            walk: Walk {
                root: self.graph.definition_at(root),
                passed_over: 0,
                reached: Vec::new(),
                truncated: false,
                turned_away: 0,
                unexpanded: 0,
                below_confidence: 0,
                unresolved: 0,
            },
            visited: HashMap::with_hasher(FixedState::default()),
            queue: VecDeque::from([root]),
        };
        state.visited.insert(root, 0);

        while let Some(current) = state.queue.pop_front() {
            let depth = state.visited[&current];
            if depth >= limits.depth {
                state.walk.unexpanded += 1;
                continue;
            }
            let edges = edges_of(self, self.cascade.canonical(current));
            state.take(edges, depth + 1);
            if state.walk.truncated {
                state.walk.unexpanded += state.queue.len();
                break;
            }
        }
        state.walk.unresolved = self.unresolved;
        state.walk
    }
}

struct WalkState<'g> {
    graph: &'g Graph,
    limits: Limits,
    walk: Walk<'g>,
    visited: HashMap<SymbolAt, u32, FixedState>,
    queue: VecDeque<SymbolAt>,
}

impl WalkState<'_> {
    fn take(&mut self, edges: Vec<Edge>, depth: u32) {
        for edge in edges {
            if self.visited.contains_key(&edge.far) {
                continue;
            }
            if edge.confidence < self.limits.min_confidence {
                self.walk.below_confidence += 1;
                continue;
            }
            if self.limits.max_symbols > 0 && self.visited.len() >= self.limits.max_symbols {
                self.walk.truncated = true;
                self.walk.turned_away += 1;
                continue;
            }
            self.visited.insert(edge.far, depth);
            self.queue.push_back(edge.far);
            self.walk.reached.push(Reached {
                symbol: self.graph.definition_at(edge.far),
                depth,
                confidence: edge.confidence,
                line: edge.line,
            });
        }
    }
}
