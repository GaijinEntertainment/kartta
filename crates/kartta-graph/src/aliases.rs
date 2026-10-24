use std::borrow::Cow;

use kartta_lang::SymbolKind;

use crate::resolve::{Call, Cascade, derives};
use crate::stored::SymbolAt;
use crate::strings::Str;
use crate::twins::{holder_depth, kept};

/// Where a spelling is read: the file whose includes reach its names, the class its first name is looked up in
/// before any namespace, the scope around it, and the separator of its scopes.
#[derive(Clone, Copy)]
struct Reading<'s> {
    file: u32,
    class: Option<Str>,
    scope: &'s str,
    separator: &'s str,
}

impl Cascade<'_> {
    /// The class a receiver's spelling names through the type aliases its names name, by its bare name, with the
    /// spelling it reaches: `IteratorImpl` for `NodeList::iterator`, where `typedef List<Node> NodeList;` and `List`
    /// declares `typedef IteratorImpl<T, List> iterator;`. `None` where no name of the spelling names an alias, where
    /// the chain comes back to an alias it passed, or where it ends at a name no class of the graph has.
    pub(crate) fn through_aliases(&self, call: &Call, written: &str) -> Option<(Str, String)> {
        let separator = self.conventions.scope_separator?;
        let caller = call.caller_at();
        let mut reading = Reading {
            file: call.file,
            class: self.graph.sym(caller).owner,
            scope: self.scope_around(caller),
            separator,
        };
        let mut spelled = Cow::Borrowed(written);
        let mut passed = Vec::new();
        while let Some((next, alias)) = self.alias_step(reading, &spelled) {
            if next == spelled {
                break;
            }
            if passed.contains(&alias) {
                return None;
            }
            passed.push(alias);
            reading.class = self.graph.sym(alias).owner;
            reading.scope = self.graph.enclosing_scope(alias, separator).unwrap_or("");
            spelled = Cow::Owned(next);
        }
        let Cow::Owned(spelled) = spelled else {
            return None;
        };
        let bare = spelled.rsplit(separator).next()?;
        let class = self.graph.strings.get(bare)?;
        self.named(class)
            .iter()
            .any(|&at| derives(self.graph.sym(at).kind))
            .then_some((class, spelled))
    }

    // constraint: C++ looks a name up in the class around the use before the namespaces around it, and a name after a
    // class's scope in that class alone, so the first name of a spelling names an alias of the class it is read in,
    // of namesake classes the one the scope around the use is, before one outside every class, a name after a class
    // an alias that class declares, and a name after namespaces an alias those namespaces hold, where no class of
    // that spelling is nearer; a class's alias of a type the walk records none for hides the names around it all the
    // same
    /// The spelling with its first name that names an alias replaced by what the alias names, and that alias.
    fn alias_step(&self, reading: Reading<'_>, spelled: &str) -> Option<(String, SymbolAt)> {
        let separator = reading.separator;
        let strings = &self.graph.strings;
        let mut class = None;
        let mut start = 0;
        for (index, segment) in spelled.split(separator).enumerate() {
            let end = start + segment.len();
            start = end + separator.len();
            let Some(name) = strings.get(segment) else {
                class = None;
                continue;
            };
            let written = &spelled[..end];
            let depth = |at: &SymbolAt| self.holder_depth(reading, written, *at);
            let named = self.named(name);
            let owner = if index == 0 { reading.class } else { class };
            let of_the_class = owner
                .map(|_| kept(named, self.alias_of(owner)))
                .unwrap_or_default();
            let in_scope = kept(&of_the_class, |at| depth(at).is_some());
            let nested = if index > 0 || !in_scope.is_empty() || self.reads_a_located_class(reading)
            {
                in_scope
            } else {
                Cow::Borrowed(&of_the_class[..])
            };
            let nested = self.reached_tier(reading.file, &nested);
            let alias = if nested.is_empty() {
                (index == 0 || class.is_none())
                    .then(|| self.namespace_alias(reading, named, written))
                    .flatten()
            } else {
                Some(self.one_named_type(&nested)?)
            };
            if let Some((aliased, at)) = alias {
                let mut next = strings.resolve(aliased).to_owned();
                next.push_str(&spelled[end..]);
                return Some((next, at));
            }
            let in_scope = |at: &SymbolAt| derives(self.graph.sym(*at).kind) && depth(at).is_some();
            class = named.iter().any(in_scope).then_some(name);
        }
        None
    }

    /// What the alias outside every class that `written` names from the scopes around it names, of the symbols of its
    /// last name `named`: of the aliases those scopes hold, the ones the file reaches nearest, of those the innermost
    /// scope's, where no class of that spelling sits as deep.
    fn namespace_alias(
        &self,
        reading: Reading<'_>,
        named: &[SymbolAt],
        written: &str,
    ) -> Option<(Str, SymbolAt)> {
        let depth = |at: &SymbolAt| self.holder_depth(reading, written, *at);
        let is_alias = self.alias_of(None);
        let aliases = kept(named, |at| is_alias(at) && depth(at).is_some());
        let aliases = self.reached_tier(reading.file, &aliases);
        let nearest = aliases.iter().filter_map(depth).max()?;
        let classes = kept(named, |&at| derives(self.graph.sym(at).kind));
        let classes = self.reached_tier(reading.file, &classes);
        if classes
            .iter()
            .filter_map(depth)
            .any(|class| class >= nearest)
        {
            return None;
        }
        self.one_named_type(&kept(&aliases, |at| depth(at) == Some(nearest)))
    }

    /// How deep into the scope around `reading` the scope sits that holds the symbol at `at` under the spelling
    /// `written`; `None` where no scope around it holds it so.
    fn holder_depth(&self, reading: Reading<'_>, written: &str, at: SymbolAt) -> Option<usize> {
        let qualified = self
            .graph
            .strings
            .resolve(self.graph.sym(at).qualified_name);
        holder_depth(qualified, written, reading.scope, reading.separator)
    }

    /// Whether the scope around `reading` is a class of the name its class has, which an out-of-line member whose
    /// record lacks its namespaces leaves unlocated.
    fn reads_a_located_class(&self, reading: Reading<'_>) -> bool {
        reading.class.is_some_and(|class| {
            self.named(class).iter().any(|&at| {
                let symbol = self.graph.sym(at);
                derives(symbol.kind)
                    && self.graph.strings.resolve(symbol.qualified_name) == reading.scope
            })
        })
    }

    fn alias_of(&self, owner: Option<Str>) -> impl Fn(&SymbolAt) -> bool + '_ {
        move |&at| {
            let symbol = self.graph.sym(at);
            symbol.kind == SymbolKind::TypeAlias && symbol.owner == owner
        }
    }

    /// The type the aliases of `tier` name and the first of them, where they all name one.
    fn one_named_type(&self, tier: &[SymbolAt]) -> Option<(Str, SymbolAt)> {
        let value_type = |at: &SymbolAt| self.graph.file(at.file).value_types(at.index);
        let (first, rest) = tier.split_first()?;
        let named_type = value_type(first)?;
        rest.iter()
            .all(|at| value_type(at) == Some(named_type))
            .then_some((named_type, *first))
    }
}
