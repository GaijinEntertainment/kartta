use std::cell::RefCell;
use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{CallableRole, Conventions, InheritedMembers, OwnMembers, SymbolKind};

use crate::candidates::{CandidateSet, Candidates, INDEXED_FROM};
use crate::confidence;
use crate::graph::{Graph, QUALIFIER_SEPARATORS, is_call_target_kind};
use crate::scope::{Ancestors, Reach};
use crate::stored::{Entry, Site, SymbolAt};
use crate::strings::Str;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Call {
    pub(crate) file: u32,
    pub(crate) caller: u32,
    pub(crate) callee: Str,
    pub(crate) receiver: Option<Str>,
    pub(crate) written_from_root: bool,
    pub(crate) local: Option<u32>,
    pub(crate) line: u32,
}

impl Call {
    pub(crate) const fn caller_at(&self) -> SymbolAt {
        SymbolAt {
            file: self.file,
            index: self.caller,
        }
    }
}

pub(crate) type Bound = (SymbolAt, u8);

#[derive(Debug, Default)]
struct Bindings {
    named: Vec<(Str, Option<Str>)>,
    dot_dirs: Vec<Str>,
}

type Derived<K, V> = RefCell<HashMap<K, V, FixedState>>;

pub(crate) struct Cascade<'g> {
    pub(crate) graph: &'g Graph,
    pub(crate) conventions: Conventions,
    star: Option<Str>,
    bindings: Derived<u32, Rc<Bindings>>,
    candidates: Derived<Str, Rc<Candidates<'g>>>,
    referenceable: Derived<Str, Rc<Candidates<'g>>>,
    includes: Derived<u32, Rc<[u32]>>,
    closures: Derived<u32, Rc<[u32]>>,
    canonical: Derived<SymbolAt, SymbolAt>,
    declarations: Derived<SymbolAt, Rc<[SymbolAt]>>,
    constructor_types: Derived<SymbolAt, Option<Rc<[SymbolAt]>>>,
    pub(crate) type_ancestors: Derived<SymbolAt, Option<Rc<Ancestors>>>,
    pub(crate) owner_ancestors: Derived<Str, Option<Rc<Ancestors>>>,
}

impl<'g> Cascade<'g> {
    pub(crate) fn new(graph: &'g Graph) -> Self {
        Self {
            graph,
            conventions: graph.conventions(),
            star: graph.strings.get("*"),
            bindings: RefCell::default(),
            candidates: RefCell::default(),
            referenceable: RefCell::default(),
            includes: RefCell::default(),
            closures: RefCell::default(),
            canonical: RefCell::default(),
            declarations: RefCell::default(),
            constructor_types: RefCell::default(),
            type_ancestors: RefCell::default(),
            owner_ancestors: RefCell::default(),
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
                    local: Some(call.local),
                    line: call.line,
                }
            }
        }
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
        is_call_target_kind(kind)
            && (self.conventions.values_are_call_targets
                || !matches!(kind, SymbolKind::Variable | SymbolKind::Constant))
    }

    pub(crate) fn named(&self, name: Str) -> &'g [SymbolAt] {
        self.graph.named(name)
    }

    pub(crate) fn holder(&self, at: SymbolAt) -> Option<SymbolAt> {
        self.graph
            .file(at.file)
            .holder(at.index)
            .map(|index| SymbolAt {
                file: at.file,
                index,
            })
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
        let written = self.graph.strings.resolve(qualified);
        let last = written
            .rsplit(QUALIFIER_SEPARATORS)
            .next()
            .unwrap_or(written);
        let tail = self
            .graph
            .strings
            .get(last)
            .filter(|&tail| tail != qualified)
            .map_or(&[][..], |tail| self.named(tail));
        self.named(qualified)
            .iter()
            .chain(tail)
            .copied()
            .filter(move |&at| self.graph.sym(at).qualified_name == qualified)
    }

    pub(crate) fn is_candidate(&self, caller: SymbolAt, at: SymbolAt) -> bool {
        self.is_candidate_in(self.holder(caller).unwrap_or(caller), at)
    }

    fn is_candidate_in(&self, scope: SymbolAt, at: SymbolAt) -> bool {
        let file = self.graph.file(at.file);
        let symbol = &file.symbols[at.index as usize];
        self.is_call_target(symbol.kind)
            && (file.locals.is_empty()
                || file.holder(at.index).is_none_or(|holder| {
                    symbol.owner.is_some()
                        && SymbolAt {
                            file: at.file,
                            index: holder,
                        } == scope
                }))
    }

    pub(crate) fn call_targets<'a>(
        &'a self,
        caller: SymbolAt,
        candidates: impl Iterator<Item = SymbolAt> + Clone + 'a,
    ) -> impl Iterator<Item = SymbolAt> + Clone + 'a {
        let scope = self.holder(caller).unwrap_or(caller);
        candidates.filter(move |&at| self.is_candidate_in(scope, at))
    }

    pub(crate) fn free_target(
        &self,
        caller: SymbolAt,
        candidates: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        self.call_targets(caller, candidates)
            .find(|&at| self.graph.sym(at).owner.is_none())
    }

    fn pick(
        &self,
        pick: Pick,
        mut candidates: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        match pick {
            Pick::FreeTarget(caller) => self.free_target(caller, candidates),
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
            .filter(|&at| !self.graph.sym(at).flags.is_test() && self.holder(at).is_none())
            .collect()
    }

    pub(crate) fn includes(&self, file: u32) -> Rc<[u32]> {
        if let Some(includes) = self.includes.borrow().get(&file) {
            return Rc::clone(includes);
        }
        let mut slots: Vec<u32> = self
            .graph
            .file(file)
            .includes
            .iter()
            .filter_map(|&target| self.graph.slot_of(target))
            .collect();
        slots.sort_unstable_by_key(|&slot| self.graph.path_rank(slot));
        let slots: Rc<[u32]> = slots.into();
        self.includes.borrow_mut().insert(file, Rc::clone(&slots));
        slots
    }

    pub(crate) fn closure(&self, file: u32) -> Rc<[u32]> {
        if let Some(closure) = self.closures.borrow().get(&file) {
            return Rc::clone(closure);
        }
        let mut visited: HashSet<u32, FixedState> = HashSet::with_hasher(FixedState::default());
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
        let closure: Rc<[u32]> = closure.into();
        self.closures.borrow_mut().insert(file, Rc::clone(&closure));
        closure
    }

    pub(crate) fn canonical(&self, at: SymbolAt) -> SymbolAt {
        if let Some(&canonical) = self.canonical.borrow().get(&at) {
            return canonical;
        }
        let canonical = self.linked_definition(at).unwrap_or(at);
        self.canonical.borrow_mut().insert(at, canonical);
        canonical
    }

    fn linked_definition(&self, declaration: SymbolAt) -> Option<SymbolAt> {
        let callable = self
            .graph
            .file(declaration.file)
            .callable(declaration.index)?;
        if callable.role != CallableRole::Declaration {
            return None;
        }
        let mut definitions = self
            .graph
            .occurrences(callable.key)
            .iter()
            .filter(|occurrence| occurrence.role == CallableRole::Definition)
            .map(|occurrence| occurrence.at)
            .filter(|definition| {
                self.closure(definition.file)
                    .binary_search(&declaration.file)
                    .is_ok()
            });
        let definition = definitions.next()?;
        definitions.next().is_none().then_some(definition)
    }

    pub(crate) fn declarations(&self, definition: SymbolAt) -> Rc<[SymbolAt]> {
        if let Some(declarations) = self.declarations.borrow().get(&definition) {
            return Rc::clone(declarations);
        }
        let declarations: Rc<[SymbolAt]> = if self.conventions.owner_chain_separator.is_none()
            && self.bare_call_reaches_members()
        {
            self.linked_declarations(definition).into()
        } else {
            Rc::from([])
        };
        self.declarations
            .borrow_mut()
            .insert(definition, Rc::clone(&declarations));
        declarations
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
        if let Some(types) = self.constructor_types.borrow().get(&at) {
            return types.clone();
        }
        let symbol = self.graph.sym(at);
        let types = (self.conventions.owner_chain_separator.is_none()
            && symbol.kind == SymbolKind::Method
            && symbol.owner == Some(symbol.name))
        .then(|| self.seen_types(at).into());
        self.constructor_types
            .borrow_mut()
            .insert(at, types.clone());
        types
    }

    fn seen_types(&self, constructor: SymbolAt) -> Vec<SymbolAt> {
        let viewers: Vec<u32> = std::iter::once(constructor.file)
            .chain(self.declarations(constructor).iter().map(|at| at.file))
            .collect();
        let sees = |viewer: u32, ty: SymbolAt| {
            self.graph.file(ty.file).dir == self.graph.file(viewer).dir
                || self.closure(viewer).binary_search(&ty.file).is_ok()
        };
        self.named(self.graph.sym(constructor).name)
            .iter()
            .copied()
            .filter(|&ty| {
                is_type_kind(self.graph.sym(ty).kind)
                    && viewers.iter().any(|&viewer| sees(viewer, ty))
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
        if let Some(bindings) = self.bindings.borrow().get(&file) {
            return Rc::clone(bindings);
        }
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
            bindings.named.retain(|&(held, _)| held != name);
            bindings.named.push((name, binding.target_dir));
        }
        let bindings = Rc::new(bindings);
        self.bindings
            .borrow_mut()
            .insert(file, Rc::clone(&bindings));
        bindings
    }

    fn package_name(&self, dir: Str) -> Option<Str> {
        self.graph
            .package_files(dir)
            .iter()
            .find_map(|&slot| self.graph.file(slot).package_stamp())
    }

    fn binding(&self, file: u32, name: Str) -> Option<ImportedDir> {
        self.bindings(file)
            .named
            .iter()
            .find(|&&(held, _)| held == name)
            .map(|&(_, dir)| dir)
    }

    pub(crate) fn resolve_call(&self, call: &Call) -> Option<Bound> {
        let caller = call.caller_at();

        if let Some(local) = call.local {
            let at = SymbolAt {
                file: call.file,
                index: local,
            };
            return self
                .is_call_target(self.graph.sym(at).kind)
                .then_some((at, confidence::SAME_FILE));
        }

        if let Some(receiver) = call.receiver
            && let Some(target) = self.binding(call.file, receiver)
        {
            return self.imported(&self.candidates(call), target, Pick::FreeTarget(caller));
        }

        if self
            .graph
            .strings
            .resolve(call.callee)
            .contains(QUALIFIER_SEPARATORS)
            && let Some(at) = self.qualified(call.callee)
        {
            let symbol = self.graph.sym(at);
            if self.is_candidate(caller, at) && symbol.qualified_name != symbol.name {
                return Some((at, confidence::QUALIFIED));
            }
        }
        if let Some(tail) = self.trailing_pair(call.callee)
            && let Some(at) = self.qualified(tail)
            && self.is_candidate(caller, at)
        {
            return Some((at, confidence::QUALIFIED_TAIL));
        }

        self.resolve_by_name(call)
    }

    fn candidates(&self, call: &Call) -> CandidateSet<'g> {
        let named = self.named(call.callee);
        let scoped = || {
            let targets = self
                .call_targets(call.caller_at(), named.iter().copied())
                .collect();
            CandidateSet::Own(Candidates::scanned(named, targets))
        };
        if named.len() < INDEXED_FROM {
            return scoped();
        }
        let shared = self.shared_candidates(call.callee, named);
        let caller = call.caller_at();
        let scope = self.holder(caller).unwrap_or(caller);
        if shared
            .locals()
            .iter()
            .any(|&local| local.file == call.file && self.is_candidate_in(scope, local))
        {
            return scoped();
        }
        CandidateSet::Shared(shared)
    }

    fn shared_candidates(&self, name: Str, named: &'g [SymbolAt]) -> Rc<Candidates<'g>> {
        if let Some(shared) = self.candidates.borrow().get(&name) {
            return Rc::clone(shared);
        }
        let (locals, targets): (Vec<SymbolAt>, Vec<SymbolAt>) = named
            .iter()
            .copied()
            .filter(|&at| self.is_call_target(self.graph.sym(at).kind))
            .partition(|&at| self.holder(at).is_some());
        let shared = Rc::new(Candidates::indexed(self.graph, named, targets, locals));
        self.candidates
            .borrow_mut()
            .insert(name, Rc::clone(&shared));
        shared
    }

    fn resolve_by_name(&self, call: &Call) -> Option<Bound> {
        let candidates = self.candidates(call);
        if let Some(at) = self.member_of_the_receiver_s_owner(call, &candidates) {
            return Some((at, confidence::RECEIVER));
        }
        let targets = candidates.targets().iter().copied();
        let reach = self.reach(call, &candidates);
        let reach = reach.as_ref();

        if let Some(at) = self.scoped_target(
            call,
            candidates.in_file(self.graph, call.file).iter().copied(),
            reach,
        ) {
            return Some((at, confidence::SAME_FILE));
        }
        if call.receiver.is_none()
            && let Some(at) =
                self.dot_imported(&candidates, call.file, Pick::FreeTarget(call.caller_at()))
        {
            return Some((at, confidence::IMPORT));
        }
        if self.conventions.directory_packages
            && let Some(at) = self.scoped_target(
                call,
                self.declared_in(&candidates, self.graph.file(call.file).dir)
                    .into_iter(),
                reach,
            )
        {
            return Some((at, confidence::PACKAGE));
        }
        if let Some(at) = self.included(call, &candidates, reach) {
            return Some((at, confidence::INCLUDE));
        }
        if let Some(at) = self.re_exported(call) {
            return Some((at, confidence::RE_EXPORT));
        }

        let shape = self.shape(call, &targets, reach);
        if !targets.clone().any(|at| self.admits(&shape, at)) {
            return None;
        }
        self.member_of_the_caller_s_owner(call, &candidates)
            .or_else(|| {
                self.one_callable(&shape, targets.clone())
                    .map(|at| (at, confidence::UNIQUE))
            })
            .or_else(|| self.resolve_by_proximity(call, &candidates, reach))
    }

    fn imported(
        &self,
        candidates: &Candidates<'_>,
        target: Option<Str>,
        pick: Pick,
    ) -> Option<Bound> {
        let at = self.pick(pick, self.declared_in(candidates, target?).into_iter())?;
        self.graph
            .sym(at)
            .exported
            .then_some((at, confidence::IMPORT))
    }

    fn dot_imported(&self, candidates: &Candidates<'_>, file: u32, pick: Pick) -> Option<SymbolAt> {
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

    fn included(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
        reach: Option<&Reach>,
    ) -> Option<SymbolAt> {
        let mut unique = None;
        for &included in self.includes(call.file).iter() {
            let defined = candidates.in_file(self.graph, included);
            if defined.is_empty() {
                continue;
            }
            let Some(at) = self.scoped_target(call, defined.iter().copied(), reach) else {
                continue;
            };
            if unique.is_some() {
                return None;
            }
            unique = Some(at);
        }
        unique
    }

    fn re_exported(&self, call: &Call) -> Option<SymbolAt> {
        let mut walk = ReExportWalk {
            cascade: self,
            caller: call.caller_at(),
            visited: HashSet::with_hasher(FixedState::default()),
        };
        self.includes(call.file)
            .iter()
            .find_map(|&included| walk.follow(included, call.callee))
    }

    fn trailing_pair(&self, callee: Str) -> Option<Str> {
        let separator = self.conventions.scope_separator?;
        let written = self.graph.strings.resolve(callee);
        let last = written.rfind(separator)?;
        let previous = written[..last].rfind(separator)?;
        self.graph
            .strings
            .get(&written[previous + separator.len()..])
    }

    // constraint: a use can name a value even where a call cannot target one, so every kind but a field and a variant
    // is a candidate, whatever `values_are_call_targets` says
    fn referenceable(&self, name: Str) -> Rc<Candidates<'g>> {
        if let Some(held) = self.referenceable.borrow().get(&name) {
            return Rc::clone(held);
        }
        let named = self.named(name);
        let targets = named
            .iter()
            .copied()
            .filter(|&at| is_call_target_kind(self.graph.sym(at).kind))
            .collect();
        let referenceable = Rc::new(Candidates::scanned(named, targets));
        self.referenceable
            .borrow_mut()
            .insert(name, Rc::clone(&referenceable));
        referenceable
    }

    pub(crate) fn resolve_reference(&self, file: u32, index: u32) -> Option<Bound> {
        let stored = self.graph.file(file);
        let site = stored.references[index as usize];

        if let Some(qualifier) = site.qualifier {
            let target = self.binding(file, qualifier)?;
            return self.imported(&self.referenceable(site.name), target, Pick::PackageLevel);
        }
        let named = self.named(site.name);
        if let Some(at) = self.pick(
            Pick::PackageLevel,
            self.graph.in_file(named, file).iter().copied(),
        ) {
            return Some((at, confidence::SAME_FILE));
        }
        if !self.bindings(file).dot_dirs.is_empty()
            && let Some(at) =
                self.dot_imported(&self.referenceable(site.name), file, Pick::PackageLevel)
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
enum Pick {
    FreeTarget(SymbolAt),
    PackageLevel,
}

struct ReExportWalk<'c, 'g> {
    cascade: &'c Cascade<'g>,
    caller: SymbolAt,
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
            if let Some(at) = cascade.free_target(self.caller, defined.iter().copied()) {
                return Some(at);
            }
            if let Some(at) = self.follow(source, wanted) {
                return Some(at);
            }
        }
        None
    }
}

pub(crate) const fn is_type_kind(kind: SymbolKind) -> bool {
    derives(kind) || matches!(kind, SymbolKind::Enum | SymbolKind::TypeAlias)
}

pub(crate) const fn derives(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface | SymbolKind::Trait
    )
}
