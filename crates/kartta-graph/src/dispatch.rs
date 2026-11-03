use kartta_lang::{Implements, SameMethod, SymbolKind};

use crate::implementations::Types;
use crate::resolve::Cascade;
use crate::stored::{StoredSymbol, SymbolAt};
use crate::strings::Str;

/// The methods a call may bind and still run one method.
#[derive(Default)]
pub(crate) struct Dispatching {
    /// The methods of its name, each once, that its owner implements or overrides.
    pub(crate) parents: Vec<SymbolAt>,
    /// Its owner's class and the types above it, where the language reads them.
    lineage: Option<Vec<SymbolAt>>,
}

impl Dispatching {
    /// Whether a call from `caller` bound to `parent` may run the method. From a class that `parent`'s type is or is
    /// above, as the parents read for implementors or the member walk reach it, the call runs on that class's own
    /// value, or on its base's own member, so it runs the method only where the method's owner is that class or below
    /// it; from anywhere else it may run on a value of any implementor.
    pub(crate) fn may_run<'g>(
        &self,
        (types, cascade): (&Types<'g>, &Cascade<'g>),
        caller: SymbolAt,
        parent: SymbolAt,
    ) -> bool {
        let Some(lineage) = &self.lineage else {
            return true;
        };
        let Some(class) = cascade.class_of(caller) else {
            return true;
        };
        lineage.contains(&class)
            || cascade.class_of(parent).is_none_or(|above| {
                above != class
                    && !types.ancestors(class, caller.file).contains(&above)
                    && !cascade.is_above(above, class)
            })
    }
}

/// The methods of `method`'s name that a call may bind and still run `method`: those a type its owner implements,
/// extends or includes declares, as [`Types::ancestors`] reads them, under [`Implements::Overrides`] those of them it
/// overrides, or under [`Implements::MethodSets`], those an interface near the owner's package declares whose every
/// method the owner's package gives the owner. Only a method `called` keeps is looked at, so a name's uncalled methods
/// cost nothing. A constructor has none.
pub(crate) fn parent_methods<'g>(
    types: &Types<'g>,
    cascade: &Cascade<'g>,
    method: SymbolAt,
    called: impl Fn(SymbolAt) -> bool,
) -> Dispatching {
    let graph = cascade.graph;
    let symbol = graph.sym(method);
    let Some(owner) = symbol.owner.filter(|_| symbol.kind == SymbolKind::Method) else {
        return Dispatching::default();
    };
    if cascade.is_a_constructor(symbol) {
        return Dispatching::default();
    }
    let candidates = graph
        .named(symbol.name)
        .iter()
        .copied()
        .filter(|&at| at != method && graph.sym(at).kind == SymbolKind::Method && called(at));
    let mut dispatching = match cascade.conventions.implements {
        Implements::MethodSets(same) => {
            let interfaces = MethodSets {
                types,
                cascade,
                same,
                owner,
                package: method.file,
            };
            if interfaces.is_an_interface(owner) {
                return Dispatching::default();
            }
            Dispatching {
                parents: candidates
                    .filter(|&at| interfaces.declares_satisfied(at))
                    .collect(),
                lineage: None,
            }
        }
        implements => {
            let Some(class) = cascade.class_of(method) else {
                return Dispatching::default();
            };
            let mut lineage = types.ancestors(class, method.file);
            if implements == Implements::Traits {
                lineage.retain(|&ty| {
                    matches!(
                        graph.sym(ty).kind,
                        SymbolKind::Trait | SymbolKind::Interface
                    )
                });
            }
            if lineage.is_empty() {
                return Dispatching::default();
            }
            let stands_for = |at: SymbolAt| {
                let Implements::Overrides(SameMethod(overrides)) = implements else {
                    return true;
                };
                let signature = |symbol: &StoredSymbol| {
                    symbol.signature.map(|text| graph.strings.resolve(text))
                };
                signature(symbol)
                    .zip(signature(graph.sym(at)))
                    .is_some_and(|(own, parent)| overrides(own, parent))
            };
            let parents = candidates
                .filter(|&at| {
                    cascade.class_of(at).is_some_and(|ty| lineage.contains(&ty)) && stands_for(at)
                })
                .collect();
            lineage.push(class);
            Dispatching {
                parents,
                lineage: Some(lineage),
            }
        }
    };
    let parents = &mut dispatching.parents;
    for parent in parents.iter_mut() {
        *parent = cascade.canonical(*parent);
    }
    parents.sort_unstable();
    parents.dedup();
    parents.retain(|&at| at != method);
    dispatching
}

/// The interfaces one type implements by its method set: the type `owner` names in the package of the file at
/// `package`.
struct MethodSets<'a, 'g> {
    types: &'a Types<'g>,
    cascade: &'a Cascade<'g>,
    same: SameMethod,
    owner: Str,
    package: u32,
}

impl MethodSets<'_, '_> {
    fn in_the_package(&self, file: u32) -> bool {
        let graph = self.cascade.graph;
        let (a, b) = (graph.file(file), graph.file(self.package));
        a.dir == b.dir && a.package == b.package
    }

    fn is_an_interface(&self, name: Str) -> bool {
        let graph = self.cascade.graph;
        graph
            .named(name)
            .iter()
            .any(|&at| graph.sym(at).kind == SymbolKind::Interface && self.in_the_package(at.file))
    }

    /// Whether `method` is a method of an interface the owner implements, declared in its package, or in one that its
    /// package imports or that imports its package.
    fn declares_satisfied(&self, method: SymbolAt) -> bool {
        let graph = self.cascade.graph;
        let interface = (graph.sym(method).owner).and_then(|owner| {
            (graph
                .in_file(graph.named(owner), method.file)
                .iter()
                .copied())
            .find(|&at| graph.sym(at).kind == SymbolKind::Interface)
        });
        interface.is_some_and(|interface| {
            (self.in_the_package(interface.file)
                || self.imports(self.package, interface.file)
                || self.imports(interface.file, self.package))
                && self.is_satisfied(interface, &mut Vec::new())
        })
    }

    // constraint: an import includes every file of the package it names that the importer's build reads, so the
    // includers of the first such file are the package's importers
    /// Whether the package of the file at `file` imports that of the file at `other`.
    fn imports(&self, file: u32, other: u32) -> bool {
        let graph = self.cascade.graph;
        let dir = graph.file(file).dir;
        let mut importers = (graph.package_files(graph.file(other).dir).iter())
            .map(|&held| graph.includers_of(held).peekable());
        importers
            .find_map(|mut includers| includers.peek().is_some().then_some(includers))
            .is_some_and(|mut includers| includers.any(|includer| graph.file(includer).dir == dir))
    }

    fn is_satisfied(&self, interface: SymbolAt, walked: &mut Vec<SymbolAt>) -> bool {
        if walked.contains(&interface) {
            return true;
        }
        walked.push(interface);
        let graph = self.cascade.graph;
        let name = graph.sym(interface).name;
        let mut methods = (graph.file(interface.file).symbols.iter())
            .filter(|symbol| symbol.kind == SymbolKind::Method && symbol.owner == Some(name))
            .peekable();
        let embedded = self.types.named_by_parents(interface);
        (methods.peek().is_some() || !embedded.is_empty())
            && methods.all(|declared| self.owner_declares(declared))
            && embedded.iter().all(|named| {
                named.iter().any(|&at| {
                    graph.sym(at).kind == SymbolKind::Interface && self.is_satisfied(at, walked)
                })
            })
    }

    fn owner_declares(&self, declared: &StoredSymbol) -> bool {
        let graph = self.cascade.graph;
        let strings = &graph.strings;
        let Some(spelled) = declared
            .signature
            .map(|signature| strings.resolve(signature))
        else {
            return false;
        };
        graph.named(declared.name).iter().any(|&at| {
            let symbol = graph.sym(at);
            symbol.kind == SymbolKind::Method
                && symbol.owner == Some(self.owner)
                && self.in_the_package(at.file)
                && (symbol.signature)
                    .is_some_and(|signature| (self.same.0)(strings.resolve(signature), spelled))
        })
    }
}
