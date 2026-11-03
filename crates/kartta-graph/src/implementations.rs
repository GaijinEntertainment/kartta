use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};

use kartta_lang::{SymbolKind, ValueUses};

use crate::graph::{
    Candidate, Definition, Graph, QUALIFIER_SEPARATORS, Selector, distinct, is_parent,
    last_segment, names_as_parent,
};
use crate::resolve::Cascade;
use crate::stored::{Parent, StoredAssertion, SymbolAt};
use crate::strings::Str;

/// A type that names another as its parent, or names one of that type's implementors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Implementor<'g> {
    /// The implementing, extending or including definition.
    pub symbol: Definition<'g>,
    /// How many parents away from the type asked about it is: 1 names that type as a parent.
    pub depth: u32,
}

/// The types that implement, extend or include one type, transitively, and what the query left out.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Implementations<'g> {
    /// The type the name selects.
    pub root: Definition<'g>,
    /// The other types [`Graph::definitions`] lists for the name, whose implementors these are not.
    pub passed_over: usize,
    /// The most implemented of those, the most direct implementors first, then in path order, at most five.
    pub candidates: Vec<Candidate<'g>>,
    /// The implementors, each once, nearest first, then in path order, at most the query's limit.
    pub found: Vec<Implementor<'g>>,
    /// The implementors within the depth bound, before the limit.
    pub total: usize,
    /// The implementors at the depth bound whose own implementors the query did not look for, and the type asked
    /// about itself at a depth of 0.
    pub unexpanded: usize,
}

impl Graph {
    /// The types that name the type `name` selects as a parent, transitively up to `depth` parents away, at most
    /// `limit` of them, or `None` when the name selects no type.
    ///
    /// A parent is what a language records a type to implement, extend, embed or include: a base class, an
    /// interface, a trait, a mixed-in module; only a type, an enum or a namespace that records one is an implementor.
    /// A compile-time assertion that a type implements an interface, as Go's `var _ I = T{}` or Rust's `impl I for T`
    /// in a file that does not declare `T`, makes the interface a parent of the type it names, read from the asserting
    /// file, or from the file a Rust path reaches, and each name adds nothing where it reaches no one type.
    /// A name selects among the kinds [`is_parent`] admits as [`Graph::definitions`]
    /// does: the type whose qualified name it is, else the type of that bare name, and where several share it, the one
    /// the most distinct types name as a direct parent, the first in path order on a tie; else, for a name with a
    /// separator, the type whose qualified name ends with it, and none where several do; a path prefix and a line narrow
    /// them as [`Graph::callers`] describes.
    ///
    /// A parent written in one file names a type the way the language's scopes reach it, a type alias among them: a
    /// parent that reaches an alias names the alias, whose implementors are not the aliased type's. Where a package is
    /// a directory, as in Go, a bare parent names a type of its own directory and a qualified one a type of the
    /// directory its import binds. Where a language joins an owner chain, as Ruby, Python, Rust, TypeScript and
    /// JavaScript do, the parent is looked up from the scope that encloses the type that names it, inside out, and a
    /// bare one that lookup does not reach names nothing; where a module also imports a name from the module that
    /// exports it, as TypeScript does, the lookup reaches only the types of the naming file and of the files it
    /// imports, however deep. Where a language nests scopes, as C++ namespaces do, the same lookup runs out to the
    /// root, and the innermost scope that holds a type of the parent's spelling decides. A parent written from the
    /// root, as every parent is under [`ParentSpelling::FromRoot`](kartta_lang::ParentSpelling::FromRoot), names the
    /// type of exactly that qualified name. Elsewhere, and for a parent those lookups do not reach, it names the type
    /// of that name nearest the file: in it, then in the files it
    /// includes, then in its directory, then anywhere, whose qualified name ends with what a qualified parent spells;
    /// types of the name tied at the nearest of those each count the parent. A type that implements an interface
    /// without naming it, as Go's structural typing allows, is not found. Every implementor within `depth` is found
    /// before `limit` applies, so the limit bounds the answer, not the work, and `total` counts them all.
    #[must_use]
    pub fn implementations<'s>(
        &self,
        name: impl Into<Selector<'s>>,
        depth: u32,
        limit: usize,
    ) -> Option<Implementations<'_>> {
        let selector = name.into();
        let types = Types::new(self);
        let Selected {
            root,
            mut direct,
            implementor_counts,
        } = types.select(selector)?;
        let (passed_over, candidates) = types.passed_over(selector, root, &implementor_counts);
        let mut visited: HashSet<SymbolAt, FixedState> = HashSet::default();
        visited.insert(root);
        let mut level = vec![root];
        let mut found: Vec<Implementor<'_>> = Vec::new();
        let mut total = 0;

        for at_depth in 1..=depth {
            if level.is_empty() {
                break;
            }
            let mut next = direct.take().unwrap_or_else(|| {
                level
                    .iter()
                    .flat_map(|&ty| types.implementors_of(ty))
                    .collect()
            });
            next.retain(|&at| visited.insert(at));
            next.sort_unstable_by_key(|&at| self.order_of(at));
            total += next.len();
            let unlisted = limit.saturating_sub(found.len());
            found.extend(next.iter().take(unlisted).map(|&at| Implementor {
                symbol: self.definition_at(at),
                depth: at_depth,
            }));
            level = next;
        }

        Some(Implementations {
            root: self.definition_at(root),
            passed_over,
            candidates,
            found,
            total,
            unexpanded: level.len(),
        })
    }
}

pub(crate) struct Types<'g> {
    graph: &'g Graph,
    cascade: Cascade<'g>,
}

impl<'g> Types<'g> {
    pub(crate) fn new(graph: &'g Graph) -> Self {
        Self {
            graph,
            cascade: Cascade::new(graph),
        }
    }

    pub(crate) fn is_a_parent(&self, ty: SymbolAt) -> bool {
        !self.implementors_of(ty).is_empty()
    }

    fn passed_over(
        &self,
        selector: Selector<'_>,
        chosen: SymbolAt,
        implementor_counts: &ImplementorCounts,
    ) -> (usize, Vec<Candidate<'g>>) {
        let others = distinct(
            self.graph
                .selected(selector.name)
                .filter(|&at| at != chosen && self.is_selectable(at, selector)),
        );
        let count = others.len();
        let uncounted: Vec<SymbolAt> = others
            .iter()
            .copied()
            .filter(|at| !implementor_counts.contains_key(at))
            .collect();
        let counted = self.implementors_of_each(&uncounted);
        let ranked = others
            .into_iter()
            .map(|at| {
                let count = implementor_counts
                    .get(&at)
                    .copied()
                    .or_else(|| counted.get(&at).map(Vec::len))
                    .unwrap_or_default();
                (at, count)
            })
            .collect();
        (count, self.graph.candidates(ranked))
    }
}

impl Types<'_> {
    fn is_type_at(&self, at: SymbolAt) -> bool {
        names_as_parent(self.graph.sym(at).kind)
    }

    fn is_selectable(&self, at: SymbolAt, selector: Selector<'_>) -> bool {
        is_parent(self.graph.sym(at).kind) && self.graph.is_within(at, selector)
    }

    fn select(&self, selector: Selector<'_>) -> Option<Selected> {
        let key = self.graph.strings.get(selector.name);
        if let Some(key) = key {
            let qualified: Vec<SymbolAt> = self
                .cascade
                .qualified_as(key)
                .filter(|&at| self.is_selectable(at, selector))
                .collect();
            if !qualified.is_empty() {
                return self.most_implemented(qualified);
            }
            let named: Vec<SymbolAt> = self
                .graph
                .named(key)
                .iter()
                .copied()
                .filter(|&at| self.is_selectable(at, selector))
                .collect();
            if !named.is_empty() {
                return self.most_implemented(named);
            }
        }
        if !selector.name.contains(QUALIFIER_SEPARATORS) {
            return None;
        }
        let mut suffixed = self
            .graph
            .selected(selector.name)
            .filter(|&at| self.is_selectable(at, selector));
        let first = suffixed.next()?;
        suffixed.all(|at| at == first).then(|| Selected {
            root: first,
            direct: None,
            implementor_counts: ImplementorCounts::default(),
        })
    }

    fn most_implemented(&self, mut candidates: Vec<SymbolAt>) -> Option<Selected> {
        candidates.sort_unstable_by_key(|&at| self.graph.order_of(at));
        candidates.dedup();
        let (&first, others) = candidates.split_first()?;
        if others.is_empty() {
            return Some(Selected {
                root: first,
                direct: None,
                implementor_counts: ImplementorCounts::default(),
            });
        }
        let mut implementors = self.implementors_of_each(&candidates);
        let count = |at: SymbolAt| implementors.get(&at).map_or(0, Vec::len);
        let mut best = first;
        for &at in others {
            if count(at) > count(best) {
                best = at;
            }
        }
        let implementor_counts = implementors
            .iter()
            .map(|(&at, found)| (at, found.len()))
            .collect();
        Some(Selected {
            root: best,
            direct: implementors.remove(&best),
            implementor_counts,
        })
    }

    fn implementors_of(&self, ty: SymbolAt) -> Vec<SymbolAt> {
        let mut implementors = Vec::new();
        let asked = Namesakes {
            name: self.graph.sym(ty).name,
            types: std::slice::from_ref(&ty),
        };
        self.implementors_among(asked, &mut |_, implementor| implementors.push(implementor));
        implementors
    }

    fn implementors_of_each(&self, types: &[SymbolAt]) -> ImplementorsOfEach {
        let mut types = types.to_vec();
        types.sort_unstable_by_key(|&at| (self.graph.sym(at).name, at));
        types.dedup();
        let mut found: Vec<(SymbolAt, SymbolAt)> = Vec::new();
        for namesakes in types.chunk_by(|&a, &b| self.graph.sym(a).name == self.graph.sym(b).name) {
            let asked = Namesakes {
                name: self.graph.sym(namesakes[0]).name,
                types: namesakes,
            };
            self.implementors_among(asked, &mut |ty, implementor| found.push((ty, implementor)));
        }
        found.sort_unstable();
        found.dedup();
        let mut each: ImplementorsOfEach = types.iter().map(|&ty| (ty, Vec::new())).collect();
        for (ty, implementor) in found {
            each.entry(ty).or_default().push(implementor);
        }
        each
    }

    fn implementors_among(&self, asked: Namesakes<'_>, found: &mut impl FnMut(SymbolAt, SymbolAt)) {
        for &slot in self.graph.parenting_files(asked.name) {
            self.declared_in(slot, asked, found);
            self.asserted_in(slot, asked, found);
        }
    }

    fn declared_in(
        &self,
        slot: u32,
        asked: Namesakes<'_>,
        found: &mut impl FnMut(SymbolAt, SymbolAt),
    ) {
        for parent in &self.graph.file(slot).parents {
            let child = SymbolAt::new(slot, parent.symbol);
            if !is_implementor_kind(self.graph.sym(child).kind)
                || self.graph.listing_key(parent.written) != Some(asked.name)
            {
                continue;
            }
            self.named_by(child, parent, asked)
                .into_iter()
                .filter(|&ty| ty != child)
                .for_each(|ty| found(ty, child));
        }
    }

    fn asserted_in(
        &self,
        slot: u32,
        asked: Namesakes<'_>,
        found: &mut impl FnMut(SymbolAt, SymbolAt),
    ) {
        for assertion in &self.graph.file(slot).assertions {
            if self.graph.listing_key(assertion.interface) != Some(asked.name) {
                continue;
            }
            let named = self.asserted_interface(slot, assertion, asked);
            if named.is_empty() {
                continue;
            }
            let written = (assertion.type_name, assertion.type_file);
            let Some(implementor) = self.asserted_name(slot, written) else {
                continue;
            };
            named
                .into_iter()
                .filter(|&ty| ty != implementor)
                .for_each(|ty| found(ty, implementor));
        }
    }

    // constraint: where a package is not a directory, a file's scope reaches one type of the name
    fn asserted_interface(
        &self,
        slot: u32,
        assertion: &StoredAssertion,
        asked: Namesakes<'_>,
    ) -> Vec<SymbolAt> {
        let interface = assertion.interface;
        if self.cascade.conventions.directory_packages {
            return self.file_named_by(slot, interface, asked);
        }
        (self
            .asserted_name(slot, (interface, assertion.interface_file))
            .into_iter())
        .filter(|at| asked.types.contains(at))
        .collect()
    }

    // constraint: a name its file does not declare comes in through the one glob whose module's file declares it, as a
    // file's own item shadows a glob's and two globs' names cannot be told apart
    fn read_from(&self, slot: u32, file: Option<Str>, types: &[SymbolAt]) -> u32 {
        if let Some(file) = file {
            return self.graph.slot_of(file).unwrap_or(slot);
        }
        let declared_in = |slot: &u32| types.iter().any(|at| at.file == *slot);
        if declared_in(&slot) {
            return slot;
        }
        let mut globbed = (self.cascade.dot_imported_dirs(slot).into_iter())
            .filter_map(|target| self.graph.slot_of(target))
            .filter(declared_in);
        match (globbed.next(), globbed.next()) {
            (Some(module), None) => module,
            _ => slot,
        }
    }

    fn named_by(&self, child: SymbolAt, parent: &Parent, asked: Namesakes<'_>) -> Vec<SymbolAt> {
        let as_written = self.graph.strings.resolve(parent.written);
        let written = as_written.trim_start_matches(QUALIFIER_SEPARATORS);
        let conventions = self.cascade.conventions;
        if conventions.directory_packages {
            return self.in_packages(child.file, written, asked);
        }
        if conventions.owner_chain_separator.is_some() {
            if let Some(owner) = self.cascade.base_owner(child, as_written, names_as_parent) {
                return self.owners_reached(child.file, owner, asked);
            }
            if !written.contains(QUALIFIER_SEPARATORS) {
                return Vec::new();
            }
        }
        if parent.written_from_root {
            return self.nearest_of_its_name(child.file, Spelling::FromRoot(written), asked);
        }
        self.named_from_scopes(child, written, asked)
    }

    fn owners_reached(&self, file: u32, owner: Str, asked: Namesakes<'_>) -> Vec<SymbolAt> {
        let imported = matches!(
            self.cascade.conventions.value_uses,
            ValueUses::Imported | ValueUses::Expanded
        )
        .then(|| self.cascade.closure(file));
        asked
            .types
            .iter()
            .copied()
            .filter(|&ty| {
                self.graph.sym(ty).qualified_name == owner
                    && imported
                        .as_ref()
                        .is_none_or(|reached| reached.binary_search(&ty.file).is_ok())
            })
            .collect()
    }

    fn named_from_scopes(
        &self,
        child: SymbolAt,
        written: &str,
        asked: Namesakes<'_>,
    ) -> Vec<SymbolAt> {
        let Some(separator) = self.cascade.conventions.scope_separator else {
            return self.nearest_of_its_name(child.file, Spelling::Written(written), asked);
        };
        let enclosing = self
            .graph
            .enclosing_scope(child, separator)
            .unwrap_or_default();
        let qualified = |at: SymbolAt| {
            self.graph
                .strings
                .resolve(self.graph.sym(at).qualified_name)
        };
        let depth = |at: SymbolAt| lookup_depth(qualified(at), enclosing, written, separator);
        let reached = self
            .graph
            .named(asked.name)
            .iter()
            .copied()
            .filter(|&at| self.is_type_at(at))
            .filter_map(depth)
            .max();
        let Some(innermost) = reached else {
            return self.nearest_of_its_name(child.file, Spelling::Written(written), asked);
        };
        let mut reached: Vec<SymbolAt> = asked
            .types
            .iter()
            .copied()
            .filter(|&at| depth(at) == Some(innermost))
            .collect();
        reached.sort_unstable_by_key(|&at| self.graph.sym(at).qualified_name);
        reached
            .chunk_by(|&a, &b| self.graph.sym(a).qualified_name == self.graph.sym(b).qualified_name)
            .flat_map(|copies| {
                let copies = Namesakes {
                    name: asked.name,
                    types: copies,
                };
                self.nearest_of_its_name(
                    child.file,
                    Spelling::FromRoot(qualified(copies.types[0])),
                    copies,
                )
            })
            .collect()
    }

    fn file_named_by(&self, file: u32, written: Str, asked: Namesakes<'_>) -> Vec<SymbolAt> {
        let written = self
            .graph
            .strings
            .resolve(written)
            .trim_start_matches(QUALIFIER_SEPARATORS);
        if self.cascade.conventions.directory_packages {
            self.in_packages(file, written, asked)
        } else {
            self.nearest_of_its_name(file, Spelling::Asserted(written), asked)
        }
    }

    // constraint: an assertion names its implementing type as the file writes it: a type the file declares is the one
    // a bare name means, and any other name that selects several types, as an external test package shadowing its own
    // package's type does, names none; a member, as an associated type, is no such name, and a type a name alone
    // reaches lies within what the file's package is built with
    fn asserted_name(&self, slot: u32, (written, file): (Str, Option<Str>)) -> Option<SymbolAt> {
        let (name, mut types) = self.of_last_segment(written)?;
        let gate = self.cascade.gate(slot);
        types.retain(|&at| {
            self.graph.sym(at).owner.is_none()
                && (gate.as_ref()).is_none_or(|gate| gate.admits_at(self.graph, at))
        });
        let file = self.read_from(slot, file, &types);
        if name == written
            && let Some(&own) = types.iter().find(|at| at.file == file)
        {
            return Some(own);
        }
        match self.file_named_by(
            file,
            written,
            Namesakes {
                name,
                types: &types,
            },
        )[..]
        {
            [found] => Some(found),
            _ => None,
        }
    }

    /// The types `written` names from `file`, read as a parent that file writes.
    pub(crate) fn named_in_file(&self, file: u32, written: Str) -> Vec<SymbolAt> {
        self.of_last_segment(written)
            .map_or_else(Vec::new, |(name, types)| {
                self.file_named_by(
                    file,
                    written,
                    Namesakes {
                        name,
                        types: &types,
                    },
                )
            })
    }

    fn of_last_segment(&self, written: Str) -> Option<(Str, Vec<SymbolAt>)> {
        let written = self
            .graph
            .strings
            .resolve(written)
            .trim_start_matches(QUALIFIER_SEPARATORS);
        let name = self.graph.strings.get(last_segment(written))?;
        let types = self
            .graph
            .named(name)
            .iter()
            .copied()
            .filter(|&at| self.graph.sym(at).kind.is_type())
            .collect();
        Some((name, types))
    }

    fn in_packages(&self, file: u32, written: &str, asked: Namesakes<'_>) -> Vec<SymbolAt> {
        let dir = match written.rsplit_once(QUALIFIER_SEPARATORS) {
            Some((qualifier, _)) => self
                .graph
                .strings
                .get(qualifier)
                .and_then(|qualifier| self.cascade.binding(file, qualifier))
                .flatten(),
            None => Some(self.graph.file(file).dir),
        };
        asked
            .types
            .iter()
            .copied()
            .filter(|&ty| Some(self.graph.file(ty.file).dir) == dir)
            .collect()
    }

    fn nearest_of_its_name(
        &self,
        file: u32,
        spelling: Spelling<'_>,
        asked: Namesakes<'_>,
    ) -> Vec<SymbolAt> {
        let spelled =
            |at: SymbolAt| self.is_of_spelled_kind(at, spelling) && self.is_spelled(at, spelling);
        let mut nearest: Vec<SymbolAt> = asked
            .types
            .iter()
            .copied()
            .filter(|&at| spelled(at))
            .collect();
        if nearest.iter().all(|&at| at.file == file) {
            return nearest;
        }
        let distance = self.cascade.distance_from(file);
        let reach = self
            .graph
            .named(asked.name)
            .iter()
            .copied()
            .filter(|&at| spelled(at))
            .map(&distance)
            .min();
        nearest.retain(|&ty| ty.file == file || reach.is_none_or(|reach| distance(ty) <= reach));
        nearest
    }

    fn is_of_spelled_kind(&self, at: SymbolAt, spelling: Spelling<'_>) -> bool {
        match spelling {
            Spelling::Asserted(_) => self.graph.sym(at).kind.is_type(),
            Spelling::FromRoot(_) | Spelling::Written(_) => self.is_type_at(at),
        }
    }

    fn is_spelled(&self, at: SymbolAt, spelling: Spelling<'_>) -> bool {
        let qualified = self
            .graph
            .strings
            .resolve(self.graph.sym(at).qualified_name);
        match spelling {
            Spelling::FromRoot(written) => qualified == written,
            Spelling::Asserted(written) | Spelling::Written(written) => {
                !written.contains(QUALIFIER_SEPARATORS)
                    || qualified == written
                    || qualified
                        .strip_suffix(written)
                        .is_some_and(|head| head.ends_with(QUALIFIER_SEPARATORS))
            }
        }
    }
}

/// The direct implementors each type a name selects has, as its selection counted them.
type ImplementorCounts = HashMap<SymbolAt, usize, FixedState>;

/// The direct implementors of each type asked about, each once, in symbol order.
type ImplementorsOfEach = HashMap<SymbolAt, Vec<SymbolAt>, FixedState>;

/// The types of one name whose implementors one read of the parents written for that name looks for.
#[derive(Debug, Clone, Copy)]
struct Namesakes<'a> {
    name: Str,
    types: &'a [SymbolAt],
}

struct Selected {
    root: SymbolAt,
    direct: Option<Vec<SymbolAt>>,
    implementor_counts: ImplementorCounts,
}

#[derive(Debug, Clone, Copy)]
enum Spelling<'w> {
    FromRoot(&'w str),
    Written(&'w str),
    // constraint: an assertion's implementing type can be an enum, which no parent names
    Asserted(&'w str),
}

fn lookup_depth(qualified: &str, enclosing: &str, written: &str, separator: &str) -> Option<usize> {
    let head = qualified.strip_suffix(written)?;
    if head.is_empty() {
        return Some(0);
    }
    let scope = head.strip_suffix(separator)?;
    let encloses = enclosing == scope
        || enclosing
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with(separator));
    encloses.then(|| scope.split(separator).count())
}

const fn is_implementor_kind(kind: SymbolKind) -> bool {
    kind.is_type() || matches!(kind, SymbolKind::Namespace)
}
