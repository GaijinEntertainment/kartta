use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::VecDeque;
use std::hash::Hash;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{Access, SymbolKind};

use crate::confidence;
use crate::graph::{Definition, Graph, QUALIFIER_SEPARATORS, is_call_target_kind, narrow};
use crate::resolve::{Call, Cascade, UseMiss};
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

/// Which of a symbol's sites [`Graph::references`] answers, and how many of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SiteQuery {
    /// The least [`confidence`](crate::confidence) a site needs, 0 to 100; 0 takes every site.
    pub min_confidence: u8,
    /// The uses kept: those that read the symbol for [`Access::Read`], that assign it for [`Access::Write`], and the
    /// updates alone for [`Access::Update`]; an update reads and assigns, and no call is kept. `None` keeps every site.
    pub access: Option<Access>,
    /// How many sites the answer lists at most.
    pub limit: usize,
}

impl SiteQuery {
    /// A query for at most `limit` sites, of any confidence and any access.
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        Self {
            min_confidence: 0,
            access: None,
            limit,
        }
    }

    /// This query with the confidence floor set to `min_confidence`.
    #[must_use]
    pub const fn with_min_confidence(self, min_confidence: u8) -> Self {
        Self {
            min_confidence,
            ..self
        }
    }

    /// This query keeping only the uses that touch the symbol as `access` says.
    #[must_use]
    pub const fn with_access(self, access: Access) -> Self {
        Self {
            access: Some(access),
            ..self
        }
    }

    const fn keeps(self, access: Option<Access>) -> bool {
        match (self.access, access) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(Access::Read), Some(access)) => access.reads(),
            (Some(Access::Write), Some(access)) => access.writes(),
            (Some(Access::Update), Some(access)) => matches!(access, Access::Update),
        }
    }
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
    /// line for each way it touches the symbol.
    pub call: bool,
    /// How the site touches the symbol; a call, and a use as a type, read it.
    pub access: Access,
}

/// How many sites of each kind a symbol has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct SiteCounts {
    /// The calls.
    pub calls: usize,
    /// The other uses that read the symbol, or name it as a type.
    pub reads: usize,
    /// The uses that assign it without reading it.
    pub writes: usize,
    /// The uses that read and assign it.
    pub updates: usize,
}

impl SiteCounts {
    const fn add(&mut self, access: Option<Access>) {
        match access {
            None => self.calls += 1,
            Some(Access::Read) => self.reads += 1,
            Some(Access::Write) => self.writes += 1,
            Some(Access::Update) => self.updates += 1,
        }
    }
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
    /// The sites that pass the confidence floor and the access the query keeps, before the limit.
    pub total: usize,
    /// The sites the confidence floor left out.
    pub below_confidence: usize,
    /// The sites that pass the confidence floor, of each kind, whatever access the query keeps.
    pub counts: SiteCounts,
    /// The uses that take a member of the symbol's name on a value and bind no definition, because the graph could
    /// not follow the value's type and no other tier picked one of the members of the name, and those of the own
    /// receiver that several bases may hold: any of them, this one included, may be meant. Counted for a member only,
    /// and 0 for any other symbol.
    pub ambiguous: usize,
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

    /// The sites that call or use the symbol `name` selects that `query` keeps, or `None` when it selects none. A name
    /// selects among every symbol a use can name, a value and a field included where calls target neither, and a bare
    /// name shared by several of them selects the one with the most sites.
    ///
    /// The uses are those the language records. A member taken on a value binds through the type the language spells
    /// for the value, else the one field of the name in the use's own file, and where the language says so, in the
    /// files it includes or reaches. A member of `this` or `self` binds the member of the class around the use or of a
    /// base the graph finds, and where a bare name in a member reads the class first, the field of the own class or
    /// of the one base that lookup finds. Every site of the name is resolved before the query's limit applies, so the
    /// limit bounds the answer, not the work, and `total` counts them all.
    #[must_use]
    pub fn references(&self, name: &str, query: SiteQuery) -> Option<References<'_>> {
        let mut calls = Calls::wanting(self, Wanted::Referents);
        let root = calls.select(name, Calls::site_count)?;

        let named = self.sym(root).name;
        calls.resolve_callers(named);
        calls.resolve_uses(named);
        let mut sites: Vec<(Edge, Option<Access>)> = edges_of(&calls.callers, root)
            .iter()
            .map(|&edge| (edge, None))
            .chain(
                edges_of(&calls.uses, root)
                    .iter()
                    .map(|&(edge, access)| (edge, Some(access))),
            )
            .collect();
        let mut below_confidence = 0;
        let mut counts = SiteCounts::default();
        sites.retain(|&(edge, access)| {
            if edge.confidence < query.min_confidence {
                below_confidence += 1;
                return false;
            }
            counts.add(access);
            query.keeps(access)
        });
        let total = sites.len();
        let by_file_and_line = |&(edge, access): &(Edge, Option<Access>)| {
            (
                self.path_rank(edge.far.file),
                edge.line,
                access,
                edge.far.index,
            )
        };
        if query.limit < sites.len() {
            if let Some(last) = query.limit.checked_sub(1) {
                sites.select_nth_unstable_by_key(last, by_file_and_line);
            }
            sites.truncate(query.limit);
        }
        sites.sort_unstable_by_key(by_file_and_line);

        Some(References {
            symbol: self.definition_at(root),
            passed_over: calls.passed_over(name, root),
            total,
            sites: sites
                .into_iter()
                .map(|(edge, access)| Reference {
                    site_of: self.definition_at(edge.far),
                    line: edge.line,
                    confidence: edge.confidence,
                    call: access.is_none(),
                    access: access.unwrap_or(Access::Read),
                })
                .collect(),
            below_confidence,
            counts,
            ambiguous: if self.sym(root).owner.is_some() {
                calls.ambiguous.get(&named).copied().unwrap_or(0)
            } else {
                0
            },
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

fn edges_of<T>(held: &HashMap<SymbolAt, Vec<T>, FixedState>, at: SymbolAt) -> &[T] {
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
    pub(crate) cascade: Cascade<'g>,
    wanted: Wanted,
    unresolved: usize,
    callers: HashMap<SymbolAt, Vec<Edge>, FixedState>,
    called_names: HashSet<Str, FixedState>,
    uncertain: HashMap<Str, Uncertain, FixedState>,
    uses: HashMap<SymbolAt, Vec<(Edge, Access)>, FixedState>,
    used_names: HashSet<Str, FixedState>,
    ambiguous: HashMap<Str, usize, FixedState>,
}

// constraint: the calls of one name that bind no definition, bind one only by proximity, or bind their own caller
// through a value, by how they are written: whether one is bare, and the files that make one through a receiver,
// sorted and once each; whether a member of the name exists is looked up once, when a free definition first asks
#[derive(Debug, Default)]
struct Uncertain {
    bare: bool,
    through_a_receiver_in: Vec<u32>,
    has_members: Option<bool>,
}

impl Uncertain {
    fn add(&mut self, call: &Call) {
        match call.receiver {
            None => self.bare = true,
            Some(_) if self.through_a_receiver_in.last() != Some(&call.file) => {
                self.through_a_receiver_in.push(call.file);
            }
            Some(_) => {}
        }
    }
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

    pub(crate) fn reached_by(&mut self, at: SymbolAt) -> impl Iterator<Item = (SymbolAt, u8)> + '_ {
        let name = self.graph.sym(at).name;
        self.resolve_callers(name);
        self.resolve_uses(name);
        let uses = edges_of(&self.uses, at).iter().map(|&(edge, _)| edge);
        edges_of(&self.callers, at)
            .iter()
            .copied()
            .chain(uses)
            .map(|edge| (edge.far, edge.confidence))
    }

    // constraint: a call that binds no definition, binds one only by proximity, or binds its own caller through a value
    // may reach any other definition the way it is written admits: a bare call any definition of the name, and a call
    // through a receiver a member, or a free definition its file sees or that no member of the name could stand for,
    // as a module's function, a function a table holds, or one the extractor could not place in its class is called
    pub(crate) fn is_called_uncertainly(&mut self, at: SymbolAt) -> bool {
        let symbol = self.graph.sym(at);
        self.resolve_callers(symbol.name);
        let (graph, cascade) = (self.graph, &self.cascade);
        let Some(uncertain) = self.uncertain.get_mut(&symbol.name) else {
            return false;
        };
        let files = &uncertain.through_a_receiver_in;
        if uncertain.bare || (symbol.owner.is_some() && !files.is_empty()) {
            return true;
        }
        if files.is_empty() {
            return false;
        }
        if files.iter().any(|&file| cascade.sees(file, at)) {
            return true;
        }
        let has_members = uncertain.has_members.get_or_insert_with(|| {
            graph.named(symbol.name).iter().any(|&member| {
                let member = graph.sym(member);
                member.owner.is_some() && cascade.is_call_target(member.kind)
            })
        });
        !*has_members
    }

    fn wanting(graph: &'g Graph, wanted: Wanted) -> Self {
        Self {
            graph,
            cascade: Cascade::new(graph),
            wanted,
            unresolved: 0,
            callers: HashMap::with_hasher(FixedState::default()),
            called_names: HashSet::with_hasher(FixedState::default()),
            uncertain: HashMap::with_hasher(FixedState::default()),
            uses: HashMap::with_hasher(FixedState::default()),
            used_names: HashSet::with_hasher(FixedState::default()),
            ambiguous: HashMap::with_hasher(FixedState::default()),
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
                Wanted::Referents => is_call_target_kind(kind) || kind == SymbolKind::Field,
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

    fn renamings(&self, name: Str) -> Vec<Str> {
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
        names
    }

    fn sites_of_name(&self, name: Str) -> Vec<Site> {
        let mut keys: Vec<Str> = self
            .renamings(name)
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
            let resolved = self.cascade.resolve_call(&call).ok();
            let through_a_value = call
                .receiver
                .is_some_and(|receiver| self.graph.strings.resolve(receiver).is_empty());
            if resolved.is_none_or(|(callee, confidence)| {
                confidence < confidence::UNIQUE
                    || (through_a_value && self.cascade.canonical(callee) == call.caller_at())
            }) {
                self.uncertain.entry(name).or_default().add(&call);
            }
            let Some((callee, confidence)) = resolved else {
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
            let Ok((callee, confidence)) = self.cascade.resolve_call(&call) else {
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
        let mut best: HashMap<(SymbolAt, SymbolAt, u32, Access), u8, FixedState> =
            HashMap::with_hasher(FixedState::default());
        let mut ambiguous = 0;
        let renamings = self.renamings(name);
        for site in renamings.iter().flat_map(|&alias| self.graph.uses(alias)) {
            let (used, confidence) = match self.cascade.resolve_reference(site.file, site.index) {
                Ok(bound) => bound,
                Err(UseMiss::Ambiguous) => {
                    ambiguous += 1;
                    continue;
                }
                Err(UseMiss::Elsewhere) => continue,
            };
            let reference = self.graph.file(site.file).references[site.index as usize];
            let at = SymbolAt {
                file: site.file,
                index: reference.site_of(),
            };
            let key = (
                self.cascade.canonical(used),
                at,
                reference.line,
                reference.access(),
            );
            let held = best.entry(key).or_insert(confidence);
            *held = (*held).max(confidence);
        }
        if ambiguous > 0 {
            self.ambiguous.insert(name, ambiguous);
        }
        for ((used, far, line, access), confidence) in best {
            let edge = Edge {
                far,
                confidence,
                line,
            };
            self.uses.entry(used).or_default().push((edge, access));
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
