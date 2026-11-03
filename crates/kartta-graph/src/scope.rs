use std::borrow::Cow;
use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{
    BareReceivers, CallsOnValues, InheritedMembers, MemberCalls, ParentSpelling, ReceiverMembers,
    SymbolKind, UnlistedReceivers, UntypedOwners, ValueUses,
};

use crate::candidates::Candidates;
use crate::confidence;
use crate::graph::{QUALIFIER_SEPARATORS, agree, distinct, last_segment, tails};
use crate::resolution::Miss;
use crate::resolve::{Bound, Call, Cascade, derived, derives};
use crate::stored::{Parent, SymbolAt, VALUE_TYPE_SEPARATOR};
use crate::strings::Str;
use crate::twins::{Twin, kept};

/// The distance [`Cascade::distance_from`] gives a definition in a file that the file it measures from neither is,
/// includes nor shares a directory with.
const FAR: u8 = 3;

/// The distance [`Cascade::distance_from`] gives a definition in a file that the file it measures from includes.
const IMPORTED: u8 = 1;

/// What a set of candidates offers a call: no target its shape admits, one, or several on different owners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scoped {
    Nothing,
    One(SymbolAt),
    Several,
}

/// What the class a member call's operand is declared as gives the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerMember {
    /// The member of a class of that name the caller's file reaches, or of the base of that class that declares it.
    Member(SymbolAt),
    /// Classes of that name in several scopes hold the member nearest the caller's file, and the receiver's spelling
    /// names none of them, or bases of the class that neither derives from declare it.
    Ambiguous,
    /// No class of that name that the caller's file reaches holds the member, nor a base the graph locates: the name is
    /// a type alias of no class the graph holds, a class of another project, or a class whose base lies outside the
    /// graph, so the call resolves as one on an operand of no declared class.
    Elsewhere,
    /// The language has no nested scopes, or the call names no receiver.
    NotDeclared,
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
        let mut walked: HashSet<Str, FixedState> = HashSet::default();
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
        let named = candidates.named;
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
        let mut reached: HashSet<Str, FixedState> = HashSet::default();
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

    /// The class whose body declares `at`, a member, read through a declaration where the member is defined outside
    /// its class, else the one class of its owner's name nearest its file; `None` where several tie.
    pub(crate) fn class_of(&self, at: SymbolAt) -> Option<SymbolAt> {
        let owner = self.owner(at)?;
        self.body_holding(owner, self.position(at))
            .or_else(|| {
                self.declarations(at)
                    .iter()
                    .find_map(|&declaration| self.body_holding(owner, self.position(declaration)))
            })
            .or_else(|| {
                self.folded_lines(at)
                    .find_map(|line| self.body_holding(owner, (at.file, line)))
            })
            .or_else(|| match self.located_types(owner, at.file, |_| true)[..] {
                [one] => Some(one),
                _ => None,
            })
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

    /// Whether `at` is a member of the kind a receiver reaches first: a class's own method for a receiver on the class,
    /// and any other member for any other receiver. Every member is, where the language keeps no class's own methods
    /// apart.
    pub(crate) fn is_of_the_kind(&self, on_a_class: bool, at: SymbolAt) -> bool {
        self.conventions
            .class_member_separator
            .is_none_or(|separator| (self.member_separator(at) == Some(separator)) == on_a_class)
    }

    /// Of `owned`, the first member of the kind a receiver reaches first, else the first.
    pub(crate) fn first_of_the_kind(
        &self,
        on_a_class: bool,
        mut owned: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        owned
            .clone()
            .find(|&at| self.is_of_the_kind(on_a_class, at))
            .or_else(|| owned.next())
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
        if self.is_unbound_namespace(at) {
            return false;
        }
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

    /// What a base written in `child`'s scope names: where a language joins an owner chain, the qualified name of the
    /// symbol of a kind `base_kind` admits that the lookup from that scope outward reaches first; elsewhere the base's
    /// last segment, whatever it names.
    pub(crate) fn base_owner(
        &self,
        child: SymbolAt,
        written: &str,
        base_kind: fn(SymbolKind) -> bool,
    ) -> Option<Str> {
        let Some(separator) = self.conventions.owner_chain_separator else {
            return self.graph.strings.get(last_segment(written));
        };
        let enclosing = self.graph.enclosing_scope(child, separator);
        let (mut scope, name) = written
            .strip_prefix(separator)
            .map_or((enclosing, written), |rooted| (None, rooted));

        loop {
            let Some(at) = scope else {
                return self.base_named(name, base_kind);
            };
            if let Some(owner) = self.base_named(&format!("{at}{separator}{name}"), base_kind) {
                return Some(owner);
            }
            scope = at.rsplit_once(separator).map(|(outer, _)| outer);
        }
    }

    fn base_named(&self, qualified: &str, base_kind: fn(SymbolKind) -> bool) -> Option<Str> {
        let qualified = self.graph.strings.get(qualified)?;
        let at = self.qualified(qualified)?;
        base_kind(self.graph.sym(at).kind).then_some(qualified)
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
                FAR
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
                    owner: self.base_owner(child, written, hands_members_on)?,
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
            walked: HashSet::default(),
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

    pub(crate) fn owner_derives_from(&self, own: Str, owner: Str) -> bool {
        own == owner
            || self
                .owner_ancestors(own)
                .is_some_and(|up| up.owners.contains(&owner))
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
            return if self.takes_a_member_alone(call)
                || call.names_a_runtime_method
                || (!self.through_a_module(call) && any(&Shape::Member))
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
                _ => None,
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

    // constraint: a type of that name the caller's file neither holds nor includes is a namesake, as a minified bundle's
    // `class $` is of jQuery's `$`, and in generated code a name says nothing of what it holds
    fn receiver_names_a_type(&self, call: &Call) -> bool {
        let strings = &self.graph.strings;
        let written = call
            .receiver
            .map_or("", |receiver| strings.resolve(receiver));
        if written.is_empty() || self.graph.flags(call.caller_at()).is_generated() {
            return false;
        }
        let last = (self.conventions.owner_chain_separator)
            .and_then(|separator| written.rsplit(separator).next())
            .unwrap_or(written);
        strings.get(last).is_some_and(|last| {
            (self.named(last).iter())
                .any(|&at| self.graph.sym(at).kind.is_type() && self.sees(call.file, at))
        })
    }

    fn takes_a_member_alone(&self, call: &Call) -> bool {
        match self.conventions.member_calls {
            MemberCalls::NearbyFreeFunctionsUnlessTyped => self.receiver_names_a_type(call),
            MemberCalls::Members => true,
            MemberCalls::ModulePaths => !self.receiver_names_a_module(call),
            _ => false,
        }
    }

    /// The path a call through a module spells, where only a module's path reaches a free function and the language
    /// joins an owner chain.
    pub(crate) fn module_path(&self, call: &Call) -> Option<&str> {
        self.conventions.owner_chain_separator?;
        let receiver = call.receiver.filter(|_| self.through_a_module(call))?;
        Some(self.graph.strings.resolve(receiver))
    }

    // constraint: a path through a module names a member, or an item of the caller's own file, only where the item's
    // qualified name ends with the path, as an inline module's `m::f` does for `m::f()`; another member of the name, or
    // another item of the name in that file, is a namesake the path does not reach
    pub(crate) fn path_names(&self, (call, path): (&Call, Option<&str>), at: SymbolAt) -> bool {
        let (Some(path), Some(separator)) = (path, self.conventions.owner_chain_separator) else {
            return true;
        };
        let symbol = self.graph.sym(at);
        if symbol.owner.is_none() && at.file != call.file {
            return true;
        }
        let strings = &self.graph.strings;
        (strings.resolve(symbol.qualified_name))
            .strip_suffix(strings.resolve(symbol.name))
            .and_then(|scope| scope.strip_suffix(separator))
            .and_then(|scope| scope.strip_suffix(path))
            .is_some_and(|before| before.is_empty() || before.ends_with(separator))
    }

    // constraint: where only a module's path reaches a free function, a call written without a path reaches a free item
    // of an inline module of its own file only from inside that module's body, as Rust's `rng()` outside `mod test`
    // does not reach `test::rng`; a member it reaches through a value of its type wherever the type is in scope, so
    // only a member of a private type of a module the call is outside of is out of its reach, and such a member still
    // leaves the call ambiguous, since the value's type is unknown
    /// What a call binds among the items of its own file, `in_file`: the one its path names and it reaches.
    pub(crate) fn same_file_target(
        &self,
        path: (&Call, Option<&str>),
        in_file: &[SymbolAt],
        reach: Option<&Reach>,
    ) -> Option<SymbolAt> {
        let line = self.line_without_a_path(path);
        let reached = (in_file.iter().copied()).filter(|&at| {
            self.path_names(path, at)
                && line.is_none_or(|line| self.owner(at).is_some() || self.held_around(at, line))
        });
        let at = self.scoped_target(path.0, reached, reach)?;
        line.is_none_or(|line| self.member_in_reach(at, line))
            .then_some(at)
    }

    /// The line a call written without a path stands on where only a module's path reaches a free function, its
    /// caller's for a call with no line of its own.
    fn line_without_a_path(&self, (call, path): (&Call, Option<&str>)) -> Option<u32> {
        if self.conventions.member_calls != MemberCalls::ModulePaths || path.is_some() {
            return None;
        }
        Some(match call.line {
            0 => (self.graph.file(call.file).symbols)
                .get(call.caller as usize)
                .map_or(0, |caller| caller.line),
            line => line,
        })
    }

    /// Whether every inline module around `at` holds `line` of its file.
    pub(crate) fn held_around(&self, at: SymbolAt, line: u32) -> bool {
        let (Some(separator), Some(scope)) = (
            self.conventions.owner_chain_separator,
            self.graph.qualifier(at),
        ) else {
            return true;
        };
        let strings = &self.graph.strings;
        (scope
            .split(separator)
            .filter_map(|segment| strings.get(segment)))
        .flat_map(|segment| self.graph.in_file(self.named(segment), at.file))
        .all(|&module| {
            let home = self.body(module);
            self.graph.sym(module).kind != SymbolKind::Namespace
                || !self.in_body(home, at)
                || home.contains(at.file, line)
        })
    }

    /// Whether a call on `line` reaches the member `at` through a value: not where its owner is a private type of a
    /// module that does not hold the line.
    fn member_in_reach(&self, at: SymbolAt, line: u32) -> bool {
        let (Some(owner), Some(separator)) =
            (self.owner(at), self.conventions.owner_chain_separator)
        else {
            return true;
        };
        let strings = &self.graph.strings;
        let written = strings.resolve(owner);
        let Some(name) = written
            .rsplit(separator)
            .next()
            .and_then(|name| strings.get(name))
        else {
            return true;
        };
        (self.graph.in_file(self.named(name), at.file).iter()).all(|&ty| {
            let symbol = self.graph.sym(ty);
            symbol.exported()
                || !symbol.kind.is_type()
                || symbol.qualified_name != owner
                || self.held_around(ty, line)
        })
    }

    // constraint: where only a module's path reaches a free function, a path through a module reaches a member only
    // through the owner its receiver names, which the receiver's tiers bind, so it is a call of a free function
    fn through_a_module(&self, call: &Call) -> bool {
        self.conventions.member_calls == MemberCalls::ModulePaths
            && self.receiver_names_a_module(call)
    }

    fn receiver_names_a_module(&self, call: &Call) -> bool {
        let Some(receiver) = self.named_receiver(call) else {
            return false;
        };
        let receiver = self.graph.strings.resolve(receiver);
        let last = self
            .conventions
            .owner_chain_separator
            .and_then(|separator| receiver.rsplit(separator).next())
            .unwrap_or(receiver);
        last.starts_with(|first: char| first.is_ascii_lowercase() || first == '_')
            && !self
                .conventions
                .own_type_receivers
                .is_some_and(|own| own.contains(&receiver))
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
        let targets = self.call_targets(call, candidates);
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
            return self.one_owner_reached_by_name(call, std::iter::once(first).chain(admitted));
        }

        Scoped::One(match &shape {
            Shape::Own(reach) if reach.own_type_body.is_some_and(|home| !home.alone_in_file) => {
                std::iter::once(first)
                    .chain(admitted)
                    .find(|&at| self.declares_own(reach, at))
                    .unwrap_or(first)
            }
            _ if call.receiver.is_some() => std::iter::once(first)
                .chain(admitted)
                .find(|&at| self.is_of_the_kind(call.on_a_class, at))
                .unwrap_or(first),
            _ => first,
        })
    }

    // constraint: only a tie between owners drops a table's member, since an importer can bind the table under
    // another name, `let sheets = require(...)`, and still reach the member where no other owner offers one
    fn one_owner_reached_by_name(
        &self,
        call: &Call,
        admitted: impl Iterator<Item = SymbolAt>,
    ) -> Scoped {
        let (UntypedOwners::Tables, Some(receiver)) =
            (self.conventions.untyped_owners, call.receiver)
        else {
            return Scoped::Several;
        };
        let own = self
            .graph
            .file(call.file)
            .symbols
            .get(call.caller as usize)
            .and_then(|caller| caller.owner);
        let mut reached = admitted.filter(|&at| {
            self.owner(at).is_some_and(|owner| {
                own == Some(owner)
                    || self.spells(receiver, owner)
                    || self
                        .named(owner)
                        .iter()
                        .any(|&ty| self.graph.sym(ty).kind.is_type())
            })
        });
        let Some(first) = reached.next() else {
            return Scoped::Several;
        };
        let owner = self.owner(first);
        if reached.all(|at| self.owner(at) == owner) {
            Scoped::One(first)
        } else {
            Scoped::Several
        }
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

    pub(crate) fn named_receiver(&self, call: &Call) -> Option<Str> {
        call.receiver
            .filter(|&receiver| !self.graph.strings.resolve(receiver).is_empty())
    }

    // constraint: owners of one name in several directories, a package's own type and a copy of it elsewhere, are told
    // apart by nearness: the caller's file, then its directory, then its package, as two versions of one crate in a
    // cargo registry are, then the first in path order; where packages are directories the directory is the package,
    // and a package of the same declared name elsewhere is another one; one class reopened in several files holds both
    // kinds of member where any of its bodies does, so the kind the receiver reaches first is told apart before
    // nearness
    fn nearest(
        &self,
        call: &Call,
        owned: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        let of_the_kind = owned
            .clone()
            .filter(|&at| self.is_of_the_kind(call.on_a_class, at));
        self.nearest_of(call, of_the_kind)
            .or_else(|| self.nearest_of(call, owned))
    }

    fn nearest_of(
        &self,
        call: &Call,
        mut owned: impl Iterator<Item = SymbolAt> + Clone,
    ) -> Option<SymbolAt> {
        let caller = self.graph.file(call.file);
        owned
            .clone()
            .find(|at| at.file == call.file)
            .or_else(|| {
                owned
                    .clone()
                    .find(|at| self.graph.file(at.file).dir == caller.dir)
            })
            .or_else(|| {
                let package = caller
                    .package
                    .filter(|_| !self.conventions.directory_packages)?;
                owned
                    .clone()
                    .find(|at| self.graph.file(at.file).package == Some(package))
            })
            .or_else(|| owned.next())
    }

    // constraint: where scopes nest, as C++ namespaces do, the class an operand is declared as is one its file reaches
    // through its includes, the nearest first: its own file, the files it includes, then the files those reach, since a
    // class of that name in another namespace comes in deeper; a class of that name only elsewhere belongs to another
    // project, so its member is not the one called; of namesakes equally near, the one the receiver's spelling names
    // binds, the first of a template and its specializations alike
    pub(crate) fn member_of_a_reached_owner(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> OwnerMember {
        let (Some(_), Some(receiver)) =
            (self.conventions.scope_separator, self.named_receiver(call))
        else {
            return OwnerMember::NotDeclared;
        };
        let written = self.graph.strings.resolve(receiver);
        if let Some(field) = field_spelled(written) {
            return self
                .class_of_own_field(call, field)
                .map_or(OwnerMember::Elsewhere, |class| {
                    self.member_of_a_reached_class(call, candidates, class)
                });
        }
        let Some(class) = self.bare_owner(receiver) else {
            return OwnerMember::Elsewhere;
        };
        self.member_of_a_reached_class(call, candidates, (class, written))
    }

    // constraint: C++ finds a name a member's body does not declare in the member's class before any namespace, then in
    // its bases, so a field of the caller's class, else of a class it derives from, that the caller's file reaches is
    // the operand, and the class its declaration names behind as many pointers as the call goes through holds the
    // member called; reached fields that name different classes leave the call to the tiers that read no class, as a
    // name no field answers does
    fn class_of_own_field<'s>(
        &'s self,
        call: &Call,
        (pointers, name): (usize, &str),
    ) -> Option<(Str, &'s str)> {
        let strings = &self.graph.strings;
        let name = strings.get(name)?;
        let own = self
            .graph
            .file(call.file)
            .symbols
            .get(call.caller as usize)?
            .owner?;
        let named = self.named(name);
        let own_fields = kept(named, |&at| self.is_field_of(at, |owner| owner == own));
        let fields = if own_fields.is_empty() {
            self.nearest_base_fields(own, named)?
        } else {
            own_fields
        };
        let tier = self.reached_tier(call.file, &fields);
        let mut declared = tier.iter().map(|at| {
            let types = self.graph.file(at.file).value_types(at.index)?;
            strings
                .resolve(types)
                .split(VALUE_TYPE_SEPARATOR)
                .find_map(|spelled| {
                    let class = spelled.trim_start_matches('*');
                    (spelled.len() - class.len() == pointers).then_some(class)
                })
        });
        let class = declared.next()??;
        if !declared.all(|other| other == Some(class)) {
            return None;
        }
        let bare = self
            .conventions
            .scope_separator
            .and_then(|separator| class.rsplit_once(separator))
            .map_or(class, |(_, bare)| bare);
        Some((strings.get(bare)?, class))
    }

    fn is_field_of(&self, at: SymbolAt, owned: impl Fn(Str) -> bool) -> bool {
        let symbol = self.graph.sym(at);
        symbol.kind == SymbolKind::Field && symbol.owner.is_some_and(owned)
    }

    // constraint: a base's field hides a field of the same name further up, and fields of one name in two bases of one
    // distance make the name ambiguous, which C++ rejects rather than picking one
    /// The fields among `named` that the nearest bases of `own` declare, where one base declares them all.
    fn nearest_base_fields<'n>(
        &self,
        own: Str,
        named: &'n [SymbolAt],
    ) -> Option<Cow<'n, [SymbolAt]>> {
        if self.conventions.inherited_members == InheritedMembers::Unreached
            || !named
                .iter()
                .any(|&at| self.graph.sym(at).kind == SymbolKind::Field)
        {
            return None;
        }
        let mut walked: HashSet<Str, FixedState> = HashSet::default();
        walked.insert(own);
        let mut level = vec![own];
        while !level.is_empty() {
            let bases: Vec<Str> = level
                .iter()
                .flat_map(|&owner| self.named(owner).iter().copied())
                .filter(|&ty| derives(self.graph.sym(ty).kind))
                .flat_map(|ty| self.declared_bases(ty))
                .map(|base| base.owner)
                .filter(|&base| walked.insert(base))
                .collect();
            let fields = kept(named, |&at| {
                self.is_field_of(at, |owner| bases.contains(&owner))
            });
            if let Some(&first) = fields.first() {
                let owner = self.owner(first);
                return fields
                    .iter()
                    .all(|&at| self.owner(at) == owner)
                    .then_some(fields);
            }
            level = bases;
        }
        None
    }

    /// The member of the class of the bare name `class` that the call's file reaches, of namesakes the one `written`
    /// names, where an alias that `written` names stands for the class it names.
    fn member_of_a_reached_class(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
        (class, written): (Str, &str),
    ) -> OwnerMember {
        let aliased = self.through_aliases(call, written);
        let (class, spelled) = aliased
            .as_ref()
            .map_or((class, written), |(aliased, spelled)| {
                (*aliased, spelled.as_str())
            });
        let owned = candidates.with_owner(self.graph, class);
        let tier = self.reached_tier(call.file, &owned);
        match self.twin(call.caller_at(), spelled, &tier) {
            Some(Twin::Member(at) | Twin::Namesake(at)) => OwnerMember::Member(at),
            Some(Twin::Unnamed(_)) => OwnerMember::Ambiguous,
            None => self.member_of_a_base(call, candidates, (class, spelled)),
        }
    }

    // constraint: C++ looks a member the class does not declare up in its bases, as it does for a bare call in a
    // member: a nearer base's member hides a farther one's, and members of two bases neither derives from leave the
    // call ambiguous; a namesake of a base outside the base's body is another class's member, and a base the graph
    // locates only through a guess leaves the call to the tiers that read no class
    /// The member the bases of the class `written` names declare, where the call's file reaches the class.
    fn member_of_a_base(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
        (class, written): (Str, &str),
    ) -> OwnerMember {
        if self.conventions.inherited_members != InheritedMembers::Found {
            return OwnerMember::Elsewhere;
        }
        let types = kept(self.named(class), |&at| derives(self.graph.sym(at).kind));
        let tier = self.reached_tier(call.file, &types);
        let Some(Twin::Member(ty) | Twin::Namesake(ty)) =
            self.twin(call.caller_at(), written, &tier)
        else {
            return OwnerMember::Elsewhere;
        };
        let Some(up) = self.type_ancestors(ty) else {
            return OwnerMember::Elsewhere;
        };
        let nearest = self.nearest_base(candidates.named, class, &up);
        let mut inherited = Vec::new();
        for &owner in &up.owners {
            let owned = candidates.with_owner(self.graph, owner);
            if owned.is_empty()
                || nearest
                    .is_some_and(|nearest| nearest != owner && up.derives_from(nearest, owner))
            {
                continue;
            }
            if !up
                .qualified_per_base
                .get(&owner)
                .is_some_and(Option::is_some)
            {
                return OwnerMember::Elsewhere;
            }
            inherited.extend(
                owned
                    .iter()
                    .copied()
                    .filter(|&at| self.declares_inherited(&up, at)),
            );
        }
        let tier = self.reached_tier(call.file, &inherited);
        let Some(&first) = tier.first() else {
            return OwnerMember::Elsewhere;
        };
        if tier.iter().all(|&at| self.owner(at) == self.owner(first)) {
            OwnerMember::Member(first)
        } else {
            OwnerMember::Ambiguous
        }
    }

    /// The members of `owned` in the nearest of the file itself, the files it includes and the files those reach.
    pub(crate) fn reached_tier<'o>(&self, file: u32, owned: &'o [SymbolAt]) -> Cow<'o, [SymbolAt]> {
        let in_file = kept(owned, |at| at.file == file);
        if !in_file.is_empty() || owned.is_empty() {
            return in_file;
        }
        let includes = self.includes(file);
        let through_includes = kept(owned, |at| includes.contains(&at.file));
        if !through_includes.is_empty() {
            return through_includes;
        }
        let closure = self.closure(file);
        kept(owned, |at| closure.binary_search(&at.file).is_ok())
    }

    pub(crate) fn member_of_the_receiver_s_owner(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> Option<SymbolAt> {
        let receiver = self.named_receiver(call)?;
        if let Some(class) = self.class_from_the_root(call, receiver) {
            // constraint: a call of a class by its own name on that class is how an extraction writes a construction,
            // which names the class, whatever method of the name it holds
            if self.graph.sym(class).name == call.callee {
                return Some(class);
            }
            return (self.held_by(class, candidates))
                .or_else(|| self.member_above(class, candidates));
        }
        if let Some(at) = self.member_above_the_receiver_s_class(call, receiver, candidates) {
            return Some(at);
        }
        if let Some(rooted) = self.rooted_constant(receiver) {
            let owner = self.graph.strings.get(rooted)?;
            if self.is_on_the_caller_s_class(call) {
                return self.member_of_the_caller_s_class(call, owner, candidates);
            }
            return self.nearest(
                call,
                candidates.with_owner(self.graph, owner).iter().copied(),
            );
        }
        if let Some(at) = self.nearest(
            call,
            candidates.with_owner(self.graph, receiver).iter().copied(),
        ) {
            return Some(at);
        }
        let separator = self.conventions.owner_chain_separator?;
        let mut ending = candidates.targets.iter().copied().filter(|&at| {
            self.owner(at)
                .is_some_and(|owner| self.chain_ends_in(owner, receiver, separator))
        });
        let first = ending.next()?;
        let owner = self.owner(first);
        if !ending.clone().all(|at| self.owner(at) == owner) {
            return None;
        }
        self.nearest(call, std::iter::once(first).chain(ending))
    }

    /// The type of exactly the qualified name a receiver spells from the root, under [`ParentSpelling::FromRoot`]; the
    /// nearest where copies of the tree hold it, and `None` for a class outside the tree and for any other receiver.
    pub(crate) fn class_from_the_root(&self, call: &Call, receiver: Str) -> Option<SymbolAt> {
        if self.conventions.parent_spelling != ParentSpelling::FromRoot {
            return None;
        }
        let strings = &self.graph.strings;
        let written = strings.resolve(receiver);
        let written = written.strip_prefix(QUALIFIER_SEPARATORS)?;
        let types: Vec<SymbolAt> = self
            .qualified_as(strings.get(written)?)
            .filter(|&at| derives(self.graph.sym(at).kind))
            .collect();
        self.nearest_of(call, types.into_iter())
    }

    /// What every [`ReceiverMembers`] but `Declared` binds: the member of the name that the nearest type above the
    /// receiver's class holds, where the class holds none itself, or the field that answers ahead of it.
    fn member_above_the_receiver_s_class(
        &self,
        call: &Call,
        receiver: Str,
        candidates: &Candidates<'_>,
    ) -> Option<SymbolAt> {
        if self.conventions.receiver_members == ReceiverMembers::Declared {
            return None;
        }
        let class = self.receiver_class(call, receiver)?;
        if self.held_by(class, candidates).is_some() {
            return None;
        }
        self.member_above(class, candidates)
    }

    // constraint: an instance method of a class is no method of the class object, so a class's own method above the
    // class answers `self.class.generators` where the class itself holds only `def generators`
    /// The member of the name that the class `owner` holds as its own, else the nearest type above it.
    fn member_of_the_caller_s_class(
        &self,
        call: &Call,
        owner: Str,
        candidates: &Candidates<'_>,
    ) -> Option<SymbolAt> {
        let classes: Vec<SymbolAt> = (self.qualified_as(owner))
            .filter(|&at| derives(self.graph.sym(at).kind))
            .collect();
        let members = candidates.with_owner(self.graph, owner);
        let own = (members.iter().copied())
            .filter(|&at| classes.is_empty() || self.is_of_the_kind(true, at));
        self.nearest(call, own).or_else(|| {
            let class = self.nearest_of(call, classes.iter().copied())?;
            self.member_above(class, candidates)
        })
    }

    // constraint: an interface hands a class only a declaration, which a body any type above the class holds fulfils
    // however far up it stands, and an interface of a parent's name is no parent where a class of that name stands as
    // near, so the walk over the types that hold bodies goes first
    fn member_above(&self, class: SymbolAt, candidates: &Candidates<'_>) -> Option<SymbolAt> {
        let walk = |admitted: fn(SymbolKind) -> bool| {
            let parents = self.parents_of(&[class], admitted);
            self.nearest_holder((class, parents), candidates, admitted)
        };
        let own_field = self.field_held_by(class, candidates);
        if own_field.is_some() && self.a_field_answers_alone() {
            return own_field;
        }
        let member = walk(holds_a_body).or_else(|| walk(|_| true))?;
        Some(own_field.unwrap_or(member))
    }

    // constraint: a class's table holds what its parents gave it, so a member no type of the graph above the class has
    // stands in a class the graph does not hold, and no class's namesake answers it; only a name written in full tells
    // such a parent from a type the walk failed to find. A language writes the class a type extends as its first base,
    // and a class outside the graph can use a trait of the tree, so a trait's member stays with the tiers
    /// Whether the receiver's class holds no member of the name, no trait of the graph does, and it, or a type above
    /// it, extends a class no type of the graph is named.
    pub(crate) fn inherits_from_outside_the_graph(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> bool {
        if self.conventions.parent_spelling != ParentSpelling::FromRoot {
            return false;
        }
        let Some(class) = (self.named_receiver(call))
            .and_then(|receiver| self.receiver_class(call, receiver))
            .filter(|&class| self.held_by(class, candidates).is_none())
        else {
            return false;
        };
        let held_by_a_trait = |&at: &SymbolAt| {
            self.class_of(at)
                .is_some_and(|ty| self.graph.sym(ty).kind == SymbolKind::Trait)
        };
        if candidates.targets.iter().any(held_by_a_trait) {
            return false;
        }
        let mut walked: HashSet<SymbolAt, FixedState> = HashSet::default();
        walked.insert(class);
        let mut pending = vec![class];
        while let Some(ty) = pending.pop() {
            let parents = self.graph.file(ty.file).parents_of(ty.index);
            let types_of = |parent: &Parent| {
                self.qualified_as(parent.written)
                    .filter(|&at| derives(self.graph.sym(at).kind))
            };
            if parents
                .first()
                .is_some_and(|base| types_of(base).next().is_none())
            {
                return true;
            }
            pending.extend(
                (parents.iter().flat_map(types_of))
                    .filter(|&at| holds_a_body(self.graph.sym(at).kind) && walked.insert(at)),
            );
        }
        false
    }

    // constraint: a child reads the table its parents gave it, so the nearest declaration is the property; a parent's
    // private one stays behind in that parent, and the read names no slot of it (`zend_get_property_offset`,
    // `Zend/zend_object_handlers.c`), while a trait's is copied into the class that uses it
    // (`zend_do_traits_property_binding`, `Zend/zend_inheritance.c`), which a trait it uses in turn hands on
    /// The class that the value type of the field `name` names, of the nearest type at or above the caller's class that
    /// declares one, where the caller's class reads that field.
    pub(crate) fn class_of_an_inherited_field(&self, call: &Call, name: &str) -> Option<Str> {
        let named = self.named(self.graph.strings.get(name)?);
        let class = self.class_of(call.caller_at())?;
        let mut walked: HashSet<SymbolAt, FixedState> = HashSet::default();
        walked.insert(class);
        let mut level = vec![class];
        let mut copied = level.clone();
        while !level.is_empty() {
            let declared = level
                .iter()
                .find_map(|&ty| Some((ty, self.field_declared_by(ty, named)?)));
            if let Some((ty, field)) = declared {
                return (self.graph.sym(field).exported() || copied.contains(&ty))
                    .then(|| self.graph.file(field.file).value_types(field.index))
                    .flatten();
            }
            copied = self.parents_of(&copied, |kind| kind == SymbolKind::Trait);
            level = self.parents_of(&level, holds_a_body);
            level.retain(|&ty| walked.insert(ty));
        }
        None
    }

    /// The definition of the call's name, nearest the call, that the first of `bodies`, one a line, holding one gives.
    pub(crate) fn member_of_a_pasted_body(&self, call: &Call, bodies: &str) -> Option<SymbolAt> {
        let named = self.named(call.callee).iter().copied();
        bodies.split('\n').find_map(|line| {
            let held = |at: &SymbolAt| self.held_by_a_body(*at, line);
            self.nearest_of(call, named.clone().filter(held))
        })
    }

    /// Whether the body that `line`, a name or `@` and a filter, stands for holds the definition at `at`.
    fn held_by_a_body(&self, at: SymbolAt, line: &str) -> bool {
        let Some(body) = self.graph.file(at.file).value_types(at.index) else {
            return false;
        };
        line.strip_prefix('@').map_or_else(
            || self.graph.strings.resolve(body) == line,
            |filter| self.graph.pastes(body, filter),
        )
    }

    /// The field among `named` that the body of `ty` declares.
    fn field_declared_by(&self, ty: SymbolAt, named: &[SymbolAt]) -> Option<SymbolAt> {
        let symbol = self.graph.sym(ty);
        let body = self.body(ty);
        (self.graph.in_file(named, ty.file).iter().copied()).find(|&at| {
            self.is_field_of(at, |owner| owner == symbol.name)
                && body.contains(at.file, self.graph.sym(at).line)
        })
    }

    /// The class a receiver names: the caller's own for a receiver the language writes for a value of that type, else
    /// the class of the receiver's name nearest the call.
    fn receiver_class(&self, call: &Call, receiver: Str) -> Option<SymbolAt> {
        let written = self.graph.strings.resolve(receiver);
        if (self.conventions.own_type_receivers).is_some_and(|own| own.contains(&written)) {
            return self.class_of(call.caller_at());
        }
        self.class_in_the_caller_s_scope(call, receiver)
            .or_else(|| {
                let located = self.located_types(receiver, call.file, |_| true);
                if self.past_the_imports(call.file, &located) {
                    return None;
                }
                self.nearest_of(call, located.into_iter())
            })
    }

    // constraint: where a bare receiver names a class, the language reads that name through the file's imports, else
    // under the file's namespace, so a type those give stands ahead of a namesake a transitive include reaches
    /// The class of the receiver's name that the caller's file holds or includes itself, else one of the caller's
    /// scope.
    fn class_in_the_caller_s_scope(&self, call: &Call, receiver: Str) -> Option<SymbolAt> {
        if !matches!(self.conventions.bare_receivers, BareReceivers::Classes(_)) {
            return None;
        }
        let types =
            (self.named(receiver).iter().copied()).filter(|&ty| derives(self.graph.sym(ty).kind));
        let includes = self.includes(call.file);
        let rank = |&slot: &u32| self.graph.path_rank(slot);
        let imported = types.clone().filter(|ty| {
            ty.file == call.file || includes.binary_search_by_key(&rank(&ty.file), rank).is_ok()
        });
        self.nearest_of(call, imported).or_else(|| {
            let caller = call.caller_at();
            let scope = self
                .graph
                .qualifier(self.class_of(caller).unwrap_or(caller));
            self.nearest_of(call, types.filter(|&ty| self.graph.qualifier(ty) == scope))
        })
    }

    /// Whether the call's receiver, at its last segment, spells the owner of `at` in any case: `session` or
    /// `self.session` spells `Session`.
    pub(crate) fn receiver_spells_owner(&self, call: &Call, at: SymbolAt) -> bool {
        self.named_receiver(call)
            .zip(self.owner(at))
            .is_some_and(|(receiver, owner)| self.spells(receiver, owner))
    }

    /// Whether `receiver`, at its last segment, spells the last segment of `owner` in any case.
    fn spells(&self, receiver: Str, owner: Str) -> bool {
        let strings = &self.graph.strings;
        let Some(spelled) = strings.resolve(receiver).rsplit(['.', ':']).next() else {
            return false;
        };
        let name = strings.resolve(owner).rsplit(['.', ':']).next();
        spelled
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && name.is_some_and(|name| name.eq_ignore_ascii_case(spelled))
    }

    pub(crate) fn member_of_the_caller_s_owner(
        &self,
        call: &Call,
        candidates: &Candidates<'_>,
    ) -> Option<Bound> {
        let receiver = self.named_receiver(call)?;
        let listed = (self.conventions.own_type_receivers)
            .is_none_or(|own| own.contains(&self.graph.strings.resolve(receiver)));
        if !listed && self.conventions.unlisted_receivers == UnlistedReceivers::NoOwnMember {
            return None;
        }
        let caller_owner = self
            .graph
            .file(call.file)
            .symbols
            .get(call.caller as usize)?
            .owner?;
        let owned = candidates.with_owner(self.graph, caller_owner);
        if owned.is_empty() {
            return None;
        }
        let owned = owned.iter().copied();
        let at = match self.qualified_owner(call.caller_at()) {
            Some(own) => self.nearest(
                call,
                owned.filter(|&at| self.qualified_owner(at).is_none_or(|other| other == own)),
            ),
            None => self.nearest(call, owned),
        }?;
        Some((at, confidence::SAME_FILE))
    }

    /// Whether `call` reaches `at`, a member of the caller's own owner, through one field of a receiver the language
    /// lists for that owner, `this.slot` and not `this.a.b`, where [`UnlistedReceivers::OwnMemberFieldsAsGuess`] holds.
    pub(crate) fn own_member_on_a_field(&self, call: &Call, at: SymbolAt) -> bool {
        if self.conventions.unlisted_receivers != UnlistedReceivers::OwnMemberFieldsAsGuess {
            return false;
        }
        let owner = self.owner(at);
        let Some((root, field)) = self
            .named_receiver(call)
            .and_then(|receiver| self.graph.strings.resolve(receiver).split_once('.'))
        else {
            return false;
        };
        owner.is_some()
            && owner == self.owner(call.caller_at())
            && (self.conventions.own_type_receivers).is_some_and(|own| own.contains(&root))
            && field
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    /// What a call through one of [`kartta_lang::Conventions::base_receivers`] binds: the member of the name that the
    /// first base the caller's class declares holds, else the nearest class above that base, and nothing where the
    /// graph holds neither or a field hides the member. `None` for any other receiver, and for a caller in no class the
    /// graph holds, as a method of a class the language writes without a name, whose call the tiers below bind.
    pub(crate) fn member_of_the_caller_s_base(&self, call: &Call) -> Option<Result<Bound, Miss>> {
        let receiver = self.named_receiver(call)?;
        if !(self.conventions.base_receivers).contains(&self.graph.strings.resolve(receiver)) {
            return None;
        }
        let class = self.class_of(call.caller_at())?;
        Some(
            self.held_by_a_base(class, &self.candidates(call))
                .filter(|&at| self.graph.sym(at).kind != SymbolKind::Field)
                .map(|at| (at, confidence::RECEIVER))
                .ok_or(Miss::NotAdmitted),
        )
    }

    // constraint: the base receiver names the class the caller's class extends, which a language writes as its first
    // base, PHP's `extends` before its interfaces and its traits; above that class, the member is the one the nearest
    // type holds, a class's traits before the class it extends in turn
    fn held_by_a_base(&self, class: SymbolAt, candidates: &Candidates<'_>) -> Option<SymbolAt> {
        let base = *self.declared_bases(class).first()?;
        let mut level = self.base_types(base, class, holds_a_body);
        level.retain(|&ty| self.graph.sym(ty).kind != SymbolKind::Trait);
        self.nearest_holder((class, level), candidates, holds_a_body)
    }

    /// The member of the name that the nearest type holds, walking from `level`, the types nearest `class`, up through
    /// the parents of each level, a level's traits first, taking the types `admitted` admits; the first field of the
    /// name in that walk where one stands ahead of the member, or where the walk finds none and a field answers alone.
    fn nearest_holder(
        &self,
        (class, mut level): (SymbolAt, Vec<SymbolAt>),
        candidates: &Candidates<'_>,
        admitted: fn(SymbolKind) -> bool,
    ) -> Option<SymbolAt> {
        let mut walked: HashSet<SymbolAt, FixedState> = HashSet::default();
        walked.insert(class);
        level.retain(|&ty| walked.insert(ty));
        let mut field = None;
        while !level.is_empty() {
            let held = self.held_at_level(&level, candidates, &mut field);
            if held.is_some() {
                return field.or(held);
            }
            level = self.parents_of(&level, admitted);
            level.retain(|&ty| walked.insert(ty));
        }
        field.filter(|_| self.a_field_answers_alone())
    }

    /// The member of the name that the first type of `level` to hold one holds, keeping in `field` the first field of
    /// the name a type ahead of that one holds.
    fn held_at_level(
        &self,
        level: &[SymbolAt],
        candidates: &Candidates<'_>,
        field: &mut Option<SymbolAt>,
    ) -> Option<SymbolAt> {
        level.iter().find_map(|&ty| {
            let held = self.held_by(ty, candidates);
            if held.is_none() {
                *field = field.or_else(|| self.field_held_by(ty, candidates));
            }
            held
        })
    }

    /// Whether `above` is among the types the member walk reaches above `class`, however far up.
    pub(crate) fn is_above(&self, above: SymbolAt, class: SymbolAt) -> bool {
        let mut walked = vec![class];
        let mut level = vec![class];
        while !level.is_empty() {
            let mut parents = self.parents_of(&level, |_| true);
            if parents.contains(&above) {
                return true;
            }
            parents.retain(|at| !walked.contains(at));
            parents.sort_unstable();
            parents.dedup();
            walked.extend_from_slice(&parents);
            level = parents;
        }
        false
    }

    /// The types that the types of `level` name as parents, a trait before any other type.
    fn parents_of(&self, level: &[SymbolAt], admitted: fn(SymbolKind) -> bool) -> Vec<SymbolAt> {
        let mut parents: Vec<SymbolAt> = level
            .iter()
            .flat_map(|&ty| {
                let parents = self.graph.file(ty.file).parents_of(ty.index).iter();
                parents
                    .filter_map(move |parent| {
                        Some(DeclaredBase {
                            owner: self.parent_name(ty, parent.written)?,
                            written: parent.written,
                            written_from_root: parent.written_from_root,
                        })
                    })
                    .flat_map(move |base| self.base_types(base, ty, admitted))
            })
            .collect();
        parents.sort_by_key(|&ty| self.graph.sym(ty).kind != SymbolKind::Trait);
        parents
    }

    // constraint: a parent is often a class of another module, which no scope around the child spells, so the walk
    // reads it by its last name, as `Base` for `b.Base`, and the file's imports tell the namesakes of that name apart
    /// The last name of the parent `ty` writes as `written`, which a type of the graph can hold: `None` where the name
    /// the parent starts with comes from outside the repository.
    fn parent_name(&self, ty: SymbolAt, written: Str) -> Option<Str> {
        let strings = &self.graph.strings;
        let written = strings.resolve(written);
        let head = written
            .split(QUALIFIER_SEPARATORS)
            .next()
            .unwrap_or(written);
        if self.conventions.value_uses == ValueUses::Imported
            && strings
                .get(head)
                .is_some_and(|head| self.binding(ty.file, head) == Some(None))
        {
            return None;
        }
        strings.get(last_segment(written))
    }

    // constraint: a module sees another file's class only through an import, so a class of the name in its directory
    // that it does not import is a namesake, not the name it writes
    /// Whether `located`, the types of one name nearest `file`, lie past what `file` imports, where the language
    /// imports what a module uses.
    fn past_the_imports(&self, file: u32, located: &[SymbolAt]) -> bool {
        self.conventions.value_uses == ValueUses::Imported
            && located
                .first()
                .is_some_and(|&ty| self.distance_from(file)(ty) > IMPORTED)
    }

    // constraint: a member's owner is the chain of classes inside its file, which a qualified name runs past into the
    // module, so the owner is a tail of the type's qualified name, and the body tells a namesake's members apart
    /// The candidate that `ty` itself holds.
    fn held_by(&self, ty: SymbolAt, candidates: &Candidates<'_>) -> Option<SymbolAt> {
        let strings = &self.graph.strings;
        let qualified = strings.resolve(self.graph.sym(ty).qualified_name);
        let body = self.body(ty);
        std::iter::once(qualified)
            .chain(tails(qualified))
            .filter_map(|owner| strings.get(owner))
            .find_map(|owner| {
                (candidates.with_owner(self.graph, owner).iter())
                    .copied()
                    .find(|&at| body.contains(at.file, self.graph.sym(at).line))
            })
    }

    /// Whether a field stops the walk with no member above it, not only ahead of one.
    fn a_field_answers_alone(&self) -> bool {
        self.conventions.receiver_members == ReceiverMembers::InheritedUpToAnyField
    }

    /// The field of the name that `ty` itself holds, where the convention has a field stop the walk, and the field holds
    /// a value: one recorded with a value type only declares the name.
    fn field_held_by(&self, ty: SymbolAt, candidates: &Candidates<'_>) -> Option<SymbolAt> {
        if !matches!(
            self.conventions.receiver_members,
            ReceiverMembers::InheritedUpToFields | ReceiverMembers::InheritedUpToAnyField
        ) {
            return None;
        }
        let strings = &self.graph.strings;
        let qualified = strings.resolve(self.graph.sym(ty).qualified_name);
        let body = self.body(ty);
        let holds = |owner: Str| {
            let owner = strings.resolve(owner);
            std::iter::once(qualified)
                .chain(tails(qualified))
                .any(|tail| tail == owner)
        };
        let mut named = candidates.in_file(self.graph, ty.file).iter().copied();
        named.find(|&at| {
            let symbol = self.graph.sym(at);
            symbol.kind == SymbolKind::Field
                && body.contains(at.file, symbol.line)
                && symbol.owner.is_some_and(holds)
                && self.graph.file(at.file).value_types(at.index).is_none()
        })
    }

    // constraint: a class is never its own base, so a base that shares the class's name, Quirrel's `class
    // (itemsModal.handlerClass)` in a `handlerClass` of its own, is the nearest other type of that name; a file
    // reaches a type of another scope through an include, as PHP's `use` of a class the repository's autoload maps, so
    // a type of another scope that the child's file neither holds, includes nor shares a directory with is a namesake
    // of a base outside the graph, PHPUnit's `TestCase` for `Illuminate\Foundation\Testing\TestCase`
    /// The types of a kind `admitted` admits that `base`, which `child` declares, names, the nearest to `child` first.
    fn base_types(
        &self,
        base: DeclaredBase,
        child: SymbolAt,
        admitted: fn(SymbolKind) -> bool,
    ) -> Vec<SymbolAt> {
        let admits = |ty: SymbolAt| ty != child && admitted(self.graph.sym(ty).kind);
        if base.written_from_root {
            let mut located = self.located_base_types(base, child);
            located.retain(|&ty| admits(ty));
            return located;
        }
        let mut located = self.located_types(base.owner, child.file, admits);
        if self.past_the_imports(child.file, &located) {
            return Vec::new();
        }
        if located
            .first()
            .is_some_and(|&ty| self.distance_from(child.file)(ty) == FAR)
        {
            let scope = self.graph.qualifier(child);
            located.retain(|&ty| self.graph.qualifier(ty) == scope);
        }
        located
    }

    /// The owner `at` is a member of, as its qualified name spells it: `A\App` for `A\App::m`.
    fn qualified_owner(&self, at: SymbolAt) -> Option<&str> {
        let owner = self
            .graph
            .qualifier(at)?
            .trim_end_matches(QUALIFIER_SEPARATORS);
        (!owner.is_empty()).then_some(owner)
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
        let values = self.bodies_values(call, candidates);
        let [value_in_dir, value_in_parent_dir, value_first_party] =
            values.fold([false; 3], |[dir, parent, first], at| {
                let file = self.graph.file(at.file);
                [
                    dir || file.dir == caller_file.dir,
                    parent || file.parent_dir == caller_file.parent_dir,
                    first || !self.graph.sym(at).flags.is_vendored(),
                ]
            });
        let same_dir = candidates.in_dir(self.graph, caller_file.dir);
        let (nearby, confidence, value_nearby) = if same_dir.is_empty() && !value_in_dir {
            let parent_dir = candidates.in_parent_dir(self.graph, caller_file.parent_dir);
            if !parent_dir.is_empty() || value_in_parent_dir {
                (parent_dir, confidence::PARENT_DIR, value_in_parent_dir)
            } else if candidates.turned_away {
                // constraint: the code outside vendored code is no nearer the caller than the one callable of the name
                // is, so a name that packages the caller's is not built with also define binds no guess there
                return None;
            } else {
                let nearby = candidates.first_party(self.graph);
                (nearby, confidence::FIRST_PARTY, value_first_party)
            }
        } else {
            (same_dir, confidence::SAME_DIR, value_in_dir)
        };
        if value_nearby {
            return None;
        }
        let nearby = nearby.iter().copied();
        let shape = if call.receiver.is_some() && confidence == confidence::FIRST_PARTY {
            Shape::Member
        } else {
            self.shape(call, &nearby, reach)
        };
        // constraint: code outside the graph can define a name the graph holds, and a definition a call's arguments turn
        // away shows the name has overloads, so the nearest definition that fits is a guess
        let guess = |at| {
            if self.passes_too_few_as_near(call, candidates.named, confidence) {
                None
            } else if self.takes_too_few_arguments(call, candidates.named) {
                Some((at, confidence::FIRST_PARTY))
            } else {
                Some((at, confidence))
            }
        };
        self.one_callable(&shape, nearby.clone())
            .or_else(|| self.constructor_of_a_type_outside(&shape, &nearby))
            .and_then(guess)
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

// constraint: an interface holds no body a call through the base receiver can run
/// Whether a type of `kind` can hold a member's body.
fn holds_a_body(kind: SymbolKind) -> bool {
    kind != SymbolKind::Interface
}

/// Whether a base of `kind` hands its members on to the types that name it: a class, a struct, an interface, a trait or
/// a namespace.
const fn hands_members_on(kind: SymbolKind) -> bool {
    derives(kind) || matches!(kind, SymbolKind::Namespace)
}

/// The field a receiver spells after the operator that takes the member, `.lock` or `->lock`, before the `[]` of a call
/// on an element, `->locks[]`, and how many pointers the call goes through: `->` is one, and `[]` one more.
fn field_spelled(receiver: &str) -> Option<(usize, &str)> {
    let (pointers, name) = receiver
        .strip_prefix("->")
        .map(|name| (1, name))
        .or_else(|| receiver.strip_prefix('.').map(|name| (0, name)))?;
    Some(
        name.strip_suffix("[]")
            .map_or((pointers, name), |element| (pointers + 1, element)),
    )
}

pub(crate) const fn is_value(kind: SymbolKind) -> bool {
    matches!(kind, SymbolKind::Variable | SymbolKind::Constant)
}
