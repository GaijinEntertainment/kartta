use std::cell::RefCell;
use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{
    CallableRole, Conventions, IncludeReach, InheritedMembers, OwnMembers, SymbolKind,
};

use crate::candidates::{CandidateSet, Candidates, INDEXED_FROM};
use crate::confidence;
use crate::graph::{Graph, QUALIFIER_SEPARATORS, is_call_target_kind};
use crate::resolution::Miss;
use crate::scope::{Ancestors, Reach, Scoped, is_value};
use crate::stored::{Entry, Site, SymbolAt};
use crate::strings::Str;
use crate::typed::{Typed, Value};

const THROUGH_INCLUDES_MAX_DEFINITIONS: usize = 32;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Call {
    pub(crate) file: u32,
    pub(crate) caller: u32,
    pub(crate) callee: Str,
    pub(crate) receiver: Option<Str>,
    pub(crate) written_from_root: bool,
    pub(crate) names_a_runtime_method: bool,
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
    pub(crate) receiver_types: Derived<(u32, Str), Value<'g>>,
    empty_receiver: Option<Str>,
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

    // constraint: a file sees the definitions of its own, of the files it includes, and where a package is a
    // directory, of its package
    pub(crate) fn sees(&self, file: u32, at: SymbolAt) -> bool {
        file == at.file
            || self.includes(file).contains(&at.file)
            || (self.conventions.directory_packages
                && self.graph.file(file).dir == self.graph.file(at.file).dir)
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

    pub(crate) fn binding(&self, file: u32, name: Str) -> Option<ImportedDir> {
        self.bindings(file)
            .named
            .iter()
            .find(|&&(held, _)| held == name)
            .map(|&(_, dir)| dir)
    }

    pub(crate) fn dot_imported_dirs(&self, file: u32) -> Vec<Str> {
        self.bindings(file).dot_dirs.clone()
    }

    pub(crate) fn resolve_call(&self, call: &Call) -> Result<Bound, Miss> {
        let caller = call.caller_at();

        if let Some(local) = call.local {
            let at = SymbolAt {
                file: call.file,
                index: local,
            };
            return self
                .is_call_target(self.graph.sym(at).kind)
                .then_some((at, confidence::SAME_FILE))
                .ok_or(Miss::NotAdmitted);
        }

        if let Some(receiver) = call.receiver
            && let Some(target) = self.binding(call.file, receiver)
        {
            if target.is_none() {
                return Err(Miss::ExternalImport);
            }
            return self
                .imported(
                    &self.candidates(call),
                    (call.file, target),
                    Pick::FreeTarget(caller),
                )
                .ok_or(Miss::NotExported);
        }
        if self.reads_type_paths()
            && let Some(receiver) = call.receiver
            && !self.graph.strings.resolve(receiver).is_empty()
        {
            return match self.typed_member(call, receiver) {
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

        if self
            .graph
            .strings
            .resolve(call.callee)
            .contains(QUALIFIER_SEPARATORS)
            && let Some(at) = self.qualified(call.callee)
        {
            let symbol = self.graph.sym(at);
            if self.is_candidate(caller, at) && symbol.qualified_name != symbol.name {
                return Ok((at, confidence::QUALIFIED));
            }
        }
        if let Some(tail) = self.trailing_pair(call.callee)
            && let Some(at) = self.qualified(tail)
            && self.is_candidate(caller, at)
        {
            return Ok((at, confidence::QUALIFIED_TAIL));
        }
        if let Some(at) = self.nested_in_written_owner(call) {
            return Ok((at, confidence::QUALIFIED_TAIL));
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

    fn resolve_by_name(&self, call: &Call) -> Result<Bound, Miss> {
        let candidates = self.candidates(call);
        if let Some(at) = self.member_of_the_receiver_s_owner(call, &candidates) {
            return Ok((at, confidence::RECEIVER));
        }
        let targets = candidates.targets().iter().copied();
        let reach = self.reach(call, &candidates);
        let reach = reach.as_ref();

        if let Some(at) = self.scoped_target(
            call,
            candidates.in_file(self.graph, call.file).iter().copied(),
            reach,
        ) {
            return Ok((at, confidence::SAME_FILE));
        }
        if call.receiver.is_none()
            && let Some(at) =
                self.dot_imported(&candidates, call.file, Pick::FreeTarget(call.caller_at()))
        {
            return Ok((at, confidence::IMPORT));
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
        if let Some(at) = self.included(call, &candidates, reach) {
            return Ok((at, confidence::INCLUDE));
        }
        if !call.names_a_runtime_method
            && let Some(at) = self.re_exported(call)
        {
            return Ok((at, confidence::RE_EXPORT));
        }

        let shape = self.shape(call, &targets, reach);
        if !targets.clone().any(|at| self.admits(&shape, at)) {
            return Err(self.unadmitted(call));
        }
        self.member_of_the_caller_s_owner(call, &candidates)
            .or_else(|| {
                self.one_callable(&shape, targets.clone())
                    .map(|at| (at, confidence::UNIQUE))
            })
            .or_else(|| self.resolve_by_proximity(call, &candidates, reach))
            .ok_or(Miss::Ambiguous)
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
        pick: Pick,
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
        let named = candidates.named();
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
        let mut unique = Scoped::Nothing;
        for defined in defined_per_file.filter(|defined| !defined.is_empty()) {
            match self.scoped(call, defined.iter().copied(), reach) {
                Scoped::Nothing => {}
                Scoped::One(at) if unique == Scoped::Nothing => unique = Scoped::One(at),
                Scoped::One(_) | Scoped::Several => return Scoped::Several,
            }
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
        let caller = call.caller_at();
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
                    && self.is_candidate(caller, at)
            })
            .min_by_key(|&at| (!self.sees(call.file, at), self.graph.order_of(at)))
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
            return self.imported(
                &self.referenceable(site.name),
                (file, target),
                Pick::PackageLevel,
            );
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
    derives(kind)
        || matches!(
            kind,
            SymbolKind::Enum | SymbolKind::TypeAlias | SymbolKind::DefinedType
        )
}

pub(crate) const fn derives(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface | SymbolKind::Trait
    )
}
