use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::VecDeque;
use std::hash::Hash;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};

use crate::graph::{Definition, Graph, QUALIFIER_SEPARATORS, is_call_target_kind, narrow};
use crate::resolve::Cascade;
use crate::stored::{Entry, Site, SymbolAt};
use crate::strings::Str;

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

/// A site that uses a symbol: a call of it, or a use of its name outside a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Reference<'g> {
    /// The symbol whose body holds the site.
    pub site_of: Definition<'g>,
    /// The one-based line of the site.
    pub line: u32,
    /// How sure the resolution of the site is, 15 to 95: the [`confidence`](crate::confidence) of the tier that
    /// bound it.
    pub confidence: u8,
    /// Whether the site calls the symbol. A caller counts once, at the line of its call; any other use counts once a
    /// line.
    pub call: bool,
}

/// The sites that use one symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct References<'g> {
    /// The symbol the name selects.
    pub symbol: Definition<'g>,
    /// The other symbols the name selects, whose sites these are not.
    pub passed_over: usize,
    /// The sites, by file and line, at most the query's limit.
    pub sites: Vec<Reference<'g>>,
    /// The sites that pass the confidence floor, before the limit.
    pub total: usize,
    /// The sites the confidence floor left out.
    pub below_confidence: usize,
}

/// The shortest chain of calls the graph holds from one symbol to another.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CallPath<'g> {
    /// The callable the first name selects.
    pub from: Definition<'g>,
    /// The callable the second name selects.
    pub to: Definition<'g>,
    /// The callers along the chain, `from` first, each calling the next and the last calling `to`; empty when no
    /// chain was found, and when `from` is `to`.
    pub hops: Vec<Reached<'g>>,
    /// Whether a chain was found within the depth bound; a symbol reaches itself with no hop.
    pub found: bool,
    /// Whether the symbol cap stopped the search before it covered the depth bound.
    pub truncated: bool,
    /// The callers the cap turned away at the callable where it stopped the search.
    pub turned_away: usize,
    /// The callers other than `from` the search reached at the depth bound, whose own callers it did not look at: a
    /// chain longer than the bound may pass through them.
    pub at_depth_bound: usize,
    /// The callers the confidence floor left out, counted at each callable the search looked at the callers of: a
    /// caller that calls it from several lines counts once there, at its most confident call.
    pub below_confidence: usize,
    /// The other callables each name selects, which the search did not start or end at: `from`'s, then `to`'s.
    pub passed_over: (usize, usize),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Edge {
    pub(crate) far: SymbolAt,
    pub(crate) confidence: u8,
    pub(crate) line: u32,
}

impl Edge {
    pub(crate) fn reached(self, graph: &Graph, depth: u32) -> Reached<'_> {
        Reached {
            symbol: graph.definition_at(self.far),
            depth,
            confidence: self.confidence,
            line: self.line,
        }
    }
}

#[derive(Default)]
pub(crate) struct LeftOut {
    pub(crate) truncated: bool,
    pub(crate) turned_away: usize,
    pub(crate) unexpanded: usize,
    pub(crate) below_confidence: usize,
}

fn keep_stronger<K: Eq + Hash>(strongest: &mut HashMap<K, Edge, FixedState>, key: K, edge: Edge) {
    let held = strongest.entry(key).or_insert(edge);
    if (edge.confidence, Reverse(edge.line)) > (held.confidence, Reverse(held.line)) {
        *held = edge;
    }
}

impl Graph {
    /// The callers of the symbol `name` selects, transitively within `limits`, or `None` when it selects none.
    ///
    /// A name selects as [`Graph::definitions`] does, and one symbol among them, a callable before any symbol that is
    /// not one: the symbol whose qualified name it is, else, among the definitions of a bare name, the one with the
    /// most callers, else the one callable a qualified name ends with at a separator. Where several share the qualified
    /// name, as free functions of one name do, the one with the most callers is selected, the first in path order on a
    /// tie. A declaration and the definition it links to are one callable, and a call of either is a call of that
    /// definition.
    ///
    /// Calls are resolved when this asks, from the call sites the graph keeps: the cost grows with the call sites of
    /// each name the walk reaches.
    #[must_use]
    pub fn callers(&self, name: &str, limits: Limits) -> Option<Walk<'_>> {
        let mut calls = Calls::new(self);
        let root = calls.select(name, Calls::caller_count)?;
        let mut walk = calls.walk(root, limits, Calls::callers_of);
        walk.passed_over = calls.passed_over(name, root);
        Some(walk)
    }

    /// The callees of the symbol `name` selects, transitively within `limits`, or `None` when it selects none; a name
    /// selects as [`Graph::callers`] describes.
    #[must_use]
    pub fn callees(&self, name: &str, limits: Limits) -> Option<Walk<'_>> {
        let mut calls = Calls::new(self);
        let root = calls.select(name, Calls::caller_count)?;
        let mut walk = calls.walk(root, limits, Calls::walked_callees);
        walk.passed_over = calls.passed_over(name, root);
        Some(walk)
    }

    /// The sites that call or use the symbol `name` selects, at least `min_confidence` sure on the 0 to 100 scale of
    /// [`confidence`](crate::confidence), 0 taking every site, at most `limit` of them, or `None` when it selects none.
    /// A name selects among every symbol a use can name, a value included where calls target no values, and a bare
    /// name shared by several of them selects the one with the most sites.
    ///
    /// The uses are those the language records: a use through a value of the owner's type is not one. Every site of
    /// the name is resolved before `limit` applies, so the limit bounds the answer, not the work, and `total` counts
    /// them all.
    #[must_use]
    pub fn references(
        &self,
        name: &str,
        min_confidence: u8,
        limit: usize,
    ) -> Option<References<'_>> {
        let mut calls = Calls::wanting(self, Wanted::Referents);
        let root = calls.select(name, Calls::site_count)?;

        let named = self.sym(root).name;
        calls.resolve_callers(named);
        calls.resolve_uses(named);
        let mut uses: Vec<(Edge, bool)> = edges_of(&calls.callers, root)
            .iter()
            .map(|&edge| (edge, true))
            .chain(
                edges_of(&calls.uses, root)
                    .iter()
                    .map(|&edge| (edge, false)),
            )
            .collect();
        let before = uses.len();
        uses.retain(|(edge, _)| edge.confidence >= min_confidence);
        let below_confidence = before - uses.len();
        let total = uses.len();
        let by_file_and_line = |&(edge, call): &(Edge, bool)| {
            (
                self.path_rank(edge.far.file),
                edge.line,
                call,
                edge.far.index,
            )
        };
        if limit < uses.len() {
            if let Some(last) = limit.checked_sub(1) {
                uses.select_nth_unstable_by_key(last, by_file_and_line);
            }
            uses.truncate(limit);
        }
        uses.sort_unstable_by_key(by_file_and_line);

        Some(References {
            symbol: self.definition_at(root),
            passed_over: calls.passed_over(name, root),
            total,
            sites: uses
                .into_iter()
                .map(|(edge, call)| Reference {
                    site_of: self.definition_at(edge.far),
                    line: edge.line,
                    confidence: edge.confidence,
                    call,
                })
                .collect(),
            below_confidence,
        })
    }

    /// The shortest chain of calls from the symbol `from` selects to the one `to` selects, within `limits`, or `None`
    /// when either selects none; each name selects as [`Graph::callers`] describes. A chain the graph does not hold is
    /// no proof that the code holds none: calls are resolved by name and miss dynamic dispatch.
    #[must_use]
    pub fn call_path(&self, from: &str, to: &str, limits: Limits) -> Option<CallPath<'_>> {
        let mut calls = Calls::new(self);
        let start = calls.select(from, Calls::caller_count)?;
        let target = calls.select(to, Calls::caller_count)?;
        let mut path = calls.path(start, target, limits);
        path.passed_over = (
            calls.passed_over(from, start),
            calls.passed_over(to, target),
        );
        Some(path)
    }
}

fn edges_of(held: &HashMap<SymbolAt, Vec<Edge>, FixedState>, at: SymbolAt) -> &[Edge] {
    held.get(&at).map_or(&[], Vec::as_slice)
}

type Rank<'g> = fn(&mut Calls<'g>, SymbolAt) -> usize;

pub(crate) type EdgesOf<'g> = for<'c> fn(&'c mut Calls<'g>, SymbolAt) -> Cow<'c, [Edge]>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Callables,
    Referents,
}

pub(crate) struct Calls<'g> {
    graph: &'g Graph,
    cascade: Cascade<'g>,
    wanted: Wanted,
    unresolved: usize,
    callers: HashMap<SymbolAt, Vec<Edge>, FixedState>,
    called_names: HashSet<Str, FixedState>,
    uses: HashMap<SymbolAt, Vec<Edge>, FixedState>,
    used_names: HashSet<Str, FixedState>,
}

impl<'g> Calls<'g> {
    pub(crate) fn new(graph: &'g Graph) -> Self {
        Self::wanting(graph, Wanted::Callables)
    }

    pub(crate) fn canonical(&self, at: SymbolAt) -> SymbolAt {
        self.cascade.canonical(at)
    }

    pub(crate) const fn unresolved(&self) -> usize {
        self.unresolved
    }

    fn wanting(graph: &'g Graph, wanted: Wanted) -> Self {
        Self {
            graph,
            cascade: Cascade::new(graph),
            wanted,
            unresolved: 0,
            callers: HashMap::with_hasher(FixedState::default()),
            called_names: HashSet::with_hasher(FixedState::default()),
            uses: HashMap::with_hasher(FixedState::default()),
            used_names: HashSet::with_hasher(FixedState::default()),
        }
    }

    fn select(&mut self, name: &str, rank: Rank<'g>) -> Option<SymbolAt> {
        let key = self.graph.strings.get(name);
        if let Some(key) = key {
            let holders = self.wanted_first(self.cascade.qualified_as(key).collect());
            if !holders.is_empty() {
                return self.ranked_first(&holders, rank);
            }
        }
        let named = key.map_or_else(Vec::new, |key| self.graph.named(key).to_vec());
        let named = self.wanted_first(named);
        if !named.is_empty() {
            return self.ranked_first(&named, rank);
        }
        if !name.contains(QUALIFIER_SEPARATORS) {
            return None;
        }
        let mut pool = self
            .wanted_first(self.graph.selected(name).collect())
            .into_iter()
            .map(|at| self.cascade.canonical(at));
        let first = pool.next()?;
        pool.all(|at| at == first).then_some(first)
    }

    fn wanted_first(&self, mut candidates: Vec<SymbolAt>) -> Vec<SymbolAt> {
        let is_wanted = |at: SymbolAt| {
            let kind = self.graph.sym(at).kind;
            match self.wanted {
                Wanted::Callables => self.cascade.is_call_target(kind),
                Wanted::Referents => is_call_target_kind(kind),
            }
        };
        if self.wanted == Wanted::Referents || candidates.iter().any(|&at| is_wanted(at)) {
            candidates.retain(|&at| is_wanted(at));
        }
        candidates
    }

    fn ranked_first(&mut self, candidates: &[SymbolAt], rank: Rank<'g>) -> Option<SymbolAt> {
        let (&first, others) = candidates.split_first()?;
        if self.cascade.is_one_callable(first, others.iter().copied()) {
            return Some(self.cascade.canonical(first));
        }
        let mut best: Option<(SymbolAt, usize)> = None;
        for &at in candidates {
            let callable = self.cascade.canonical(at);
            let ranked = rank(self, callable);
            if best.is_none_or(|(_, held)| ranked > held) {
                best = Some((callable, ranked));
            }
        }
        best.map(|(callable, _)| callable)
    }

    fn passed_over(&self, name: &str, chosen: SymbolAt) -> usize {
        let mut others: Vec<SymbolAt> = self
            .wanted_first(self.graph.selected(name).collect())
            .into_iter()
            .map(|at| self.cascade.canonical(at))
            .filter(|&at| at != chosen)
            .collect();
        others.sort_unstable();
        others.dedup();
        others.len()
    }

    fn sites_of_name(&self, name: Str) -> Vec<Site> {
        let mut names = vec![name];
        let mut seen: HashSet<Str, FixedState> = HashSet::with_hasher(FixedState::default());
        seen.insert(name);
        let mut next = 0;
        while let Some(&source) = names.get(next) {
            next += 1;
            let renamed = self.graph.renamed_from(source).iter();
            names.extend(
                renamed
                    .map(|rename| rename.local_name)
                    .filter(|&local| seen.insert(local)),
            );
        }
        let mut keys: Vec<Str> = names
            .into_iter()
            .filter_map(|name| self.graph.site_key(name))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        let mut sites = Vec::new();
        for key in keys {
            self.graph.sites_into(key, &mut sites);
        }
        if sites.len() > 1 {
            sites.sort_unstable();
        }
        sites
    }

    pub(crate) fn callers_of(&mut self, target: SymbolAt) -> Cow<'_, [Edge]> {
        Cow::Borrowed(self.caller_edges(target))
    }

    pub(crate) fn caller_edges(&mut self, target: SymbolAt) -> &[Edge] {
        self.resolve_callers(self.graph.sym(target).name);
        edges_of(&self.callers, target)
    }

    fn caller_count(&mut self, target: SymbolAt) -> usize {
        self.resolve_callers(self.graph.sym(target).name);
        self.callers.get(&target).map_or(0, Vec::len)
    }

    fn resolve_callers(&mut self, name: Str) {
        if !self.called_names.insert(name) {
            return;
        }
        let mut strongest: HashMap<(SymbolAt, SymbolAt), Edge, FixedState> =
            HashMap::with_hasher(FixedState::default());
        for site in self.sites_of_name(name) {
            let call = self.cascade.call(site);
            let Some((callee, confidence)) = self.cascade.resolve_call(&call) else {
                continue;
            };
            let callable = self.cascade.canonical(callee);
            if self.graph.sym(callable).name != name {
                continue;
            }
            keep_stronger(
                &mut strongest,
                (callable, call.caller_at()),
                Edge {
                    far: call.caller_at(),
                    confidence,
                    line: call.line,
                },
            );
        }
        let mut by_callable: HashMap<SymbolAt, Vec<Edge>, FixedState> =
            HashMap::with_hasher(FixedState::default());
        for ((callable, _), edge) in strongest {
            by_callable.entry(callable).or_default().push(edge);
        }
        for (callable, mut edges) in by_callable {
            self.sort(&mut edges);
            self.callers.insert(callable, edges);
        }
    }

    pub(crate) fn callees_of(&mut self, body: SymbolAt) -> Vec<Edge> {
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

    fn walked_callees(&mut self, body: SymbolAt) -> Cow<'_, [Edge]> {
        Cow::Owned(self.callees_of(body))
    }

    fn site_count(&mut self, target: SymbolAt) -> usize {
        let name = self.graph.sym(target).name;
        self.resolve_uses(name);
        self.caller_count(target) + self.uses.get(&target).map_or(0, Vec::len)
    }

    fn resolve_uses(&mut self, name: Str) {
        if !self.used_names.insert(name) {
            return;
        }
        let mut best: HashMap<(SymbolAt, SymbolAt, u32), u8, FixedState> =
            HashMap::with_hasher(FixedState::default());
        for site in self.graph.uses(name) {
            let Some((used, confidence)) = self.cascade.resolve_reference(site.file, site.index)
            else {
                continue;
            };
            let reference = self.graph.file(site.file).references[site.index as usize];
            let at = SymbolAt {
                file: site.file,
                index: reference.site_of,
            };
            let key = (self.cascade.canonical(used), at, reference.line);
            let held = best.entry(key).or_insert(confidence);
            *held = (*held).max(confidence);
        }
        for ((used, far, line), confidence) in best {
            self.uses.entry(used).or_default().push(Edge {
                far,
                confidence,
                line,
            });
        }
    }

    fn sort(&self, edges: &mut [Edge]) {
        edges.sort_unstable_by_key(|edge| (self.graph.path_rank(edge.far.file), edge.far.index));
    }

    fn walk(&mut self, root: SymbolAt, limits: Limits, edges_of: EdgesOf<'g>) -> Walk<'g> {
        let graph = self.graph;
        let mut reached = Vec::new();
        let left_out = self.reach(WalkState::new(root, limits), edges_of, |edge, depth| {
            reached.push(edge.reached(graph, depth));
        });
        Walk {
            root: graph.definition_at(root),
            passed_over: 0,
            reached,
            truncated: left_out.truncated,
            turned_away: left_out.turned_away,
            unexpanded: left_out.unexpanded,
            below_confidence: left_out.below_confidence,
            unresolved: self.unresolved,
        }
    }

    pub(crate) fn reach(
        &mut self,
        mut state: WalkState,
        edges_of: EdgesOf<'g>,
        mut visit: impl FnMut(Edge, u32),
    ) -> LeftOut {
        while let Some(current) = state.queue.pop_front() {
            let depth = state.visited[&current];
            if depth >= state.limits.depth {
                state.left_out.unexpanded += 1;
                continue;
            }
            let edges = edges_of(self, self.cascade.canonical(current));
            state.take(&edges, depth + 1, &mut visit);
            if state.left_out.truncated {
                state.left_out.unexpanded += state.queue.len();
                break;
            }
        }
        state.left_out
    }

    fn path(&mut self, start: SymbolAt, target: SymbolAt, limits: Limits) -> CallPath<'g> {
        let mut search = PathSearch {
            start,
            limits,
            toward: HashMap::with_hasher(FixedState::default()),
            queue: VecDeque::new(),
            turned_away: 0,
            at_depth_bound: 0,
            below_confidence: 0,
        };
        search.toward.insert(target, (0, None));
        if limits.depth > 0 {
            search.queue.push_back(target);
        }
        let mut end = if start == target {
            PathEnd::Found
        } else {
            PathEnd::Exhausted
        };

        while end == PathEnd::Exhausted
            && let Some(current) = search.queue.pop_front()
        {
            let depth = search.toward[&current].0;
            self.resolve_callers(self.graph.sym(current).name);
            let edges = self.callers.get(&current).map_or(&[][..], Vec::as_slice);
            end = search.take(&self.cascade, (current, depth + 1), edges);
        }

        CallPath {
            from: self.graph.definition_at(start),
            to: self.graph.definition_at(target),
            hops: search.hops(self.graph, end),
            found: end == PathEnd::Found,
            truncated: end == PathEnd::Truncated,
            turned_away: search.turned_away,
            at_depth_bound: search.at_depth_bound,
            below_confidence: search.below_confidence,
            passed_over: (0, 0),
        }
    }
}

pub(crate) struct WalkState {
    limits: Limits,
    left_out: LeftOut,
    visited: HashMap<SymbolAt, u32, FixedState>,
    queue: VecDeque<SymbolAt>,
}

impl WalkState {
    pub(crate) fn new(root: SymbolAt, limits: Limits) -> Self {
        let mut visited = HashMap::with_hasher(FixedState::default());
        visited.insert(root, 0);
        Self {
            limits,
            left_out: LeftOut::default(),
            visited,
            queue: VecDeque::from([root]),
        }
    }

    fn take(&mut self, edges: &[Edge], depth: u32, visit: &mut impl FnMut(Edge, u32)) {
        for &edge in edges {
            if self.visited.contains_key(&edge.far) {
                continue;
            }
            if edge.confidence < self.limits.min_confidence {
                self.left_out.below_confidence += 1;
                continue;
            }
            if self.limits.max_symbols > 0 && self.visited.len() >= self.limits.max_symbols {
                self.left_out.truncated = true;
                self.left_out.turned_away += 1;
                continue;
            }
            self.visited.insert(edge.far, depth);
            self.queue.push_back(edge.far);
            visit(edge, depth);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathEnd {
    Found,
    Truncated,
    Exhausted,
}

type Toward = Option<(SymbolAt, Edge)>;

struct PathSearch {
    start: SymbolAt,
    limits: Limits,
    toward: HashMap<SymbolAt, (u32, Toward), FixedState>,
    queue: VecDeque<SymbolAt>,
    turned_away: usize,
    at_depth_bound: usize,
    below_confidence: usize,
}

impl PathSearch {
    fn take(
        &mut self,
        cascade: &Cascade<'_>,
        (nearer_the_end, depth): (SymbolAt, u32),
        edges: &[Edge],
    ) -> PathEnd {
        let is_taken = |search: &Self, edge: &Edge| {
            !search.toward.contains_key(&cascade.canonical(edge.far))
                && edge.confidence >= search.limits.min_confidence
        };
        for (at, &edge) in edges.iter().enumerate() {
            let reached = cascade.canonical(edge.far);
            if self.toward.contains_key(&reached) {
                continue;
            }
            if edge.confidence < self.limits.min_confidence {
                self.below_confidence += 1;
                continue;
            }
            if self.limits.max_symbols > 0 && self.toward.len() >= self.limits.max_symbols {
                self.turned_away = 1 + edges[at + 1..]
                    .iter()
                    .filter(|edge| is_taken(self, edge))
                    .count();
                return PathEnd::Truncated;
            }
            if reached == self.start {
                self.toward
                    .insert(reached, (depth, Some((nearer_the_end, edge))));
                return PathEnd::Found;
            }
            self.reach(reached, depth, Some((nearer_the_end, edge)));
        }
        PathEnd::Exhausted
    }

    fn reach(&mut self, at: SymbolAt, depth: u32, toward: Toward) {
        self.toward.insert(at, (depth, toward));
        if depth < self.limits.depth {
            self.queue.push_back(at);
        } else {
            self.at_depth_bound += 1;
        }
    }

    fn hops<'g>(&self, graph: &'g Graph, end: PathEnd) -> Vec<Reached<'g>> {
        let mut hops = Vec::new();
        let mut node = self.start;
        while end == PathEnd::Found
            && let Some(&(_, Some((callee, edge)))) = self.toward.get(&node)
        {
            hops.push(Reached {
                symbol: graph.definition_at(node),
                depth: narrow(hops.len()),
                confidence: edge.confidence,
                line: edge.line,
            });
            node = callee;
        }
        hops
    }
}
