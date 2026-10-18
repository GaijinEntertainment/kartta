use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{CallsOnValues, InheritedMembers, MemberCalls, SymbolKind};

use crate::candidates::Candidates;
use crate::confidence;
use crate::graph::{agree, distinct, last_segment};
use crate::resolve::{Bound, Call, Cascade, derived, derives};
use crate::stored::SymbolAt;
use crate::strings::Str;

/// What a set of candidates offers a call: no target its shape admits, one, or several on different owners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scoped {
    Nothing,
    One(SymbolAt),
    Several,
}

#[derive(Debug, Clone)]
pub(crate) enum Shape {
    Free,
    FreeFunction,
    Global,
    Member,
    Own(Reach),
    Inherited(Reach),
    FreeOrInherited(Reach),
    FreeOrEnclosing(Str),
}

#[derive(Debug, Clone)]
pub(crate) struct Reach {
    caller: SymbolAt,
    own: Str,
    own_type_body: Option<Home>,
    out_of_line_home: Option<Home>,
    ancestors: Option<Rc<Ancestors>>,
    may_own_name: bool,
    nearest_base: Option<Str>,
}

#[derive(Debug, Default)]
pub(crate) struct Ancestors {
    owners: HashSet<Str, FixedState>,
    homes: HashSet<(Str, Home), FixedState>,
    qualified_per_base: HashMap<Str, Option<Str>, FixedState>,
    bases_of: HashMap<Str, HashSet<Str, FixedState>, FixedState>,
}

impl Ancestors {
    fn derives_from(&self, owner: Str, base: Str) -> bool {
        let mut walked: HashSet<Str, FixedState> = HashSet::with_hasher(FixedState::default());
        walked.insert(owner);
        let mut pending = vec![owner];
        while let Some(next) = pending.pop() {
            let ups = self.bases_of.get(&next).into_iter().flatten().copied();
            if ups.clone().any(|up| up == base) {
                return true;
            }
            pending.extend(ups.filter(|&up| walked.insert(up)));
        }
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Home {
    file: u32,
    start: u32,
    end: u32,
    alone_in_file: bool,
    qualified: Option<Str>,
}

impl Home {
    const fn contains(self, file: u32, line: u32) -> bool {
        file == self.file && self.start <= line && line <= self.end
    }

    const fn whole_file(file: u32) -> Self {
        Self {
            file,
            start: 1,
            end: 0,
            alone_in_file: true,
            qualified: None,
        }
    }
}

#[derive(Debug)]
struct TypeWalk {
    ty: SymbolAt,
    own: Str,
    found: Ancestors,
    walked: HashSet<SymbolAt, FixedState>,
    pending: Vec<SymbolAt>,
}

#[derive(Debug, Clone, Copy)]
struct DeclaredBase {
    owner: Str,
    written: Str,
    written_from_root: bool,
}

impl Cascade<'_> {
    fn position(&self, at: SymbolAt) -> (u32, u32) {
        (at.file, self.graph.sym(at).line)
    }

    fn owner(&self, at: SymbolAt) -> Option<Str> {
        self.graph.sym(at).owner
    }

    fn folded_lines(&self, at: SymbolAt) -> impl Iterator<Item = u32> + Clone + '_ {
        self.graph.file(at.file).folded_lines(at.index)
    }

    fn body(&self, ty: SymbolAt) -> Home {
        let symbol = self.graph.sym(ty);
        Home {
            file: ty.file,
            start: symbol.line,
            end: symbol.end_line,
            alone_in_file: false,
            qualified: Some(symbol.qualified_name),
        }
    }

    fn home(&self, ty: SymbolAt) -> Home {
        let name = self.graph.sym(ty).name;
        let alone_in_file = !self
            .graph
            .in_file(self.named(name), ty.file)
            .iter()
            .any(|&other| other != ty && derives(self.graph.sym(other).kind));
        Home {
            alone_in_file,
            ..self.body(ty)
        }
    }

    fn in_body(&self, home: Home, at: SymbolAt) -> bool {
        let (file, line) = self.position(at);
        home.contains(file, line)
    }

    fn holds_declaration(&self, home: Home, at: SymbolAt, declarations: &[SymbolAt]) -> bool {
        declarations
            .iter()
            .any(|&declaration| self.in_body(home, declaration))
            || self
                .folded_lines(at)
                .any(|line| home.contains(at.file, line))
    }

    fn certifies(&self, home: Home, at: SymbolAt, declarations: &[SymbolAt]) -> bool {
        self.in_body(home, at)
            || self.holds_declaration(home, at, declarations)
            || (home.alone_in_file
                && at.file == home.file
                && declarations
                    .iter()
                    .all(|declaration| declaration.file == home.file))
    }

    pub(crate) fn reach(&self, call: &Call, candidates: &Candidates<'_>) -> Option<Reach> {
        if call.receiver.is_some() || call.written_from_root {
            return None;
        }
        let named = candidates.named();
        let caller = call.caller_at();
        let own = self
            .graph
            .file(call.file)
            .symbols
            .get(call.caller as usize)?
            .owner?;
        let mut reach = Reach {
            caller,
            own,
            own_type_body: None,
            out_of_line_home: None,
            ancestors: None,
            may_own_name: false,
            nearest_base: None,
        };
        if !self.bare_call_reaches_members() {
            return Some(reach);
        }

        let own_members = candidates.with_owner(self.graph, own);
        let (caller_type, declared_in_caller_file) =
            if self.conventions.owner_chain_separator.is_none()
                && (self.conventions.inherited_members != InheritedMembers::Unreached
                    || !own_members.is_empty())
            {
                self.caller_type(caller, own)
            } else {
                (None, true)
            };
        reach.own_type_body = caller_type.map(|ty| self.home(ty));
        reach.out_of_line_home = (!declared_in_caller_file).then(|| Home::whole_file(call.file));

        let mut members_of_own = own_members
            .iter()
            .copied()
            .filter(|&at| self.is_own_member(&reach, at))
            .peekable();
        let has_own_name = members_of_own.peek().is_some();
        let own_data_member = self.conventions.inherited_members == InheritedMembers::Found
            && named.iter().any(|&at| {
                self.graph.sym(at).kind == SymbolKind::Field
                    && self.is_own_member(&reach, at)
                    && self.declares_own(&reach, at)
            });
        let certainly_own = members_of_own.any(|at| self.declares_own(&reach, at));
        if !own_data_member && !certainly_own {
            reach.may_own_name = has_own_name;
            reach.ancestors = self.ancestors_of(own, caller_type);
            reach.nearest_base = caller_type
                .and(reach.ancestors.as_deref())
                .and_then(|up| self.nearest_base(named, own, up));
        }
        Some(reach)
    }

    fn nearest_base(&self, named: &[SymbolAt], own: Str, up: &Ancestors) -> Option<Str> {
        let mut reached: HashSet<Str, FixedState> = HashSet::with_hasher(FixedState::default());
        if self.conventions.inherited_members != InheritedMembers::Found
            || up.owners.len() < 2
            || up.owners.contains(&own)
            || up.homes.iter().any(|&(owner, _)| !reached.insert(owner))
        {
            return None;
        }
        let declaring = distinct(
            named
                .iter()
                .filter(|&&at| self.declares_inherited(up, at))
                .filter_map(|&at| self.owner(at)),
        );
        let mut nearest = declaring.iter().copied().filter(|&owner| {
            declaring
                .iter()
                .all(|&other| other == owner || up.derives_from(owner, other))
        });
        match (nearest.next(), nearest.next()) {
            (Some(owner), None) if declaring.len() > 1 => Some(owner),
            _ => None,
        }
    }

    fn caller_type(&self, caller: SymbolAt, own: Str) -> (Option<SymbolAt>, bool) {
        let file = caller.file;
        let located = self.located_types(own, file, |_| true);
        let first = located.first().copied();
        let declared_in_file = first.is_some_and(|ty| ty.file == file);
        let caller_type = self
            .body_holding(own, self.position(caller))
            .or_else(|| {
                self.declarations(caller)
                    .iter()
                    .map(|&declaration| self.position(declaration))
                    .chain(self.folded_lines(caller).map(|line| (file, line)))
                    .filter_map(|at| self.body_holding(own, at))
                    .min_by_key(|&ty| (self.graph.path_rank(ty.file), ty.index))
            })
            .or_else(|| (located.len() == 1).then_some(first).flatten());
        (caller_type, declared_in_file)
    }

    fn body_holding(&self, own: Str, (file, line): (u32, u32)) -> Option<SymbolAt> {
        self.graph
            .in_file(self.named(own), file)
            .iter()
            .copied()
            .find(|&ty| derives(self.graph.sym(ty).kind) && self.body(ty).contains(file, line))
    }

    pub(crate) fn is_own_member(&self, reach: &Reach, at: SymbolAt) -> bool {
        self.owner(at) == Some(reach.own)
            && (self.conventions.owner_chain_separator.is_none()
                || self.member_separator(at) == self.member_separator(reach.caller))
            && !self.in_another_body(reach, at)
    }

    fn in_another_body(&self, reach: &Reach, at: SymbolAt) -> bool {
        let Some(home) = reach.own_type_body else {
            return false;
        };
        let definition = self.canonical(at);
        let declarations = self.declarations(definition);
        if self.in_body(home, definition) || self.holds_declaration(home, definition, &declarations)
        {
            return false;
        }
        let another = |position: (u32, u32)| {
            self.body_holding(reach.own, position).is_some_and(|ty| {
                ty.file == home.file || Some(self.graph.sym(ty).qualified_name) != home.qualified
            })
        };

        another(self.position(definition))
            || declarations
                .iter()
                .any(|&declaration| another(self.position(declaration)))
            || self
                .folded_lines(definition)
                .any(|line| another((definition.file, line)))
    }

    fn declares_own(&self, reach: &Reach, at: SymbolAt) -> bool {
        let declarations = self.declarations(at);
        self.is_own_member(reach, at)
            && (self.conventions.owner_chain_separator.is_some()
                || [reach.own_type_body, reach.out_of_line_home]
                    .into_iter()
                    .flatten()
                    .any(|home| self.certifies(home, at, &declarations)))
    }

    fn constructs(&self, constructor: SymbolAt, ty: SymbolAt) -> bool {
        matches!(
            self.graph.sym(ty).kind,
            SymbolKind::Class | SymbolKind::Struct
        ) && self.owner(constructor) == Some(self.member_owner(ty))
            && (self.conventions.owner_chain_separator.is_some()
                || self
                    .constructor_types(constructor)
                    .is_some_and(|types| types.contains(&ty)))
    }

    fn member_separator(&self, at: SymbolAt) -> Option<&str> {
        let owner = self.graph.strings.resolve(self.owner(at)?);
        self.graph.qualifier(at)?.strip_prefix(owner)
    }

    fn declares_inherited(&self, up: &Ancestors, at: SymbolAt) -> bool {
        let Some(owner) = self.owner(at) else {
            return false;
        };
        let declarations = self.declarations(at);
        up.owners.contains(&owner)
            && (self.conventions.owner_chain_separator.is_some()
                || (up
                    .qualified_per_base
                    .get(&owner)
                    .is_some_and(Option::is_some)
                    && up.homes.iter().any(|&(base, home)| {
                        base == owner && self.certifies(home, at, &declarations)
                    })))
    }

    pub(crate) fn admits(&self, shape: &Shape, at: SymbolAt) -> bool {
        let owner = self.owner(at);
        let free = || owner.is_none() || self.stands_for_its_type(at);
        match shape {
            Shape::Free => free(),
            Shape::FreeFunction => free() && !is_value(self.graph.sym(at).kind),
            Shape::Global => {
                let symbol = self.graph.sym(at);
                free() && (owner.is_some() || symbol.qualified_name == symbol.name)
            }
            Shape::Member => owner.is_some(),
            Shape::Own(reach) => self.is_own_member(reach, at),
            Shape::Inherited(reach) => {
                owner.is_some_and(|owner| self.is_inherited_member(reach, at, owner))
            }
            Shape::FreeOrInherited(reach) => {
                free() || owner.is_some_and(|owner| self.is_inherited_member(reach, at, owner))
            }
            Shape::FreeOrEnclosing(own) => {
                free() || owner.is_some_and(|owner| self.chain_encloses(owner, *own))
            }
        }
    }

    fn is_inherited_member(&self, reach: &Reach, at: SymbolAt, owner: Str) -> bool {
        reach.ancestors.as_deref().is_some_and(|up| {
            up.owners.contains(&owner)
                && reach
                    .nearest_base
                    .is_none_or(|nearest| nearest == owner || !up.derives_from(nearest, owner))
        }) && (self.conventions.owner_chain_separator.is_none()
            || self.member_separator(at) == self.member_separator(reach.caller)
            || self
                .qualified(owner)
                .is_none_or(|ty| self.graph.sym(ty).kind != SymbolKind::Class))
    }

    fn chain_encloses(&self, outer: Str, inner: Str) -> bool {
        let Some(separator) = self.conventions.owner_chain_separator else {
            return false;
        };
        let strings = &self.graph.strings;
        strings
            .resolve(inner)
            .strip_prefix(strings.resolve(outer))
            .is_some_and(|rest| rest.starts_with(separator))
    }

    fn member_owner(&self, ty: SymbolAt) -> Str {
        let symbol = self.graph.sym(ty);
        match self.conventions.owner_chain_separator {
            Some(_) => symbol.qualified_name,
            None => symbol.name,
        }
    }

    pub(crate) fn base_owner(&self, child: SymbolAt, written: &str) -> Option<Str> {
        let Some(separator) = self.conventions.owner_chain_separator else {
            return self.graph.strings.get(last_segment(written));
        };
        let enclosing = self.graph.enclosing_scope(child, separator);
        let (mut scope, name) = written
            .strip_prefix(separator)
            .map_or((enclosing, written), |rooted| (None, rooted));

        loop {
            let Some(at) = scope else {
                return self.type_named(name);
            };
            if let Some(owner) = self.type_named(&format!("{at}{separator}{name}")) {
                return Some(owner);
            }
            scope = at.rsplit_once(separator).map(|(outer, _)| outer);
        }
    }

    fn type_named(&self, qualified: &str) -> Option<Str> {
        let qualified = self.graph.strings.get(qualified)?;
        let at = self.qualified(qualified)?;
        is_type(self.graph.sym(at).kind).then_some(qualified)
    }

    fn located_types(
        &self,
        name: Str,
        file: u32,
        matches: impl Fn(SymbolAt) -> bool,
    ) -> Vec<SymbolAt> {
        let types: Vec<SymbolAt> = self
            .named(name)
            .iter()
            .copied()
            .filter(|&at| derives(self.graph.sym(at).kind) && matches(at))
            .collect();
        if types.is_empty() {
            return types;
        }
        let distance = self.distance_from(file);
        let nearest = types.iter().map(|&at| distance(at)).min();
        types
            .into_iter()
            .filter(|&at| Some(distance(at)) == nearest)
            .collect()
    }

    pub(crate) fn distance_from(&self, file: u32) -> impl Fn(SymbolAt) -> u8 + '_ {
        let included = self.closure(file);
        let dir = self.graph.file(file).dir;
        move |at| {
            if at.file == file {
                0
            } else if included.binary_search(&at.file).is_ok() {
                1
            } else if self.graph.file(at.file).dir == dir {
                2
            } else {
                3
            }
        }
    }

    fn located_base_types(&self, base: DeclaredBase, child: SymbolAt) -> Vec<SymbolAt> {
        let strings = &self.graph.strings;
        let written = strings.resolve(base.written);
        let qualified_as = |qualified: &str| -> Vec<SymbolAt> {
            let Some(qualified) = strings.get(qualified) else {
                return Vec::new();
            };
            self.located_types(base.owner, child.file, |ty| {
                self.graph.sym(ty).qualified_name == qualified
            })
        };
        if base.written_from_root {
            return qualified_as(written);
        }

        let Some(separator) = self.conventions.scope_separator else {
            return self.located_types(base.owner, child.file, |_| true);
        };
        let mut scope = self.graph.enclosing_scope(child, separator);
        while let Some(at) = scope {
            let found = qualified_as(&format!("{at}{separator}{written}"));
            if !found.is_empty() {
                return found;
            }
            scope = at.rsplit_once(separator).map(|(outer, _)| outer);
        }
        if written.contains(separator) {
            qualified_as(written)
        } else {
            self.located_types(base.owner, child.file, |_| true)
        }
    }

    fn declared_bases(&self, child: SymbolAt) -> Vec<DeclaredBase> {
        self.graph
            .file(child.file)
            .parents_of(child.index)
            .iter()
            .filter_map(|parent| {
                let written = self.graph.strings.resolve(parent.written);
                Some(DeclaredBase {
                    owner: self.base_owner(child, written)?,
                    written: parent.written,
                    written_from_root: parent.written_from_root,
                })
            })
            .collect()
    }

    fn ancestors_of(&self, own: Str, caller_type: Option<SymbolAt>) -> Option<Rc<Ancestors>> {
        if self.conventions.inherited_members == InheritedMembers::Unreached {
            return None;
        }
        caller_type.map_or_else(|| self.owner_ancestors(own), |ty| self.type_ancestors(ty))
    }

    /// Whether a base the type declares, or a base of one of those, is a class of the owner's name.
    pub(crate) fn has_ancestor(&self, ty: SymbolAt, owner: Str) -> bool {
        self.type_ancestors(ty)
            .is_some_and(|up| up.owners.contains(&owner))
    }

    fn type_ancestors(&self, ty: SymbolAt) -> Option<Rc<Ancestors>> {
        derived(&self.type_ancestors, ty, || {
            self.walk_type_ancestors(ty).map(Rc::new)
        })
    }

    fn walk_type_ancestors(&self, ty: SymbolAt) -> Option<Ancestors> {
        if self.declared_bases(ty).is_empty() {
            return None;
        }
        let mut walk = TypeWalk {
            ty,
            own: self.member_owner(ty),
            found: Ancestors::default(),
            walked: HashSet::with_hasher(FixedState::default()),
            pending: vec![ty],
        };
        walk.walked.insert(ty);
        while let Some(next) = walk.pending.pop() {
            for base in self.declared_bases(next) {
                self.walk_base(&mut walk, next, base);
            }
        }
        Some(walk.found)
    }

    fn walk_base(&self, walk: &mut TypeWalk, next: SymbolAt, base: DeclaredBase) {
        let qualified = |at: SymbolAt| self.graph.sym(at).qualified_name;
        let located = self.located_base_types(base, next);
        let names_another_type = located.is_empty()
            && self
                .named(base.owner)
                .iter()
                .any(|&at| derives(self.graph.sym(at).kind));
        let located_others: Vec<SymbolAt> = located
            .iter()
            .copied()
            .filter(|&at| base.owner != walk.own || qualified(at) != qualified(walk.ty))
            .collect();
        if (base.owner != walk.own || !located_others.is_empty()) && !names_another_type {
            walk.found.owners.insert(base.owner);
            walk.found
                .bases_of
                .entry(self.member_owner(next))
                .or_default()
                .insert(base.owner);
            for &at in &located_others {
                walk.found.homes.insert((base.owner, self.home(at)));
                agree(
                    &mut walk.found.qualified_per_base,
                    base.owner,
                    Some(qualified(at)),
                );
            }
        }
        let walked = &mut walk.walked;
        walk.pending
            .extend(located.into_iter().filter(|&at| walked.insert(at)));
    }

    fn owner_ancestors(&self, own: Str) -> Option<Rc<Ancestors>> {
        derived(&self.owner_ancestors, own, || {
            match self.conventions.owner_chain_separator {
                Some(_) => self.chain_ancestors(own),
                None => self.union_of_type_ancestors(own),
            }
            .map(Rc::new)
        })
    }

    fn union_of_type_ancestors(&self, own: Str) -> Option<Ancestors> {
        let mut union: Option<Ancestors> = None;
        for &ty in self.named(own) {
            let Some(up) = self.type_ancestors(ty) else {
                continue;
            };
            let union = union.get_or_insert_with(Ancestors::default);
            union.owners.extend(&up.owners);
            union.homes.extend(&up.homes);
            for (&base, &qualified) in &up.qualified_per_base {
                agree(&mut union.qualified_per_base, base, qualified);
            }
        }
        union
    }

    fn chain_ancestors(&self, own: Str) -> Option<Ancestors> {
        let first = self.chain_bases(own);
        if first.is_empty() {
            return None;
        }
        let mut found = Ancestors::default();
        let mut pending = first;
        while let Some(next) = pending.pop() {
            if next != own && found.owners.insert(next) {
                pending.extend(self.chain_bases(next));
            }
        }
        Some(found)
    }

    fn chain_bases(&self, owner: Str) -> Vec<Str> {
        let Some(separator) = self.conventions.owner_chain_separator else {
            return Vec::new();
        };
        let written = self.graph.strings.resolve(owner);
        let last = written.rsplit(separator).next().unwrap_or(written);
        let Some(last) = self.graph.strings.get(last) else {
            return Vec::new();
        };
        self.named(last)
            .iter()
            .copied()
            .filter(|&at| self.graph.sym(at).qualified_name == owner)
            .flat_map(|at| self.declared_bases(at))
            .map(|base| base.owner)
            .collect()
    }

    pub(crate) fn shape(
        &self,
        call: &Call,
        candidates: &(impl Iterator<Item = SymbolAt> + Clone),
        reach: Option<&Reach>,
    ) -> Shape {
        let any = |shape: &Shape| candidates.clone().any(|at| self.admits(shape, at));

        if call.receiver.is_some() {
            return if self.conventions.member_calls == MemberCalls::Members
                || call.names_a_runtime_method
                || any(&Shape::Member)
            {
                Shape::Member
            } else {
                Shape::FreeFunction
            };
        }
        if call.written_from_root {
            return Shape::Global;
        }

        let own = reach.and_then(|reach| {
            let own = Shape::Own(reach.clone());
            if any(&own) {
                return Some(own);
            }
            let inherited = Shape::Inherited(reach.clone());
            let up = reach.ancestors.as_deref().filter(|_| any(&inherited))?;
            let found_first = || {
                !reach.may_own_name
                    && candidates
                        .clone()
                        .filter(|&at| self.admits(&inherited, at))
                        .all(|at| self.declares_inherited(up, at))
            };
            match self.conventions.inherited_members {
                InheritedMembers::Found if found_first() => Some(inherited),
                InheritedMembers::Found | InheritedMembers::Admitted => {
                    Some(Shape::FreeOrInherited(reach.clone()))
                }
                InheritedMembers::Unreached => None,
            }
        });
        match (own, reach) {
            (Some(shape), Some(reach))
                if self.implicit_receiver()
                    || (!any(&Shape::Free)
                        && (self.bare_call_reaches_own_type() || self.in_own_namespace(reach))) =>
            {
                shape
            }
            (_, Some(reach)) if self.conventions.bare_call_reaches_enclosing_owners => {
                Shape::FreeOrEnclosing(reach.own)
            }
            _ => Shape::Free,
        }
    }

    fn in_own_namespace(&self, reach: &Reach) -> bool {
        let strings = &self.graph.strings;
        let own = strings.resolve(reach.own);
        let separator = self.conventions.owner_chain_separator;
        let chain_ends = std::iter::successors(Some(own), |chain| {
            separator
                .and_then(|separator| chain.split_once(separator))
                .map(|(_, end)| end)
        });
        chain_ends
            .filter_map(|end| strings.get(end))
            .flat_map(|end| self.named(end))
            .any(|&at| {
                let namespace = self.graph.sym(at);
                namespace.kind == SymbolKind::Namespace
                    && namespace.qualified_name == reach.own
                    && self.in_body(self.body(at), reach.caller)
            })
    }

    pub(crate) fn scoped_target(
        &self,
        call: &Call,
        candidates: impl Iterator<Item = SymbolAt> + Clone,
        reach: Option<&Reach>,
    ) -> Option<SymbolAt> {
        match self.scoped(call, candidates, reach) {
            Scoped::One(at) => Some(at),
            Scoped::Nothing | Scoped::Several => None,
        }
    }

    pub(crate) fn scoped(
        &self,
        call: &Call,
        candidates: impl Iterator<Item = SymbolAt> + Clone,
        reach: Option<&Reach>,
    ) -> Scoped {
        let targets = self.call_targets(call.caller_at(), candidates);
        let shape = match self.shape(call, &targets, reach) {
            Shape::FreeOrInherited(_) | Shape::FreeOrEnclosing(_) => Shape::Free,
            shape => shape,
        };
        let mut admitted = targets.filter(|&at| self.admits(&shape, at));
        let Some(first) = admitted.next() else {
            return Scoped::Nothing;
        };
        let owner = self.owner(first);
        if !admitted.clone().all(|at| self.owner(at) == owner) {
            return Scoped::Several;
        }

        Scoped::One(match &shape {
            Shape::Own(reach) if reach.own_type_body.is_some_and(|home| !home.alone_in_file) => {
                std::iter::once(first)
                    .chain(admitted)
                    .find(|&at| self.declares_own(reach, at))
                    .unwrap_or(first)
            }
            _ => first,
        })
    }

    pub(crate) fn one_callable(
        &self,
        shape: &Shape,
        candidates: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        let mut admitted = candidates.clone().filter(|&at| self.admits(shape, at));
        let first = admitted.next()?;
        let lookup_finds_first = self.implicit_receiver()
            && match shape {
                Shape::Own(reach) => admitted
                    .clone()
                    .chain([first])
                    .all(|at| self.declares_own(reach, at)),
                Shape::Inherited(_) => true,
                _ => false,
            };

        if lookup_finds_first {
            self.is_one_counted(shape, first, admitted)
        } else {
            self.is_one_counted(shape, first, candidates)
        }
        .then_some(first)
    }

    fn is_one_counted(
        &self,
        shape: &Shape,
        first: SymbolAt,
        counted: impl Iterator<Item = SymbolAt> + Clone,
    ) -> bool {
        let constructs = !matches!(shape, Shape::Member);
        let types = counted.clone();
        let counted_type_of = move |at: SymbolAt| {
            let symbol = self.graph.sym(at);
            (constructs && symbol.kind == SymbolKind::Method && symbol.owner == Some(symbol.name))
                .then(|| types.clone().find(|&ty| self.constructs(at, ty)))
                .flatten()
        };

        self.is_one_callable(
            counted_type_of(first).unwrap_or(first),
            counted.filter(|&at| counted_type_of(at).is_none()),
        )
    }

    fn named_receiver(&self, call: &Call) -> Option<Str> {
        call.receiver
            .filter(|&receiver| !self.graph.strings.resolve(receiver).is_empty())
    }

    // constraint: owners of one name in several directories, a package's own type and a copy of it elsewhere, are told
    // apart by nearness: the caller's file, then its directory, then the first in path order
    fn nearest(&self, call: &Call, owned: &[SymbolAt]) -> Option<SymbolAt> {
        let dir = self.graph.file(call.file).dir;
        owned
            .iter()
            .find(|at| at.file == call.file)
            .or_else(|| owned.iter().find(|at| self.graph.file(at.file).dir == dir))
            .or_else(|| owned.first())
            .copied()
    }

    pub(crate) fn member_of_the_receiver_s_owner(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> Option<SymbolAt> {
        let receiver = self.named_receiver(call)?;
        if let Some(at) = self.nearest(call, &candidates.with_owner(self.graph, receiver)) {
            return Some(at);
        }
        let separator = self.conventions.owner_chain_separator?;
        let mut ending = candidates.targets().iter().copied().filter(|&at| {
            self.owner(at)
                .is_some_and(|owner| self.chain_ends_in(owner, receiver, separator))
        });
        let first = ending.next()?;
        let owner = self.owner(first);
        ending.all(|at| self.owner(at) == owner).then_some(first)
    }

    pub(crate) fn member_of_the_caller_s_owner(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> Option<Bound> {
        self.named_receiver(call)?;
        let caller_owner = self
            .graph
            .file(call.file)
            .symbols
            .get(call.caller as usize)?
            .owner?;
        let at = *candidates.with_owner(self.graph, caller_owner).first()?;
        Some((at, confidence::SAME_FILE))
    }

    fn chain_ends_in(&self, owner: Str, receiver: Str, separator: &str) -> bool {
        let strings = &self.graph.strings;
        strings
            .resolve(owner)
            .strip_suffix(strings.resolve(receiver))
            .is_some_and(|chain| chain.ends_with(separator))
    }

    pub(crate) fn resolve_by_proximity(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
        reach: Option<&Reach>,
    ) -> Option<Bound> {
        if self.conventions.calls_on_values == CallsOnValues::NoNearestGuess
            && call
                .receiver
                .is_some_and(|receiver| self.graph.strings.resolve(receiver).is_empty())
        {
            return None;
        }
        let caller_file = self.graph.file(call.file);
        let same_dir = candidates.in_dir(self.graph, caller_file.dir);
        let (nearby, confidence) = if same_dir.is_empty() {
            let parent_dir = candidates.in_parent_dir(self.graph, caller_file.parent_dir);
            if parent_dir.is_empty() {
                (candidates.first_party(self.graph), confidence::FIRST_PARTY)
            } else {
                (parent_dir, confidence::PARENT_DIR)
            }
        } else {
            (same_dir, confidence::SAME_DIR)
        };
        let nearby = nearby.iter().copied();
        let shape = if call.receiver.is_some() && confidence == confidence::FIRST_PARTY {
            Shape::Member
        } else {
            self.shape(call, &nearby, reach)
        };
        self.one_callable(&shape, nearby.clone())
            .or_else(|| self.constructor_of_a_type_outside(&shape, &nearby))
            .map(|at| (at, confidence))
    }

    fn constructor_of_a_type_outside(
        &self,
        shape: &Shape,
        nearby: &(impl Iterator<Item = SymbolAt> + Clone),
    ) -> Option<SymbolAt> {
        if !matches!(
            shape,
            Shape::Free
                | Shape::FreeFunction
                | Shape::Global
                | Shape::FreeOrInherited(_)
                | Shape::FreeOrEnclosing(_)
        ) {
            return None;
        }
        let outside = |at: SymbolAt| {
            self.constructor_types(at)
                .is_some_and(|types| !nearby.clone().any(|other| types.contains(&other)))
        };
        let mut constructors = nearby.clone();
        let first = constructors.next()?;
        (nearby.clone().all(outside) && self.is_one_callable(first, constructors)).then_some(first)
    }
}

pub(crate) const fn is_type(kind: SymbolKind) -> bool {
    derives(kind) || matches!(kind, SymbolKind::Namespace)
}

pub(crate) const fn is_value(kind: SymbolKind) -> bool {
    matches!(kind, SymbolKind::Variable | SymbolKind::Constant)
}
