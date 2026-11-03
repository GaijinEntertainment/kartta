use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::VecDeque;
use std::hash::Hash;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{Access, DataUses};

use crate::confidence;
use crate::graph::{
    Candidate, Definition, Graph, QUALIFIER_SEPARATORS, Selector, distinct, is_data_kind,
    is_referent, keep_first, listed, narrow,
};
use crate::resolution::Miss;
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
    /// The most called of those, the most first, then in path order, at most five.
    pub candidates: Vec<Candidate<'g>>,
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
    /// call: a symbol that calls another from several lines counts once there, at its most confident call. A walk over
    /// callers also counts the files of [`file_scope`](Self::file_scope) the floor left out.
    pub below_confidence: usize,
    /// The calls at the symbols a walk over callees expanded that bound no definition the graph holds: calls into
    /// code outside it, and calls the cascade could not tell apart. A construction of a class outside the graph is one
    /// such call, whatever the language records for its constructor. A walk over callers counts none.
    pub unresolved: usize,
    /// The calls of the root's name that bound no definition because several definitions of the name may be meant and
    /// none is nearer, made as a call of the root can be made: through a receiver for a member, and for a free
    /// definition bare, or through a receiver where no member of the name is a call target. Any of them may call the
    /// root, and none is among [`reached`](Self::reached). Counted for the root alone, so every depth counts the same;
    /// a root local to a body counts none, and neither does a walk over callees.
    pub ambiguous: usize,
    /// The files whose top level, outside every definition, calls or uses the root's name and sees the root, in path
    /// order: callers no symbol holds, so the walk does not go past them. A walk over callees lists none, and neither
    /// does a walk from a root no call binds, such as a field, whose name a top level reads rather than calls.
    pub file_scope: Vec<FileScope<'g>>,
    /// The root's call of itself, which [`reached`](Self::reached) does not list, since the walk starts there and
    /// reaches each symbol once; one under the confidence floor counts in
    /// [`below_confidence`](Self::below_confidence) instead. A symbol the walk reaches further out that calls itself
    /// is listed once, as every symbol is.
    pub self_call: Option<SelfCall>,
}

/// A call of a symbol from its own body: a recursion, or a call of another definition of its name that bound the
/// symbol itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SelfCall {
    /// How sure the resolution of the call is, 15 to 95: the [`confidence`](crate::confidence) of the tier of the
    /// cascade that bound it.
    pub confidence: u8,
    /// The one-based line of the call; where the symbol calls itself from several lines, the first line among the
    /// calls the most confident tier bound.
    pub line: u32,
}

/// A file whose top level calls or uses a name, outside every definition. The graph keeps only the names a file's top
/// level spells, so neither the line nor whether a call or a use spelled it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FileScope<'g> {
    /// The root-relative path of the file.
    pub file: &'g str,
    /// How sure the name is to mean the symbol, by how the file sees it: the [`confidence`](crate::confidence) of a
    /// definition in the same file, in its package, or in a file it includes.
    pub confidence: u8,
}

/// Which of a symbol's sites [`Graph::references`] answers, and how many of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SiteQuery {
    /// The least [`confidence`](crate::confidence) a site needs, 0 to 100; 0 takes every site.
    pub min_confidence: u8,
    /// The uses kept: those that read the symbol or name it as a type for [`Access::Read`], that assign it for
    /// [`Access::Write`], the updates alone for [`Access::Update`], and the uses as a type alone for [`Access::Type`];
    /// an update reads and assigns, and no call is kept. `None` keeps every site.
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
            (Some(Access::Read), Some(access)) => access.reads() || matches!(access, Access::Type),
            (Some(Access::Write), Some(access)) => access.writes(),
            (Some(Access::Update), Some(access)) => matches!(access, Access::Update),
            (Some(Access::Type), Some(access)) => matches!(access, Access::Type),
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
    /// How the site touches the symbol; a call reads it.
    pub access: Access,
}

/// How many sites of each kind a symbol has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct SiteCounts {
    /// The calls.
    pub calls: usize,
    /// The other uses that read the symbol.
    pub reads: usize,
    /// The uses that assign it without reading it.
    pub writes: usize,
    /// The uses that read and assign it.
    pub updates: usize,
    /// The uses that name it as a type.
    pub types: usize,
}

impl SiteCounts {
    const fn add(&mut self, access: Option<Access>) {
        match access {
            None => self.calls += 1,
            Some(Access::Read) => self.reads += 1,
            Some(Access::Write) => self.writes += 1,
            Some(Access::Update) => self.updates += 1,
            Some(Access::Type) => self.types += 1,
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
    /// The most used of those, the most sites first, then in path order, at most five.
    pub candidates: Vec<Candidate<'g>>,
    /// The sites, by file and line, at most the query's limit.
    pub sites: Vec<Reference<'g>>,
    /// The sites that pass the confidence floor and the access the query keeps, before the limit.
    pub total: usize,
    /// The sites the confidence floor left out.
    pub below_confidence: usize,
    /// The sites that pass the confidence floor, of each kind, whatever access the query keeps.
    pub counts: SiteCounts,
    /// The calls of the symbol's name that [`Walk::ambiguous`] counts for a walk from it, and the uses that take a
    /// member of its name on a value and bind no definition, because the graph could not follow the value's type and
    /// no other tier picked one of the members of the name, and those of the own receiver that several bases may
    /// hold: any of them, this one included, may be meant. The uses are counted for a member only.
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
    /// The most called of each end's passed-over callables, `from`'s then `to`'s, the most first, then in path order,
    /// at most five each.
    pub candidates: (Vec<Candidate<'g>>, Vec<Candidate<'g>>),
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
    pub(crate) self_call: Option<Edge>,
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
    /// A selector's path prefix keeps only the symbols in the files under it, and its line only those that start on
    /// it, before any of these rules: the prefix reaches one of several copies that share a qualified name, and the
    /// line one of several symbols of the name in one file.
    ///
    /// Calls are resolved when this asks, from the call sites the graph keeps: the cost grows with the call sites of
    /// each name the walk reaches. The walk also lists the files whose top level names the root and sees it, which the
    /// confidence floor bounds as it bounds a call.
    #[must_use]
    pub fn callers<'s>(&self, name: impl Into<Selector<'s>>, limits: Limits) -> Option<Walk<'_>> {
        let selector = name.into();
        let mut calls = Calls::new(self);
        let root = calls.select(selector, Calls::caller_count)?;
        let mut walk = calls.walk(root, limits, Calls::callers_of);
        if calls.cascade.is_call_target(self.sym(root)) {
            let below_confidence;
            (walk.file_scope, below_confidence) =
                self.file_scope(&calls, root, limits.min_confidence);
            walk.below_confidence += below_confidence;
        }
        walk.ambiguous = calls.ambiguous_calls(root);
        (walk.passed_over, walk.candidates) =
            calls.passed_over(selector, root, Calls::caller_count);
        Some(walk)
    }

    /// The callees of the symbol `name` selects, transitively within `limits`, or `None` when it selects none; a name
    /// selects as [`Graph::callers`] describes.
    #[must_use]
    pub fn callees<'s>(&self, name: impl Into<Selector<'s>>, limits: Limits) -> Option<Walk<'_>> {
        let selector = name.into();
        let mut calls = Calls::new(self);
        let root = calls.select(selector, Calls::caller_count)?;
        let mut walk = calls.walk(root, limits, Calls::walked_callees);
        (walk.passed_over, walk.candidates) =
            calls.passed_over(selector, root, Calls::caller_count);
        Some(walk)
    }

    /// Whether `name` selects a symbol as [`Graph::callers`] selects one, and so as [`Graph::callees`] and each end of
    /// [`Graph::call_path`] do. It ranks none of the symbols a name selects among and resolves no call: those decide
    /// which symbol a name selects, not whether it selects one.
    #[must_use]
    pub fn selects_as_callers<'s>(&self, name: impl Into<Selector<'s>>) -> bool {
        Calls::new(self).selectable(name.into()).is_some()
    }

    /// The sites that call or use the symbol `name` selects that `query` keeps, or `None` when it selects none. A name
    /// selects among every symbol a use can name, a value and a field included where calls target neither, and a bare
    /// name shared by several of them selects the one with the most sites; a path prefix and a line narrow them
    /// as [`Graph::callers`] describes.
    ///
    /// The uses are those the language records. A member taken on a value binds through the type the language spells
    /// for the value, else the one field of the name in the use's own file, and where the language says so, in the
    /// files it includes or reaches. A member of `this` or `self` binds the member of the class around the use or of a
    /// base the graph finds, and where a bare name in a member reads the class first, the field of the own class or
    /// of the one base that lookup finds. Every site of the name is resolved before the query's limit applies, so the
    /// limit bounds the answer, not the work, and `total` counts them all.
    #[must_use]
    pub fn references<'s>(
        &self,
        name: impl Into<Selector<'s>>,
        query: SiteQuery,
    ) -> Option<References<'_>> {
        let selector = name.into();
        let mut calls = Calls::wanting(self, Wanted::Referents);
        let root = calls.select(selector, Calls::site_count)?;

        let named = self.sym(root).name;
        calls.resolve_callers(named);
        calls.resolve_uses(named);
        let mut sites: Vec<(Edge, Option<Access>)> = listed(&calls.callers, &root)
            .iter()
            .map(|&edge| (edge, None))
            .chain(
                listed(&calls.uses, &root)
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
        keep_first(&mut sites, query.limit, |left, right| {
            by_file_and_line(left).cmp(&by_file_and_line(right))
        });
        sites.sort_unstable_by_key(by_file_and_line);
        let (passed_over, candidates) = calls.passed_over(selector, root, Calls::site_count);
        let ambiguous_uses = if self.sym(root).owner.is_some() {
            calls.ambiguous.get(&named).copied().unwrap_or(0)
        } else {
            0
        };

        Some(References {
            symbol: self.definition_at(root),
            passed_over,
            candidates,
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
            ambiguous: ambiguous_uses + calls.ambiguous_calls(root),
        })
    }

    /// The shortest chain of calls from the symbol `from` selects to the one `to` selects, within `limits`, or `None`
    /// when either selects none; each name selects as [`Graph::callers`] describes. A chain the graph does not hold is
    /// no proof that the code holds none: calls are resolved by name and miss dynamic dispatch.
    #[must_use]
    pub fn call_path<'s>(
        &self,
        from: impl Into<Selector<'s>>,
        to: impl Into<Selector<'s>>,
        limits: Limits,
    ) -> Option<CallPath<'_>> {
        let (from, to) = (from.into(), to.into());
        let mut calls = Calls::new(self);
        let start = calls.select(from, Calls::caller_count)?;
        let target = calls.select(to, Calls::caller_count)?;
        let mut path = calls.path(start, target, limits);
        let (from_count, from_candidates) = calls.passed_over(from, start, Calls::caller_count);
        let (to_count, to_candidates) = calls.passed_over(to, target, Calls::caller_count);
        path.passed_over = (from_count, to_count);
        path.candidates = (from_candidates, to_candidates);
        Some(path)
    }

    /// The files whose top level names the symbol at `root` and sees it, in path order, those under `min_confidence`
    /// left out, and how many the floor left out.
    fn file_scope(
        &self,
        calls: &Calls<'_>,
        root: SymbolAt,
        min_confidence: u8,
    ) -> (Vec<FileScope<'_>>, usize) {
        let mut files: Vec<(u32, u8)> = calls.file_scope_callers(root).collect();
        let named = files.len();
        files.retain(|&(_, confidence)| confidence >= min_confidence);
        files.sort_unstable_by_key(|&(file, _)| self.path_rank(file));
        let below_confidence = named - files.len();
        let files = files
            .into_iter()
            .map(|(file, confidence)| FileScope {
                file: self.strings.resolve(self.file(file).path),
                confidence,
            })
            .collect();
        (files, below_confidence)
    }
}

type Rank<'g> = fn(&mut Calls<'g>, SymbolAt) -> usize;

pub(crate) type EdgesOf<'g> = for<'c> fn(&'c mut Calls<'g>, SymbolAt) -> Cow<'c, [Edge]>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Callables,
    Referents,
}

/// What a selector selects before any ranking: the symbols to rank, at least one, or the one callable they all are.
enum Selectable {
    Among(Vec<SymbolAt>),
    One(SymbolAt),
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
    ambiguous_members: HashSet<Str, FixedState>,
    unbound_use_files: HashMap<Str, Vec<u32>, FixedState>,
}

// constraint: the calls of one name that bind no definition, bind one only by proximity or a fallback, or bind their
// own caller through a value, by how they are written: whether one is bare, and the files that make one through a
// receiver, sorted and once each, with how many of each way bound none as ambiguous; whether a member of the name
// exists is found once, when a free definition first asks
#[derive(Debug, Default)]
struct Uncertain {
    bare: bool,
    through_a_receiver_in: Vec<u32>,
    ambiguous_bare: usize,
    ambiguous_through_a_receiver: usize,
    has_members: Option<bool>,
}

impl Uncertain {
    fn add(&mut self, call: &Call, missed: Option<Miss>) {
        let ambiguous = usize::from(missed == Some(Miss::Ambiguous));
        if call.receiver.is_none() {
            self.bare = true;
            self.ambiguous_bare += ambiguous;
            return;
        }
        self.ambiguous_through_a_receiver += ambiguous;
        if self.through_a_receiver_in.last() != Some(&call.file) {
            self.through_a_receiver_in.push(call.file);
        }
    }

    fn has_members(&mut self, cascade: &Cascade<'_>, name: Str) -> bool {
        *self.has_members.get_or_insert_with(|| {
            cascade.named(name).iter().any(|&member| {
                let member = cascade.graph.sym(member);
                member.owner.is_some() && cascade.is_call_target(member)
            })
        })
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
        let uses = listed(&self.uses, &at).iter().map(|&(edge, _)| edge);
        listed(&self.callers, &at)
            .iter()
            .copied()
            .chain(uses)
            .map(|edge| (edge.far, edge.confidence))
    }

    // constraint: a call that binds no definition, binds one only by proximity or a fallback, or binds its own caller
    // through a value may reach any other definition the way it is written admits: a bare call any definition of the
    // name, and a call through a receiver a member, or a free definition its file sees or that no member of the name
    // could stand for, as a module's function, a function a table holds, or one the extractor could not place in its
    // class is called
    pub(crate) fn is_called_uncertainly(&mut self, at: SymbolAt) -> bool {
        let symbol = self.graph.sym(at);
        self.resolve_callers(symbol.name);
        let cascade = &self.cascade;
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
        !uncertain.has_members(cascade, symbol.name)
    }

    // constraint: a use that binds no definition, as one of a member on a value whose type the graph could not follow,
    // may be meant for a member of its name wherever that member is, and for a free value in a file that sees it; one
    // left ambiguous among members of every kind, as a method taken as a value on such a value is, for a member of any
    // kind wherever it is, and one left ambiguous among fields alone for a field alone
    pub(crate) fn is_used_uncertainly(&mut self, at: SymbolAt) -> bool {
        let symbol = self.graph.sym(at);
        self.resolve_uses(symbol.name);
        if symbol.owner.is_some() && self.ambiguous_members.contains(&symbol.name) {
            return true;
        }
        if !is_data_kind(symbol.kind) {
            return false;
        }
        let cascade = &self.cascade;
        self.unbound_use_files
            .get(&symbol.name)
            .is_some_and(|files| {
                symbol.owner.is_some() || files.iter().any(|&file| cascade.sees(file, at))
            })
    }

    // constraint: the cascade leaves a call ambiguous among the definitions its shape admits, and a call through a
    // receiver takes the member shape wherever a member of the name is a call target, a bare call a free shape; a
    // definition local to a body is a candidate of a call in that body alone, which binds it as a local call
    pub(crate) fn ambiguous_calls(&mut self, at: SymbolAt) -> usize {
        let symbol = self.graph.sym(at);
        if !self.cascade.is_call_target(symbol) || self.cascade.holder(at).is_some() {
            return 0;
        }
        self.resolve_callers(symbol.name);
        let cascade = &self.cascade;
        let Some(uncertain) = self.uncertain.get_mut(&symbol.name) else {
            return 0;
        };
        let through_a_receiver = uncertain.ambiguous_through_a_receiver;
        if symbol.owner.is_some() {
            return through_a_receiver;
        }
        if through_a_receiver == 0 || uncertain.has_members(cascade, symbol.name) {
            return uncertain.ambiguous_bare;
        }
        uncertain.ambiguous_bare + through_a_receiver
    }

    fn wanting(graph: &'g Graph, wanted: Wanted) -> Self {
        Self {
            graph,
            cascade: Cascade::new(graph),
            wanted,
            unresolved: 0,
            callers: HashMap::default(),
            called_names: HashSet::default(),
            uncertain: HashMap::default(),
            uses: HashMap::default(),
            used_names: HashSet::default(),
            ambiguous: HashMap::default(),
            ambiguous_members: HashSet::default(),
            unbound_use_files: HashMap::default(),
        }
    }

    fn select(&mut self, selector: Selector<'_>, rank: Rank<'g>) -> Option<SymbolAt> {
        match self.selectable(selector)? {
            Selectable::Among(candidates) => self.ranked_first(&candidates, rank),
            Selectable::One(at) => Some(at),
        }
    }

    fn selectable(&self, selector: Selector<'_>) -> Option<Selectable> {
        let within = |at: &SymbolAt| self.graph.is_within(*at, selector);
        let key = self.graph.strings.get(selector.name);
        if let Some(key) = key {
            let holders =
                self.wanted_first(self.cascade.qualified_as(key).filter(within).collect());
            if !holders.is_empty() {
                return Some(Selectable::Among(holders));
            }
        }
        let named = key.map_or_else(Vec::new, |key| {
            self.graph
                .named(key)
                .iter()
                .copied()
                .filter(within)
                .collect()
        });
        let named = self.wanted_first(named);
        if !named.is_empty() {
            return Some(Selectable::Among(named));
        }
        if !selector.name.contains(QUALIFIER_SEPARATORS) {
            return None;
        }
        let mut pool = self
            .wanted_first(self.graph.selected(selector.name).filter(within).collect())
            .into_iter()
            .map(|at| self.cascade.canonical(at));
        let first = pool.next()?;
        pool.all(|at| at == first).then_some(Selectable::One(first))
    }

    fn wanted_first(&self, mut candidates: Vec<SymbolAt>) -> Vec<SymbolAt> {
        let is_wanted = |at: SymbolAt| {
            let symbol = self.graph.sym(at);
            match self.wanted {
                Wanted::Callables => self.cascade.is_call_target(symbol),
                Wanted::Referents => is_referent(symbol.kind),
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

    fn others(&self, selector: Selector<'_>, chosen: SymbolAt) -> Vec<SymbolAt> {
        let selected = self
            .graph
            .selected(selector.name)
            .filter(|&at| self.graph.is_within(at, selector))
            .collect();
        let others = self
            .wanted_first(selected)
            .into_iter()
            .map(|at| self.cascade.canonical(at))
            .filter(|&at| at != chosen);
        distinct(others)
    }

    fn passed_over(
        &mut self,
        selector: Selector<'_>,
        chosen: SymbolAt,
        rank: Rank<'g>,
    ) -> (usize, Vec<Candidate<'g>>) {
        let others = self.others(selector, chosen);
        let count = others.len();
        let ranked = others.into_iter().map(|at| (at, rank(self, at))).collect();
        (count, self.graph.candidates(ranked))
    }

    fn renamings(&self, name: Str) -> Vec<Str> {
        let mut names = vec![name];
        let mut seen: HashSet<Str, FixedState> = HashSet::default();
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
        let keys = self
            .renamings(name)
            .into_iter()
            .filter_map(|name| self.graph.listing_key(name));
        let mut sites = Vec::new();
        for key in distinct(keys) {
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

    /// What a change to `target` reaches first: its callers, and for a field, a variable or a constant, its users.
    pub(crate) fn dependents_of(&mut self, target: SymbolAt) -> Cow<'_, [Edge]> {
        if is_data_kind(self.graph.sym(target).kind) {
            Cow::Owned(self.user_edges(target))
        } else {
            Cow::Borrowed(self.caller_edges(target))
        }
    }

    /// The users of a field, a variable or a constant: the symbols that call it, or whose code reads or writes it, each
    /// once, at its most confident site, in path order; none for a symbol of any other kind.
    pub(crate) fn user_edges(&mut self, target: SymbolAt) -> Vec<Edge> {
        let name = self.graph.sym(target).name;
        if !is_data_kind(self.graph.sym(target).kind) {
            return Vec::new();
        }
        self.resolve_callers(name);
        self.resolve_uses(name);
        let uses = listed(&self.uses, &target).iter().map(|&(edge, _)| edge);
        let mut strongest: HashMap<SymbolAt, Edge, FixedState> = HashMap::default();
        for edge in listed(&self.callers, &target).iter().copied().chain(uses) {
            if edge.far != target {
                keep_stronger(&mut strongest, edge.far, edge);
            }
        }
        let mut edges: Vec<Edge> = strongest.into_values().collect();
        self.sort(&mut edges);
        edges
    }

    /// The files whose top level calls or uses the name of the definition at `at` and see it, in the order the graph
    /// indexes them, each with how sure the name is to mean it. A file keeps only the names its top level spells, so
    /// neither the line nor whether a call or a use spelled it is known.
    pub(crate) fn file_scope_callers(&self, at: SymbolAt) -> impl Iterator<Item = (u32, u8)> + '_ {
        let symbol = self.graph.sym(at);
        let files = self
            .graph
            .listing_key(symbol.name)
            .map_or(&[][..], |key| self.graph.named_at_file_scope(key));
        let pasted = (self.graph.file(at.file).value_types(at.index))
            .filter(|_| self.cascade.reads_pasted_bodies())
            .map_or_else(Vec::new, |body| self.read_through_a_body(body, symbol.name));
        let seen = files.iter().filter_map(move |&file| {
            self.cascade
                .seen_from(file, at)
                .map(|confidence| (file, confidence))
        });
        let read_through_a_body = (pasted.into_iter())
            .filter(move |&file| self.cascade.seen_from(file, at).is_none())
            .map(|file| (file, confidence::RECEIVER));
        seen.chain(read_through_a_body)
    }

    /// The files whose top level reads `name` through the body named `body`, by that name or a filter that pastes it.
    fn read_through_a_body(&self, body: Str, name: Str) -> Vec<u32> {
        let strings = &self.graph.strings;
        let name = strings.resolve(name);
        let read = |spelled: &str| {
            (strings.get(&format!("{spelled}#{name}")))
                .map_or(&[][..], |read| self.graph.named_at_file_scope(read))
        };
        let mut files = read(strings.resolve(body)).to_vec();
        for filter in self.graph.filters_pasting(body) {
            files.extend_from_slice(read(&format!("@{filter}")));
        }
        files.sort_unstable();
        files.dedup();
        files
    }

    pub(crate) fn caller_edges(&mut self, target: SymbolAt) -> &[Edge] {
        self.resolve_callers(self.graph.sym(target).name);
        listed(&self.callers, &target)
    }

    fn caller_count(&mut self, target: SymbolAt) -> usize {
        self.caller_edges(target).len()
    }

    fn resolve_callers(&mut self, name: Str) {
        if !self.called_names.insert(name) {
            return;
        }
        let mut strongest: HashMap<(SymbolAt, SymbolAt), Edge, FixedState> = HashMap::default();
        for site in self.sites_of_name(name) {
            let call = self.cascade.call(site);
            let resolved = self.cascade.resolve_call(&call);
            let bound = resolved.ok();
            let through_a_value = call
                .receiver
                .is_some_and(|receiver| self.graph.strings.resolve(receiver).is_empty());
            if bound.is_none_or(|(callee, confidence)| {
                confidence < confidence::UNIQUE
                    || self.graph.sym(callee).flags.is_fallback()
                    || (through_a_value && self.cascade.canonical(callee) == call.caller_at())
            }) {
                self.uncertain
                    .entry(name)
                    .or_default()
                    .add(&call, resolved.err());
            }
            let Some((callee, confidence)) = bound else {
                continue;
            };
            let callable = self.cascade.canonical(callee);
            if self.graph.sym(callable).name != name {
                continue;
            }
            let edge = Edge {
                far: call.caller_at(),
                confidence,
                line: call.line,
            };
            for twin in self.cascade.twins(&call, (callee, confidence)) {
                keep_stronger(&mut strongest, (twin, call.caller_at()), edge);
            }
            keep_stronger(&mut strongest, (callable, call.caller_at()), edge);
        }
        let mut by_callable: HashMap<SymbolAt, Vec<Edge>, FixedState> = HashMap::default();
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

        let mut strongest: HashMap<SymbolAt, Edge, FixedState> = HashMap::default();
        for site in sites {
            let call = self.cascade.call(site);
            let Ok((callee, confidence)) = self.cascade.resolve_call(&call) else {
                self.unresolved +=
                    usize::from(!self.cascade.constructs_outside_the_graph(site, &call));
                continue;
            };
            let callable = self.cascade.canonical(callee);
            for twin in self.cascade.twins(&call, (callee, confidence)) {
                let edge = Edge {
                    far: twin,
                    confidence,
                    line: call.line,
                };
                keep_stronger(&mut strongest, twin, edge);
            }
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
        self.caller_count(target) + listed(&self.uses, &target).len()
    }

    fn resolve_uses(&mut self, name: Str) {
        if !self.used_names.insert(name) {
            return;
        }
        let mut best: HashMap<(SymbolAt, SymbolAt, u32, Access), u8, FixedState> =
            HashMap::default();
        let mut ambiguous = 0;
        let mut among_members = false;
        let judges_data = self.graph.conventions.data_uses == DataUses::Every;
        let mut unbound_files: Vec<u32> = Vec::new();
        let renamings = self.renamings(name);
        for site in renamings.iter().flat_map(|&alias| self.graph.uses(alias)) {
            let (used, confidence) = match self.cascade.resolve_use(site.file, site.index) {
                Ok(bound) => bound,
                Err(missed) => {
                    ambiguous += usize::from(missed != UseMiss::Elsewhere);
                    among_members |= missed == UseMiss::Ambiguous;
                    unbound_files.extend(judges_data.then_some(site.file));
                    continue;
                }
            };
            let stored = self.graph.file(site.file).uses[site.index as usize];
            let at = SymbolAt::new(site.file, stored.site_of());
            let twins = self.cascade.use_twins(site.file, (used, confidence));
            for used in twins.into_iter().chain([used]) {
                let key = (
                    self.cascade.canonical(used),
                    at,
                    stored.line,
                    stored.access(),
                );
                let held = best.entry(key).or_insert(confidence);
                *held = (*held).max(confidence);
            }
        }
        if ambiguous > 0 {
            self.ambiguous.insert(name, ambiguous);
        }
        if among_members {
            self.ambiguous_members.insert(name);
        }
        if !unbound_files.is_empty() {
            unbound_files.sort_unstable();
            unbound_files.dedup();
            self.unbound_use_files.insert(name, unbound_files);
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
        let self_call = left_out.self_call;
        let dropped = self_call.is_some_and(|edge| edge.confidence < limits.min_confidence);
        Walk {
            root: graph.definition_at(root),
            passed_over: 0,
            candidates: Vec::new(),
            reached,
            truncated: left_out.truncated,
            turned_away: left_out.turned_away,
            unexpanded: left_out.unexpanded,
            below_confidence: left_out.below_confidence + usize::from(dropped),
            unresolved: self.unresolved,
            ambiguous: 0,
            file_scope: Vec::new(),
            self_call: self_call.filter(|_| !dropped).map(|edge| SelfCall {
                confidence: edge.confidence,
                line: edge.line,
            }),
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
            toward: HashMap::default(),
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
            let edges = listed(&self.callers, &current);
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
            candidates: (Vec::new(), Vec::new()),
        }
    }
}

pub(crate) struct WalkState {
    root: SymbolAt,
    limits: Limits,
    left_out: LeftOut,
    visited: HashMap<SymbolAt, u32, FixedState>,
    queue: VecDeque<SymbolAt>,
}

impl WalkState {
    pub(crate) fn new(root: SymbolAt, limits: Limits) -> Self {
        let mut visited = HashMap::default();
        visited.insert(root, 0);
        Self {
            root,
            limits,
            left_out: LeftOut::default(),
            visited,
            queue: VecDeque::from([root]),
        }
    }

    fn take(&mut self, edges: &[Edge], depth: u32, visit: &mut impl FnMut(Edge, u32)) {
        for &edge in edges {
            // constraint: the root, at a depth of 0, is the one symbol whose edges are taken at a depth of 1
            if depth == 1 && edge.far == self.root {
                self.left_out.self_call = Some(edge);
                continue;
            }
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
