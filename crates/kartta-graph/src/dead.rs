use foldhash::fast::FixedState;
use hashbrown::HashMap;
use kartta_lang::{CallableRole, SymbolKind, TypeUses};

use crate::calls::Calls;
use crate::confidence;
use crate::graph::{Definition, Graph, agree, narrow};
use crate::implementations::Types;
use crate::resolve::is_type_kind;
use crate::stored::SymbolAt;
use crate::strings::Str;

/// A definition that no call and no use the graph holds reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Dead<'g> {
    /// The definition.
    pub definition: Definition<'g>,
    /// Whether a call or a use the graph holds spells the definition's name: they were resolved, and none reaches it.
    /// False when nothing the graph holds spells the name.
    pub spelled: bool,
}

/// The definitions a dead-code query did not judge, each counted once, under the first reason that holds for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct Excluded {
    /// Of a kind the query did not ask for.
    pub kind: usize,
    /// Test code, which a test runner calls.
    pub test: usize,
    /// Generated code.
    pub generated: usize,
    /// Entry points, which run with no call that spells their name.
    pub entry: usize,
    /// Vendored code, which the repository carries but whose callers mostly live elsewhere.
    pub vendored: usize,
    /// Of a kind that no call and no use the language records shows to be used: a field, a value, a namespace, a type
    /// where the language does not record a use wherever code names one, and any definition local to a body, which no
    /// use binds, so its holder can pass or return it unseen.
    pub unobservable: usize,
    /// Definitions that a call of their name may reach which binds no definition, binds one only by proximity, or
    /// binds its own caller through a value: a bare call may reach any definition of the name, and a call through a
    /// receiver a member, or a free definition that no member of the name could stand for or that its file sees, in
    /// its own file, in the files it includes, and where a package is a directory, in its package.
    pub uncertain: usize,
    /// Methods a call through another type may reach: one an interface or a trait declares a method of the name for,
    /// one that a parent of its owner declares too, and one of an owner with a parent the graph does not hold.
    pub dispatched: usize,
}

/// The definitions that no call and no use reaches, and what the query left out.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DeadCode<'g> {
    /// The dead definitions, in path order and in the order each file lists them, at most the query's limit.
    pub found: Vec<Dead<'g>>,
    /// The dead definitions, before the limit.
    pub total: usize,
    /// The definitions judged and found reached.
    pub live: usize,
    /// The definitions left unjudged, by reason.
    pub excluded: Excluded,
}

impl Graph {
    /// The definitions that no call and no use the graph holds reaches, in files whose path starts with `path_prefix`
    /// and of the `kinds` asked for, every kind when it is empty, at most `limit` of them in path order; 0 answers the
    /// counts alone.
    ///
    /// A declaration and the definition it links to are one definition, judged once. A call or a use from the
    /// definition's own body does not reach it, so recursion alone keeps nothing alive. A call binds one definition,
    /// yet a build may link any other that the tier which bound it held equal, so it reaches every definition of the
    /// bound one's qualified name: anywhere for a qualified call or a call through the receiver's type, in the
    /// directory for a call bound through an import or a package, and in the file for a call bound in a file. A
    /// declaration that several definitions implement links to none of them, so a call of it reaches each of them, and
    /// a call of one of them reaches it. A call or a use made at file scope has no symbol to resolve from, so it
    /// reaches every definition its name spells in its own file, in the files it includes, and where a package is a
    /// directory, in its package. A type that another type names as a parent is reached.
    ///
    /// Only what the graph can see used is judged: a function, a method or a macro, through its calls, and a type where
    /// the language records a use wherever code names one, when none of them is local to a body. The answer counts the
    /// rest as excluded, with test, generated, vendored and entry code, the definitions a call that binds nothing
    /// certain may reach, and the methods a call through another type may reach. A definition found dead is a
    /// candidate, not a proof: a call the graph cannot resolve, a function passed as a value where the language records
    /// no use, or a caller outside the repository reaches it unseen.
    ///
    /// Every call and use that spells the name of a judged definition is resolved, so a query over a whole graph costs
    /// what resolving the graph's calls costs, and `limit` bounds the answer, not the work.
    #[must_use]
    pub fn dead_code(
        &self,
        kinds: &[SymbolKind],
        path_prefix: Option<&str>,
        limit: usize,
    ) -> DeadCode<'_> {
        let mut judge = Judge {
            graph: self,
            calls: Calls::new(self),
            types: Types::new(self),
            kinds,
            answer: DeadCode {
                found: Vec::new(),
                total: 0,
                live: 0,
                excluded: Excluded::default(),
            },
            alternatives: HashMap::with_hasher(FixedState::default()),
            interface_owners: HashMap::with_hasher(FixedState::default()),
            interface_methods: None,
            dispatching: HashMap::with_hasher(FixedState::default()),
        };
        let mut slots: Vec<u32> = self
            .file_slots()
            .filter(|&slot| {
                path_prefix.is_none_or(|prefix| {
                    self.strings
                        .resolve(self.file(slot).path)
                        .starts_with(prefix)
                })
            })
            .collect();
        slots.sort_unstable_by_key(|&slot| self.path_rank(slot));
        for slot in slots {
            for index in 0..narrow(self.file(slot).symbols.len()) {
                judge.judge(SymbolAt { file: slot, index }, limit);
            }
        }
        judge.answer
    }
}

struct Judge<'g, 'k> {
    graph: &'g Graph,
    calls: Calls<'g>,
    types: Types<'g>,
    kinds: &'k [SymbolKind],
    answer: DeadCode<'g>,
    alternatives: HashMap<Str, Alternatives, FixedState>,
    interface_owners: HashMap<Str, bool, FixedState>,
    interface_methods: Option<HashMap<Str, usize, FixedState>>,
    dispatching: HashMap<(Str, Str), bool, FixedState>,
}

// constraint: the definitions of one name that a call or a use reaches, under how far the tier that bound it holds
// definitions of the qualified name equal: a qualified call or a call through the receiver's type each one anywhere,
// a call bound in a package each one in its directory, and a call bound in a file each one in that file
#[derive(Default)]
struct Alternatives {
    anywhere: HashMap<Str, Caller, FixedState>,
    in_dir: HashMap<(Str, Str), Caller, FixedState>,
    in_file: HashMap<(Str, u32), Caller, FixedState>,
}

// constraint: the one symbol every call and use comes from, `None` when they come from several
type Caller = Option<SymbolAt>;

impl Alternatives {
    fn add(&mut self, (qualified, dir, file): (Str, Str, u32), edges: &[(SymbolAt, u8)]) {
        for &(far, confidence) in edges {
            match confidence {
                confidence::QUALIFIED | confidence::QUALIFIED_TAIL | confidence::RECEIVER => {
                    agree(&mut self.anywhere, qualified, Some(far));
                }
                confidence::IMPORT | confidence::PACKAGE => {
                    agree(&mut self.in_dir, (qualified, dir), Some(far));
                }
                confidence::SAME_FILE | confidence::INCLUDE | confidence::RE_EXPORT => {
                    agree(&mut self.in_file, (qualified, file), Some(far));
                }
                _ => {}
            }
        }
    }
}

// constraint: a call or a use from a definition's own body, or for a type from a member it owns, shows nothing else
// needs it, so recursion and a method's receiver keep nothing alive
fn is_own(graph: &Graph, at: SymbolAt, far: SymbolAt) -> bool {
    let symbol = graph.sym(at);
    far == at
        || (is_type_kind(symbol.kind)
            && graph
                .sym(far)
                .owner
                .is_some_and(|owner| owner == symbol.name || owner == symbol.qualified_name))
}

impl Judge<'_, '_> {
    fn judge(&mut self, at: SymbolAt, limit: usize) {
        if self.calls.cascade.canonical(at) != at {
            return;
        }
        let symbol = *self.graph.sym(at);
        let flags = self.graph.flags(at);
        let observable = self.is_observable(at);
        let excluded = &mut self.answer.excluded;
        let reason = if !self.kinds.is_empty() && !self.kinds.contains(&symbol.kind) {
            Some(&mut excluded.kind)
        } else if flags.is_test() {
            Some(&mut excluded.test)
        } else if flags.is_generated() {
            Some(&mut excluded.generated)
        } else if flags.is_entry() {
            Some(&mut excluded.entry)
        } else if flags.is_vendored() {
            Some(&mut excluded.vendored)
        } else if !observable {
            Some(&mut excluded.unobservable)
        } else {
            None
        };
        if let Some(count) = reason {
            *count += 1;
            return;
        }
        if self.is_reached(at) {
            self.answer.live += 1;
            return;
        }
        if self.calls.is_called_uncertainly(at) {
            self.answer.excluded.uncertain += 1;
            return;
        }
        if symbol.kind == SymbolKind::Method && self.may_be_dispatched(at) {
            self.answer.excluded.dispatched += 1;
            return;
        }
        self.answer.total += 1;
        if self.answer.found.len() < limit {
            self.answer.found.push(Dead {
                definition: self.graph.definition_at(at),
                spelled: self.graph.is_spelled(symbol.name),
            });
        }
    }

    // constraint: no use binds a local, so a definition local to a body that its holder passes or returns as a value is
    // used where nothing records it
    fn is_observable(&self, at: SymbolAt) -> bool {
        if self.graph.file(at.file).holder(at.index).is_some() {
            return false;
        }
        match self.graph.sym(at).kind {
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Macro => true,
            kind if is_type_kind(kind) => self.graph.conventions.type_uses == TypeUses::Every,
            _ => false,
        }
    }

    fn is_reached(&mut self, at: SymbolAt) -> bool {
        let graph = self.graph;
        self.calls
            .reached_by(at)
            .any(|(far, _)| !is_own(graph, at, far))
            || self.is_an_alternative_reached(at)
            || self.is_a_counterpart_reached(at)
            || self.is_named_at_file_scope(at)
            || (is_type_kind(self.graph.sym(at).kind) && self.types.is_a_parent(at))
    }

    // constraint: a call reaches the one definition the cascade binds, yet a build may link any other that the tier
    // which bound it held equal
    fn is_an_alternative_reached(&mut self, at: SymbolAt) -> bool {
        let graph = self.graph;
        let declarations = self.calls.cascade.declarations(at);
        let alternatives = self.alternatives_of(graph.sym(at).name);
        let from_elsewhere = |caller: Option<&Caller>| caller.is_some_and(|&held| held != Some(at));
        std::iter::once(at)
            .chain(declarations.iter().copied())
            .any(|held| {
                let qualified = graph.sym(held).qualified_name;
                let dir = graph.file(held.file).dir;
                from_elsewhere(alternatives.anywhere.get(&qualified))
                    || from_elsewhere(alternatives.in_dir.get(&(qualified, dir)))
                    || from_elsewhere(alternatives.in_file.get(&(qualified, held.file)))
            })
    }

    fn alternatives_of(&mut self, name: Str) -> &Alternatives {
        if !self.alternatives.contains_key(&name) {
            let gathered = self.gather_alternatives(name);
            self.alternatives.insert(name, gathered);
        }
        &self.alternatives[&name]
    }

    fn gather_alternatives(&mut self, name: Str) -> Alternatives {
        let graph = self.graph;
        let mut alternatives = Alternatives::default();
        for &bound in graph.named(name) {
            if self.calls.cascade.canonical(bound) != bound {
                continue;
            }
            let edges: Vec<(SymbolAt, u8)> = self
                .calls
                .reached_by(bound)
                .filter(|&(far, _)| !is_own(graph, bound, far))
                .collect();
            if edges.is_empty() {
                continue;
            }
            let declarations = self.calls.cascade.declarations(bound);
            for held in std::iter::once(bound).chain(declarations.iter().copied()) {
                let place = (
                    graph.sym(held).qualified_name,
                    graph.file(held.file).dir,
                    held.file,
                );
                alternatives.add(place, &edges);
            }
        }
        alternatives
    }

    // constraint: a declaration that several definitions implement links to none of them, yet a call of the
    // declaration reaches one of them, and a call of one of them needs the declaration
    fn is_a_counterpart_reached(&mut self, at: SymbolAt) -> bool {
        let graph = self.graph;
        let Some(callable) = graph.file(at.file).callable(at.index) else {
            return false;
        };
        let role = callable.role;
        graph
            .occurrences(callable.key)
            .iter()
            .filter(|occurrence| occurrence.role != role)
            .any(|occurrence| {
                let (declaration, definition) = match role {
                    CallableRole::Declaration => (at, occurrence.at),
                    CallableRole::Definition => (occurrence.at, at),
                };
                let counterpart = self.calls.cascade.canonical(occurrence.at);
                counterpart != at
                    && self
                        .calls
                        .cascade
                        .closure(definition.file)
                        .binary_search(&declaration.file)
                        .is_ok()
                    && self
                        .calls
                        .reached_by(counterpart)
                        .any(|(far, _)| far != at && !is_own(graph, counterpart, far))
            })
    }

    fn is_named_at_file_scope(&self, at: SymbolAt) -> bool {
        let Some(key) = self.graph.listing_key(self.graph.sym(at).name) else {
            return false;
        };
        self.graph
            .named_at_file_scope(key)
            .iter()
            .any(|&file| self.calls.cascade.sees(file, at))
    }

    fn may_be_dispatched(&mut self, method: SymbolAt) -> bool {
        let symbol = *self.graph.sym(method);
        let Some(owner) = symbol.owner else {
            return false;
        };
        let own = usize::from(self.is_interface(owner));
        let declared_by_interfaces = self.interface_methods().get(&symbol.name).copied();
        if declared_by_interfaces.unwrap_or(0) > own {
            return true;
        }
        if let Some(&held) = self.dispatching.get(&(owner, symbol.name)) {
            return held;
        }
        let dispatched = self
            .types_named(owner)
            .any(|ty| self.has_a_dispatching_parent(ty, symbol.name));
        self.dispatching.insert((owner, symbol.name), dispatched);
        dispatched
    }

    fn is_interface(&mut self, owner: Str) -> bool {
        if let Some(&held) = self.interface_owners.get(&owner) {
            return held;
        }
        let interface = self.types_named(owner).any(|ty| {
            matches!(
                self.graph.sym(ty).kind,
                SymbolKind::Interface | SymbolKind::Trait
            )
        });
        self.interface_owners.insert(owner, interface);
        interface
    }

    // constraint: how many methods of each name the interfaces and traits declare, gathered once, when the first
    // method a query judges dead is looked at
    fn interface_methods(&mut self) -> &HashMap<Str, usize, FixedState> {
        if self.interface_methods.is_none() {
            let gathered = self.gather_interface_methods();
            self.interface_methods = Some(gathered);
        }
        self.interface_methods.get_or_insert_default()
    }

    fn gather_interface_methods(&mut self) -> HashMap<Str, usize, FixedState> {
        let graph = self.graph;
        let members = graph
            .file_slots()
            .flat_map(|slot| graph.file(slot).symbols.iter())
            .filter(|symbol| symbol.kind == SymbolKind::Method)
            .filter_map(|symbol| Some((symbol.name, symbol.owner?)));
        let mut methods: HashMap<Str, usize, FixedState> =
            HashMap::with_hasher(FixedState::default());
        for (name, owner) in members {
            if self.is_interface(owner) {
                *methods.entry(name).or_default() += 1;
            }
        }
        methods
    }

    fn types_named(&self, owner: Str) -> impl Iterator<Item = SymbolAt> + '_ {
        let key = self.graph.listing_key(owner);
        key.into_iter()
            .flat_map(|key| self.graph.named(key).iter().copied())
            .filter(move |&ty| {
                let symbol = self.graph.sym(ty);
                is_type_kind(symbol.kind)
                    && (symbol.qualified_name == owner || symbol.name == owner)
            })
    }

    fn has_a_dispatching_parent(&self, ty: SymbolAt, name: Str) -> bool {
        self.graph
            .file(ty.file)
            .parents_of(ty.index)
            .iter()
            .any(|parent| {
                let mut bases = self
                    .graph
                    .listing_key(parent.written)
                    .into_iter()
                    .flat_map(|key| self.graph.named(key).iter().copied())
                    .filter(|&base| is_type_kind(self.graph.sym(base).kind))
                    .peekable();
                bases.peek().is_none() || bases.any(|base| self.declares(base, name))
            })
    }

    fn declares(&self, ty: SymbolAt, name: Str) -> bool {
        let type_name = self.graph.sym(ty).name;
        self.graph.named(name).iter().any(|&member| {
            let symbol = self.graph.sym(member);
            symbol.kind == SymbolKind::Method
                && symbol
                    .owner
                    .is_some_and(|owner| self.graph.listing_key(owner) == Some(type_name))
        })
    }
}
