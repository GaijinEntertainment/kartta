use foldhash::fast::FixedState;
use hashbrown::HashSet;

use kartta_lang::SymbolKind;

use crate::graph::{Definition, Graph, QUALIFIER_SEPARATORS};
use crate::resolve::{Cascade, is_type_kind};
use crate::scope::is_type;
use crate::stored::{Parent, SymbolAt};
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
    /// A compile-time assertion that a type implements an interface, as Go's `var _ I = T{}` in a file that does not
    /// declare `T`, makes the interface a parent of the type it names, read from the asserting file.
    /// A name selects among classes, structs, interfaces, traits and namespaces as [`Graph::definitions`] does: the
    /// type whose qualified name it is, else the type of that bare name, and where several share it, the one the
    /// most types name as a parent, the first in path order on a tie.
    ///
    /// A parent written in one file names a type the way the language's scopes reach it. Where a package is a
    /// directory, as in Go, a bare parent names a type of its own directory and a qualified one a type of the
    /// directory its import binds. Where a language joins an owner chain, as Ruby and Python do, the parent is
    /// looked up from the scope that encloses the type that names it, inside out, and a bare one that lookup does not
    /// reach names nothing. Where a language nests scopes, as C++ namespaces do, the same lookup runs out to the root,
    /// and the innermost scope that holds a type of the parent's spelling decides. Elsewhere, and for a parent those
    /// lookups do not reach, it names the type of that name nearest the file: in it, then in the files it includes,
    /// then in its directory, then anywhere, whose qualified name ends with what a qualified parent spells; types of
    /// the name tied at the nearest of those each count the parent. A type that implements an interface without naming
    /// it, as Go's structural typing allows, is not found. Every implementor within `depth` is found before `limit`
    /// applies, so the limit bounds the answer, not the work, and `total` counts them all.
    #[must_use]
    pub fn implementations(
        &self,
        name: &str,
        depth: u32,
        limit: usize,
    ) -> Option<Implementations<'_>> {
        let types = Types {
            graph: self,
            cascade: Cascade::new(self),
        };
        let (root, mut direct) = types.select(name)?;
        let mut visited: HashSet<SymbolAt, FixedState> =
            HashSet::with_hasher(FixedState::default());
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
            passed_over: types.passed_over(name, root),
            found,
            total,
            unexpanded: level.len(),
        })
    }
}

struct Types<'g> {
    graph: &'g Graph,
    cascade: Cascade<'g>,
}

impl Types<'_> {
    fn is_type_at(&self, at: SymbolAt) -> bool {
        is_type(self.graph.sym(at).kind)
    }

    fn select(&self, name: &str) -> Option<Selected> {
        let key = self.graph.strings.get(name);
        if let Some(key) = key {
            let qualified: Vec<SymbolAt> = self
                .cascade
                .qualified_as(key)
                .filter(|&at| self.is_type_at(at))
                .collect();
            if !qualified.is_empty() {
                return self.most_implemented(qualified);
            }
            let named: Vec<SymbolAt> = self
                .graph
                .named(key)
                .iter()
                .copied()
                .filter(|&at| self.is_type_at(at))
                .collect();
            if !named.is_empty() {
                return self.most_implemented(named);
            }
        }
        if !name.contains(QUALIFIER_SEPARATORS) {
            return None;
        }
        let suffixed: Vec<SymbolAt> = self
            .graph
            .selected(name)
            .filter(|&at| self.is_type_at(at))
            .collect();
        self.most_implemented(suffixed)
    }

    fn most_implemented(&self, mut candidates: Vec<SymbolAt>) -> Option<Selected> {
        candidates.sort_unstable_by_key(|&at| self.graph.order_of(at));
        candidates.dedup();
        let (&first, others) = candidates.split_first()?;
        if others.is_empty() {
            return Some((first, None));
        }
        let distinct = |ty: SymbolAt| {
            let mut implementors = self.implementors_of(ty);
            implementors.sort_unstable();
            implementors.dedup();
            implementors
        };
        let mut best = (first, distinct(first));
        for &at in others {
            let implementors = distinct(at);
            if implementors.len() > best.1.len() {
                best = (at, implementors);
            }
        }
        Some((best.0, Some(best.1)))
    }

    fn passed_over(&self, name: &str, chosen: SymbolAt) -> usize {
        let mut others: Vec<SymbolAt> = self
            .graph
            .selected(name)
            .filter(|&at| at != chosen && self.is_type_at(at))
            .collect();
        others.sort_unstable();
        others.dedup();
        others.len()
    }

    fn implementors_of(&self, ty: SymbolAt) -> Vec<SymbolAt> {
        let key = self.graph.sym(ty).name;
        let mut implementors = Vec::new();
        for &slot in self.graph.parenting_files(key) {
            self.declared_in(slot, ty, &mut implementors);
            self.asserted_in(slot, ty, &mut implementors);
        }
        implementors
    }

    fn declared_in(&self, slot: u32, ty: SymbolAt, implementors: &mut Vec<SymbolAt>) {
        let key = self.graph.sym(ty).name;
        let declared = self
            .graph
            .file(slot)
            .parents
            .iter()
            .map(|parent| {
                let child = SymbolAt {
                    file: slot,
                    index: parent.symbol,
                };
                (child, parent)
            })
            .filter(|&(child, parent)| {
                child != ty
                    && is_implementor_kind(self.graph.sym(child).kind)
                    && self.graph.site_key(parent.written) == Some(key)
                    && self.names(child, parent, ty)
            })
            .map(|(child, _)| child);
        implementors.extend(declared);
    }

    fn asserted_in(&self, slot: u32, ty: SymbolAt, implementors: &mut Vec<SymbolAt>) {
        let key = self.graph.sym(ty).name;
        let asserted = self
            .graph
            .file(slot)
            .assertions
            .iter()
            .filter(|assertion| {
                self.graph.site_key(assertion.iface) == Some(key)
                    && self.file_names(slot, assertion.iface, ty)
            })
            .filter_map(|assertion| self.asserted_type(slot, assertion.type_name))
            .filter(|&implementor| implementor != ty);
        implementors.extend(asserted);
    }

    fn names(&self, child: SymbolAt, parent: &Parent, ty: SymbolAt) -> bool {
        let as_written = self.graph.strings.resolve(parent.written);
        let written = as_written.trim_start_matches(QUALIFIER_SEPARATORS);
        let conventions = self.cascade.conventions;
        if conventions.directory_packages {
            return self.names_in_packages(child.file, written, ty);
        }
        if conventions.owner_chain_separator.is_some() {
            if let Some(owner) = self.cascade.base_owner(child, as_written) {
                return owner == self.graph.sym(ty).qualified_name;
            }
            if !written.contains(QUALIFIER_SEPARATORS) {
                return false;
            }
        }
        if parent.written_from_root {
            return self.nearest_of_its_name(child.file, Spelling::FromRoot(written), ty);
        }
        self.names_from_scopes(child, written, ty)
    }

    fn names_from_scopes(&self, child: SymbolAt, written: &str, ty: SymbolAt) -> bool {
        let nearest_spelled =
            || self.nearest_of_its_name(child.file, Spelling::Written(written), ty);
        let Some(separator) = self.cascade.conventions.scope_separator else {
            return nearest_spelled();
        };
        let strings = &self.graph.strings;
        let symbol = self.graph.sym(child);
        let enclosing = strings
            .resolve(symbol.qualified_name)
            .strip_suffix(strings.resolve(symbol.name))
            .and_then(|chain| chain.strip_suffix(separator))
            .unwrap_or_default();
        let qualified = |at: SymbolAt| strings.resolve(self.graph.sym(at).qualified_name);
        let depth = |at: SymbolAt| lookup_depth(qualified(at), enclosing, written, separator);
        let reached = self
            .graph
            .named(self.graph.sym(ty).name)
            .iter()
            .copied()
            .filter(|&at| self.is_type_at(at))
            .filter_map(depth)
            .max();
        reached.map_or_else(nearest_spelled, |innermost| {
            depth(ty) == Some(innermost)
                && self.nearest_of_its_name(child.file, Spelling::FromRoot(qualified(ty)), ty)
        })
    }

    fn file_names(&self, file: u32, written: Str, ty: SymbolAt) -> bool {
        let written = self
            .graph
            .strings
            .resolve(written)
            .trim_start_matches(QUALIFIER_SEPARATORS);
        if self.cascade.conventions.directory_packages {
            self.names_in_packages(file, written, ty)
        } else {
            self.nearest_of_its_name(file, Spelling::Written(written), ty)
        }
    }

    // constraint: an assertion names its implementing type as the file writes it, and a name that selects several
    // types, as an external test package shadowing its own package's type does, names none
    fn asserted_type(&self, file: u32, type_name: Str) -> Option<SymbolAt> {
        let written = self
            .graph
            .strings
            .resolve(type_name)
            .trim_start_matches(QUALIFIER_SEPARATORS);
        let last = written
            .rsplit(QUALIFIER_SEPARATORS)
            .next()
            .unwrap_or(written);
        let named = self.graph.named(self.graph.strings.get(last)?);
        let mut types = named.iter().copied().filter(|&at| {
            is_type_kind(self.graph.sym(at).kind) && self.file_names(file, type_name, at)
        });
        let found = types.next()?;
        types.next().is_none().then_some(found)
    }

    fn names_in_packages(&self, file: u32, written: &str, ty: SymbolAt) -> bool {
        let dir = self.graph.file(ty.file).dir;
        match written.rsplit_once(QUALIFIER_SEPARATORS) {
            Some((qualifier, _)) => {
                self.graph
                    .strings
                    .get(qualifier)
                    .and_then(|qualifier| self.cascade.binding(file, qualifier))
                    == Some(Some(dir))
            }
            None => self.graph.file(file).dir == dir,
        }
    }

    fn nearest_of_its_name(&self, file: u32, spelling: Spelling<'_>, ty: SymbolAt) -> bool {
        if !self.is_type_at(ty) || !self.is_spelled(ty, spelling) {
            return false;
        }
        if ty.file == file {
            return true;
        }
        let closure = self.cascade.closure(file);
        let dir = self.graph.file(file).dir;
        let scope = |at: SymbolAt| {
            if at.file == file {
                0
            } else if closure.binary_search(&at.file).is_ok() {
                1
            } else if self.graph.file(at.file).dir == dir {
                2
            } else {
                3
            }
        };
        let reach = scope(ty);
        !self
            .graph
            .named(self.graph.sym(ty).name)
            .iter()
            .any(|&at| self.is_type_at(at) && scope(at) < reach && self.is_spelled(at, spelling))
    }

    fn is_spelled(&self, at: SymbolAt, spelling: Spelling<'_>) -> bool {
        let qualified = self
            .graph
            .strings
            .resolve(self.graph.sym(at).qualified_name);
        match spelling {
            Spelling::FromRoot(written) => qualified == written,
            Spelling::Written(written) => {
                !written.contains(QUALIFIER_SEPARATORS)
                    || qualified == written
                    || qualified
                        .strip_suffix(written)
                        .is_some_and(|head| head.ends_with(QUALIFIER_SEPARATORS))
            }
        }
    }
}

type Selected = (SymbolAt, Option<Vec<SymbolAt>>);

#[derive(Debug, Clone, Copy)]
enum Spelling<'w> {
    FromRoot(&'w str),
    Written(&'w str),
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
    is_type_kind(kind) || matches!(kind, SymbolKind::Namespace)
}
