use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};
use std::hash::Hash;
use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{
    BareReceivers, BuiltApart, CallableRole, CallsApart, Conventions, FILE_SCOPE, IncludeReach,
    IncludedRuntimeMembers, InheritedMembers, MemberCalls, NamespaceCalls, OwnMembers, Receivers,
    SymbolFlags, SymbolKind, Twins, UntypedMembers, ValueUses, path,
};

use crate::candidates::{CandidateSet, Candidates, Gate, INDEXED_FROM};
use crate::confidence;
use crate::graph::{Graph, QUALIFIER_SEPARATORS, is_call_target_symbol, or_if_none, tails};
use crate::members::DataNamed;
use crate::resolution::Miss;
use crate::scope::{Ancestors, OwnerMember, Reach, Scoped, Shape, is_value};
use crate::stored::{Entry, Site, StoredSymbol, SymbolAt, UseReceiver};
use crate::strings::Str;
use crate::twins::Twin;
use crate::typed::{Typed, Value};

pub(crate) const THROUGH_INCLUDES_MAX_DEFINITIONS: usize = 32;

/// What the constant a receiver names holds, where capitalised receivers name constants, as its
/// [value types](kartta_lang::Symbol::value_types) record.
#[derive(Clone, Copy)]
enum ConstantValue<'g> {
    /// The receiver names a class or a module, a constant the graph does not hold, or a value that records no type.
    NoValueType,
    /// A value of a class the empty path says the extraction cannot name, or values of several classes.
    Untold,
    /// The class, spelled as a receiver names it, and whether the constant is that class rather than an instance.
    Holds(&'g str, bool),
}

#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a mark of how the call is written, read on its own by the tiers that care, as the stored \
              call keeps them in separate bits"
)]
pub(crate) struct Call {
    pub(crate) file: u32,
    pub(crate) caller: u32,
    pub(crate) callee: Str,
    pub(crate) receiver: Option<Str>,
    pub(crate) written_from_root: bool,
    pub(crate) names_a_runtime_method: bool,
    pub(crate) on_a_class: bool,
    pub(crate) binds_apart: bool,
    pub(crate) calls_a_method: bool,
    pub(crate) arguments: u8,
    pub(crate) local: Option<SymbolAt>,
    pub(crate) line: u32,
}

impl Call {
    /// A bare call of `callee` from `site` with no line, as a use asks the tiers of a call.
    pub(crate) const fn bare(site: SymbolAt, callee: Str) -> Self {
        Self {
            file: site.file,
            caller: site.index,
            callee,
            receiver: None,
            written_from_root: false,
            names_a_runtime_method: false,
            on_a_class: false,
            binds_apart: false,
            calls_a_method: false,
            arguments: 0,
            local: None,
            line: 0,
        }
    }

    pub(crate) const fn caller_at(&self) -> SymbolAt {
        SymbolAt::new(self.file, self.caller)
    }

    const fn form(&self) -> (bool, bool, u8, bool) {
        (
            self.binds_apart,
            self.calls_a_method,
            self.arguments,
            self.arguments != 0 && self.receiver.is_some(),
        )
    }
}

pub(crate) type Bound = (SymbolAt, u8);

/// Why a use binds no definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UseMiss {
    /// What it names lies outside the graph, or nothing the graph holds can stand for it.
    Elsewhere,
    /// Definitions of the name may be meant, and no tier picks one.
    Ambiguous,
    /// Fields of the name may be meant, and no tier picks one; no other member of the name is a candidate.
    AmbiguousField,
}

// constraint: a named binding is the name it binds, the directory or file it imports, and the name the import reaches
// there, which differs from the bound name under an alias; a required module is the name an included file goes by or
// the alias a `require` gives that file, beside that file
#[derive(Debug, Default)]
struct Bindings {
    named: Vec<(Str, Option<Str>, Str)>,
    dot_dirs: Vec<Str>,
    modules: bool,
    required: Vec<(Str, u32)>,
}

type Derived<K, V> = RefCell<HashMap<K, V, FixedState>>;

// constraint: the call targets of one name, of the call's form, a call apart, a method call or neither, with the
// arguments it passes, and among those the ones the caller's package binds by name where it is gated, else every one
type CandidatesKey = (Str, (bool, bool, u8, bool), Option<Str>);

pub(crate) fn derived<K: Eq + Hash, V: Clone>(
    cache: &Derived<K, V>,
    key: K,
    derive: impl FnOnce() -> V,
) -> V {
    if let Some(held) = cache.borrow().get(&key) {
        return held.clone();
    }
    let value = derive();
    cache.borrow_mut().insert(key, value.clone());
    value
}

pub(crate) struct Cascade<'g> {
    pub(crate) graph: &'g Graph,
    pub(crate) conventions: Conventions,
    star: Option<Str>,
    bindings: Derived<u32, Rc<Bindings>>,
    candidates: Derived<CandidatesKey, Rc<Candidates<'g>>>,
    referenceable: Derived<Str, Rc<Candidates<'g>>>,
    pub(crate) data_named: Derived<Str, Rc<DataNamed<'g>>>,
    includes: Derived<u32, Rc<[u32]>>,
    closures: Derived<u32, Rc<[u32]>>,
    roots: Derived<u32, Rc<[u32]>>,
    canonical: Derived<SymbolAt, SymbolAt>,
    declarations: Derived<SymbolAt, Rc<[SymbolAt]>>,
    constructor_types: Derived<SymbolAt, Option<Rc<[SymbolAt]>>>,
    pub(crate) type_ancestors: Derived<SymbolAt, Option<Rc<Ancestors>>>,
    pub(crate) owner_ancestors: Derived<Str, Option<Rc<Ancestors>>>,
    pub(crate) receiver_types: Derived<(SymbolAt, Str), Value<'g>>,
    pub(crate) evaluations: Cell<u16>,
    empty_receiver: Option<Str>,
    // constraint: a written definition is likelier a call's callee than a namesake a macro invocation writes, so a call
    // the cascade binds to a made one or leaves ambiguous among a name's made ones runs again on a twin that holds the
    // written ones alone, and takes what that run binds
    written_only: bool,
    written: OnceCell<Box<Self>>,
    made_names: Derived<Str, bool>,
}

impl<'g> Cascade<'g> {
    pub(crate) fn new(graph: &'g Graph) -> Self {
        Self::of(graph, false)
    }

    fn of(graph: &'g Graph, written_only: bool) -> Self {
        Self {
            graph,
            conventions: graph.conventions,
            star: graph.strings.get("*"),
            bindings: RefCell::default(),
            candidates: RefCell::default(),
            referenceable: RefCell::default(),
            data_named: RefCell::default(),
            includes: RefCell::default(),
            closures: RefCell::default(),
            roots: RefCell::default(),
            canonical: RefCell::default(),
            declarations: RefCell::default(),
            constructor_types: RefCell::default(),
            type_ancestors: RefCell::default(),
            owner_ancestors: RefCell::default(),
            receiver_types: RefCell::default(),
            evaluations: Cell::new(0),
            empty_receiver: graph.strings.get(""),
            written_only,
            written: OnceCell::new(),
            made_names: RefCell::default(),
        }
    }

    pub(crate) fn call(&self, site: Site) -> Call {
        let file = self.graph.file(site.file);
        match site.entry() {
            Entry::Call(index) => {
                let call = file.calls[index as usize];
                self.with_returned_receiver(Call {
                    file: site.file,
                    caller: call.caller(),
                    callee: call.callee,
                    receiver: call.receiver,
                    written_from_root: call.is_written_from_root(),
                    names_a_runtime_method: call.receiver.is_some()
                        && self.is_runtime_method(call.callee),
                    on_a_class: call.is_on_a_class(),
                    binds_apart: call.binds_apart(),
                    calls_a_method: call.calls_a_method(),
                    arguments: call.arguments(),
                    local: None,
                    line: call.line(),
                })
            }
            Entry::LocalCall(index) => {
                let call = file.local_calls[index as usize];
                Call {
                    file: site.file,
                    caller: call.caller,
                    callee: file.symbols[call.local as usize].name,
                    receiver: None,
                    written_from_root: false,
                    names_a_runtime_method: false,
                    on_a_class: false,
                    binds_apart: false,
                    calls_a_method: false,
                    arguments: 0,
                    local: Some(SymbolAt::new(site.file, call.local)),
                    line: call.line,
                }
            }
        }
    }

    fn is_runtime_method(&self, callee: Str) -> bool {
        let methods = self.conventions.runtime_methods;
        !methods.is_empty()
            && methods
                .binary_search(&self.graph.strings.resolve(callee))
                .is_ok()
    }

    pub(crate) fn reads_pasted_bodies(&self) -> bool {
        self.conventions.receivers == Receivers::PastedBodies
    }

    pub(crate) fn implicit_receiver(&self) -> bool {
        self.conventions.own_members == OwnMembers::First
    }

    pub(crate) fn bare_call_reaches_own_type(&self) -> bool {
        self.conventions.own_members == OwnMembers::Fallback
    }

    pub(crate) fn bare_call_reaches_members(&self) -> bool {
        self.implicit_receiver()
            || self.conventions.inherited_members != InheritedMembers::Unreached
    }

    pub(crate) const fn is_call_target(&self, symbol: &StoredSymbol) -> bool {
        is_call_target_symbol(symbol)
            && (self.conventions.values_are_call_targets || !is_value(symbol.kind))
    }

    pub(crate) fn named(&self, name: Str) -> &'g [SymbolAt] {
        self.graph.named(name)
    }

    pub(crate) fn holder(&self, at: SymbolAt) -> Option<SymbolAt> {
        self.graph
            .file(at.file)
            .holder(at.index)
            .map(|index| SymbolAt::new(at.file, index))
    }

    pub(crate) fn qualified(&self, qualified: Str) -> Option<SymbolAt> {
        self.qualified_as(qualified).min_by_key(|&at| {
            (
                !self.is_call_target(self.graph.sym(at)),
                self.graph.order_of(at),
            )
        })
    }

    pub(crate) fn qualified_as(&self, qualified: Str) -> impl Iterator<Item = SymbolAt> + '_ {
        let named = |tail: &str| {
            self.graph
                .strings
                .get(tail)
                .map_or(&[][..], |tail| self.named(tail))
        };
        let is_it = move |at: &SymbolAt| self.graph.sym(*at).qualified_name == qualified;
        let mut tails = tails(self.graph.strings.resolve(qualified));
        let last = self
            .named(qualified)
            .iter()
            .chain(tails.next().map_or(&[][..], named))
            .copied()
            .filter(is_it);
        or_if_none(last, tails.flat_map(named).copied().filter(is_it))
    }

    pub(crate) fn is_candidate(&self, call: &Call, at: SymbolAt) -> bool {
        let caller = call.caller_at();
        self.is_candidate_in((self.holder(caller).unwrap_or(caller), call), at)
    }

    fn is_candidate_in(&self, (scope, call): (SymbolAt, &Call), at: SymbolAt) -> bool {
        let file = self.graph.file(at.file);
        let symbol = &file.symbols[at.index as usize];
        self.is_call_target(symbol)
            && self.is_of_the_call_s_form(call, symbol.kind)
            && !self.names_a_type(call, symbol)
            && Self::takes_the_arguments(call, symbol)
            && self.binds_if_made(symbol.kind, symbol.flags)
            && (file.locals.is_empty()
                || file.holder(at.index).is_none_or(|holder| {
                    symbol.owner.is_some() && SymbolAt::new(at.file, holder) == scope
                }))
    }

    // constraint: a value a macro invocation writes is a list's entry, rust-analyzer's `sym::Some` among them, so a
    // call of its name means a namesake outside the graph
    const fn binds_if_made(&self, kind: SymbolKind, flags: SymbolFlags) -> bool {
        !flags.is_made() || !(self.written_only || is_value(kind))
    }

    // constraint: a method call calls a method, never the value of a variable or a constant
    const fn is_of_the_call_s_form(&self, call: &Call, kind: SymbolKind) -> bool {
        (matches!(self.conventions.calls_apart, CallsApart::None)
            || call.binds_apart == matches!(kind, SymbolKind::Macro))
            && !(call.calls_a_method && is_value(kind))
    }

    // constraint: a method call names a member, never a type or the constructor that stands for one
    fn names_a_type(&self, call: &Call, symbol: &StoredSymbol) -> bool {
        call.calls_a_method && (symbol.kind.is_type() || self.is_constructor(symbol))
    }

    fn is_constructor(&self, symbol: &StoredSymbol) -> bool {
        self.conventions.owner_chain_separator.is_none()
            && symbol.kind == SymbolKind::Method
            && symbol.owner == Some(symbol.name)
    }

    /// Whether `symbol` is the method a construction runs: one named for its class, where the language writes a
    /// constructor so, or of the language's [`Conventions::constructor`](kartta_lang::Conventions) name.
    pub(crate) fn is_a_constructor(&self, symbol: &StoredSymbol) -> bool {
        self.is_constructor(symbol)
            || self.conventions.constructor == Some(self.graph.strings.resolve(symbol.name))
    }

    // constraint: a method call names no type, so a file it includes that offers one of the name offers it nothing, yet
    // the member another file offers is then the one left of a name shared across files, which is a guess
    fn another_include_offers_a_type(&self, call: &Call, member: SymbolAt) -> bool {
        let named = self.named(call.callee);
        call.calls_a_method
            && (self.includes(call.file).iter())
                .filter(|&&file| file != member.file)
                .flat_map(|&file| self.graph.in_file(named, file))
                .any(|&at| self.names_a_type(call, self.graph.sym(at)))
    }

    // constraint: the object a member call passes fills a method's implicit parameter, which its counts leave out, and
    // nothing of a namespace, whatever a bare call of its name passes the macro it holds
    const fn arguments_taken(call: &Call, symbol: &StoredSymbol) -> (u8, u8) {
        match (call.receiver.is_some(), symbol.kind) {
            (true, SymbolKind::Method) => (
                symbol.fewest_arguments().saturating_add(1),
                symbol.most_arguments.saturating_add(1),
            ),
            (true, SymbolKind::Namespace) => (0, 0),
            _ => (symbol.fewest_arguments(), symbol.most_arguments),
        }
    }

    const fn takes_the_arguments(call: &Call, symbol: &StoredSymbol) -> bool {
        !Self::passes_too_many(call, symbol) && !Self::passes_too_few(call, symbol)
    }

    const fn passes_too_many(call: &Call, symbol: &StoredSymbol) -> bool {
        call.arguments > Self::arguments_taken(call, symbol).1
    }

    // constraint: a use asks the tiers as a call of `0` arguments, and a construction counts none, so a count of `0`
    // shows no call short of a definition
    const fn passes_too_few(call: &Call, symbol: &StoredSymbol) -> bool {
        call.arguments != 0 && call.arguments < Self::arguments_taken(call, symbol).0
    }

    // constraint: the definition a call passes too few sits as near as the one it fits, so nearness did not single that
    // one out: the count did, and code outside the graph takes what the name's definitions do not
    pub(crate) fn passes_too_few_as_near(
        &self,
        call: &Call,
        named: &[SymbolAt],
        confidence: u8,
    ) -> bool {
        let caller = self.graph.file(call.file);
        self.overloads_can_lie_outside(call)
            && named.iter().any(|&at| {
                let file = self.graph.file(at.file);
                let symbol = self.graph.sym(at);
                let as_near = if confidence == confidence::SAME_DIR {
                    file.dir == caller.dir
                } else {
                    file.parent_dir == caller.parent_dir
                };
                as_near
                    && self.is_call_target(symbol)
                    && self.is_of_the_call_s_form(call, symbol.kind)
                    && Self::passes_too_few(call, symbol)
            })
    }

    pub(crate) fn call_targets<'a>(
        &'a self,
        call: &'a Call,
        candidates: impl Iterator<Item = SymbolAt> + Clone + 'a,
    ) -> impl Iterator<Item = SymbolAt> + Clone + 'a {
        let caller = call.caller_at();
        let scope = self.holder(caller).unwrap_or(caller);
        candidates.filter(move |&at| self.is_candidate_in((scope, call), at))
    }

    pub(crate) fn free_target(
        &self,
        call: &Call,
        candidates: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        self.call_targets(call, candidates)
            .find(|&at| self.graph.sym(at).owner.is_none())
    }

    fn pick(
        &self,
        pick: Pick<'_>,
        mut candidates: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        match pick {
            Pick::FreeTarget(call) => self.free_target(call, candidates),
            Pick::PackageLevel => candidates.find(|&at| self.is_package_level(at)),
        }
    }

    fn is_package_level(&self, at: SymbolAt) -> bool {
        let symbol = self.graph.sym(at);
        symbol.owner.is_none() && is_call_target_symbol(symbol) && self.holder(at).is_none()
    }

    fn declared_in(&self, candidates: &Candidates<'_>, dir: Str) -> Vec<SymbolAt> {
        candidates
            .in_dir(self.graph, dir)
            .iter()
            .copied()
            .filter(|&at| !self.graph.flags(at).is_test() && self.holder(at).is_none())
            .collect()
    }

    // constraint: a directory can hold a package and its external test package, and a package's test files are
    // compiled only with its tests, so a caller sees the package its own file declares, test files only from a test
    fn in_package_of(&self, candidates: &Candidates<'_>, caller: SymbolAt) -> Vec<SymbolAt> {
        let file = self.graph.file(caller.file);
        let test = self.graph.sym(caller).flags.is_test();
        candidates
            .in_dir(self.graph, file.dir)
            .iter()
            .copied()
            .filter(|&at| {
                let symbol = self.graph.sym(at);
                (test || !symbol.flags.is_test())
                    && self.graph.file(at.file).package == file.package
                    && self.holder(at).is_none()
            })
            .collect()
    }

    pub(crate) fn includes(&self, file: u32) -> Rc<[u32]> {
        derived(&self.includes, file, || {
            let mut slots: Vec<u32> = self
                .graph
                .file(file)
                .includes
                .iter()
                .filter_map(|&target| self.graph.slot_of(target))
                .collect();
            slots.sort_unstable_by_key(|&slot| self.graph.path_rank(slot));
            slots.into()
        })
    }

    pub(crate) fn closure(&self, file: u32) -> Rc<[u32]> {
        derived(&self.closures, file, || {
            let mut visited: HashSet<u32, FixedState> = HashSet::default();
            visited.insert(file);
            let mut pending = vec![file];
            while let Some(current) = pending.pop() {
                let includes = self.includes(current);
                pending.extend(
                    includes
                        .iter()
                        .copied()
                        .filter(|&included| visited.insert(included)),
                );
            }
            let mut closure: Vec<u32> = visited.into_iter().collect();
            closure.sort_unstable();
            closure.into()
        })
    }

    /// Whether `target` is `file` or a file `file` reaches through its includes, however deep.
    pub(crate) fn reaches(&self, file: u32, target: u32) -> bool {
        self.closure(file).binary_search(&target).is_ok()
    }

    pub(crate) fn sees(&self, file: u32, at: SymbolAt) -> bool {
        self.seen_from(file, at).is_some()
    }

    /// How sure a name `file` spells is to mean the definition at `at`, by how the file sees it: its own definitions,
    /// where a package is a directory those of its package, and those of the files it includes, the nearest first;
    /// `None` where it does not see it.
    pub(crate) fn seen_from(&self, file: u32, at: SymbolAt) -> Option<u8> {
        if file == at.file {
            Some(confidence::SAME_FILE)
        } else if self.conventions.directory_packages
            && self.graph.file(file).dir == self.graph.file(at.file).dir
        {
            Some(confidence::PACKAGE)
        } else if self.includes(file).contains(&at.file) {
            Some(confidence::INCLUDE)
        } else {
            None
        }
    }

    pub(crate) fn canonical(&self, at: SymbolAt) -> SymbolAt {
        derived(&self.canonical, at, || {
            self.linked_definition(at).unwrap_or(at)
        })
    }

    // constraint: where includes paste text in place, a definition the declaration's own file reaches, as in an inline
    // implementation header a class header includes at its end, is in every translation unit that declares it; of
    // several definitions either way none is meant, a platform's conditional alternatives or a library's copies most
    // often; test code is built apart, so a declaration outside it is in no translation unit that defines it in a test
    fn linked_definition(&self, declaration: SymbolAt) -> Option<SymbolAt> {
        let callable = self
            .graph
            .file(declaration.file)
            .callable(declaration.index)?;
        if callable.role != CallableRole::Declaration {
            return None;
        }
        let pasted = self.conventions.include_reach == IncludeReach::Transitive;
        let in_test = self.graph.sym(declaration).flags.is_test();
        let mut definitions = self
            .graph
            .occurrences(callable.key)
            .iter()
            .filter(|occurrence| occurrence.role == CallableRole::Definition)
            .map(|occurrence| occurrence.at)
            .filter(|&definition| in_test || !self.graph.sym(definition).flags.is_test())
            .filter(|definition| {
                self.reaches(definition.file, declaration.file)
                    || (pasted && self.reaches(declaration.file, definition.file))
            });
        let definition = definitions.next()?;
        definitions.next().is_none().then_some(definition)
    }

    pub(crate) fn declarations(&self, definition: SymbolAt) -> Rc<[SymbolAt]> {
        derived(&self.declarations, definition, || {
            if self.conventions.owner_chain_separator.is_none() && self.bare_call_reaches_members()
            {
                self.linked_declarations(definition).into()
            } else {
                Rc::from([])
            }
        })
    }

    fn linked_declarations(&self, definition: SymbolAt) -> Vec<SymbolAt> {
        let Some(callable) = self.graph.file(definition.file).callable(definition.index) else {
            return Vec::new();
        };
        if callable.role != CallableRole::Definition {
            return Vec::new();
        }
        self.graph
            .occurrences(callable.key)
            .iter()
            .filter(|occurrence| occurrence.role == CallableRole::Declaration)
            .map(|occurrence| occurrence.at)
            .filter(|&declaration| self.canonical(declaration) == definition)
            .collect()
    }

    pub(crate) fn constructor_types(&self, at: SymbolAt) -> Option<Rc<[SymbolAt]>> {
        derived(&self.constructor_types, at, || {
            self.is_constructor(self.graph.sym(at))
                .then(|| self.seen_types(at).into())
        })
    }

    fn seen_types(&self, constructor: SymbolAt) -> Vec<SymbolAt> {
        let viewers: Vec<u32> = std::iter::once(constructor.file)
            .chain(self.declarations(constructor).iter().map(|at| at.file))
            .collect();
        let sees = |viewer: u32, ty: SymbolAt| {
            self.graph.file(ty.file).dir == self.graph.file(viewer).dir
                || self.reaches(viewer, ty.file)
        };
        self.named(self.graph.sym(constructor).name)
            .iter()
            .copied()
            .filter(|&ty| {
                self.graph.sym(ty).kind.is_type() && viewers.iter().any(|&viewer| sees(viewer, ty))
            })
            .collect()
    }

    pub(crate) fn stands_for_its_type(&self, at: SymbolAt) -> bool {
        self.constructor_types(at)
            .is_some_and(|types| types.is_empty())
    }

    pub(crate) fn is_one_callable(
        &self,
        first: SymbolAt,
        mut others: impl Iterator<Item = SymbolAt>,
    ) -> bool {
        let callable = self.canonical(first);
        others.all(|at| self.canonical(at) == callable)
    }

    fn bindings(&self, file: u32) -> Rc<Bindings> {
        derived(&self.bindings, file, || Rc::new(self.read_bindings(file)))
    }

    fn read_bindings(&self, file: u32) -> Bindings {
        let mut bindings = Bindings::default();
        for binding in &self.graph.file(file).bindings {
            match binding.alias.map(|alias| self.graph.strings.resolve(alias)) {
                Some("_") => continue,
                Some(".") => {
                    bindings.dot_dirs.extend(binding.target_dir);
                    continue;
                }
                _ => {}
            }
            let name = binding.alias.unwrap_or_else(|| {
                binding
                    .target_dir
                    .and_then(|dir| self.package_name(dir))
                    .unwrap_or(binding.last_segment)
            });
            let mut target = binding.target_dir;
            // constraint: a script alias names the file its `require` resolved to, which a module name cannot, since
            // two files can share one or declare none; the name stays bound so that no tier below answers it
            if self.reads_modules()
                && let Some(module) = target.and_then(|file| self.graph.slot_of(file))
            {
                bindings.required.push((name, module));
                target = None;
            }
            bindings.named.retain(|&(held, _, _)| held != name);
            bindings.named.push((name, target, binding.last_segment));
            bindings.modules |= target.is_some_and(|target| self.graph.slot_of(target).is_some());
        }
        bindings.required.extend(
            (self.includes(file).iter()).filter_map(|&slot| Some((self.module_name(slot)?, slot))),
        );
        bindings
    }

    // constraint: the compiler names a script file with no module clause for the `require` that reaches it, whose last
    // component is the stem of the file it finds; the graph keeps no directive kind, so an `include` names one too, and
    // no other language's include names a module
    fn module_name(&self, slot: u32) -> Option<Str> {
        let file = self.graph.file(slot);
        file.package.or_else(|| {
            if !self.reads_modules() {
                return None;
            }
            let name = path::base(self.graph.strings.resolve(file.path));
            self.graph
                .strings
                .get(name.rsplit_once('.').map_or(name, |(stem, _)| stem))
        })
    }

    // constraint: a script module is the file that declares it, so a qualifier no import binds names the module of a
    // file its own file requires; code that compiles qualifies no definition the module keeps private
    fn required_definition(
        &self,
        file: u32,
        (qualifier, name): (Str, Str),
        pick: Pick<'_>,
    ) -> Option<SymbolAt> {
        let module = self.required_module(file, qualifier)?;
        self.pick(
            pick,
            self.graph.in_file(self.named(name), module).iter().copied(),
        )
    }

    fn required_module(&self, file: u32, qualifier: Str) -> Option<u32> {
        (self.bindings(file).required.iter())
            .find(|&&(declared, _)| declared == qualifier)
            .map(|&(_, module)| module)
    }

    // constraint: daslang looks `mod::f` up through `mod` and what it requires publicly, never through the caller, so a
    // module one file declares answers the call or nothing answers it, whether the caller's `require` or another's
    // brought the module; of two declarers neither is known to be the one a call reaches
    fn declared_module(&self, file: u32, qualifier: Str) -> Option<u32> {
        if !self.reads_modules() || self.graph.file(file).package == Some(qualifier) {
            return None;
        }
        let mut modules = (self.named(qualifier).iter())
            .filter(|&&at| self.graph.sym(at).kind == SymbolKind::Namespace);
        let module = modules.next()?.file;
        modules.next().is_none().then_some(module)
    }

    fn reads_modules(&self) -> bool {
        self.conventions.bare_receivers == BareReceivers::Modules
    }

    fn package_name(&self, dir: Str) -> Option<Str> {
        self.graph
            .package_files(dir)
            .iter()
            .find_map(|&slot| self.graph.file(slot).package_stamp())
    }

    pub(crate) fn binding(&self, file: u32, name: Str) -> Option<ImportedDir> {
        self.binding_and_source(file, name).map(|(dir, _)| dir)
    }

    fn binding_and_source(&self, file: u32, name: Str) -> Option<(ImportedDir, Str)> {
        self.bindings(file)
            .named
            .iter()
            .find(|&&(held, _, _)| held == name)
            .map(|&(_, dir, source)| (dir, source))
    }

    pub(crate) fn dot_imported_dirs(&self, file: u32) -> Vec<Str> {
        self.bindings(file).dot_dirs.clone()
    }

    pub(crate) fn is_unbound_namespace(&self, at: SymbolAt) -> bool {
        self.conventions.namespace_calls == NamespaceCalls::Unbound
            && self.graph.sym(at).kind == SymbolKind::Namespace
    }

    pub(crate) fn resolve_call(&self, call: &Call) -> Result<Bound, Miss> {
        let through;
        let constant = self.constant_value(call);
        let call = match constant {
            ConstantValue::Holds(class, on_a_class) => {
                through = Call {
                    receiver: Some(self.graph.strings.get(class).ok_or(Miss::NotAdmitted)?),
                    on_a_class,
                    ..*call
                };
                &through
            }
            ConstantValue::NoValueType | ConstantValue::Untold => call,
        };
        let resolved = self.resolve_by_tiers(call).and_then(|(at, confidence)| {
            if self.is_unbound_namespace(at) || !self.constant_receiver_reaches(call, at) {
                Err(Miss::NotAdmitted)
            } else if matches!(constant, ConstantValue::Untold)
                || self.writes_on_a_value(call)
                || self.own_member_on_a_field(call, at)
            {
                Ok((at, confidence::FIRST_PARTY))
            } else {
                Ok((at, confidence))
            }
        });
        let made_may_decide = match resolved {
            Ok((at, _)) => self.graph.sym(at).flags.is_made(),
            Err(miss) => miss == Miss::Ambiguous && self.names_a_made_definition(call.callee),
        };
        if !self.written_only
            && made_may_decide
            && let bound @ Ok(_) = self
                .written
                .get_or_init(|| Box::new(Self::of(self.graph, true)))
                .resolve_call(call)
        {
            return bound;
        }
        resolved.and_then(|(at, confidence)| {
            if !self.graph.sym(at).flags.is_made() {
                self.built_with(call.file, (at, confidence))
                    .ok_or(Miss::NotAdmitted)
            } else if confidence < confidence::SAME_DIR {
                Err(Miss::Ambiguous)
            } else {
                Ok((at, confidence.min(confidence::MADE)))
            }
        })
    }

    /// The definitions beside `bound` that `call` reaches too, as the language's [`Twins`] say.
    pub(crate) fn twins(&self, call: &Call, bound: Bound) -> Vec<SymbolAt> {
        match self.conventions.twins {
            Twins::Every => self.twins_in_reach(call, bound),
            Twins::Package(built_apart) => self.package_twins(built_apart, call.file, bound),
            _ => Vec::new(),
        }
    }

    /// The definitions beside `bound` that a use in `file` reaches too, as a call does where a package holds them; a
    /// use of a definition with twins in a file reaches that one alone.
    pub(crate) fn use_twins(&self, file: u32, bound: Bound) -> Vec<SymbolAt> {
        match self.conventions.twins {
            Twins::Package(built_apart) => self.package_twins(built_apart, file, bound),
            _ => Vec::new(),
        }
    }

    fn package_twins(&self, built_apart: BuiltApart, file: u32, bound: Bound) -> Vec<SymbolAt> {
        if bound.0.file == file || !Self::stands_for_a_package(bound) {
            return Vec::new();
        }
        (self.declarations_of(file, bound.0))
            .filter(|&at| at != bound.0 && !self.apart(built_apart, file, at.file))
            .collect()
    }

    fn apart(&self, built_apart: BuiltApart, file: u32, other: u32) -> bool {
        match (
            self.graph.build_constraint(file),
            self.graph.build_constraint(other),
        ) {
            (None, None) => false,
            (one, other) => (built_apart.0)(one.unwrap_or_default(), other.unwrap_or_default()),
        }
    }

    // constraint: a tier below the import's, the package's and the receiver's does not know the name is the package's,
    // and a file's own declaration is the one it is built with
    const fn stands_for_a_package((_, confidence): Bound) -> bool {
        matches!(
            confidence,
            confidence::IMPORT | confidence::PACKAGE | confidence::RECEIVER
        )
    }

    /// Whether a definition is one of `bound`'s qualified name, kind and holder in its package that `file` sees.
    fn alike(&self, file: u32, bound: SymbolAt) -> impl Fn(&SymbolAt) -> bool {
        let symbol = self.graph.sym(bound);
        let dir = self.graph.file(bound.file).dir;
        let held = self.graph.file(file).symbols.first();
        let sees_tests = held.is_some_and(|symbol| symbol.flags.is_test());
        let holder = self.holder(bound);
        move |&at| {
            let other = self.graph.sym(at);
            other.qualified_name == symbol.qualified_name
                && self.graph.file(at.file).dir == dir
                && (sees_tests || !other.flags.is_test())
                && self.holder(at) == holder
        }
    }

    /// The declarations `bound` stands for in its package, itself among them.
    fn declarations_of(&self, file: u32, bound: SymbolAt) -> impl Iterator<Item = SymbolAt> {
        let named = self.named(self.graph.sym(bound).name);
        self.graph
            .package_files(self.graph.file(bound.file).dir)
            .iter()
            .flat_map(move |&slot| self.graph.in_file(named, slot).iter().copied())
            .filter(self.alike(file, bound))
    }

    /// Of the declarations `bound` stands for, the own one of `file`, the file of the site, else `bound` unless a build
    /// never compiles it with `file`, else the first such a build does. A name no such build declares binds nothing.
    pub(crate) fn built_with(&self, file: u32, (bound, confidence): Bound) -> Option<Bound> {
        let Twins::Package(built_apart) = self.conventions.twins else {
            return Some((bound, confidence));
        };
        if bound.file == file || !Self::stands_for_a_package((bound, confidence)) {
            return Some((bound, confidence));
        }
        let named = self.named(self.graph.sym(bound).name);
        let own = (self.graph.in_file(named, file).iter().copied()).find(self.alike(file, bound));
        let apart = |other: u32| self.apart(built_apart, file, other);
        let built = || {
            if apart(bound.file) {
                self.declarations_of(file, bound).find(|at| !apart(at.file))
            } else {
                Some(bound)
            }
        };
        own.or_else(built).map(|at| (at, confidence))
    }

    /// The definitions beside `bound` where the tier that bound it, the caller's file or the type its receiver names,
    /// holds them equal: those of its qualified name, kind and holder in its file, and, where the caller's file reaches
    /// `bound` through a file it includes, in every file it includes. A member and its twins are exported, and in
    /// `bound`'s own file only those of a type the file defines more than once. None where the call is made inside
    /// `bound` or one of them, which a build compiles alone.
    fn twins_in_reach(&self, call: &Call, (bound, confidence): Bound) -> Vec<SymbolAt> {
        if !matches!(confidence, confidence::SAME_FILE | confidence::RECEIVER) {
            return Vec::new();
        }
        let symbol = self.graph.sym(bound);
        let named = self.named(symbol.name);
        // constraint: a trait impl's members are never exported, and one type's impls of several traits, or of one
        // trait for several arguments, each define a member of one name
        if named.len() < 2 || (symbol.owner.is_some() && !symbol.exported()) {
            return Vec::new();
        }
        let holder = self.holder(bound);
        let is_twin = |&at: &SymbolAt| {
            let other = self.graph.sym(at);
            at != bound
                && other.qualified_name == symbol.qualified_name
                && other.kind == symbol.kind
                && (other.owner.is_none() || other.exported())
                && self.holder(at) == holder
        };
        let includes = (bound.file != call.file)
            .then(|| self.includes(call.file))
            .filter(|includes| includes.contains(&bound.file));
        let files = std::iter::once(bound.file).chain(
            (includes
                .iter()
                .flat_map(|includes| includes.iter().copied()))
            .filter(|&file| file != bound.file),
        );
        let mut twins: Vec<SymbolAt> = files
            .flat_map(|file| self.graph.in_file(named, file).iter().copied())
            .filter(is_twin)
            .collect();
        if symbol.owner.is_some()
            && twins.iter().any(|twin| twin.file == bound.file)
            && !self.owner_is_defined_twice(symbol, bound.file)
        {
            twins.retain(|twin| twin.file != bound.file);
        }
        let caller = call.caller_at();
        let body = self.holder(caller).unwrap_or(caller);
        if body == bound || twins.contains(&body) {
            twins.clear();
        }
        twins
    }

    // constraint: one type's members of one name in one file are those of impls that differ in its arguments or in the
    // trait they implement, `impl Ptr<str>` beside `impl<T> Ptr<[T]>`, which no build picks between
    /// Whether the file at `file` defines the type that owns the member `symbol` more than once.
    fn owner_is_defined_twice(&self, symbol: &StoredSymbol, file: u32) -> bool {
        let strings = &self.graph.strings;
        let Some(separator) = self.conventions.owner_chain_separator else {
            return false;
        };
        let Some((owner, _)) = strings
            .resolve(symbol.qualified_name)
            .rsplit_once(separator)
        else {
            return false;
        };
        let bare = owner.rsplit_once(separator).map_or(owner, |(_, bare)| bare);
        let Some(bare) = strings.get(bare) else {
            return false;
        };
        (self.graph.in_file(self.named(bare), file).iter())
            .filter(|&&at| {
                let ty = self.graph.sym(at);
                ty.kind.is_type() && strings.resolve(ty.qualified_name) == owner
            })
            .nth(1)
            .is_some()
    }

    fn names_a_made_definition(&self, name: Str) -> bool {
        derived(&self.made_names, name, || {
            self.named(name)
                .iter()
                .any(|&at| self.graph.sym(at).flags.is_made())
        })
    }

    // constraint: a receiver that names a constant holds the class or the module of that name, or a value where the
    // graph holds only a value of it, so a member of a class that one does not derive from is no method it has, and a
    // class the graph does not hold is the runtime's or a library's outside the graph; a value that records its type
    // stands only for the spelling that names it, as `IRB::TOPLEVEL_BINDING` is no `TOPLEVEL_BINDING`. The caller's own
    // class written from the root answers none of its instance methods, and a method of any module where the class
    // can gain it at run time, as a module's `included` hook extends it
    /// Whether the call reaches `at`: where the receiver names a constant, `at` is a member of the class or the module
    /// the receiver names or of one it derives from, or the graph holds only values of the name; any other receiver
    /// reaches every definition.
    fn constant_receiver_reaches(&self, call: &Call, at: SymbolAt) -> bool {
        let (BareReceivers::Constants, Some(separator), Some(receiver)) = (
            self.conventions.bare_receivers,
            self.conventions.owner_chain_separator,
            call.receiver,
        ) else {
            return true;
        };
        let caller = self.graph.sym(call.caller_at());
        if caller.owner == Some(receiver) || caller.qualified_name == receiver {
            return true;
        }
        let strings = &self.graph.strings;
        let written = strings.resolve(receiver);
        if !ends_in_a_constant(written, separator) {
            return true;
        }
        let on_own_class = self.is_on_the_caller_s_class(call);
        if on_own_class && self.is_instance_member(at) {
            return false;
        }
        let (written, from) = written.strip_prefix(separator).map_or_else(
            || (written, strings.resolve(caller.qualified_name)),
            |rooted| (rooted, ""),
        );
        let spells =
            |qualified: Str| spelled_from(from, (written, separator), strings.resolve(qualified));
        let owner = self.graph.sym(at).owner;
        if owner.is_some_and(spells) {
            return true;
        }
        let symbols = std::iter::successors(Some(written), |tail| {
            tail.split_once(separator).map(|(_, tail)| tail)
        })
        .filter_map(|tail| strings.get(tail))
        .flat_map(|tail| self.named(tail).iter().copied());
        let mut classes = (symbols.clone())
            .map(|at| self.graph.sym(at))
            .filter(|symbol| {
                (derives(symbol.kind) || symbol.kind == SymbolKind::Namespace)
                    && spells(symbol.qualified_name)
            })
            .peekable();
        if classes.peek().is_none() {
            return symbols.clone().any(|at| {
                let symbol = self.graph.sym(at);
                is_value(symbol.kind)
                    && (self.graph.file(at.file).value_types(at.index).is_none()
                        || spells(symbol.qualified_name))
            });
        }
        (on_own_class && !self.is_of_the_kind(true, at))
            || owner.is_some_and(|owner| {
                classes.any(|class| self.owner_derives_from(class.qualified_name, owner))
            })
    }

    /// Whether `call`'s receiver is the class that owns the caller, written from the root, as an extraction records a
    /// class it read from the caller and not from a spelling.
    pub(crate) fn is_on_the_caller_s_class(&self, call: &Call) -> bool {
        let owner = self.graph.sym(call.caller_at()).owner;
        call.on_a_class
            && (call.receiver)
                .and_then(|receiver| self.rooted_constant(receiver))
                .zip(owner)
                .is_some_and(|(rooted, owner)| rooted == self.graph.strings.resolve(owner))
    }

    /// Whether `at` is a member of a class that its instances answer, and no class's own method.
    fn is_instance_member(&self, at: SymbolAt) -> bool {
        !self.is_of_the_kind(true, at)
            && self.graph.sym(at).owner.is_some_and(|owner| {
                self.qualified_as(owner)
                    .any(|ty| derives(self.graph.sym(ty).kind))
            })
    }

    // constraint: a class of the spelling answers ahead of a value, as a write can stand in a branch that runs only
    // where the class is missing: `Gem::URI = Bundler::URI` in a `rescue LoadError` beside a `module Gem::URI`
    /// What the constant that `call`'s receiver names holds.
    fn constant_value(&self, call: &Call) -> ConstantValue<'_> {
        let (BareReceivers::Constants, Some(separator), Some(receiver)) = (
            self.conventions.bare_receivers,
            self.conventions.owner_chain_separator,
            call.receiver,
        ) else {
            return ConstantValue::NoValueType;
        };
        let strings = &self.graph.strings;
        let spelled = strings.resolve(receiver);
        if !ends_in_a_constant(spelled, separator) {
            return ConstantValue::NoValueType;
        }
        let (written, from) = spelled.strip_prefix(separator).map_or_else(
            || {
                let caller = self.graph.sym(call.caller_at());
                (spelled, strings.resolve(caller.qualified_name))
            },
            |rooted| (rooted, ""),
        );
        let spelled = std::iter::successors(Some(written), |tail| {
            tail.split_once(separator).map(|(_, tail)| tail)
        })
        .filter_map(|tail| strings.get(tail))
        .flat_map(|tail| self.named(tail).iter().copied())
        .filter(|&at| {
            spelled_from(
                from,
                (written, separator),
                strings.resolve(self.graph.sym(at).qualified_name),
            )
        });
        if spelled.clone().any(|at| {
            let kind = self.graph.sym(at).kind;
            derives(kind) || kind == SymbolKind::Namespace
        }) {
            return ConstantValue::NoValueType;
        }
        let mut held = spelled
            .filter(|&at| is_value(self.graph.sym(at).kind))
            .map(|at| self.graph.file(at.file).value_types(at.index));
        let Some(first) = held.next() else {
            return ConstantValue::NoValueType;
        };
        if !held.all(|types| types == first) {
            return ConstantValue::Untold;
        }
        let Some(class) = first.map(|types| strings.resolve(types)) else {
            return ConstantValue::NoValueType;
        };
        if class.is_empty() {
            return ConstantValue::Untold;
        }
        self.graph
            .class_named(class)
            .map_or(ConstantValue::Holds(class, false), |class| {
                ConstantValue::Holds(class, true)
            })
    }

    /// Where capitalised receivers name constants, the owner that a receiver written from the root spells, `Shape` for
    /// `::Shape`; `None` for any other receiver.
    pub(crate) fn rooted_constant(&self, receiver: Str) -> Option<&str> {
        let separator = self.conventions.owner_chain_separator?;
        if self.conventions.bare_receivers != BareReceivers::Constants {
            return None;
        }
        self.graph.strings.resolve(receiver).strip_prefix(separator)
    }

    fn resolve_by_tiers(&self, call: &Call) -> Result<Bound, Miss> {
        let caller = call.caller_at();

        if let Some(at) = call.local {
            let tier = if at.file == call.file {
                confidence::SAME_FILE
            } else {
                confidence::RECEIVER
            };
            return self
                .is_call_target(self.graph.sym(at))
                .then_some((at, tier))
                .ok_or(Miss::NotAdmitted);
        }

        if let Some(bound) = self.through_binding(call) {
            return bound;
        }
        if self.reads_type_paths()
            && let Some(receiver) = call.receiver
            && !self.graph.strings.resolve(receiver).is_empty()
        {
            // constraint: a call through an embedded field calls the field's value, and the type that answers for the
            // field is no callee
            let binds = |symbol: &StoredSymbol| {
                !symbol.kind.is_type()
                    && self.is_call_target(symbol)
                    && self.is_of_the_call_s_form(call, symbol.kind)
                    && Self::takes_the_arguments(call, symbol)
            };
            return match self.typed_member((caller, receiver), call.callee, binds) {
                Typed::Member(at) => Ok((at, confidence::RECEIVER)),
                Typed::Imported => Err(Miss::ExternalImport),
                Typed::Undeclared => Err(Miss::Undefined),
                Typed::NoMethod => Err(Miss::NotAdmitted),
                Typed::PastBound => Err(Miss::PastBound),
                Typed::Unknown => self.resolve_by_name(&Call {
                    receiver: self.empty_receiver.or(call.receiver),
                    ..*call
                }),
            };
        }

        if let Some(at) = self.written_inside_an_enclosing_scope(call) {
            return Ok((at, confidence::QUALIFIED_TAIL));
        }
        if self
            .graph
            .strings
            .resolve(call.callee)
            .contains(QUALIFIER_SEPARATORS)
            && let Some(at) = self.qualified(call.callee)
        {
            let symbol = self.graph.sym(at);
            if self.is_candidate(call, at) && symbol.qualified_name != symbol.name {
                return Ok((at, confidence::QUALIFIED));
            }
        }
        if let Some((scope, tail)) = self.trailing_pair(call.callee)
            && let Some(at) = self.qualified(tail)
            && self.is_candidate(call, at)
        {
            let at = self.member_of_the_written_class(call, (scope, tail), at);
            return Ok((at, confidence::QUALIFIED_TAIL));
        }
        if let Some(at) = self.nested_in_written_owner(call) {
            return Ok((at, confidence::QUALIFIED_TAIL));
        }
        if let Some(bound) = self.member_of_the_caller_s_base(call) {
            return bound;
        }

        self.resolve_by_name(call)
    }

    // constraint: a receiver that spells a chain of calls, `T::m()::n()?`, names the owner its last call returns; one
    // that reaches no type takes the value after its `|`, else the empty receiver, which the graph interned with it
    fn with_returned_receiver(&self, call: Call) -> Call {
        let strings = &self.graph.strings;
        let reads_fields = self.conventions.receivers == Receivers::InheritedFields;
        let reads_bodies = self.reads_pasted_bodies();
        if self.conventions.receivers != Receivers::ReturnPaths && !reads_fields && !reads_bodies {
            return call;
        }
        let spelled = call
            .receiver
            .map_or("", |receiver| strings.resolve(receiver));
        if reads_bodies {
            let Some(bodies) = spelled.strip_prefix('+') else {
                return call;
            };
            return Call {
                receiver: None,
                local: self.member_of_a_pasted_body(&call, bodies),
                ..call
            };
        }
        if reads_fields {
            let Some(field) = spelled.strip_prefix("->") else {
                return call;
            };
            return Call {
                receiver: self
                    .class_of_an_inherited_field(&call, field)
                    .or(self.empty_receiver),
                ..call
            };
        }
        let Some((head, rest)) = spelled.split_once("()") else {
            return call;
        };
        let called = head.rsplit_once("::").unwrap_or(("", head));
        let (rest, value) = rest.split_once('|').unwrap_or((rest, ""));
        Call {
            receiver: self
                .returned_owner(&call, called, rest)
                .or_else(|| strings.get(value)),
            ..call
        }
    }

    // constraint: each method of the path is found as the owner tier finds a call's, a path's first call written on no
    // owner as a bare call binds, and its value type gives the next owner, as a type such a call constructs does; a `?`
    // after a call takes what an `Option`, a `Result` or a `Promise` holds, a value type marked `?` spells only that, and
    // the two meet or the path reaches no type
    fn returned_owner(
        &self,
        call: &Call,
        (mut owner, mut method): (&str, &str),
        mut rest: &str,
    ) -> Option<Str> {
        let strings = &self.graph.strings;
        loop {
            let unwrapped = rest.strip_prefix('?');
            rest = unwrapped.unwrap_or(rest);
            let callee = strings.get(method)?;
            let at = if owner.is_empty() {
                let step = Call {
                    callee,
                    receiver: None,
                    ..*call
                };
                self.resolve_by_tiers(&step).ok()?.0
            } else {
                let step = Call {
                    callee,
                    receiver: Some(strings.get(owner)?),
                    ..*call
                };
                let candidates = self.candidates(&step);
                self.member_of_the_receiver_s_owner(&step, &candidates)?
            };
            let symbol = self.graph.sym(at);
            let types = if owner.is_empty() && symbol.kind.is_type() {
                symbol.qualified_name
            } else {
                self.graph.file(at.file).value_types(at.index)?
            };
            let returned = strings.resolve(types);
            let returned = match (returned.strip_prefix('?'), unwrapped) {
                (Some(held), Some(_)) => held,
                (None, None) => returned,
                _ => return None,
            };
            owner = returned;
            if rest.is_empty() {
                return strings.get(owner);
            }
            (method, rest) = rest.strip_prefix("::")?.split_once("()")?;
        }
    }

    // constraint: a qualified name written inside a namespace is looked up from the scopes around the call outward, so
    // `time_internal::f` called inside `absl` names `absl::time_internal::f`; the caller's qualified name carries
    // those scopes, and the innermost one that names a definition the caller's file sees wins, ahead of the global
    // name the call spells, the outermost scope
    fn written_inside_an_enclosing_scope(&self, call: &Call) -> Option<SymbolAt> {
        let separator = self.conventions.scope_separator?;
        let strings = &self.graph.strings;
        let written = strings.resolve(call.callee);
        if call.written_from_root || !written.contains(separator) {
            return None;
        }
        let caller = call.caller_at();
        let mut scope = strings.resolve(self.graph.sym(caller).qualified_name);
        let mut name = String::new();
        while let Some((enclosing, _)) = scope.rsplit_once(separator) {
            name.clear();
            name.push_str(enclosing);
            name.push_str(separator);
            name.push_str(written);
            let seen = strings.get(&name).and_then(|qualified| {
                self.qualified_as(qualified)
                    .filter(|&at| self.is_candidate(call, at) && self.sees(call.file, at))
                    .min_by_key(|&at| self.graph.order_of(at))
            });
            if seen.is_some() {
                return seen;
            }
            scope = enclosing;
        }
        None
    }

    // constraint: code outside the graph can define a name the graph holds, so the one definition of a name that takes
    // a call's arguments is no likelier its callee, by the name alone, than the others were before the arguments ruled
    // them out: a native overload takes what no script function of its name does
    fn arguments_turn_one_away(&self, call: &Call, named: &[SymbolAt]) -> bool {
        self.overloads_can_lie_outside(call)
            && named.iter().any(|&at| {
                let symbol = self.graph.sym(at);
                self.is_call_target(symbol) && !Self::takes_the_arguments(call, symbol)
            })
    }

    // constraint: a DSHL script defines every macro it calls, so the macro left of those a call's arguments turn away is
    // the macro called, and not a guess among overloads the graph does not hold
    fn overloads_can_lie_outside(&self, call: &Call) -> bool {
        call.arguments != 0 && self.conventions.include_reach != IncludeReach::Script
    }

    pub(crate) fn takes_too_few_arguments(&self, call: &Call, named: &[SymbolAt]) -> bool {
        self.overloads_can_lie_outside(call)
            && named.iter().any(|&at| {
                let symbol = self.graph.sym(at);
                self.is_call_target(symbol) && Self::passes_too_many(call, symbol)
            })
    }

    pub(crate) fn candidates(&self, call: &Call) -> CandidateSet<'g> {
        let named = self.named(call.callee);
        let scoped = || {
            let gate = self.gate(call.file);
            let mut turned_away = self.arguments_turn_one_away(call, named);
            let targets = self
                .call_targets(call, named.iter().copied())
                .filter(|&at| {
                    let admitted = gate
                        .as_ref()
                        .is_none_or(|gate| gate.admits_at(self.graph, at));
                    turned_away |= !admitted;
                    admitted
                })
                .collect();
            CandidateSet::Own(Candidates::scanned(named, targets, turned_away))
        };
        if named.len() < INDEXED_FROM {
            return scoped();
        }
        let shared = self.shared_candidates(call, named);
        let caller = call.caller_at();
        let scope = self.holder(caller).unwrap_or(caller);
        if shared
            .locals
            .iter()
            .any(|&local| local.file == call.file && self.is_candidate_in((scope, call), local))
        {
            return scoped();
        }
        CandidateSet::Shared(shared)
    }

    // constraint: the files of one package are built with the same packages, so the candidates a caller's package
    // reaches are shared by its files, and share the index of every candidate of the name
    fn shared_candidates(&self, call: &Call, named: &'g [SymbolAt]) -> Rc<Candidates<'g>> {
        let every = derived(&self.candidates, (call.callee, call.form(), None), || {
            let mut turned_away = false;
            let overloads_can_lie_outside = self.overloads_can_lie_outside(call);
            let (locals, targets): (Vec<SymbolAt>, Vec<SymbolAt>) = named
                .iter()
                .copied()
                .filter(|&at| {
                    let symbol = self.graph.sym(at);
                    let target = self.is_call_target(symbol);
                    let takes = Self::takes_the_arguments(call, symbol);
                    turned_away |= overloads_can_lie_outside && target && !takes;
                    target
                        && takes
                        && self.is_of_the_call_s_form(call, symbol.kind)
                        && !self.names_a_type(call, symbol)
                        && self.binds_if_made(symbol.kind, symbol.flags)
                })
                .partition(|&at| self.holder(at).is_some());
            let mut every = Candidates::indexed(self.graph, named, targets, locals);
            every.turned_away = turned_away;
            Rc::new(every)
        });
        let Some(gate) = self.gate(call.file) else {
            return every;
        };
        let key = (call.callee, call.form(), Some(gate.package));
        derived(&self.candidates, key, || {
            let targets = (every.targets.iter().copied())
                .filter(|&at| gate.admits_at(self.graph, at))
                .collect();
            Rc::new(every.gated(targets, gate))
        })
    }

    /// The packages a call in `file` binds a definition of by its name alone, where its resolver read the packages
    /// the file's package is built with; `None` admits every package.
    pub(crate) fn gate(&self, file: u32) -> Option<Gate<'g>> {
        let package = self.graph.file(file).package?;
        Some(Gate {
            package,
            reached: self.graph.built_with(package)?,
        })
    }

    fn resolve_by_name(&self, call: &Call) -> Result<Bound, Miss> {
        let candidates = self.candidates(call);
        let on_a_value;
        let call = match self.member_of_a_reached_owner(call, &candidates) {
            OwnerMember::Member(at) => return Ok((at, confidence::RECEIVER)),
            OwnerMember::Ambiguous => return Err(Miss::Ambiguous),
            OwnerMember::Elsewhere => {
                on_a_value = Call {
                    receiver: self.empty_receiver.or(call.receiver),
                    ..*call
                };
                &on_a_value
            }
            OwnerMember::NotDeclared => call,
        };
        if let Some(bound) = self.through_the_receiver_s_owner(call, &candidates) {
            return bound;
        }
        let reach = self.reach(call, &candidates);
        let reach = reach.as_ref();
        let path = (call, self.module_path(call));

        if self.names_a_class_outside_the_graph(call) {
            return Err(self.unadmitted(call));
        }
        if let Some(at) =
            self.same_file_target(path, candidates.in_file(self.graph, call.file), reach)
        {
            return Ok((at, confidence::SAME_FILE));
        }
        if call.receiver.is_none() && self.imports_from_outside(call) {
            return Err(Miss::ExternalImport);
        }
        if call.receiver.is_none()
            && let Some(at) = self.dot_imported(&candidates, call.file, Pick::FreeTarget(call))
        {
            return Ok((at, confidence::IMPORT));
        }
        if call.receiver.is_none()
            && let Some(bound) = self.glob_imported(call)
        {
            return Ok(bound);
        }
        if self.conventions.directory_packages
            && let Some(at) = self.scoped_target(
                call,
                self.in_package_of(&candidates, call.caller_at())
                    .into_iter(),
                reach,
            )
        {
            return Ok((at, confidence::PACKAGE));
        }
        // constraint: where a package is a directory, a bare call names its own package or a dot import, which the
        // tiers above read, and only a qualifier reaches another package
        if self.conventions.directory_packages && call.receiver.is_none() {
            return Err(self.unadmitted(call));
        }
        if let Some(at) = self
            .included(call, &candidates, reach)
            .filter(|&at| self.path_names(path, at))
        {
            let tier = if self.another_include_offers_a_type(call, at) {
                confidence::FIRST_PARTY
            } else {
                confidence::INCLUDE
            };
            return Ok((at, self.library_guess_or(call, at, tier)));
        }
        if !call.names_a_runtime_method
            && let Some(at) = self.re_exported(call)
        {
            return Ok((at, confidence::RE_EXPORT));
        }
        self.resolve_outside_scope(call, &candidates, reach)
            .and_then(|bound| {
                if self.path_names(path, bound.0) {
                    Ok(bound)
                } else {
                    Err(Miss::NotAdmitted)
                }
            })
    }

    // constraint: a Rust macro is in scope inside its own crate, by text order, and elsewhere only through a path or a
    // `use`, which the tiers above read, so a macro invocation binds by its name alone only a macro of the caller's
    // package, the files the package of the caller's file names, or of no package where that file names none
    fn in_the_caller_s_package<'c>(
        &self,
        call: &Call,
        candidates: &Candidates<'c>,
    ) -> Candidates<'c> {
        let package = self.graph.file(call.file).package;
        let in_the_package = (candidates.targets.iter().copied())
            .filter(|&at| self.graph.file(at.file).package == package)
            .collect();
        Candidates::scanned(candidates.named, in_the_package, false)
    }

    fn resolve_outside_scope(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
        reach: Option<&Reach>,
    ) -> Result<Bound, Miss> {
        let in_the_package;
        let candidates = if call.binds_apart {
            in_the_package = self.in_the_caller_s_package(call, candidates);
            &in_the_package
        } else {
            candidates
        };
        let targets = candidates.targets.iter().copied();
        let shape = self.shape(call, &targets, reach);
        if !targets.clone().any(|at| self.admits(&shape, at)) {
            return Err(self.unadmitted(call));
        }
        if let Some(bound) = self.member_of_the_caller_s_owner(call, candidates) {
            return Ok(bound);
        }
        // constraint: a name that packages the caller's is not built with also define is no project's own, so its one
        // callable within reach is no likelier the callee than its namesakes are, as `Ok` or `finish` in a registry
        let one_callable = || {
            self.one_callable(&shape, targets.clone())
                .filter(|_| !candidates.turned_away)
        };
        // constraint: a runtime method's name says nothing of which type the value is, so a member of the name binds
        // only where scope reaches it, or as the name's one callable when the receiver spells its owner
        if call.names_a_runtime_method {
            return one_callable()
                .filter(|&at| {
                    self.receiver_spells_owner(call, at) && self.can_bind_by_name(call, at)
                })
                .map(|at| (at, confidence::UNIQUE))
                .ok_or(Miss::NotAdmitted);
        }
        match one_callable().filter(|_| self.bodies_values(call, candidates).next().is_none()) {
            Some(at)
                if self.can_bind_by_name(call, at) && self.is_pasted_where_called(call, at) =>
            {
                Ok((at, self.library_guess_or(call, at, confidence::UNIQUE)))
            }
            Some(_) => Err(Miss::NotAdmitted),
            None => self
                .one_type_outside_tests(call, &shape, targets)
                .filter(|&at| self.is_in_the_caller_s_scope(call, at))
                .map(|at| (at, confidence::UNIQUE))
                .or_else(|| {
                    self.resolve_by_proximity(call, candidates, reach)
                        .filter(|&(at, _)| self.can_bind_by_name(call, at))
                })
                .ok_or(Miss::Ambiguous),
        }
    }

    // constraint: a variable or a constant a body declares is no candidate of a call outside that body, yet where
    // values are call targets it shows that a call of its name may call a value the graph does not follow, so it keeps
    // a callable from being the one of its name wherever it would count as a candidate; a call apart or a method call
    // calls no value
    pub(crate) fn bodies_values<'c>(
        &'c self,
        call: &Call,
        candidates: &'c Candidates<'_>,
    ) -> impl Iterator<Item = SymbolAt> + 'c {
        let indexed = candidates.holds_locals_apart();
        let held_apart =
            if !self.conventions.values_are_call_targets || call.binds_apart || call.calls_a_method
            {
                &[][..]
            } else if indexed {
                &candidates.locals[..]
            } else {
                candidates.named
            };
        held_apart.iter().copied().filter(move |&at| {
            let symbol = self.graph.sym(at);
            symbol.owner.is_none()
                && is_value(symbol.kind)
                && (indexed || self.holder(at).is_some())
        })
    }

    // constraint: a name an import of code outside the repository binds holds that code, so a bare call of it that the
    // caller's own file does not answer binds nothing the graph holds, as `useState()` with `useState` from `react`;
    // the file answers it where a definition nested in the caller shadows the import, or where a test hook such as
    // `beforeEach` is named for itself; where a package is a directory, an import binds the package and never a name a
    // bare call spells
    fn imports_from_outside(&self, call: &Call) -> bool {
        !self.conventions.directory_packages
            && !self.graph.file(call.file).bindings.is_empty()
            && self.binding(call.file, call.callee) == Some(None)
    }

    /// Whether the call's receiver is a bare name that the language writes only for a class, or a class's name written
    /// from the root, and no type the graph holds is named.
    fn names_a_class_outside_the_graph(&self, call: &Call) -> bool {
        let (BareReceivers::Classes(keywords), Some(receiver)) =
            (self.conventions.bare_receivers, call.receiver)
        else {
            return false;
        };
        let written = self.graph.strings.resolve(receiver);
        !written.is_empty()
            && written.bytes().all(|byte| {
                !byte.is_ascii() || byte.is_ascii_alphanumeric() || b"_\\".contains(&byte)
            })
            && !keywords
                .iter()
                .any(|keyword| keyword.eq_ignore_ascii_case(written))
            && !self
                .named(receiver)
                .iter()
                .any(|&at| self.graph.sym(at).kind.is_type())
            && self.class_from_the_root(call, receiver).is_none()
    }

    // constraint: a type the graph does not hold, from the standard library, the prelude or a crate outside the tree,
    // has no member in the graph, and a module the caller includes is no scope its name is looked up in, so a call
    // through it that no member of the owner binds binds nothing; nor does a call through an owner whose members of
    // the name lie only in packages the caller's is not built with, as `Vec::new()` beside a crate's own `Vec`; nor
    // does a call a field answers, since the name tiers would bind the member it hides or a namesake
    fn through_the_receiver_s_owner(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> Option<Result<Bound, Miss>> {
        if let Some(at) = self.member_of_the_receiver_s_owner(call, candidates) {
            return Some(if self.graph.sym(at).kind == SymbolKind::Field {
                Err(Miss::NotAdmitted)
            } else {
                Ok((at, confidence::RECEIVER))
            });
        }
        (self.names_a_type_outside_the_graph(call)
            || self.owned_out_of_reach(call, candidates)
            || self.inherits_from_outside_the_graph(call, candidates)
            || self.constructs_a_class_written_from_the_root(call))
        .then(|| Err(self.unadmitted(call)))
    }

    // constraint: a construction runs the constructor of its class's own table, and nothing where the table holds none,
    // so another class's constructor never answers it
    fn constructs_a_class_written_from_the_root(&self, call: &Call) -> bool {
        self.conventions.constructor == Some(self.graph.strings.resolve(call.callee))
            && self
                .named_receiver(call)
                .is_some_and(|receiver| self.class_from_the_root(call, receiver).is_some())
    }

    /// Whether a call that bound nothing runs the constructor of a construction of a class the graph does not hold,
    /// which the file's call just before it names: the class's last name on the class written from the root. The
    /// class's call stands for the site, so the counts of the calls that bound nothing leave this one out, and a
    /// constructor called by hand stays among them.
    pub(crate) fn constructs_outside_the_graph(&self, site: Site, call: &Call) -> bool {
        let Entry::Call(index) = site.entry() else {
            return false;
        };
        let strings = &self.graph.strings;
        self.conventions.constructor == Some(strings.resolve(call.callee))
            && self.names_a_class_outside_the_graph(call)
            && index.checked_sub(1).is_some_and(|before| {
                let class = self.graph.file(call.file).calls[before as usize];
                class.receiver.is_some_and(|receiver| {
                    let written = strings.resolve(receiver);
                    written.starts_with(QUALIFIER_SEPARATORS)
                        && written.rsplit(QUALIFIER_SEPARATORS).next()
                            == Some(strings.resolve(class.callee))
                })
            })
    }

    fn owned_out_of_reach(&self, call: &Call, candidates: &Candidates<'_>) -> bool {
        candidates.turned_away
            && call.receiver.is_some_and(|receiver| {
                (self.named(call.callee).iter())
                    .any(|&at| self.graph.sym(at).owner == Some(receiver))
            })
    }

    /// Whether the call goes through a type path, a receiver whose last segment is written capitalised where only a
    /// module's path reaches a free function, and the graph holds neither a type nor a value of that name that the
    /// caller's package can reach. A value a macro writes counts only under a method call: rust-analyzer's `sym::Box`
    /// is no receiver of `Box::new()`.
    fn names_a_type_outside_the_graph(&self, call: &Call) -> bool {
        let (MemberCalls::ModulePaths, Some(separator), Some(receiver)) = (
            self.conventions.member_calls,
            self.conventions.owner_chain_separator,
            call.receiver,
        ) else {
            return false;
        };
        let written = self.graph.strings.resolve(receiver);
        let last = written.rsplit(separator).next().unwrap_or(written);
        if !last.starts_with(|first: char| first.is_ascii_uppercase())
            || !last
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return false;
        }
        let gate = self.gate(call.file);
        self.graph.strings.get(last).is_none_or(|last| {
            !self.named(last).iter().any(|&at| {
                let symbol = self.graph.sym(at);
                (symbol.kind.is_type()
                    || (is_value(symbol.kind) && (call.calls_a_method || !symbol.flags.is_made())))
                    && gate
                        .as_ref()
                        .is_none_or(|gate| gate.admits_at(self.graph, at))
            })
        })
    }

    // constraint: a method call on a value the graph cannot type, a name after its `.` or an expression's empty
    // receiver, holds no type the graph can tell from one outside the tree
    fn names_a_library_method_on_a_value(&self, call: &Call, at: SymbolAt) -> bool {
        let strings = &self.graph.strings;
        call.calls_a_method
            && call.receiver.is_some_and(|receiver| {
                let written = strings.resolve(receiver);
                written.is_empty() || written.starts_with('.')
            })
            && (self.conventions.library_methods)
                .binary_search(&strings.resolve(call.callee))
                .is_ok()
            && !self.receiver_spells_owner(call, at)
    }

    /// Whether `call` is an attribute write, by [`Conventions::setter_suffix`], on a receiver that names no type: an
    /// expression's empty receiver or a name in lower case.
    fn writes_on_a_value(&self, call: &Call) -> bool {
        let strings = &self.graph.strings;
        let is_setter = self.conventions.setter_suffix.is_some_and(|suffix| {
            let callee = strings.resolve(call.callee);
            callee.strip_suffix(suffix).is_some_and(|attribute| {
                attribute.ends_with(|last: char| last.is_alphanumeric() || last == '_')
            })
        });
        is_setter
            && call.receiver.is_some_and(|receiver| {
                let written = strings.resolve(receiver);
                written.is_empty()
                    || written.starts_with(|first: char| first.is_lowercase() || first == '_')
            })
    }

    /// The confidence of a guess where `call` names a library method on a value and its receiver does not spell
    /// the owner of `at`, and `tier` otherwise.
    fn library_guess_or(&self, call: &Call, at: SymbolAt, tier: u8) -> u8 {
        if self.names_a_library_method_on_a_value(call, at) {
            confidence::FIRST_PARTY
        } else {
            tier
        }
    }

    // constraint: test code is built apart from the code it tests, so of the definitions a call reaches by name alone,
    // a test one binds a call from test code alone; it still counts among them, so a name test code shares, often a
    // stub of a builtin or of a library outside the graph, is no surer to mean another definition of it
    fn can_bind_by_name(&self, call: &Call, at: SymbolAt) -> bool {
        (!self.graph.flags(at).is_test() || self.graph.flags(call.caller_at()).is_test())
            && self.is_in_the_caller_s_scope(call, at)
    }

    // constraint: where only includes put a definition in a file's scope, a bare call of a name its file does not reach
    // means a namesake outside the graph, a native overload most often, and no definition the tree happens to hold
    fn is_in_the_caller_s_scope(&self, call: &Call, at: SymbolAt) -> bool {
        let Some(everywhere) = self.conventions.module_seen_everywhere else {
            return true;
        };
        call.receiver.is_some()
            || self.is_file_of_module(at.file, everywhere)
            || self.reaches(call.file, at.file)
    }

    fn is_file_of_module(&self, file: u32, module: &str) -> bool {
        let package = self.graph.file(file).package;
        package.is_some_and(|package| self.graph.strings.resolve(package) == module)
    }

    // constraint: where includes paste text, a call sees only what its translation unit pastes in, so the one callable
    // of a name binds where the caller's file, or a file that includes it directly, reaches that callable or a
    // declaration of it; elsewhere the name is most often one the platform or the toolchain declares, an SSE intrinsic
    // or a C library function, of which the tree holds a shim
    fn is_pasted_where_called(&self, call: &Call, at: SymbolAt) -> bool {
        if self.conventions.include_reach != IncludeReach::Transitive
            || self.reaches(call.file, at.file)
        {
            return true;
        }
        let declarations = self.declarations(at);
        let reaches = |file: u32| {
            self.reaches(file, at.file)
                || declarations
                    .iter()
                    .any(|declaration| self.reaches(file, declaration.file))
        };
        reaches(call.file) || self.graph.includers_of(call.file).any(reaches)
    }

    // constraint: a type test code defines stands in for the type outside test code that it shares the name of, or
    // holds a test's fixture, which code outside test code never constructs
    fn one_type_outside_tests(
        &self,
        call: &Call,
        shape: &Shape,
        targets: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        if self.graph.flags(call.caller_at()).is_test() {
            return None;
        }
        self.one_callable(shape, targets.filter(|&at| !self.graph.flags(at).is_test()))
            .filter(|&at| self.graph.sym(at).kind.is_type() || self.constructor_types(at).is_some())
    }

    fn through_binding(&self, call: &Call) -> Option<Result<Bound, Miss>> {
        let Some(receiver) = call.receiver else {
            return self.imported_module_export(call).map(Ok);
        };
        let Some((binding, source)) = self.binding_and_source(call.file, receiver) else {
            if let Some(at) =
                self.required_definition(call.file, (receiver, call.callee), Pick::FreeTarget(call))
            {
                return Some(Ok((at, confidence::IMPORT)));
            }
            return self.through_declared_module(call, receiver);
        };
        let Some(target) = binding else {
            // constraint: a Go file includes each package it imports, so a package named as an external import is not
            // that import
            let required = (self.reads_modules())
                .then(|| self.required_module(call.file, receiver))
                .flatten();
            let module = required.or_else(|| self.declared_module(call.file, source));
            return Some(module.map_or(Err(Miss::ExternalImport), |module| {
                self.module_definition(call, module)
            }));
        };
        if let Some(module) = self.graph.slot_of(target) {
            return self.module_export(call, module).map(Ok);
        }
        Some(
            self.imported(
                &self.candidates(call),
                (call.file, Some(target)),
                Pick::FreeTarget(call),
            )
            .ok_or(Miss::NotExported),
        )
    }

    fn through_declared_module(&self, call: &Call, qualifier: Str) -> Option<Result<Bound, Miss>> {
        if let Some(module) = self.declared_module(call.file, qualifier) {
            return Some(self.module_definition(call, module));
        }
        self.names_no_symbol(call, qualifier)
            .then_some(Err(Miss::ExternalImport))
    }

    // constraint: daslang reads `mod::f` through `mod` and what it requires publicly, and a module whose definitions the
    // graph holds is a symbol, so a qualifier no symbol holds, whoever's `require` reached it, leaves the graph; a member
    // call's receiver is a value, which no symbol need hold
    fn names_no_symbol(&self, call: &Call, qualifier: Str) -> bool {
        self.reads_modules() && !call.calls_a_method && self.named(qualifier).is_empty()
    }

    fn module_definition(&self, call: &Call, module: u32) -> Result<Bound, Miss> {
        let defined = self.graph.in_file(self.named(call.callee), module);
        self.pick(Pick::FreeTarget(call), defined.iter().copied())
            .map(|at| (at, confidence::IMPORT))
            .ok_or(Miss::NotExported)
    }

    // constraint: a bare call names an import only where its file imports a module's file, which no Go import does, so
    // the calls of a file without one ask nothing of its bindings
    fn imported_module_export(&self, call: &Call) -> Option<Bound> {
        if self.graph.file(call.file).bindings.is_empty() {
            return None;
        }
        let bindings = self.bindings(call.file);
        if !bindings.modules {
            return None;
        }
        let &(_, target, source) = bindings
            .named
            .iter()
            .find(|&&(held, _, _)| held == call.callee)?;
        let module = self.graph.slot_of(target?)?;
        self.module_item(call, (module, source), &mut Vec::new())
    }

    fn module_export(&self, call: &Call, module: u32) -> Option<Bound> {
        self.module_item(call, (module, call.callee), &mut Vec::new())
    }

    // constraint: a module's file can hold what its module exports through a re-export, a macro or an inline module
    // its definitions do not show, so a name it does not define at its top level goes on to the module's `use`s and
    // globs, to a macro that a file below exports and then to the tiers below; a type shares its name with a function
    // only where the language keeps types apart from values, as Rust's `struct kevent` beside the `extern` function
    // `kevent`, so the call names the function; no path reaches a macro the file does not export, and a call from the
    // module or a module below it, the files its file includes, sees its other private definitions too; a test
    // definition is compiled into its crate's test build alone, where the crate's test code sees it through a path, so
    // test code that shares an include root with the file sees it from any module, and other code from none
    fn module_item(
        &self,
        call: &Call,
        (module, name): (u32, Str),
        visited: &mut Vec<(u32, Str)>,
    ) -> Option<Bound> {
        let defined = self.graph.in_file(self.named(name), module);
        let tests_seen = self.graph.flags(call.caller_at()).is_test();
        let in_one_crate = OnceCell::new();
        let in_one_crate =
            || *in_one_crate.get_or_init(|| self.share_an_include_root(call.file, module));
        let answers = self
            .call_targets(call, defined.iter().copied())
            .filter(|&at| {
                let symbol = self.graph.sym(at);
                self.declared_at_top_level(at)
                    && (!self.graph.flags(at).is_test() || (tests_seen && in_one_crate()))
                    && (symbol.exported() || symbol.kind != SymbolKind::Macro)
            });
        let Some(at) = answers.min_by_key(|&at| self.graph.sym(at).kind.is_type()) else {
            return self.used_by_module(call, (module, name), visited);
        };
        (self.graph.sym(at).exported() || self.is_within_module(call.file, module))
            .then_some((at, confidence::IMPORT))
    }

    // constraint: a Rust module answers a name it does not define through its `use` of the name, read whether written
    // `pub use` or not, since code that compiles reaches no private `use` from outside the module; else through its
    // globs, which its own names shadow, one answer among them; a macro invocation that neither answers goes on to a
    // macro a file below exports, since a `use` of a function brings no macro of its name; each module is read once
    // under each name, so a chain or a cycle of re-exports ends
    fn used_by_module(
        &self,
        call: &Call,
        (module, name): (u32, Str),
        visited: &mut Vec<(u32, Str)>,
    ) -> Option<Bound> {
        if visited.contains(&(module, name)) {
            return None;
        }
        visited.push((module, name));
        let reached = match self.binding_and_source(module, name) {
            Some((target, source)) => (target.and_then(|target| self.graph.slot_of(target)))
                .and_then(|target| self.module_item(call, (target, source), visited)),
            None => self.globbed_by_module(call, (module, name), visited),
        };
        reached.or_else(|| self.macro_exported_below(call, (module, name)))
    }

    fn globbed_by_module(
        &self,
        call: &Call,
        (module, name): (u32, Str),
        visited: &mut Vec<(u32, Str)>,
    ) -> Option<Bound> {
        let bindings = self.bindings(module);
        let mut unique: Option<Bound> = None;
        for &target in &bindings.dot_dirs {
            let Some(bound) = (self.graph.slot_of(target))
                .and_then(|target| self.module_item(call, (target, name), visited))
            else {
                continue;
            };
            if unique.is_some_and(|(at, _)| at != bound.0) {
                return None;
            }
            unique = Some(bound);
        }
        unique
    }

    fn is_within_module(&self, file: u32, module: u32) -> bool {
        file == module || self.reaches(module, file)
    }

    fn share_an_include_root(&self, file: u32, other: u32) -> bool {
        let roots = self.include_roots(other);
        self.include_roots(file)
            .iter()
            .any(|root| roots.binary_search(root).is_ok())
    }

    // constraint: a Rust crate is the files its root, which no file includes, includes directly or through others
    fn include_roots(&self, file: u32) -> Rc<[u32]> {
        derived(&self.roots, file, || self.walk_to_roots(file).into())
    }

    fn walk_to_roots(&self, file: u32) -> Vec<u32> {
        let mut visited: HashSet<u32, FixedState> = HashSet::default();
        visited.insert(file);
        let mut pending = vec![file];
        let mut roots = Vec::new();
        while let Some(current) = pending.pop() {
            let mut includers = self.graph.includers_of(current).peekable();
            if includers.peek().is_none() {
                roots.push(current);
            }
            pending.extend(includers.filter(|&includer| visited.insert(includer)));
        }
        roots.sort_unstable();
        roots
    }

    fn at_top_level(&self, at: SymbolAt) -> bool {
        self.declared_at_top_level(at) && !self.graph.flags(at).is_test()
    }

    fn declared_at_top_level(&self, at: SymbolAt) -> bool {
        let symbol = self.graph.sym(at);
        symbol.owner.is_none() && symbol.qualified_name == symbol.name && self.holder(at).is_none()
    }

    // constraint: Rust's `#[macro_export]` places a macro at its crate's root whatever module defines it, so a macro
    // invocation of a name the module answers in no other way binds the one exported macro of the name that a file it
    // includes, directly or through others, defines; two leave the call to the tiers below
    fn macro_exported_below(&self, call: &Call, (module, name): (u32, Str)) -> Option<Bound> {
        if !self.is_of_the_call_s_form(call, SymbolKind::Macro) {
            return None;
        }
        let closure = OnceCell::new();
        let mut exported = self.named(name).iter().copied().filter(|&at| {
            let symbol = self.graph.sym(at);
            symbol.kind == SymbolKind::Macro
                && symbol.exported()
                && at.file != module
                && self.at_top_level(at)
                && closure
                    .get_or_init(|| self.closure(module))
                    .binary_search(&at.file)
                    .is_ok()
        });
        let at = exported.next()?;
        exported
            .next()
            .is_none()
            .then_some((at, confidence::IMPORT))
    }

    fn unadmitted(&self, call: &Call) -> Miss {
        if self.named(call.callee).is_empty() && self.qualified(call.callee).is_none() {
            Miss::Undefined
        } else {
            Miss::NotAdmitted
        }
    }

    fn imported(
        &self,
        candidates: &Candidates<'_>,
        (importer, target): (u32, Option<Str>),
        pick: Pick<'_>,
    ) -> Option<Bound> {
        let dir = target?;
        let declared = if self.is_external_test_of(importer, dir) {
            self.in_tested_package(candidates, importer)
        } else {
            self.declared_in(candidates, dir)
        };
        let at = self.pick(pick, declared.into_iter())?;
        self.graph
            .sym(at)
            .exported()
            .then_some((at, confidence::IMPORT))
    }

    // constraint: the go command compiles a package's test files into the package an external test package of the same
    // directory imports, so such an import sees what a test file of the package declares, most often `export_test.go`
    fn is_external_test_of(&self, importer: u32, dir: Str) -> bool {
        let file = self.graph.file(importer);
        self.conventions.directory_packages
            && file.dir == dir
            && file
                .symbols
                .first()
                .is_some_and(|symbol| symbol.flags.is_test())
    }

    fn in_tested_package(&self, candidates: &Candidates<'_>, importer: u32) -> Vec<SymbolAt> {
        let file = self.graph.file(importer);
        candidates
            .in_dir(self.graph, file.dir)
            .iter()
            .copied()
            .filter(|&at| {
                self.graph.file(at.file).package != file.package && self.holder(at).is_none()
            })
            .collect()
    }

    fn dot_imported(
        &self,
        candidates: &Candidates<'_>,
        file: u32,
        pick: Pick<'_>,
    ) -> Option<SymbolAt> {
        let mut unique = None;
        for &dir in &self.bindings(file).dot_dirs {
            let Some(at) = self
                .pick(pick, self.declared_in(candidates, dir).into_iter())
                .filter(|&at| self.graph.sym(at).exported())
            else {
                continue;
            };
            if unique.is_some() {
                return None;
            }
            unique = Some(at);
        }
        unique
    }

    // constraint: a dot import of a module's file, a Rust glob, binds what that file shows the call as a named import
    // of it does, below the caller's own file, which shadows a glob; two such imports that answer leave the call to
    // the tiers below
    fn glob_imported(&self, call: &Call) -> Option<Bound> {
        let mut unique = None;
        for &target in &self.bindings(call.file).dot_dirs {
            let Some(bound) =
                (self.graph.slot_of(target)).and_then(|module| self.module_export(call, module))
            else {
                continue;
            };
            if unique.is_some() {
                return None;
            }
            unique = Some(bound);
        }
        unique
    }

    // constraint: the name binds only where one target is unique among every directly included file, so a file that
    // offers several makes the tier abstain as two files that offer one each do
    fn included(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
        reach: Option<&Reach>,
    ) -> Option<SymbolAt> {
        let includes = self.includes(call.file);
        let defined_directly = includes
            .iter()
            .map(|&included| candidates.in_file(self.graph, included));
        match self.offered(call, reach, defined_directly) {
            Scoped::One(at) => Some(at),
            Scoped::Several => None,
            Scoped::Nothing => self.through_includes(call, candidates, reach),
        }
    }

    // constraint: past the direct includes a name binds only where few definitions share it and one file of those the
    // includes reach offers it, as a common name's one definition there is most often a namesake of code outside the
    // graph; a member call takes a member alone there, since a free function offered to it is a member read as free
    fn through_includes(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
        reach: Option<&Reach>,
    ) -> Option<SymbolAt> {
        if self.conventions.include_reach == IncludeReach::Direct {
            return None;
        }
        let named_receiver = call
            .receiver
            .is_some_and(|receiver| !self.graph.strings.resolve(receiver).is_empty());
        let named = candidates.named;
        if named_receiver || named.len() > THROUGH_INCLUDES_MAX_DEFINITIONS {
            return None;
        }
        let closure = self.closure(call.file);
        let defined_further_in = named
            .chunk_by(|left, right| left.file == right.file)
            .filter(|defined| {
                defined[0].file != call.file && closure.binary_search(&defined[0].file).is_ok()
            });
        match self.offered(call, reach, defined_further_in) {
            Scoped::One(at) if call.receiver.is_none() || self.graph.sym(at).owner.is_some() => {
                Some(at).filter(|&at| !self.game_keeps_another_copy(call, named, at))
            }
            Scoped::One(_) | Scoped::Nothing | Scoped::Several => None,
        }
    }

    // constraint: an include of a shared file resolves to the copy nearest that file, not to the copy of the game that
    // compiles it, so past the direct includes a reached macro is another game's where a copy sits nearer the call
    fn game_keeps_another_copy(&self, call: &Call, named: &[SymbolAt], reached: SymbolAt) -> bool {
        self.conventions.include_reach == IncludeReach::Script
            && self
                .nearest_copy(call.file, self.call_targets(call, named.iter().copied()))
                .is_some_and(|nearest| nearest != reached)
    }

    fn offered<'a>(
        &self,
        call: &Call,
        reach: Option<&Reach>,
        defined_per_file: impl Iterator<Item = &'a [SymbolAt]>,
    ) -> Scoped {
        let rule = self.conventions.included_runtime_members;
        let narrowed = |at: SymbolAt| match rule {
            IncludedRuntimeMembers::SpelledOrOwn => call.names_a_runtime_method,
            IncludedRuntimeMembers::SpelledOwnOrExported => {
                call.names_a_runtime_method
                    || (call.calls_a_method && !self.graph.sym(at).exported())
            }
            _ => false,
        };
        let kept = |at: SymbolAt| !narrowed(at) || self.spelled_or_own(call, at);
        let turns_away = rule == IncludedRuntimeMembers::SpelledOwnOrExported;
        let mut turned_away = false;
        let mut unique = Scoped::Nothing;
        for defined in defined_per_file.filter(|defined| !defined.is_empty()) {
            let offered = defined.iter().copied();
            match self.scoped(call, offered.filter(|&at| turns_away || kept(at)), reach) {
                Scoped::One(at) if !kept(at) => turned_away = true,
                Scoped::Nothing => {}
                Scoped::One(at) if unique == Scoped::Nothing => unique = Scoped::One(at),
                Scoped::One(_) | Scoped::Several => return Scoped::Several,
            }
        }
        match unique {
            Scoped::One(_) if turned_away => Scoped::Several,
            unique => unique,
        }
    }

    /// Whether `at` is no member, or a member whose owner the call's receiver spells, the caller's own owner is or
    /// derives from, or is an interface or a type alias.
    fn spelled_or_own(&self, call: &Call, at: SymbolAt) -> bool {
        let Some(owner) = self.graph.sym(at).owner else {
            return true;
        };
        self.receiver_spells_owner(call, at)
            || self
                .graph
                .sym(call.caller_at())
                .owner
                .is_some_and(|own| self.owner_derives_from(own, owner))
            || self.qualified(owner).is_some_and(|ty| {
                matches!(
                    self.graph.sym(ty).kind,
                    SymbolKind::Interface | SymbolKind::TypeAlias
                )
            })
    }

    // constraint: a daslang call picks its overload by argument types the graph does not read, and what a public
    // `require` brings overloads the names of every file, `empty` and `string` first, so it reaches no call
    fn re_exported(&self, call: &Call) -> Option<SymbolAt> {
        if self.conventions.value_uses == ValueUses::Expanded {
            return None;
        }
        let mut walk = ReExportWalk {
            cascade: self,
            call,
            visited: HashSet::default(),
        };
        self.includes(call.file)
            .iter()
            .find_map(|&included| walk.follow(included, call.callee))
    }

    /// The last two names a callee spells, `Widget::draw` of `b::Widget::draw`, with the scope written before the
    /// member, `b::Widget`.
    fn trailing_pair(&self, callee: Str) -> Option<(&'g str, Str)> {
        let separator = self.conventions.scope_separator?;
        let written = self.graph.strings.resolve(callee);
        let last = written.rfind(separator)?;
        let previous = written[..last].rfind(separator)?;
        let tail = self
            .graph
            .strings
            .get(&written[previous + separator.len()..])?;
        Some((&written[..last], tail))
    }

    // constraint: a member keeps its class's bare name in its qualified name, so namesake classes in two namespaces
    // share it, and the scope written before the member names the class C++ looks up from the scopes around the call
    // outward; the first in path order stands where that scope names none of them, and where the graph locates no
    // class of the first, as for an out-of-line definition no declaration links to, whose class may be the one named
    /// Of the members `tail` qualifies, the one of the class `scope` names, else `first`.
    fn member_of_the_written_class(
        &self,
        call: &Call,
        (scope, tail): (&str, Str),
        first: SymbolAt,
    ) -> SymbolAt {
        let caller = call.caller_at();
        let mut found = self
            .qualified_as(tail)
            .filter(|&at| self.is_candidate(call, at));
        let (Some(one), Some(two)) = (found.next(), found.next()) else {
            return first;
        };
        if self.class_of(first).is_none() {
            return first;
        }
        let mut tier: Vec<SymbolAt> = [one, two].into_iter().chain(found).collect();
        tier.sort_by_key(|&at| self.graph.order_of(at));
        let written = match self.conventions.scope_separator {
            Some(separator) if call.written_from_root => Cow::Owned(format!("{separator}{scope}")),
            _ => Cow::Borrowed(scope),
        };
        self.twin(caller, &written, &tier)
            .map_or(first, Twin::member)
    }

    // constraint: a member nested in a class can keep the class's whole qualified name, `ns::Widget::Kind`, which a
    // callee inside the namespace writes from the class alone, `Widget::Kind`
    fn nested_in_written_owner(&self, call: &Call) -> Option<SymbolAt> {
        let separator = self.conventions.scope_separator?;
        let strings = &self.graph.strings;
        let written = strings.resolve(call.callee);
        let (scope, name) = written.rsplit_once(separator)?;
        let owner = scope.rsplit(separator).next()?;
        let tail = &written[written.len() - owner.len() - separator.len() - name.len()..];
        let (owner, name) = (strings.get(owner)?, strings.get(name)?);
        self.named(name)
            .iter()
            .copied()
            .filter(|&at| {
                let symbol = self.graph.sym(at);
                symbol.owner == Some(owner)
                    && strings
                        .resolve(symbol.qualified_name)
                        .strip_suffix(tail)
                        .is_some_and(|head| head.ends_with(separator))
                    && self.is_candidate(call, at)
            })
            .min_by_key(|&at| (!self.sees(call.file, at), self.graph.order_of(at)))
    }

    // constraint: a use can name a value even where a call cannot target one, so every kind but a field and a variant
    // is a candidate, whatever `values_are_call_targets` says
    fn referenceable(&self, name: Str) -> Rc<Candidates<'g>> {
        derived(&self.referenceable, name, || {
            let named = self.named(name);
            let targets = named
                .iter()
                .copied()
                .filter(|&at| is_call_target_symbol(self.graph.sym(at)))
                .collect();
            Rc::new(Candidates::scanned(named, targets, false))
        })
    }

    // constraint: a member of a value binds through the type its path spells, or of the owner it names where receivers
    // name owners, and a path neither reaches binds nothing but in three cases: an empty path, a Go path the graph
    // cannot follow to a type, and a C++ path to a type the graph holds that declares no such member, nor does a base;
    // those bind the one field of the name their own file declares outside other bodies, or where the language says
    // so, its includes' then theirs, or nothing where the language leaves them unbound; the package, an import and the
    // family bind a namesake too often, where a test table or an anonymous struct holds the value
    fn member_use(&self, site: SymbolAt, name: Str, path: Option<Str>) -> Result<Bound, UseMiss> {
        if let Some(path) = path
            && self.reads_type_paths()
        {
            // constraint: a member taken without a call is a field or a method value, so every member kind binds but a
            // label
            match self.typed_member((site, path), name, |symbol| !symbol.flags.is_label()) {
                Typed::Member(at) => return Ok((at, confidence::RECEIVER)),
                Typed::Imported | Typed::Undeclared | Typed::NoMethod | Typed::PastBound => {
                    return Err(UseMiss::Elsewhere);
                }
                Typed::Unknown => {}
            }
        } else if let Some(owner) =
            path.filter(|&path| !self.graph.strings.resolve(path).is_empty())
            && let Some(found) = self.path_member(site, owner, name)
        {
            return found;
        }
        if self.conventions.untyped_members == UntypedMembers::Unbound {
            return Err(UseMiss::Ambiguous);
        }
        let body = self.holder(site).unwrap_or(site);
        let mut fields = self
            .graph
            .in_file(self.named(name), site.file)
            .iter()
            .copied()
            .filter(|&at| {
                self.graph.sym(at).kind == SymbolKind::Field
                    && self.holder(at).is_none_or(|holder| holder == body)
            });
        match (fields.next(), fields.next()) {
            (Some(field), None) => Ok((field, confidence::SAME_FILE)),
            (None, _) if self.conventions.untyped_members == UntypedMembers::IncludedField => {
                self.included_field(site.file, name)
            }
            (None, _) => Err(UseMiss::Ambiguous),
            (Some(_), Some(_)) => Err(UseMiss::AmbiguousField),
        }
    }

    pub(crate) fn resolve_use(&self, file: u32, index: u32) -> Result<Bound, UseMiss> {
        let bound = self.bound_use(file, index)?;
        self.built_with(file, bound).ok_or(UseMiss::Elsewhere)
    }

    fn bound_use(&self, file: u32, index: u32) -> Result<Bound, UseMiss> {
        let site = self.graph.file(file).uses[index as usize];
        let at = SymbolAt::new(file, site.site_of());
        match site.receiver() {
            UseReceiver::Bare => self.class_scope_use(at, site.name).unwrap_or_else(|| {
                self.value_use((at, site.line), site.name)
                    .ok_or(UseMiss::Elsewhere)
            }),
            UseReceiver::Qualifier(qualifier) => match self
                .binding(file, qualifier)
                .filter(|target| target.is_some() || !self.reads_modules())
            {
                Some(target) => {
                    // constraint: a qualifier bound to a module's file names what that module shows a call through it
                    if let Some(module) = target.and_then(|target| self.graph.slot_of(target)) {
                        return (self.module_export(&Call::bare(at, site.name), module))
                            .ok_or(UseMiss::Elsewhere);
                    }
                    let candidates = self.referenceable(site.name);
                    self.imported(&candidates, (file, target), Pick::PackageLevel)
                        .ok_or(UseMiss::Elsewhere)
                }
                // constraint: a walk that sees one file takes a sibling file's value for a qualifier, and Go lets no
                // package-level name share an import's, so a qualifier no import binds names a value
                None if self.reads_type_paths() => self.member_use(at, site.name, Some(qualifier)),
                // constraint: a script module is the file that declares it, so its own name as a qualifier names what
                // the bare name its file declares does
                None if self.graph.file(file).package == Some(qualifier) => {
                    (self.bare_use((file, site.line), site.name)).ok_or(UseMiss::Elsewhere)
                }
                None => self
                    .required_definition(file, (qualifier, site.name), Pick::PackageLevel)
                    .map(|at| (at, confidence::IMPORT))
                    .ok_or(UseMiss::Elsewhere),
            },
            UseReceiver::Own => self.own_member(at, site.name),
            UseReceiver::Path(path) => self.member_use(at, site.name, path),
        }
    }

    fn value_use(&self, (site, line): (SymbolAt, u32), name: Str) -> Option<Bound> {
        let bare = || self.bare_use((site.file, line), name);
        let imported = || bare().or_else(|| self.imported_value(site, name));
        match self.conventions.value_uses {
            ValueUses::Imported => imported(),
            ValueUses::ImportedThenReached => {
                let bound = imported();
                match bound {
                    Some((at, _)) if self.graph.sym(at).kind != SymbolKind::Enum => bound,
                    _ => self
                        .reached_variable(site.file, name)
                        .or(bound)
                        .or_else(|| self.scripted_value(site.file, name)),
                }
            }
            ValueUses::Expanded => imported().or_else(|| {
                let written = self.graph.strings.resolve(name);
                if self.conventions.native_names.contains(&written) {
                    return None;
                }
                self.implicit_value(name)
                    .or_else(|| self.expanded_value((site, line), name))
            }),
            ValueUses::Included => self.included_value(site.file, name),
            _ => bare().or_else(|| self.used_through_imports(site, name)),
        }
    }

    // constraint: a bare use names what a bare call of the name names through its file's imports of a module's file,
    // a named import before a glob, below the file's own definitions, which shadow both
    fn used_through_imports(&self, site: SymbolAt, name: Str) -> Option<Bound> {
        let call = Call::bare(site, name);
        self.imported_module_export(&call)
            .or_else(|| self.glob_imported(&call))
    }

    fn implicit_value(&self, name: Str) -> Option<Bound> {
        let module = self.conventions.module_seen_everywhere?;
        let is_implicit = |file: u32| self.is_file_of_module(file, module);
        self.one_declaring(self.named(name), is_implicit, |_| true)
            .map(|at| (at, confidence::INCLUDE))
    }

    // constraint: the graph expands no macro, and a macro's quoted code compiles in the file that expands it, which
    // requires the macro's file, so the files that include the use's file stand in for that file
    fn expanded_value(&self, (site, line): (SymbolAt, u32), name: Str) -> Option<Bound> {
        let one_bound = |expanding: &mut dyn Iterator<Item = u32>| {
            let mut reached = expanding.filter_map(|file| {
                let includer = SymbolAt::new(file, FILE_SCOPE.0);
                self.bare_use((file, line), name)
                    .or_else(|| self.imported_value(includer, name))
                    .map(|(at, _)| at)
            });
            let first = reached.next()?;
            reached
                .all(|at| at == first)
                .then_some((first, confidence::UNIQUE))
        };
        one_bound(&mut self.graph.includers_of(site.file))
            .or_else(|| one_bound(&mut self.includers_past_re_exports(site.file).into_iter()))
    }

    // constraint: a file that re-exports a macro's file hands the macro to every file that requires it, so those expand
    // the macro as well, however long the chain of re-exports
    fn includers_past_re_exports(&self, file: u32) -> HashSet<u32, FixedState> {
        let mut includers: HashSet<u32, FixedState> = HashSet::default();
        let mut passed: HashSet<u32, FixedState> = HashSet::default();
        let mut pending = vec![file];
        while let Some(current) = pending.pop() {
            let path = self.graph.file(current).path;
            pending.extend(self.graph.includers_of(current).filter(|&includer| {
                includers.insert(includer);
                let re_exports = &self.graph.file(includer).re_exports;
                re_exports.iter().any(|it| it.source_file == Some(path)) && passed.insert(includer)
            }));
        }
        includers
    }

    // constraint: a module imports a name its source exports, so an included file's unexported namesake binds only
    // where no export and no re-export offers the name, as Python imports a name whatever its underscore says
    fn imported_value(&self, site: SymbolAt, name: Str) -> Option<Bound> {
        let named = self.named(name);
        let includes = self.includes(site.file);
        let declaring = |exported: bool| {
            includes.iter().filter_map(move |&included| {
                let declared = self.graph.in_file(named, included).iter().copied();
                let declared =
                    declared.filter(move |&at| self.graph.sym(at).exported() == exported);
                self.pick(Pick::PackageLevel, declared)
            })
        };
        let mut exporting = declaring(true);
        match (exporting.next(), exporting.next()) {
            (Some(at), None) => return Some((at, confidence::INCLUDE)),
            (Some(_), Some(_)) => return None,
            (None, _) => {}
        }
        let call = Call::bare(site, name);
        let mut walk = ReExportWalk {
            cascade: self,
            call: &call,
            visited: HashSet::default(),
        };
        let mut renamed = includes
            .iter()
            .filter_map(|&included| walk.follow(included, name));
        match (renamed.next(), renamed.next()) {
            (Some(at), None) => return Some((at, confidence::RE_EXPORT)),
            (Some(at), Some(other)) if at == other => return Some((at, confidence::RE_EXPORT)),
            (Some(_), Some(_)) => return None,
            (None, _) => {}
        }
        let mut unexported = declaring(false);
        match (unexported.next(), unexported.next()) {
            (Some(at), None) => Some((at, confidence::INCLUDE)),
            (Some(_), Some(_)) => None,
            (None, _) => self.reached_value(site.file, named),
        }
    }

    // constraint: a file and everything it includes compile as one script, so a name no direct include offers binds the
    // one file of those the includes reach in turn that declares it; two such files bind neither, as two direct
    // includes do
    fn reached_value(&self, file: u32, named: &[SymbolAt]) -> Option<Bound> {
        if self.conventions.value_uses != ValueUses::ImportedThenReached {
            return None;
        }
        self.one_reached(file, named, |_| true)
            .map(|at| (at, confidence::INCLUDE))
    }

    // constraint: a DSHL `interval` names the ranges of the global of its name, whichever file writes it, and the
    // compiler reads the global, so the one variable a file's closure holds takes a read an interval would take or tie
    fn reached_variable(&self, file: u32, name: Str) -> Option<Bound> {
        let variable = |at: SymbolAt| self.graph.sym(at).kind == SymbolKind::Variable;
        self.one_reached(file, self.named(name), variable)
            .map(|at| (at, confidence::INCLUDE))
    }

    // constraint: an included file compiles inside every script that includes it, so a name its closure leaves open
    // binds the one file of all those scripts that declares it; each game's copy of a file declares it again, so of
    // several the copy nearest the use binds
    fn scripted_value(&self, file: u32, name: Str) -> Option<Bound> {
        let compiled_with = |declaring: u32| self.share_an_include_root(file, declaring);
        let mut declaring = self
            .declaring(self.named(name), compiled_with, |_| true)
            .peekable();
        let first = declaring.next()?;
        if declaring.peek().is_none() {
            return Some((first, confidence::UNIQUE));
        }
        self.nearest_copy(file, std::iter::once(first).chain(declaring))
            .map(|at| (at, confidence::UNIQUE))
    }

    // constraint: nothing in a tree marks a game, and a library under the top directory of one game's tree is no part
    // of it, so a copy counts as the use's own only past that directory; two copies equally near bind neither
    fn nearest_copy(&self, file: u32, copies: impl Iterator<Item = SymbolAt>) -> Option<SymbolAt> {
        let dir = |file: u32| self.graph.strings.resolve(self.graph.file(file).dir);
        let own = dir(file);
        let mut nearest = (1, None);
        for at in copies {
            let shared = path::shared_segments(dir(at.file), own);
            if shared > nearest.0 {
                nearest = (shared, Some(at));
            } else if shared == nearest.0 {
                nearest.1 = None;
            }
        }
        nearest.1
    }

    fn one_reached(
        &self,
        file: u32,
        named: &[SymbolAt],
        wanted: impl Fn(SymbolAt) -> bool + Copy,
    ) -> Option<SymbolAt> {
        if named.is_empty() {
            return None;
        }
        let closure = self.closure(file);
        self.one_declaring(
            named,
            |declaring| closure.binary_search(&declaring).is_ok(),
            wanted,
        )
    }

    fn one_declaring(
        &self,
        named: &[SymbolAt],
        in_scope: impl Fn(u32) -> bool,
        wanted: impl Fn(SymbolAt) -> bool + Copy,
    ) -> Option<SymbolAt> {
        let mut declaring = self.declaring(named, in_scope, wanted);
        match (declaring.next(), declaring.next()) {
            (Some(at), None) => Some(at),
            _ => None,
        }
    }

    fn declaring<'n>(
        &'n self,
        named: &'n [SymbolAt],
        in_scope: impl Fn(u32) -> bool + 'n,
        wanted: impl Fn(SymbolAt) -> bool + Copy + 'n,
    ) -> impl Iterator<Item = SymbolAt> + 'n {
        named
            .chunk_by(|left, right| left.file == right.file)
            .filter(move |defined| in_scope(defined[0].file))
            .filter_map(move |defined| {
                let wanted = defined.iter().copied().filter(move |&at| wanted(at));
                self.pick(Pick::PackageLevel, wanted)
            })
    }

    // constraint: where only a module's path reaches a free item, a name written without a path reads the item of the
    // innermost inline module that holds its line and declares the name; the modules that hold one line nest, so the
    // longest qualifier is the innermost, of two items of one module, as `#[cfg]` writes, the first binds, and a module
    // is no value, so one of the name leaves the item outside it that a glob brings in
    fn declared_value(&self, named: &[SymbolAt], (file, line): (u32, u32)) -> Option<SymbolAt> {
        let declared = self.graph.in_file(named, file).iter().copied();
        if self.conventions.member_calls != MemberCalls::ModulePaths {
            return self.pick(Pick::PackageLevel, declared);
        }
        declared
            .rev()
            .filter(|&at| {
                self.is_package_level(at)
                    && self.graph.sym(at).kind != SymbolKind::Namespace
                    && self.held_around(at, line)
            })
            .max_by_key(|&at| self.graph.qualifier(at).map_or(0, str::len))
    }

    fn bare_use(&self, (file, line): (u32, u32), name: Str) -> Option<Bound> {
        let stored = self.graph.file(file);
        let named = self.named(name);
        if let Some(at) = self.declared_value(named, (file, line)) {
            return Some((at, confidence::SAME_FILE));
        }
        if !self.bindings(file).dot_dirs.is_empty()
            && let Some(at) = self.dot_imported(&self.referenceable(name), file, Pick::PackageLevel)
        {
            return Some((at, confidence::IMPORT));
        }
        if !self.conventions.directory_packages {
            return None;
        }
        let in_package = named.iter().copied().filter(|&at| {
            let declaring = self.graph.file(at.file);
            declaring.dir == stored.dir && declaring.package == stored.package
        });
        self.pick(Pick::PackageLevel, in_package)
            .map(|at| (at, confidence::PACKAGE))
    }
}

type ImportedDir = Option<Str>;

#[derive(Debug, Clone, Copy)]
enum Pick<'c> {
    FreeTarget(&'c Call),
    PackageLevel,
}

struct ReExportWalk<'c, 'g> {
    cascade: &'c Cascade<'g>,
    call: &'c Call,
    visited: HashSet<(u32, Str), FixedState>,
}

impl ReExportWalk<'_, '_> {
    // constraint: each barrel is searched once under each name, so a chain of any length ends
    fn follow(&mut self, file: u32, target: Str) -> Option<SymbolAt> {
        let cascade = self.cascade;
        if !self.visited.insert((file, target)) {
            return None;
        }
        let re_exports = &cascade.graph.file(file).re_exports;
        let names_it = re_exports
            .iter()
            .any(|re_export| re_export.local_name == target);
        for re_export in re_exports {
            let wanted = if re_export.local_name == target {
                re_export.source_name
            } else if !names_it && Some(re_export.local_name) == cascade.star {
                target
            } else {
                continue;
            };
            let Some(source) = re_export
                .source_file
                .and_then(|path| cascade.graph.slot_of(path))
            else {
                continue;
            };
            let defined = cascade.graph.in_file(cascade.named(wanted), source);
            if let Some(at) = cascade.free_target(self.call, defined.iter().copied()) {
                return Some(at);
            }
            if let Some(at) = self.follow(source, wanted) {
                return Some(at);
            }
        }
        None
    }
}

fn ends_in_a_constant(written: &str, separator: &str) -> bool {
    written
        .rsplit(separator)
        .next()
        .is_some_and(|last| last.starts_with(|first: char| first.is_ascii_uppercase()))
}

// constraint: a constant is looked up from the scopes around its use outward, so `Version` inside `module Gem` names
// `Gem::Version`, and elsewhere only a type of that whole name
/// Whether `written`, a constant path that `separator` joins, used inside the definition qualified as `from`, names
/// the type qualified as `qualified`: the whole name, or that name inside a scope that encloses `from`.
fn spelled_from(from: &str, (written, separator): (&str, &str), qualified: &str) -> bool {
    qualified == written
        || qualified
            .strip_suffix(written)
            .and_then(|scope| scope.strip_suffix(separator))
            .and_then(|scope| from.strip_prefix(scope))
            .is_some_and(|inside| {
                !inside.starts_with(|next: char| next.is_alphanumeric() || next == '_')
            })
}

pub(crate) const fn derives(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface | SymbolKind::Trait
    )
}
