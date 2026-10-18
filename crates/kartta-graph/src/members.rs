use foldhash::fast::FixedState;
use hashbrown::HashSet;

use crate::confidence;
use crate::resolve::{Bound, Cascade, UseMiss, derives};
use crate::stored::SymbolAt;
use crate::strings::Str;

/// What the bases of a type hold under a name, searched in the order they are written, each before the next.
enum Inherited {
    Member(SymbolAt),
    Nothing,
    /// A base the graph cannot name without a guess comes first, so the member may be its.
    Unknown,
}

impl Cascade<'_> {
    // constraint: `this` and `self` reach the members the class around the use declares, and failing those the ones
    // a base declares, where every base searched before is found without a guess; a use in a function local to a
    // method reaches the method's class
    pub(crate) fn own_member(&self, site: SymbolAt, name: Str) -> Result<Bound, UseMiss> {
        let owner = self
            .graph
            .sym(site)
            .owner
            .or_else(|| {
                self.holder(site)
                    .and_then(|holder| self.graph.sym(holder).owner)
            })
            .ok_or(UseMiss::Ambiguous)?;
        if let Some(at) = self.member_in(site.file, owner, name) {
            return Ok((at, confidence::RECEIVER));
        }
        let class = self.class_around(site, owner).ok_or(UseMiss::Ambiguous)?;
        self.inherited_member(class, name)
    }

    // constraint: a receiver path names the owner of the member, as a call's receiver names one, declared in the
    // use's file or in the one file of those it includes that declares a member of the name under it
    pub(crate) fn owner_member(
        &self,
        site: SymbolAt,
        owner: Str,
        name: Str,
    ) -> Result<Bound, UseMiss> {
        if let Some(at) = self.member_in(site.file, owner, name) {
            return Ok((at, confidence::RECEIVER));
        }
        let includes = self.includes(site.file);
        let mut declared = includes
            .iter()
            .filter_map(|&included| self.member_in(included, owner, name));
        match (declared.next(), declared.next()) {
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

    fn inherited_member(&self, ty: SymbolAt, name: Str) -> Result<Bound, UseMiss> {
        let mut searched = HashSet::with_hasher(FixedState::default());
        searched.insert(ty);
        match self.inherited(ty, name, &mut searched) {
            Inherited::Member(at) => Ok((at, confidence::SAME_FILE)),
            Inherited::Nothing => Err(UseMiss::Elsewhere),
            Inherited::Unknown => Err(UseMiss::Ambiguous),
        }
    }

    fn member_in(&self, file: u32, owner: Str, name: Str) -> Option<SymbolAt> {
        self.graph
            .in_file(self.named(name), file)
            .iter()
            .copied()
            .find(|&at| self.graph.sym(at).owner == Some(owner))
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
