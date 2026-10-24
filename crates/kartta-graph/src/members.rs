use std::cell::OnceCell;
use std::cmp::Reverse;
use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::HashSet;
use kartta_lang::{IncludeReach, SymbolKind};

use crate::candidates::{Candidates, INDEXED_FROM};
use crate::confidence;
use crate::resolve::{
    Bound, Call, Cascade, THROUGH_INCLUDES_MAX_DEFINITIONS, UseMiss, derived, derives,
};
use crate::scope::Shape;
use crate::stored::SymbolAt;
use crate::strings::Str;
use crate::twins::{Twin, kept};

/// What the bases of a type hold under a name, searched in the order they are written, each before the next.
enum Inherited {
    Member(SymbolAt),
    Nothing,
    /// A base the graph cannot name without a guess comes first, so the member may be its.
    Unknown,
}

/// The definitions of one name a data site can bind: the fields, and the values and functions a namespace or the file
/// scope holds.
#[derive(Debug)]
pub(crate) struct DataNamed<'g> {
    fields: Candidates<'g>,
    values: Vec<SymbolAt>,
}

impl<'g> Cascade<'g> {
    pub(crate) fn data_named(&self, name: Str) -> Rc<DataNamed<'g>> {
        derived(&self.data_named, name, || {
            let named = self.named(name);
            let (fields, values): (Vec<SymbolAt>, Vec<SymbolAt>) = named
                .iter()
                .copied()
                .filter(|&at| {
                    let symbol = self.graph.sym(at);
                    symbol.kind == SymbolKind::Field
                        || (matches!(
                            symbol.kind,
                            SymbolKind::Variable | SymbolKind::Constant | SymbolKind::Function
                        ) && symbol.owner.is_none()
                            && self.holder(at).is_none())
                })
                .partition(|&at| self.graph.sym(at).kind == SymbolKind::Field);
            let fields = if named.len() < INDEXED_FROM {
                Candidates::scanned(named, fields)
            } else {
                Candidates::indexed(self.graph, named, fields, Vec::new())
            };
            Rc::new(DataNamed { fields, values })
        })
    }

    // constraint: `this` and `self` reach the members the class around the use declares, and failing those the ones
    // a base declares, where every base searched before is found without a guess; a use in a function local to a
    // method reaches the method's class; where a bare name in a member reads the class first, the own receiver reaches
    // the field that lookup finds and nothing else
    pub(crate) fn own_member(&self, site: SymbolAt, name: Str) -> Result<Bound, UseMiss> {
        let owner = self.receiver_owner(site);
        if self.implicit_receiver() {
            return owner
                .and_then(|own| self.class_member(site, own, name))
                .unwrap_or(Err(UseMiss::Elsewhere));
        }
        let owner = owner.ok_or(UseMiss::Ambiguous)?;
        if let Some(at) = self.member_in(site.file, (owner, false), name) {
            return Ok((at, confidence::RECEIVER));
        }
        let class = self.class_around(site, owner).ok_or(UseMiss::Ambiguous)?;
        self.inherited_member(class, name)
    }

    /// A bare name, looked up as C++ looks one up inside a member, where a bare name in a member reads the class
    /// first: the field of the enclosing member's class or of a base, and `None` where neither may hold the name.
    pub(crate) fn class_scope_use(
        &self,
        site: SymbolAt,
        name: Str,
    ) -> Option<Result<Bound, UseMiss>> {
        if !self.implicit_receiver() {
            return None;
        }
        self.receiver_owner(site)
            .and_then(|own| self.class_member(site, own, name))
    }

    fn receiver_owner(&self, site: SymbolAt) -> Option<Str> {
        self.graph.sym(site).owner.or_else(|| {
            self.holder(site)
                .and_then(|holder| self.graph.sym(holder).owner)
        })
    }

    /// The member of the owner a receiver path names, a class's own method first for a path on the class, or `None`
    /// where the path leaves the use to the tiers that read no owner.
    pub(crate) fn path_member(
        &self,
        site: SymbolAt,
        path: Str,
        name: Str,
    ) -> Option<Result<Bound, UseMiss>> {
        let strings = &self.graph.strings;
        let owned = match self.graph.class_named(strings.resolve(path)) {
            Some(class) => match strings.get(class) {
                Some(class) => (class, true),
                None => return Some(Err(UseMiss::Elsewhere)),
            },
            None => (path, false),
        };
        if self.conventions.include_reach != IncludeReach::Transitive {
            return Some(self.owner_member(site, owned, name));
        }
        self.included_owner_member(site, owned, name)
    }

    // constraint: a receiver path names the owner of the member, as a call's receiver names one, declared in the
    // use's file or in the one file of those it includes that declares a member of the name under it
    fn owner_member(
        &self,
        site: SymbolAt,
        (owner, on_a_class): (Str, bool),
        name: Str,
    ) -> Result<Bound, UseMiss> {
        if let Some(at) = self.member_in(site.file, (owner, on_a_class), name) {
            return Ok((at, confidence::RECEIVER));
        }
        let includes = self.includes(site.file);
        let mut declared = includes
            .iter()
            .filter_map(|&included| self.member_in(included, (owner, on_a_class), name));
        match (declared.next(), declared.next()) {
            (Some(at), None) => return Ok((at, confidence::RECEIVER)),
            (Some(_), Some(_)) => return Err(UseMiss::Ambiguous),
            (None, _) => {}
        }
        let mut in_module = includes
            .iter()
            .flat_map(|&included| self.graph.in_file(self.named(name), included))
            .copied()
            .filter(|&at| self.is_module_member(at, owner));
        match (in_module.next(), in_module.next()) {
            (Some(at), None) => return Ok((at, confidence::RECEIVER)),
            (Some(_), Some(_)) => return Err(UseMiss::Ambiguous),
            (None, _) => {}
        }
        let ty = self
            .type_seen_from(site.file, owner)
            .ok_or(UseMiss::Elsewhere)?;
        if let Some(at) = self.declared_by(ty, name) {
            return Ok((at, confidence::RECEIVER));
        }
        self.inherited_member(ty, name)
    }

    // constraint: where includes paste text in place, an owner names a class by its bare name, which several classes
    // can share, the path's scopes telling namesakes apart, and a name the graph holds no type of names a class outside
    // it, whose namesake fields are not meant; a member taken without a call is a field, or a method taken as a value,
    // `&Class::draw`, which binds as a call to it does, the first of namesakes of one qualified name among them, so the
    // use keeps an edge to the method where namesakes would leave it none; a class without the field binds the field of
    // the one base that declares it, and a type the graph holds that neither does, an alias most often, leaves the
    // field to the tiers that read no type; of namesake classes without the field, the bases searched are those of the
    // one the spelling names, else those of the first, whose field of the name is the one meant most often (ADR 3.65)
    fn included_owner_member(
        &self,
        site: SymbolAt,
        (path, on_a_class): (Str, bool),
        name: Str,
    ) -> Option<Result<Bound, UseMiss>> {
        let Some(owner) = self.bare_owner(path) else {
            let value = self.namespace_value(site, path, &self.data_named(name));
            return Some(value.ok_or(UseMiss::Elsewhere));
        };
        let written = self.spelling(path);
        let named = self.data_named(name);
        let value = self.namespace_value(site, path, &named);
        let declared = named.fields.with_owner(self.graph, owner);
        if let Some(found) = self.reached_owner(site, &declared, written) {
            return Some(self.unless_namespace_value(found, value));
        }
        let mut methods: Vec<SymbolAt> = self
            .named(name)
            .iter()
            .copied()
            .filter(|&at| {
                let symbol = self.graph.sym(at);
                symbol.kind == SymbolKind::Method && symbol.owner == Some(owner)
            })
            .collect();
        if methods
            .iter()
            .any(|&at| self.is_of_the_kind(on_a_class, at))
        {
            methods.retain(|&at| self.is_of_the_kind(on_a_class, at));
        }
        if !methods.is_empty() {
            let found = match self.nearest_to(site, &methods, written) {
                Some(Twin::Member(at) | Twin::Namesake(at)) => Ok((at, confidence::RECEIVER)),
                _ => Err(UseMiss::Ambiguous),
            };
            return Some(self.unless_namespace_value(found, value));
        }
        if let Some(value) = value {
            return Some(Ok(value));
        }
        let types: Vec<SymbolAt> = self
            .named(owner)
            .iter()
            .copied()
            .filter(|&at| self.graph.sym(at).kind.is_type())
            .collect();
        let Some(ty) = self.nearest_to(site, &types, written) else {
            return Some(Err(UseMiss::Elsewhere));
        };
        self.inherited_by(site, ty.member(), &named)
    }

    // constraint: a class member is found by its class's bare name, so a namesake class elsewhere, `other::detail`,
    // holds a member of what `cfg::detail::scale` spells; a namespace value of the qualified name the scope and the
    // name spell is meant before it, unless the member takes that qualified name itself, as the class the scope names
    // shares it with its out-of-class definition
    fn unless_namespace_value(
        &self,
        found: Result<Bound, UseMiss>,
        value: Option<Bound>,
    ) -> Result<Bound, UseMiss> {
        match (found, value) {
            (Ok((member, _)), Some((value, _)))
                if self.graph.sym(member).qualified_name
                    == self.graph.sym(value).qualified_name =>
            {
                found
            }
            (_, Some(value)) => Ok(value),
            (found, None) => found,
        }
    }

    // constraint: a scope that holds no member of the name names a namespace, which emits no symbol, so its value is
    // the one outside every owner whose qualified name is the scope and the name; C++ looks a qualified name up from
    // the scopes around the use outward, `detail::scale` inside `cfg` naming `cfg::detail::scale` first, and from the
    // root alone where the scope is written from it; a member's own qualified name leaves its namespaces out, so the
    // scopes around it are its class's; the value must lie in a file the use's file reaches
    fn namespace_value(&self, site: SymbolAt, path: Str, named: &DataNamed<'_>) -> Option<Bound> {
        let separator = self.conventions.scope_separator?;
        let strings = &self.graph.strings;
        let written = strings.resolve(path);
        let (written, rooted) = written
            .strip_prefix(separator)
            .map_or((written, false), |written| (written, true));
        let around = self.scope_around(site);
        let depth_of = |at: SymbolAt| {
            let symbol = self.graph.sym(at);
            let scope = strings
                .resolve(symbol.qualified_name)
                .strip_suffix(strings.resolve(symbol.name))?
                .strip_suffix(separator)?
                .strip_suffix(written)?;
            if scope.is_empty() {
                return Some(0);
            }
            let enclosing = scope.strip_suffix(separator)?;
            let inside = around
                .strip_prefix(enclosing)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(separator));
            (!rooted && inside).then_some(enclosing.len() + separator.len())
        };
        let (at, depth) = named
            .values
            .iter()
            .copied()
            .filter_map(|at| depth_of(at).map(|depth| (at, depth)))
            .filter(|&(at, _)| self.reaches(site.file, at.file))
            .min_by_key(|&(at, depth)| (Reverse(depth), self.graph.order_of(at)))?;
        let confidence = if depth == 0 {
            confidence::QUALIFIED
        } else {
            confidence::QUALIFIED_TAIL
        };
        Some((at, confidence))
    }

    // constraint: past the use's own file a field binds only where it is the one field of its name in the files the
    // file includes, then in those it reaches, for a name few fields share; a guess across the family binds a field
    // whose real namesake the graph never extracted
    pub(crate) fn included_field(&self, file: u32, name: Str) -> Result<Bound, UseMiss> {
        let named = self.data_named(name);
        let fields = named.fields.targets.as_slice();
        let one = |found: &[SymbolAt]| match found {
            [at] => Ok((*at, confidence::INCLUDE)),
            _ => Err(UseMiss::AmbiguousField),
        };
        let included: Vec<SymbolAt> = self
            .includes(file)
            .iter()
            .flat_map(|&included| self.graph.in_file(fields, included))
            .copied()
            .collect();
        if !included.is_empty() || fields.len() > THROUGH_INCLUDES_MAX_DEFINITIONS {
            return one(&included);
        }
        let closure = self.closure(file);
        let reached: Vec<SymbolAt> = fields
            .iter()
            .copied()
            .filter(|at| at.file != file && closure.binary_search(&at.file).is_ok())
            .collect();
        one(&reached)
    }

    // constraint: a value binds where its file declares the name, else where the files it includes do, else, for a
    // name few values share, where the files those reach do; several values of the name in the first tier that holds
    // one bind none, and a declaration in a header and its definition share the qualified name and count once
    pub(crate) fn included_value(&self, file: u32, name: Str) -> Option<Bound> {
        let named = self.data_named(name);
        let values = named.values.as_slice();
        if values.is_empty() {
            return None;
        }
        let in_file = self.graph.in_file(values, file);
        if !in_file.is_empty() {
            return self
                .one_value(in_file)
                .map(|at| (at, confidence::SAME_FILE));
        }
        let included: Vec<SymbolAt> = self
            .includes(file)
            .iter()
            .flat_map(|&included| self.graph.in_file(values, included))
            .copied()
            .collect();
        if !included.is_empty() {
            return self
                .one_value(&included)
                .map(|at| (at, confidence::INCLUDE));
        }
        if values.len() > THROUGH_INCLUDES_MAX_DEFINITIONS {
            return None;
        }
        let closure = self.closure(file);
        let reached: Vec<SymbolAt> = values
            .iter()
            .copied()
            .filter(|at| closure.binary_search(&at.file).is_ok())
            .collect();
        self.one_value(&reached).map(|at| (at, confidence::INCLUDE))
    }

    // constraint: the class scope binds the own class's field, else a base's field only where the lookup a bare call
    // makes certainly finds that base's, as one where a free definition of the name may be meant binds the wrong
    // namesake most often
    fn class_member(&self, site: SymbolAt, own: Str, name: Str) -> Option<Result<Bound, UseMiss>> {
        let named = self.data_named(name);
        let fields = named.fields.targets.as_slice();
        if fields.is_empty() {
            return None;
        }
        if let Some(found) = self.reached_owner(
            site,
            &named.fields.with_owner(self.graph, own),
            self.spelling(own),
        ) {
            return Some(found);
        }
        let call = Call {
            file: site.file,
            caller: site.index,
            callee: name,
            receiver: None,
            written_from_root: false,
            names_a_runtime_method: false,
            on_a_class: false,
            local: None,
            line: 0,
        };
        let reach = self.reach(&call, &named.fields)?;
        let targets = fields.iter().copied();
        match self.shape(&call, &targets, Some(&reach)) {
            Shape::Inherited(reach) => {
                let inherited = Shape::Inherited(reach);
                let admitted: Vec<SymbolAt> =
                    targets.filter(|&at| self.admits(&inherited, at)).collect();
                let owner = self.graph.sym(*admitted.first()?).owner;
                if admitted.iter().all(|&at| self.graph.sym(at).owner == owner) {
                    self.reached_owner(site, &admitted, None)
                } else {
                    Some(Err(UseMiss::AmbiguousField))
                }
            }
            Shape::FreeOrInherited(_) => Some(Err(UseMiss::AmbiguousField)),
            _ => None,
        }
    }

    fn inherited_by(
        &self,
        site: SymbolAt,
        ty: SymbolAt,
        named: &DataNamed<'_>,
    ) -> Option<Result<Bound, UseMiss>> {
        let inherited: Vec<SymbolAt> = named
            .fields
            .targets
            .iter()
            .copied()
            .filter(|&at| {
                self.graph
                    .sym(at)
                    .owner
                    .is_some_and(|owner| self.has_ancestor(ty, owner))
            })
            .collect();
        let owner = self.graph.sym(*inherited.first()?).owner;
        if !inherited
            .iter()
            .all(|&at| self.graph.sym(at).owner == owner)
        {
            return Some(Err(UseMiss::AmbiguousField));
        }
        self.reached_owner(site, &inherited, None)
    }

    fn one_value(&self, values: &[SymbolAt]) -> Option<SymbolAt> {
        let (&first, others) = values.split_first()?;
        let qualified = self.graph.sym(first).qualified_name;
        others
            .iter()
            .all(|&at| self.graph.sym(at).qualified_name == qualified)
            .then_some(first)
    }

    /// The field of `owned` the use's file reaches at the receiver's confidence, of the class `written` names where
    /// namesakes hold it, an ambiguous use where it reaches none or names no one class, and `None` where `owned` is
    /// empty.
    fn reached_owner(
        &self,
        site: SymbolAt,
        owned: &[SymbolAt],
        written: Option<&str>,
    ) -> Option<Result<Bound, UseMiss>> {
        if owned.is_empty() {
            return None;
        }
        Some(match self.nearest_to(site, owned, written) {
            Some(Twin::Member(at)) => Ok((at, confidence::RECEIVER)),
            _ => Err(UseMiss::AmbiguousField),
        })
    }

    // constraint: owners of one bare name in several places, a vendored copy of a class beside another, are told apart
    // by what the file sees: its own definitions and its includes', then those its includes reach; one it reaches
    // neither way is another class of the name half the time, its namesake in a copy of the library or an SDK's; of
    // namesakes in the nearer of the two, the one the receiver's spelling names; a member defined in a file its class's
    // header includes, an inline implementation header, sees what the file of its declaration sees as well, as the
    // translation unit holds the class header's text before it
    fn nearest_to(
        &self,
        site: SymbolAt,
        owned: &[SymbolAt],
        written: Option<&str>,
    ) -> Option<Twin> {
        if owned.is_empty() {
            return None;
        }
        let file = site.file;
        let closure = OnceCell::new();
        let in_closure = |target: u32| {
            closure
                .get_or_init(|| self.closure(file))
                .binary_search(&target)
                .is_ok()
        };
        let declared: Vec<u32> = self
            .declarations(site)
            .iter()
            .map(|declaration| declaration.file)
            .filter(|&declared| !in_closure(declared))
            .collect();
        let sees = |at: SymbolAt| {
            self.sees(file, at) || declared.iter().any(|&declared| self.sees(declared, at))
        };
        let reaches = |at: &SymbolAt| {
            in_closure(at.file)
                || declared
                    .iter()
                    .any(|&declared| self.reaches(declared, at.file))
        };
        let Some(written) = written else {
            if let Some(&member) = owned.iter().find(|&&at| sees(at)) {
                return Some(Twin::Member(member));
            }
            return owned.iter().copied().find(reaches).map(Twin::Member);
        };
        let in_view = kept(owned, |&at| sees(at));
        if !in_view.is_empty() {
            return self.twin(site, written, &in_view);
        }
        let in_reach = kept(owned, reaches);
        self.twin(site, written, &in_reach)
    }

    /// The receiver path as written where scopes tell namesake classes apart, as C++'s do.
    fn spelling(&self, path: Str) -> Option<&'g str> {
        self.conventions
            .scope_separator
            .map(|_| self.graph.strings.resolve(path))
    }

    fn inherited_member(&self, ty: SymbolAt, name: Str) -> Result<Bound, UseMiss> {
        let mut searched = HashSet::default();
        searched.insert(ty);
        match self.inherited(ty, name, &mut searched) {
            Inherited::Member(at) => Ok((at, confidence::SAME_FILE)),
            Inherited::Nothing => Err(UseMiss::Elsewhere),
            Inherited::Unknown => Err(UseMiss::Ambiguous),
        }
    }

    fn member_in(
        &self,
        file: u32,
        (owner, on_a_class): (Str, bool),
        name: Str,
    ) -> Option<SymbolAt> {
        let members = self
            .graph
            .in_file(self.named(name), file)
            .iter()
            .copied()
            .filter(|&at| self.graph.sym(at).owner == Some(owner));
        self.first_of_the_kind(on_a_class, members)
    }

    // constraint: a base reached twice, through two of its subclasses or a cycle of bases, was searched in full the
    // first time
    fn inherited(
        &self,
        ty: SymbolAt,
        name: Str,
        searched: &mut HashSet<SymbolAt, FixedState>,
    ) -> Inherited {
        for parent in self.graph.file(ty.file).parents_of(ty.index) {
            let Some(base) = self.type_seen_from(ty.file, parent.written) else {
                return Inherited::Unknown;
            };
            if !searched.insert(base) {
                continue;
            }
            if let Some(at) = self.declared_by(base, name) {
                return Inherited::Member(at);
            }
            match self.inherited(base, name, searched) {
                Inherited::Nothing => {}
                found @ (Inherited::Member(_) | Inherited::Unknown) => return found,
            }
        }
        Inherited::Nothing
    }

    // constraint: a member keeps its owner's chain of names, which the qualified name of the type ends with
    fn declared_by(&self, ty: SymbolAt, name: Str) -> Option<SymbolAt> {
        let symbol = self.graph.sym(ty);
        let qualified = self.graph.strings.resolve(symbol.qualified_name);
        self.graph
            .in_file(self.named(name), ty.file)
            .iter()
            .copied()
            .find(|&at| {
                let member = self.graph.sym(at);
                (symbol.line..=symbol.end_line).contains(&member.line)
                    && member
                        .owner
                        .is_some_and(|owner| self.ends_chain(qualified, owner))
            })
    }

    // constraint: where a qualified name starts with the module that declares it, a receiver that ends that module's
    // name names the module, as `mod.LIMIT` names `pkg.mod.LIMIT` after `import mod`
    fn is_module_member(&self, at: SymbolAt, owner: Str) -> bool {
        let separator = self.conventions.owner_chain_separator.unwrap_or(".");
        self.graph.sym(at).owner.is_none()
            && self.holder(at).is_none()
            && self
                .graph
                .enclosing_scope(at, separator)
                .is_some_and(|module| self.ends_chain(module, owner))
    }

    fn ends_chain(&self, qualified: &str, owner: Str) -> bool {
        let owner = self.graph.strings.resolve(owner);
        let separator = self.conventions.owner_chain_separator.unwrap_or(".");
        qualified == owner
            || qualified
                .strip_suffix(owner)
                .is_some_and(|head| head.ends_with(separator))
    }

    fn class_around(&self, site: SymbolAt, owner: Str) -> Option<SymbolAt> {
        let line = self.graph.sym(site).line;
        self.types_named(site.file, owner).find(|&ty| {
            let symbol = self.graph.sym(ty);
            (symbol.line..=symbol.end_line).contains(&line)
        })
    }

    // constraint: a type is seen from a file that declares it, or from the one file of those it includes that does
    fn type_seen_from(&self, file: u32, written: Str) -> Option<SymbolAt> {
        if let Some(ty) = self.types_named(file, written).next() {
            return Some(ty);
        }
        let includes = self.includes(file);
        let mut declared = includes
            .iter()
            .filter_map(|&included| self.types_named(included, written).next());
        match (declared.next(), declared.next()) {
            (Some(ty), None) => Some(ty),
            _ => None,
        }
    }

    fn types_named(&self, file: u32, written: Str) -> impl Iterator<Item = SymbolAt> + '_ {
        let strings = &self.graph.strings;
        let chain = strings.resolve(written);
        let separator = self.conventions.owner_chain_separator.unwrap_or(".");
        let last = chain.rsplit(separator).next().unwrap_or(chain);
        let named = strings.get(last).map_or(&[][..], |last| self.named(last));
        self.graph
            .in_file(named, file)
            .iter()
            .copied()
            .filter(move |&ty| {
                let symbol = self.graph.sym(ty);
                derives(symbol.kind)
                    && self.ends_chain(strings.resolve(symbol.qualified_name), written)
            })
    }
}
