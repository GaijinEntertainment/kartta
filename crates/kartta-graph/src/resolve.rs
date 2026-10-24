use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::hash::Hash;
use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{
    BareReceivers, CallableRole, Conventions, IncludeReach, IncludedRuntimeMembers,
    InheritedMembers, MacroInvocations, MemberCalls, NamespaceCalls, OwnMembers, SymbolKind,
    UntypedMembers, ValueUses,
};

use crate::candidates::{CandidateSet, Candidates, Gate, INDEXED_FROM};
use crate::confidence;
use crate::graph::{Graph, QUALIFIER_SEPARATORS, is_call_target_kind, or_if_none, tails};
use crate::members::DataNamed;
use crate::resolution::Miss;
use crate::scope::{Ancestors, OwnerMember, Reach, Scoped, Shape, is_value};
use crate::stored::{Entry, Site, SymbolAt, UseReceiver};
use crate::strings::Str;
use crate::twins::Twin;
use crate::typed::{Typed, Value};

pub(crate) const THROUGH_INCLUDES_MAX_DEFINITIONS: usize = 32;

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
    pub(crate) invokes_a_macro: bool,
    pub(crate) calls_a_method: bool,
    pub(crate) local: Option<u32>,
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
            invokes_a_macro: false,
            calls_a_method: false,
            local: None,
            line: 0,
        }
    }

    pub(crate) const fn caller_at(&self) -> SymbolAt {
        SymbolAt::new(self.file, self.caller)
    }

    const fn form(&self) -> (bool, bool) {
        (self.invokes_a_macro, self.calls_a_method)
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
// there, which differs from the bound name under an alias; a required module is the module an included file declares,
// beside that file
#[derive(Debug, Default)]
struct Bindings {
    named: Vec<(Str, Option<Str>, Str)>,
    dot_dirs: Vec<Str>,
    modules: bool,
    required: Vec<(Str, u32)>,
}

type Derived<K, V> = RefCell<HashMap<K, V, FixedState>>;

// constraint: the call targets of one name, of the call's form, a macro invocation, a method call or neither, and among
// those the ones the caller's package binds by name where it is gated, else every one
type CandidatesKey = (Str, (bool, bool), Option<Str>);

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
    canonical: Derived<SymbolAt, SymbolAt>,
    declarations: Derived<SymbolAt, Rc<[SymbolAt]>>,
    constructor_types: Derived<SymbolAt, Option<Rc<[SymbolAt]>>>,
    pub(crate) type_ancestors: Derived<SymbolAt, Option<Rc<Ancestors>>>,
    pub(crate) owner_ancestors: Derived<Str, Option<Rc<Ancestors>>>,
    pub(crate) receiver_types: Derived<(SymbolAt, Str), Value<'g>>,
    empty_receiver: Option<Str>,
}

impl<'g> Cascade<'g> {
    pub(crate) fn new(graph: &'g Graph) -> Self {
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
            canonical: RefCell::default(),
            declarations: RefCell::default(),
            constructor_types: RefCell::default(),
            type_ancestors: RefCell::default(),
            owner_ancestors: RefCell::default(),
            receiver_types: RefCell::default(),
            empty_receiver: graph.strings.get(""),
        }
    }

    pub(crate) fn call(&self, site: Site) -> Call {
        let file = self.graph.file(site.file);
        match site.entry() {
            Entry::Call(index) => {
                let call = file.calls[index as usize];
                Call {
                    file: site.file,
                    caller: call.caller(),
                    callee: call.callee,
                    receiver: call.receiver,
                    written_from_root: call.is_written_from_root(),
                    names_a_runtime_method: call.receiver.is_some()
                        && self.is_runtime_method(call.callee),
                    on_a_class: call.is_on_a_class(),
                    invokes_a_macro: call.invokes_a_macro(),
                    calls_a_method: call.calls_a_method(),
                    local: None,
                    line: call.line,
                }
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
                    invokes_a_macro: false,
                    calls_a_method: false,
                    local: Some(call.local),
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

    pub(crate) const fn is_call_target(&self, kind: SymbolKind) -> bool {
        is_call_target_kind(kind) && (self.conventions.values_are_call_targets || !is_value(kind))
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
                !self.is_call_target(self.graph.sym(at).kind),
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
        self.is_call_target(symbol.kind)
            && self.is_of_the_call_s_form(call, symbol.kind)
            && (file.locals.is_empty()
                || file.holder(at.index).is_none_or(|holder| {
                    symbol.owner.is_some() && SymbolAt::new(at.file, holder) == scope
                }))
    }

    // constraint: a method call calls a method, never the value of a variable or a constant
    const fn is_of_the_call_s_form(&self, call: &Call, kind: SymbolKind) -> bool {
        (matches!(
            self.conventions.macro_invocations,
            MacroInvocations::AsCalls
        ) || call.invokes_a_macro == matches!(kind, SymbolKind::Macro))
            && !(call.calls_a_method && is_value(kind))
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
            Pick::PackageLevel => candidates.find(|&at| {
                let symbol = self.graph.sym(at);
                symbol.owner.is_none()
                    && is_call_target_kind(symbol.kind)
                    && self.holder(at).is_none()
            }),
        }
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
    // often
    fn linked_definition(&self, declaration: SymbolAt) -> Option<SymbolAt> {
        let callable = self
            .graph
            .file(declaration.file)
            .callable(declaration.index)?;
        if callable.role != CallableRole::Declaration {
            return None;
        }
        let pasted = self.conventions.include_reach == IncludeReach::Transitive;
        let mut definitions = self
            .graph
            .occurrences(callable.key)
            .iter()
            .filter(|occurrence| occurrence.role == CallableRole::Definition)
            .map(|occurrence| occurrence.at)
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
            let symbol = self.graph.sym(at);
            (self.conventions.owner_chain_separator.is_none()
                && symbol.kind == SymbolKind::Method
                && symbol.owner == Some(symbol.name))
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
            bindings.named.retain(|&(held, _, _)| held != name);
            bindings
                .named
                .push((name, binding.target_dir, binding.last_segment));
            bindings.modules |= binding
                .target_dir
                .is_some_and(|target| self.graph.slot_of(target).is_some());
        }
        bindings.required = self
            .includes(file)
            .iter()
            .filter_map(|&slot| Some((self.graph.file(slot).package?, slot)))
            .collect();
        bindings
    }

    // constraint: a script module is the file that declares it, so a qualifier no import binds names the module of a
    // file its own file requires; code that compiles qualifies no definition the module keeps private
    fn required_definition(
        &self,
        file: u32,
        (qualifier, name): (Str, Str),
        pick: Pick<'_>,
    ) -> Option<SymbolAt> {
        let &(_, module) = self
            .bindings(file)
            .required
            .iter()
            .find(|&&(declared, _)| declared == qualifier)?;
        self.pick(
            pick,
            self.graph.in_file(self.named(name), module).iter().copied(),
        )
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
        let (at, confidence) = self.resolve_by_tiers(call)?;
        if self.is_unbound_namespace(at) {
            return Err(Miss::NotAdmitted);
        }
        Ok((at, confidence))
    }

    fn resolve_by_tiers(&self, call: &Call) -> Result<Bound, Miss> {
        let caller = call.caller_at();

        if let Some(local) = call.local {
            let at = SymbolAt::new(call.file, local);
            return self
                .is_call_target(self.graph.sym(at).kind)
                .then_some((at, confidence::SAME_FILE))
                .ok_or(Miss::NotAdmitted);
        }

        if let Some(bound) = self.through_binding(call) {
            return bound;
        }
        if self.reads_type_paths()
            && let Some(receiver) = call.receiver
            && !self.graph.strings.resolve(receiver).is_empty()
        {
            let binds = |kind| self.is_call_target(kind) && self.is_of_the_call_s_form(call, kind);
            return match self.typed_member((caller, receiver), call.callee, binds) {
                Typed::Member(at) => Ok((at, confidence::RECEIVER)),
                Typed::Imported => Err(Miss::ExternalImport),
                Typed::Undeclared => Err(Miss::Undefined),
                Typed::NoMethod => Err(Miss::NotAdmitted),
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

    pub(crate) fn candidates(&self, call: &Call) -> CandidateSet<'g> {
        let named = self.named(call.callee);
        let scoped = || {
            let gate = self.gate(call.file);
            let mut turned_away = false;
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
            let (locals, targets): (Vec<SymbolAt>, Vec<SymbolAt>) = named
                .iter()
                .copied()
                .filter(|&at| {
                    let kind = self.graph.sym(at).kind;
                    self.is_call_target(kind) && self.is_of_the_call_s_form(call, kind)
                })
                .partition(|&at| self.holder(at).is_some());
            Rc::new(Candidates::indexed(self.graph, named, targets, locals))
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

        // constraint: a class outside the graph has no member in the caller's file or class, and a bare name reaches a
        // class the graph holds under another name only through the file's imports, as an alias does
        let outside = self.names_a_class_outside_the_graph(call);
        if !outside
            && let Some(at) = self.scoped_target(
                call,
                (candidates.in_file(self.graph, call.file).iter().copied())
                    .filter(|&at| self.path_names(path, at)),
                reach,
            )
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
            return Ok((at, confidence::INCLUDE));
        }
        if !call.names_a_runtime_method
            && let Some(at) = self.re_exported(call)
        {
            return Ok((at, confidence::RE_EXPORT));
        }
        if outside {
            return Err(self.unadmitted(call));
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
        let candidates = if call.invokes_a_macro {
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
                if self.names_a_library_method_on_a_value(call, at) {
                    return Ok((at, confidence::FIRST_PARTY));
                }
                Ok((at, confidence::UNIQUE))
            }
            Some(_) => Err(Miss::NotAdmitted),
            None => self
                .one_type_outside_tests(call, &shape, targets)
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
    // a callable from being the one of its name wherever it would count as a candidate; a macro invocation or a method
    // call calls no value
    pub(crate) fn bodies_values<'c>(
        &'c self,
        call: &Call,
        candidates: &'c Candidates<'_>,
    ) -> impl Iterator<Item = SymbolAt> + 'c {
        let indexed = candidates.holds_locals_apart();
        let held_apart = if !self.conventions.values_are_call_targets
            || call.invokes_a_macro
            || call.calls_a_method
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

    /// Whether the call's receiver is a bare name that the language writes only for a class, and no type the graph
    /// holds is named.
    fn names_a_class_outside_the_graph(&self, call: &Call) -> bool {
        let (BareReceivers::Classes(keywords), Some(receiver)) =
            (self.conventions.bare_receivers, call.receiver)
        else {
            return false;
        };
        let written = self.graph.strings.resolve(receiver);
        !written.is_empty()
            && written
                .bytes()
                .all(|byte| !byte.is_ascii() || byte.is_ascii_alphanumeric() || byte == b'_')
            && !keywords
                .iter()
                .any(|keyword| keyword.eq_ignore_ascii_case(written))
            && !self
                .named(receiver)
                .iter()
                .any(|&at| self.graph.sym(at).kind.is_type())
    }

    // constraint: a type the graph does not hold, from the standard library, the prelude or a crate outside the tree,
    // has no member in the graph, and a module the caller includes is no scope its name is looked up in, so a call
    // through it that no member of the owner binds binds nothing; nor does a call through an owner whose members of
    // the name lie only in packages the caller's is not built with, as `Vec::new()` beside a crate's own `Vec`
    fn through_the_receiver_s_owner(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> Option<Result<Bound, Miss>> {
        if let Some(at) = self.member_of_the_receiver_s_owner(call, candidates) {
            return Some(Ok((at, confidence::RECEIVER)));
        }
        (self.names_a_type_outside_the_graph(call) || self.owned_out_of_reach(call, candidates))
            .then(|| Err(self.unadmitted(call)))
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
    /// caller's package can reach.
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
                let kind = self.graph.sym(at).kind;
                (kind.is_type() || is_value(kind))
                    && gate
                        .as_ref()
                        .is_none_or(|gate| gate.admits_at(self.graph, at))
            })
        })
    }

    // constraint: test code is built apart from the code it tests, so of the definitions a call reaches by name alone,
    // a test one binds a call from test code alone; it still counts among them, so a name test code shares, often a
    // stub of a builtin or of a library outside the graph, is no surer to mean another definition of it
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

    fn can_bind_by_name(&self, call: &Call, at: SymbolAt) -> bool {
        !self.graph.flags(at).is_test() || self.graph.flags(call.caller_at()).is_test()
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
        let Some(binding) = self.binding(call.file, receiver) else {
            return self
                .required_definition(call.file, (receiver, call.callee), Pick::FreeTarget(call))
                .map(|at| Ok((at, confidence::IMPORT)));
        };
        let Some(target) = binding else {
            return Some(Err(Miss::ExternalImport));
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
        let &(_, target, _) = bindings
            .named
            .iter()
            .find(|&&(held, _, _)| held == call.callee)?;
        self.module_export(call, self.graph.slot_of(target?)?)
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
                    && (symbol.exported || symbol.kind != SymbolKind::Macro)
            });
        let Some(at) = answers.min_by_key(|&at| self.graph.sym(at).kind.is_type()) else {
            return self.used_by_module(call, (module, name), visited);
        };
        (self.graph.sym(at).exported || self.is_within_module(call.file, module))
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
            .any(|root| roots.contains(root))
    }

    // constraint: a Rust crate is the files its root, which no file includes, includes directly or through others
    fn include_roots(&self, file: u32) -> Vec<u32> {
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
                && symbol.exported
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
            .exported
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
                .filter(|&at| self.graph.sym(at).exported)
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
        if self.conventions.include_reach != IncludeReach::Transitive {
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
                Some(at)
            }
            Scoped::One(_) | Scoped::Nothing | Scoped::Several => None,
        }
    }

    fn offered<'a>(
        &self,
        call: &Call,
        reach: Option<&Reach>,
        defined_per_file: impl Iterator<Item = &'a [SymbolAt]>,
    ) -> Scoped {
        let narrowed = call.names_a_runtime_method
            && self.conventions.included_runtime_members == IncludedRuntimeMembers::SpelledOrOwn;
        let mut unique = Scoped::Nothing;
        for defined in defined_per_file.filter(|defined| !defined.is_empty()) {
            let reached = defined
                .iter()
                .copied()
                .filter(|&at| !narrowed || self.spelled_or_own(call, at));
            match self.scoped(call, reached, reach) {
                Scoped::Nothing => {}
                Scoped::One(at) if unique == Scoped::Nothing => unique = Scoped::One(at),
                Scoped::One(_) | Scoped::Several => return Scoped::Several,
            }
        }
        unique
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

    fn re_exported(&self, call: &Call) -> Option<SymbolAt> {
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
                .filter(|&at| is_call_target_kind(self.graph.sym(at).kind))
                .collect();
            Rc::new(Candidates::scanned(named, targets, false))
        })
    }

    // constraint: a member of a value binds through the type its path spells, or of the owner it names where receivers
    // name owners, else the one field of the name its own file declares outside other bodies, or where the language
    // says so, its includes' then theirs; the package, an import and the family bind a namesake too often, where a
    // test table or an anonymous struct holds the value
    fn member_use(&self, site: SymbolAt, name: Str, path: Option<Str>) -> Result<Bound, UseMiss> {
        if let Some(path) = path
            && self.reads_type_paths()
        {
            // constraint: a member taken without a call is a field or a method value, so every member kind binds
            match self.typed_member((site, path), name, |_| true) {
                Typed::Member(at) => return Ok((at, confidence::RECEIVER)),
                Typed::Imported | Typed::Undeclared | Typed::NoMethod => {
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
        let site = self.graph.file(file).uses[index as usize];
        let at = SymbolAt::new(file, site.site_of());
        match site.receiver() {
            UseReceiver::Bare => self
                .class_scope_use(at, site.name)
                .unwrap_or_else(|| self.value_use(at, site.name).ok_or(UseMiss::Elsewhere)),
            UseReceiver::Qualifier(qualifier) => match self.binding(file, qualifier) {
                Some(target) => {
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
                    self.bare_use(file, site.name).ok_or(UseMiss::Elsewhere)
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

    fn value_use(&self, site: SymbolAt, name: Str) -> Option<Bound> {
        match self.conventions.value_uses {
            ValueUses::Declared => self.bare_use(site.file, name),
            ValueUses::Imported => self
                .bare_use(site.file, name)
                .or_else(|| self.imported_value(site, name)),
            ValueUses::Included => self.included_value(site.file, name),
        }
    }

    // constraint: a module imports a name its source exports, so an included file's unexported namesake binds only
    // where no export and no re-export offers the name, as Python imports a name whatever its underscore says
    fn imported_value(&self, site: SymbolAt, name: Str) -> Option<Bound> {
        let named = self.named(name);
        let includes = self.includes(site.file);
        let declaring = |exported: bool| {
            includes.iter().filter_map(move |&included| {
                let declared = self.graph.in_file(named, included).iter().copied();
                let declared = declared.filter(move |&at| self.graph.sym(at).exported == exported);
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
            _ => None,
        }
    }

    fn bare_use(&self, file: u32, name: Str) -> Option<Bound> {
        let stored = self.graph.file(file);
        let named = self.named(name);
        if let Some(at) = self.pick(
            Pick::PackageLevel,
            self.graph.in_file(named, file).iter().copied(),
        ) {
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

pub(crate) const fn derives(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface | SymbolKind::Trait
    )
}
