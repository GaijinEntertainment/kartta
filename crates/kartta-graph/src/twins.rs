use std::borrow::Cow;

use crate::resolve::{Cascade, derives};
use crate::stored::{StoredSymbol, SymbolAt};
use crate::strings::Str;

/// What a receiver's spelling picks among the members of one owner's name that a tier of the caller's reach holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Twin {
    /// The member of the class the spelling names, or of the one class the members belong to.
    Member(SymbolAt),
    /// The first member of classes of one qualified name, which one spelling names alike: a template and its
    /// specializations, a class under each branch of a conditional, or copies of one class.
    Namesake(SymbolAt),
    /// The tier's first member, where classes of other qualified names hold the members and the spelling names none.
    Unnamed(SymbolAt),
}

impl Twin {
    /// The member the pick carries, whatever it is.
    pub(crate) const fn member(self) -> SymbolAt {
        match self {
            Self::Member(at) | Self::Namesake(at) | Self::Unnamed(at) => at,
        }
    }
}

impl<'g> Cascade<'g> {
    /// The owner a receiver names, `Widget` for `b::Widget`, since a member keeps its class's bare name as its owner;
    /// `None` where no symbol has that name.
    pub(crate) fn bare_owner(&self, receiver: Str) -> Option<Str> {
        let scoped = self
            .conventions
            .scope_separator
            .and_then(|separator| self.graph.strings.resolve(receiver).rsplit_once(separator));
        match scoped {
            Some((_, bare)) => self.graph.strings.get(bare),
            None => Some(receiver),
        }
    }

    // constraint: C++ looks a class's name up from the scope around the use outward, so of the classes that hold the
    // tier's members, the receiver's spelling names those the innermost scope around the use holds under it; where it
    // names none, classes of one qualified name are still one name's
    pub(crate) fn twin(&self, site: SymbolAt, written: &str, tier: &[SymbolAt]) -> Option<Twin> {
        let &first = tier.first()?;
        if tier.len() == 1 {
            return Some(Twin::Member(first));
        }
        let classes: Vec<(SymbolAt, SymbolAt)> = tier
            .iter()
            .filter_map(|&at| Some((at, self.class_holding(at)?)))
            .collect();
        let one = classes.first().map(|&(_, class)| class);
        if classes.iter().all(|&(_, class)| Some(class) == one) {
            return Some(Twin::Member(first));
        }
        let named = self.named_by(site, written, &classes);
        let held = if named.is_empty() { classes } else { named };
        Some(self.namesake(site, first, &in_site_file(site, held)))
    }

    /// The class a member belongs to, or a class itself; `None` for a type that holds no members, an alias.
    fn class_holding(&self, at: SymbolAt) -> Option<SymbolAt> {
        let kind = self.graph.sym(at).kind;
        if kind.is_type() {
            return derives(kind).then_some(at);
        }
        self.class_of(at)
    }

    fn named_by(
        &self,
        site: SymbolAt,
        written: &str,
        classes: &[(SymbolAt, SymbolAt)],
    ) -> Vec<(SymbolAt, SymbolAt)> {
        let separator = self.conventions.scope_separator.unwrap_or("::");
        let scope = self.scope_around(site);
        let depths: Vec<Option<usize>> = classes
            .iter()
            .map(|&(_, class)| holder_depth(self.qualified_name(class), written, scope, separator))
            .collect();
        let Some(nearest) = depths.iter().flatten().max() else {
            return Vec::new();
        };
        classes
            .iter()
            .zip(&depths)
            .filter(|&(_, depth)| depth.as_ref() == Some(nearest))
            .map(|(&held, _)| held)
            .collect()
    }

    /// The scope a use looks a name up from: a member's class, whose qualified name keeps the namespaces the member's
    /// own leaves out, else the scope the use's function is qualified by; empty at file scope.
    pub(crate) fn scope_around(&self, site: SymbolAt) -> &'g str {
        let separator = self.conventions.scope_separator.unwrap_or("::");
        if self.site_symbol(site).is_none() {
            return "";
        }
        let body = self.holder(site).unwrap_or(site);
        self.graph
            .sym(body)
            .owner
            .and_then(|_| self.class_of(body))
            .map(|class| self.qualified_name(class))
            .or_else(|| self.graph.enclosing_scope(body, separator))
            .unwrap_or("")
    }

    // constraint: a class the use's function defines, or one whose body holds the use, is the one its names mean
    // before a namesake elsewhere
    fn namesake(&self, site: SymbolAt, first: SymbolAt, held: &[(SymbolAt, SymbolAt)]) -> Twin {
        if let Some(member) = one_class(held.iter().copied()) {
            return Twin::Member(member);
        }
        if let Some(symbol) = self.site_symbol(site) {
            let here = || {
                held.iter()
                    .copied()
                    .filter(|&(_, class)| class.file == site.file)
            };
            let defined = here().filter(|&(_, class)| {
                (symbol.line..=symbol.end_line).contains(&self.graph.sym(class).line)
            });
            let around = here().filter(|&(_, class)| {
                let class = self.graph.sym(class);
                (class.line..=class.end_line).contains(&symbol.line)
            });
            if let Some(member) = one_class(defined).or_else(|| one_class(around)) {
                return Twin::Member(member);
            }
        }
        let Some(&(member, class)) = held.first() else {
            return Twin::Unnamed(first);
        };
        let name = self.graph.sym(class).qualified_name;
        if held
            .iter()
            .all(|&(_, other)| self.graph.sym(other).qualified_name == name)
        {
            Twin::Namesake(member)
        } else {
            Twin::Unnamed(first)
        }
    }

    fn site_symbol(&self, site: SymbolAt) -> Option<&'g StoredSymbol> {
        self.graph.file(site.file).symbols.get(site.index as usize)
    }

    fn qualified_name(&self, class: SymbolAt) -> &'g str {
        self.graph
            .strings
            .resolve(self.graph.sym(class).qualified_name)
    }
}

/// The members of `owned` that `held` keeps, borrowed where it keeps one, so a tier of one member allocates nothing.
pub(crate) fn kept(owned: &[SymbolAt], held: impl Fn(&SymbolAt) -> bool) -> Cow<'_, [SymbolAt]> {
    let mut found = owned.iter().filter(|&at| held(at));
    match (found.next(), found.next()) {
        (None, _) => Cow::Borrowed(&[]),
        (Some(one), None) => Cow::Borrowed(std::slice::from_ref(one)),
        (Some(&one), Some(&two)) => [one, two].into_iter().chain(found.copied()).collect(),
    }
}

/// The member of the first pair where every pair holds the same class.
fn one_class(mut held: impl Iterator<Item = (SymbolAt, SymbolAt)>) -> Option<SymbolAt> {
    let (member, class) = held.next()?;
    held.all(|(_, other)| other == class).then_some(member)
}

// constraint: a member the use's own file declares is the one the use sees first, as a definition is before the
// declaration its file includes
fn in_site_file(site: SymbolAt, held: Vec<(SymbolAt, SymbolAt)>) -> Vec<(SymbolAt, SymbolAt)> {
    if !held.iter().any(|&(member, _)| member.file == site.file) {
        return held;
    }
    held.into_iter()
        .filter(|&(member, _)| member.file == site.file)
        .collect()
}

/// How far into the scope around a use the scope reaches that holds a class of `qualified` name under the spelling
/// `written`, by the length of its name, the global scope at 0; `None` where no scope around the use holds it so.
pub(crate) fn holder_depth(
    qualified: &str,
    written: &str,
    scope: &str,
    separator: &str,
) -> Option<usize> {
    if let Some(rooted) = written.strip_prefix(separator) {
        return (qualified == rooted).then_some(0);
    }
    let holder = if qualified == written {
        ""
    } else {
        qualified.strip_suffix(written)?.strip_suffix(separator)?
    };
    let encloses = holder.is_empty()
        || scope
            .strip_prefix(holder)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(separator));
    encloses.then_some(holder.len())
}
