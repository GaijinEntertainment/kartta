use std::collections::{HashMap, HashSet};

use kartta_lang::{Symbol, SymbolKind};

use super::{Emitter, ImplOwner};

impl Emitter<'_> {
    // constraint: an impl owner keeps its module path when the file declares a type of that qualified name, and
    // otherwise takes the path as written, which the symbols under its chain, its receivers and its trait pair
    // follow; an inner impl settles before an outer one; a scoped call's receiver takes its module path when the
    // file declares a type or module of that name, and otherwise keeps the last segment
    pub(super) fn settle_paths(&mut self) {
        if self.receiver_paths.is_empty() && self.impl_owners.is_empty() {
            return;
        }
        let mut owners = std::mem::take(&mut self.impl_owners);
        let declared: HashSet<&str> = self
            .raw
            .symbols
            .iter()
            .filter(|symbol| {
                matches!(
                    symbol.kind,
                    SymbolKind::Struct
                        | SymbolKind::Enum
                        | SymbolKind::Interface
                        | SymbolKind::TypeAlias
                        | SymbolKind::Namespace
                )
            })
            .map(|symbol| symbol.qualified_name.as_str())
            .collect();
        let mut paths: Vec<(usize, bool, String)> = self
            .receiver_paths
            .drain(..)
            .map(|(index, path)| (index, declared.contains(path.as_str()), path))
            .collect();
        owners.retain(|owner| !declared.contains(owner.tentative.as_str()));

        for owner in owners.iter().rev() {
            self.settle_owner(owner, &mut paths);
        }
        for (index, names_a_declaration, path) in paths {
            if names_a_declaration {
                self.raw.calls[index].receiver = Some(path);
            }
        }
    }

    fn settle_owner(&mut self, owner: &ImplOwner, paths: &mut [(usize, bool, String)]) {
        let settle = |name: &mut String| {
            if let Some(settled) = rebase(name, &owner.tentative, &owner.written) {
                *name = settled;
            }
        };
        for &id in &owner.members {
            let symbol = &mut self.raw.symbols[id.0 as usize];
            settle(&mut symbol.qualified_name);
            if let Some(owned) = &mut symbol.owner {
                settle(owned);
            }
        }
        for &index in &owner.receivers {
            if let Some(receiver) = &mut self.raw.calls[index].receiver {
                settle(receiver);
            }
        }
        for &at in &owner.paths {
            settle(&mut paths[at].2);
        }
        for &at in &owner.pairs {
            settle(&mut self.pending_parents[at].0);
        }
    }

    // constraint: the pair goes to the type the file declares under that qualified name, or else under that name, which
    // a type imported into an inline module spells; of the types one name matches, the first the file declares wins; a
    // scoped target that names no declaration of the file, such as `impl Trait for other::Foo` or `crate::Foo`, is
    // dropped rather than matched by its last segment, because the file cannot tell which type the path reaches, and
    // so is a type another file declares
    pub(super) fn apply_pending_parents(&mut self) {
        const fn is_type(symbol: &Symbol) -> bool {
            matches!(
                symbol.kind,
                SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Interface
            )
        }

        if self.pending_parents.is_empty() {
            return;
        }
        let symbols = &mut self.raw.symbols;
        let mut types: HashMap<&str, usize> = HashMap::new();
        let declared = || {
            symbols
                .iter()
                .enumerate()
                .filter(|(_, symbol)| is_type(symbol))
        };
        for (at, symbol) in declared() {
            types.entry(&symbol.qualified_name).or_insert(at);
        }
        for (at, symbol) in declared() {
            types.entry(&symbol.name).or_insert(at);
        }
        let targets: Vec<(usize, String)> = std::mem::take(&mut self.pending_parents)
            .into_iter()
            .filter_map(|(type_name, trait_name)| {
                types.get(type_name.as_str()).map(|&at| (at, trait_name))
            })
            .collect();

        for (at, trait_name) in targets {
            if !symbols[at].parents.contains(&trait_name) {
                symbols[at].parents.push(trait_name);
            }
        }
    }
}

pub(super) fn is_under(name: &str, chain: &str) -> bool {
    name.strip_prefix(chain)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

fn rebase(name: &str, chain: &str, written: &str) -> Option<String> {
    is_under(name, chain).then(|| format!("{written}{}", &name[chain.len()..]))
}
