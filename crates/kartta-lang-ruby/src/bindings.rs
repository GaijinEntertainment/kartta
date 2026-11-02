use std::collections::{HashMap, HashSet};

use kartta_lang::{Callable, CallableRole, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MethodKind {
    Instance,
    Singleton,
}

// constraint: a method name binds apart in each method that holds a declaration of it, since a `def` nested in a
// method is no redefinition of a top-level one
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NameKey {
    scope: Option<SymbolId>,
    kind: MethodKind,
    qualified: String,
}

impl NameKey {
    pub(crate) const fn new(scope: Option<SymbolId>, kind: MethodKind, qualified: String) -> Self {
        Self {
            scope,
            kind,
            qualified,
        }
    }
}

#[derive(Clone, Copy)]
struct Binding {
    declaration: SymbolId,
    body: SymbolId,
}

#[derive(Default)]
pub(crate) struct Bindings {
    bodies: HashMap<NameKey, Binding>,
    aliases_with_bodies: Vec<(NameKey, SymbolId, SymbolId)>,
}

impl Bindings {
    pub(crate) fn bind(&mut self, name: NameKey, declaration: SymbolId) {
        let binding = Binding {
            declaration,
            body: declaration,
        };
        self.bodies.insert(name, binding);
    }

    pub(crate) fn body(&self, name: &NameKey) -> Option<SymbolId> {
        self.bodies.get(name).map(|binding| binding.body)
    }

    pub(crate) fn declaration(&self, name: &NameKey) -> Option<SymbolId> {
        self.bodies.get(name).map(|binding| binding.declaration)
    }

    // constraint: an alias takes the body its original names at the alias, so a later redefinition of the original
    // leaves the alias on the earlier body, as Ruby does
    pub(crate) fn bind_alias(&mut self, name: NameKey, alias: SymbolId, body: Option<SymbolId>) {
        if let Some(body) = body {
            self.aliases_with_bodies.push((name.clone(), alias, body));
        }
        let binding = Binding {
            declaration: alias,
            body: body.unwrap_or(alias),
        };
        self.bodies.insert(name, binding);
    }

    // constraint: an alias whose own name a later declaration takes is no alias of the body any longer, since the
    // later declaration replaces it
    pub(crate) fn into_callables(self) -> Vec<Callable> {
        let mut callables = Vec::new();
        let mut defined = HashSet::new();
        for (name, alias, body) in self.aliases_with_bodies {
            if self
                .bodies
                .get(&name)
                .is_none_or(|binding| binding.declaration != alias)
            {
                continue;
            }

            let key = body.0.to_string();
            if defined.insert(body) {
                callables.push(Callable::new(body, key.clone(), CallableRole::Definition));
            }
            callables.push(Callable::new(alias, key, CallableRole::Declaration));
        }

        callables
    }
}
