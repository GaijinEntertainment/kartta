use std::cmp::{Ordering, Reverse};
use std::hash::Hash;
use std::sync::{Arc, OnceLock};

use foldhash::fast::FixedState;
use hashbrown::{HashMap, HashSet};
use kartta_lang::{
    Conventions, Extraction, IncludeReach, ParentSpelling, Receiver, Receivers, Symbol,
    SymbolFlags, SymbolId, SymbolKind, path,
};

use crate::order::PathOrder;
use crate::resolve::derives;
use crate::stored::{
    Entry, Folded, Local, Occurrence, Parent, Site, StoredAssertion, StoredBinding, StoredCall,
    StoredCallable, StoredFile, StoredLocalCall, StoredReExport, StoredSymbol, StoredUse, SymbolAt,
    UseAt, UseReceiver, VALUE_TYPE_SEPARATOR, ValueTypes,
};
use crate::strings::{Str, Strings};

pub(crate) const QUALIFIER_SEPARATORS: [char; 5] = ['.', ':', '\\', '#', '/'];

/// The definitions and calls of one language family's files, kept a file at a time and found by name.
///
/// A file goes in as its [`Extraction`] and comes out whole: [`Graph::insert`] replaces whatever the graph held for
/// the same path, and [`Graph::remove`] forgets it, so a graph follows a tree that changes a file at a time without
/// being built again. The graph names no language; what differs between languages reaches it as the extractions and
/// the [`Conventions`] it is built with.
#[derive(Debug, Default)]
pub struct Graph {
    pub(crate) conventions: Conventions,
    pub(crate) strings: Strings,
    pub(crate) files: Vec<Option<StoredFile>>,
    vacant: Vec<u32>,
    by_path: HashMap<Str, u32, FixedState>,
    by_name: HashMap<Str, Vec<SymbolAt>, FixedState>,
    order: PathOrder,
    sites: HashMap<Str, Vec<u32>, FixedState>,
    uses: HashMap<Str, Vec<u32>, FixedState>,
    callables: HashMap<Str, Vec<Occurrence>, FixedState>,
    package_files: HashMap<Str, Vec<u32>, FixedState>,
    // constraint: kept past its package's last file, as a package's few bytes of list cost less than tracking its files
    built_with: HashMap<Str, BuiltWith, FixedState>,
    renamed_from: HashMap<Str, Vec<Rename>, FixedState>,
    included_by: HashMap<Str, Vec<u32>, FixedState>,
    test_included_by: HashMap<Str, Vec<u32>, FixedState>,
    test_modules: OnceLock<HashSet<u32, FixedState>>,
    parented: HashMap<Str, Vec<u32>, FixedState>,
    named_at_file_scope: HashMap<Str, Vec<u32>, FixedState>,
    // constraint: by slot and apart from `StoredFile`, as most files of a language that has any hold none and most
    // languages have none
    build_constraints: HashMap<u32, Str, FixedState>,
    pub(crate) symbols: usize,
}

// constraint: the list as the extraction wrote it, then its manifests sorted
type BuiltWith = (Arc<str>, Box<[Str]>);

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rename {
    pub(crate) file: u32,
    pub(crate) local_name: Str,
}

/// One definition, as a query answers it, borrowed from the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Definition<'g> {
    /// The identifier as written.
    pub name: &'g str,
    /// The identifier qualified by what encloses it, as the language writes it.
    pub qualified_name: &'g str,
    /// The type or namespace that declares a member.
    pub owner: Option<&'g str>,
    /// What the definition is.
    pub kind: SymbolKind,
    /// The root-relative path of the file that holds it.
    pub file: &'g str,
    /// The one-based line it starts on.
    pub line: u32,
    /// The one-based line it ends on, never before `line`.
    pub end_line: u32,
    /// The declaration up to its body, where the language keeps one.
    pub signature: Option<&'g str>,
    /// Whether the definition is visible outside its module.
    pub exported: bool,
    /// Test code, generated code, vendored code, an entry point.
    pub flags: SymbolFlags,
}

/// How near a definition sits to the file a lookup asks from, nearest first. It ranks the likelier meaning of a name;
/// it proves none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Nearness {
    /// In the directory of the file the lookup asks from.
    SameDirectory,
    /// Anywhere else, and everywhere when the lookup asks from no file.
    Elsewhere,
}

impl Nearness {
    /// The word for the nearness in an answer: `same_directory` or `elsewhere`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SameDirectory => "same_directory",
            Self::Elsewhere => "elsewhere",
        }
    }
}

/// A definition a name selects, and how near it sits to the file the lookup asks from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Found<'g> {
    /// The definition.
    pub definition: Definition<'g>,
    /// How near it sits.
    pub nearness: Nearness,
}

/// The definitions a name selects, nearest first, cut to a limit: `found` holds at most the limit, and `total`
/// counts them all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Definitions<'g> {
    /// Every definition the name selects, before the limit.
    pub total: usize,
    /// The nearest of them, ranked.
    pub found: Vec<Found<'g>>,
}

/// A name a query selects a symbol by, and the files and the line it looks for the symbol in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Selector<'s> {
    /// The name, bare or qualified, read as [`Graph::definitions`] reads it.
    pub name: &'s str,
    /// The start of the root-relative path of the file that holds the symbol: one file's path, or a directory's with
    /// its closing `/`. Copies of one symbol that share its qualified name, as a vendored package and the same
    /// package in a second checkout, are told apart by it. `None` looks in every file.
    pub path_prefix: Option<&'s str>,
    /// The one-based line the symbol starts on, as [`Definition::line`] gives it. Definitions of one name in one
    /// file, as C++ overloads or a declaration beside its out-of-line definition, are told apart by it. `None` looks
    /// at every line.
    pub line: Option<u32>,
}

impl<'s> Selector<'s> {
    /// The symbol `name` selects in the files whose path starts with `path_prefix`, or in any file for `None`, on any
    /// line.
    #[must_use]
    pub const fn new(name: &'s str, path_prefix: Option<&'s str>) -> Self {
        Self {
            name,
            path_prefix,
            line: None,
        }
    }

    /// This selector, looking only at the symbols that start on `line`, or on any line for `None`.
    #[must_use]
    pub const fn with_line(self, line: Option<u32>) -> Self {
        Self { line, ..self }
    }

    /// This selector with `name` in place of its own, looking in the same files and at the same line.
    #[must_use]
    pub const fn with_name<'n>(self, name: &'n str) -> Selector<'n>
    where
        's: 'n,
    {
        Selector {
            name,
            path_prefix: self.path_prefix,
            line: self.line,
        }
    }

    /// Whether a definition that starts on `line` of the file at the root-relative `path` is one the selector looks at.
    #[must_use]
    pub fn admits(&self, path: &str, line: u32) -> bool {
        self.path_prefix
            .is_none_or(|prefix| path.starts_with(prefix))
            && self.line.is_none_or(|own| own == line)
    }
}

impl<'s> From<&'s str> for Selector<'s> {
    fn from(name: &'s str) -> Self {
        Self::new(name, None)
    }
}

/// How many of the symbols a name selects and a query passes over its answer lists.
pub(crate) const LISTED_CANDIDATES: usize = 5;

/// A symbol a name selects that a query passed over, and the count the query ranks a name's symbols by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Candidate<'g> {
    /// The symbol.
    pub symbol: Definition<'g>,
    /// Its callers for a walk over calls, its sites for references, the types that name it as a parent for
    /// implementations.
    pub count: usize,
}

impl Graph {
    /// An empty graph for files whose calls resolve under `conventions`.
    #[must_use]
    pub fn new(conventions: Conventions) -> Self {
        Self {
            conventions,
            ..Self::default()
        }
    }

    /// The conventions the graph was made with, which its files' calls resolve under and their uses are recorded by.
    #[must_use]
    pub const fn conventions(&self) -> &Conventions {
        &self.conventions
    }

    /// Takes in one file's extraction, replacing whatever the graph held for the same path.
    ///
    /// Files can arrive in any order: the graph keeps them in path order, and placing a file among those it holds
    /// renumbers a few hundred of them, and every file once in a run of at least four thousand inserts at one place.
    pub fn insert(&mut self, extraction: &Extraction) {
        self.test_modules.take();
        let path = self.strings.intern(&extraction.file);
        let slot = if let Some(&held) = self.by_path.get(&path) {
            self.forget(held);
            held
        } else {
            let slot = self.vacant.pop().unwrap_or_else(|| {
                self.files.push(None);
                narrow(self.files.len() - 1)
            });
            self.place_in_path_order(slot, &extraction.file);
            self.by_path.insert(path, slot);
            slot
        };
        let file = self.store(path, extraction);
        if let (Some(package), Some(dependencies)) = (file.package, &extraction.dependencies) {
            self.note_built_with(package, dependencies);
        }
        self.symbols += file.symbols.len();
        self.files[slot as usize] = Some(file);
        if let Some(constraint) = &extraction.build_constraint {
            let constraint = self.strings.intern(constraint);
            self.build_constraints.insert(slot, constraint);
        }
        self.index(slot);
        for target in &extraction.test_includes {
            let Some(target) = self.strings.get(target) else {
                continue;
            };
            if self.file(slot).includes.binary_search(&target).is_ok() {
                self.test_included_by.entry(target).or_default().push(slot);
            }
        }
    }

    /// Forgets the file at `path`, and answers whether the graph held it.
    pub fn remove(&mut self, path: &str) -> bool {
        let Some(slot) = (self.strings.get(path)).and_then(|path| self.by_path.remove(&path))
        else {
            return false;
        };
        self.test_modules.take();
        self.forget(slot);
        self.order.remove(slot);
        self.vacant.push(slot);
        true
    }

    /// The definitions in the file at `path`, ordered by the line each starts on, one that encloses others before
    /// them, or `None` when the graph does not hold the file. A file the graph holds with no definition answers an
    /// empty outline: its language found none, or could not parse it.
    #[must_use]
    pub fn outline(&self, path: &str) -> Option<Vec<Definition<'_>>> {
        let slot = self.slot_of(self.strings.get(path)?)?;
        let whole = self.overlapping(slot, &(0..=u32::MAX));
        Some(whole.into_iter().map(|at| self.definition_at(at)).collect())
    }

    /// The definitions `name` selects, nearest to the file `from` first, then those outside test code, then those
    /// outside vendored code, then by path and line, at most `limit` of them.
    ///
    /// A bare name selects the definitions of that name. A qualified one, `Owner.parse` or `ns::Owner::parse`,
    /// selects every definition whose qualified name equals it or ends with it at a separator, and every definition
    /// of its last segment whose own qualified name, of two segments or more, it ends with: `ns::Owner::parse` selects
    /// a member qualified as `Owner::parse`. Its last segment is the part after its last separator; where neither the
    /// name nor that part selects a definition, it is each longer part after a separator, so a name whose own last
    /// part holds a separator, `das::operator/`, `Sprite.operator . frameSize :=` or `Kartta.#handle`, selects the
    /// definition it names. The separators are those the languages qualify with: `.`, `:`, `\`, `#` and `/`.
    #[must_use]
    pub fn definitions(&self, name: &str, from: Option<&str>, limit: usize) -> Definitions<'_> {
        if limit == 0 {
            return Definitions {
                total: self.selected(name).count(),
                found: Vec::new(),
            };
        }

        let from_dir = from.map(path::dir);
        let mut found: Vec<(usize, Found<'_>)> = self
            .selected(name)
            .map(|at| {
                let definition = self.definition_at(at);
                Found {
                    nearness: match from_dir {
                        Some(dir) if path::dir(definition.file) == dir => Nearness::SameDirectory,
                        _ => Nearness::Elsewhere,
                    },
                    definition,
                }
            })
            .enumerate()
            .collect();
        let total = found.len();
        keep_first(&mut found, limit, |left, right| {
            rank_key(left).cmp(&rank_key(right))
        });
        found.sort_unstable_by_key(rank_key);

        Definitions {
            total,
            found: found.into_iter().map(|(_, found)| found).collect(),
        }
    }

    pub(crate) fn selected<'a>(&'a self, name: &'a str) -> impl Iterator<Item = SymbolAt> + 'a {
        let mut tails = tails(name);
        let ending = tails
            .next()
            .into_iter()
            .flat_map(move |tail| self.ending(name, tail));
        self.selected_with(name, ending, tails)
    }

    /// What [`Graph::selected`] answers for `name`, given `ending`, those of the definitions of its last part it
    /// selects, and `longer`, its parts after its other separators, the shortest first.
    pub(crate) fn selected_with<'a>(
        &'a self,
        name: &'a str,
        ending: impl Iterator<Item = SymbolAt> + 'a,
        longer: impl Iterator<Item = &'a str> + 'a,
    ) -> impl Iterator<Item = SymbolAt> + 'a {
        let last = self.named_as(name).iter().copied().chain(ending);
        or_if_none(last, longer.flat_map(move |tail| self.ending(name, tail)))
    }

    /// The definitions of `tail`, a part of `name` after one of its separators, whose qualified names `name` selects.
    fn ending<'a>(&'a self, name: &'a str, tail: &'a str) -> impl Iterator<Item = SymbolAt> + 'a {
        let at_separator =
            |head: Option<&str>| head.is_some_and(|head| head.ends_with(QUALIFIER_SEPARATORS));
        self.named_as(tail).iter().copied().filter(move |&at| {
            let own = self.strings.resolve(self.sym(at).qualified_name);
            own == name
                || at_separator(own.strip_suffix(name))
                || (own != tail && at_separator(name.strip_suffix(own)))
        })
    }

    pub(crate) fn named_as(&self, text: &str) -> &[SymbolAt] {
        self.strings
            .get(text)
            .map_or(&[][..], |name| self.named(name))
    }

    pub(crate) fn named(&self, name: Str) -> &[SymbolAt] {
        listed(&self.by_name, &name)
    }

    /// Whether the symbol at `at` lies in the files and on the line `selector` looks at.
    pub(crate) fn is_within(&self, at: SymbolAt, selector: Selector<'_>) -> bool {
        selector.line.is_none_or(|line| self.sym(at).line == line)
            && selector.path_prefix.is_none_or(|prefix| {
                self.strings
                    .resolve(self.file(at.file).path)
                    .starts_with(prefix)
            })
    }

    /// The symbols a query passed over, `others` each with its count, ranked by count and then path order, at most
    /// [`LISTED_CANDIDATES`] of them.
    pub(crate) fn candidates(&self, mut others: Vec<(SymbolAt, usize)>) -> Vec<Candidate<'_>> {
        let rank = |&(at, count): &(SymbolAt, usize)| (Reverse(count), self.order_of(at));
        keep_first(&mut others, LISTED_CANDIDATES, |left, right| {
            rank(left).cmp(&rank(right))
        });
        others.sort_unstable_by_key(rank);
        others
            .into_iter()
            .map(|(at, count)| Candidate {
                symbol: self.definition_at(at),
                count,
            })
            .collect()
    }

    pub(crate) fn in_file<'a>(&self, named: &'a [SymbolAt], slot: u32) -> &'a [SymbolAt] {
        let rank = self.path_rank(slot);
        let start = named.partition_point(|at| self.path_rank(at.file) < rank);
        let len = named[start..].partition_point(|at| at.file == slot);
        &named[start..start + len]
    }

    /// Whether the file at `slot` is the source of `header`: nothing includes it, and it shares the header's directory
    /// and stem.
    fn is_source_of(&self, slot: u32, header: Str) -> bool {
        fn stem(name: &str) -> &str {
            name.rsplit_once('.').map_or(name, |(stem, _)| stem)
        }
        let source = self.strings.resolve(self.file(slot).path);
        let header = self.strings.resolve(header);
        self.includers_of(slot).next().is_none()
            && path::dir(source) == path::dir(header)
            && stem(path::base(source)) == stem(path::base(header))
    }

    /// Whether the file at `slot` directly includes a file that is test code by its own extraction.
    fn pastes_test_file(&self, slot: u32) -> bool {
        self.file(slot).includes.iter().any(|&target| {
            self.slot_of(target).is_some_and(|included| {
                let symbols = &self.file(included).symbols;
                symbols.first().is_some_and(|symbol| symbol.flags.is_test())
            })
        })
    }

    pub(crate) fn sites_into(&self, key: Str, sites: &mut Vec<Site>) {
        for &slot in self.sites.get(&key).into_iter().flatten() {
            self.sites_in(slot, key, sites);
        }
    }

    fn sites_in(&self, slot: u32, key: Str, sites: &mut Vec<Site>) {
        let file = self.file(slot);
        let calls = file.calls.iter().enumerate().filter(|(_, call)| {
            call.callee == key || (call.is_qualified() && self.is_listed_under(call.callee, key))
        });
        sites.extend(calls.map(|(index, _)| Site::new(slot, Entry::Call(narrow(index)))));
        let local_calls =
            file.local_calls.iter().enumerate().filter(|(_, call)| {
                self.is_listed_under(file.symbols[call.local as usize].name, key)
            });
        sites
            .extend(local_calls.map(|(index, _)| Site::new(slot, Entry::LocalCall(narrow(index)))));
    }

    fn is_listed_under(&self, callee: Str, key: Str) -> bool {
        callee == key || self.listing_key(callee) == Some(key)
    }

    pub(crate) fn listing_key(&self, name: Str) -> Option<Str> {
        qualified_tail(self.strings.resolve(name)).map_or(Some(name), |last| self.strings.get(last))
    }

    pub(crate) fn uses(&self, name: Str) -> impl Iterator<Item = UseAt> + '_ {
        self.uses
            .get(&name)
            .into_iter()
            .flatten()
            .flat_map(move |&slot| {
                let uses = self.file(slot).uses.iter().enumerate();
                uses.filter(move |(_, site)| site.name == name)
                    .map(move |(index, _)| UseAt {
                        file: slot,
                        index: narrow(index),
                    })
            })
    }

    pub(crate) fn occurrences(&self, key: Str) -> &[Occurrence] {
        listed(&self.callables, &key)
    }

    pub(crate) fn package_files(&self, dir: Str) -> &[u32] {
        listed(&self.package_files, &dir)
    }

    pub(crate) fn renamed_from(&self, source_name: Str) -> &[Rename] {
        listed(&self.renamed_from, &source_name)
    }

    pub(crate) fn slot_of(&self, path: Str) -> Option<u32> {
        self.by_path.get(&path).copied()
    }

    pub(crate) fn parenting_files(&self, name: Str) -> &[u32] {
        listed(&self.parented, &name)
    }

    pub(crate) fn is_spelled(&self, name: Str) -> bool {
        self.uses.contains_key(&name)
            || self.renamed_from.contains_key(&name)
            || self.listing_key(name).is_some_and(|key| {
                self.sites.contains_key(&key) || self.named_at_file_scope.contains_key(&key)
            })
    }

    pub(crate) fn named_at_file_scope(&self, key: Str) -> &[u32] {
        listed(&self.named_at_file_scope, &key)
    }

    /// Whether a file's top level pastes the body named `body` in by `filter`.
    pub(crate) fn pastes(&self, body: Str, filter: &str) -> bool {
        let spelled = format!("{}@{filter}", self.strings.resolve(body));
        (self.strings.get(&spelled)).is_some_and(|key| !self.named_at_file_scope(key).is_empty())
    }

    /// The filters by which file top levels paste the body named `body` in.
    pub(crate) fn filters_pasting(&self, body: Str) -> impl Iterator<Item = &str> {
        let name = self.strings.resolve(body);
        (self.named_at_file_scope(body).iter())
            .flat_map(|&file| self.file(file).file_scope_names.iter())
            .filter_map(move |&held| {
                (self.strings.resolve(held).strip_prefix(name)?).strip_prefix('@')
            })
    }

    pub(crate) fn file_slots(&self) -> impl Iterator<Item = u32> + '_ {
        self.by_path.values().copied()
    }

    pub(crate) fn includers_of(&self, slot: u32) -> impl Iterator<Item = u32> + '_ {
        self.included_by
            .get(&self.file(slot).path)
            .into_iter()
            .flatten()
            .copied()
    }

    #[expect(
        clippy::expect_used,
        reason = "every index names only the files the graph holds, and a slot is vacant only between a removal \
                  and the next insert"
    )]
    pub(crate) fn file(&self, slot: u32) -> &StoredFile {
        self.files[slot as usize]
            .as_ref()
            .expect("an index names a file the graph holds")
    }

    pub(crate) fn sym(&self, at: SymbolAt) -> &StoredSymbol {
        &self.file(at.file).symbols[at.index as usize]
    }

    pub(crate) fn qualifier(&self, at: SymbolAt) -> Option<&str> {
        let symbol = self.sym(at);
        self.strings
            .resolve(symbol.qualified_name)
            .strip_suffix(self.strings.resolve(symbol.name))
    }

    pub(crate) fn enclosing_scope(&self, at: SymbolAt, separator: &str) -> Option<&str> {
        self.qualifier(at)?.strip_suffix(separator)
    }

    pub(crate) fn path_rank(&self, slot: u32) -> u64 {
        self.order.rank(slot)
    }

    pub(crate) fn order_of(&self, at: SymbolAt) -> (u64, u32, u32) {
        (self.path_rank(at.file), self.sym(at).line, at.index)
    }

    pub(crate) fn definition_at(&self, at: SymbolAt) -> Definition<'_> {
        self.definition(at.file, self.sym(at))
    }

    /// The flags of the symbol at `at`, test code too when its file is a test module.
    pub(crate) fn flags(&self, at: SymbolAt) -> SymbolFlags {
        self.flags_in(at.file, self.sym(at).flags)
    }

    fn flags_in(&self, slot: u32, flags: SymbolFlags) -> SymbolFlags {
        if self.is_test_module(slot) {
            flags.with_test(true)
        } else {
            flags
        }
    }

    fn is_test_module(&self, slot: u32) -> bool {
        !self.test_included_by.is_empty()
            && self
                .test_modules
                .get_or_init(|| self.find_test_modules())
                .contains(&slot)
    }

    // constraint: a file is a test module when every file that includes it pulls it in as one or is a test module
    // itself, since a module that code outside the tests reaches is compiled outside them; where includes paste text,
    // a file must also paste in a test file, since a library's interface that only its tests include in the tree
    // reaches consumers outside it, and the source implementing such a header does not count against it and follows it
    fn find_test_modules(&self) -> HashSet<u32, FixedState> {
        let pasted = self.conventions.include_reach == IncludeReach::Transitive;
        let mut modules = HashSet::default();
        let mut pending: Vec<Str> = self.test_included_by.keys().copied().collect();
        while let Some(path) = pending.pop() {
            let Some(slot) = self.slot_of(path) else {
                continue;
            };
            let Some(includers) = self.included_by.get(&path) else {
                continue;
            };
            let test_includers = listed(&self.test_included_by, &path);
            let is_source = |includer: u32| pasted && self.is_source_of(includer, path);
            let in_tests_only = includers.iter().all(|includer| {
                test_includers.contains(includer)
                    || modules.contains(includer)
                    || is_source(*includer)
            });
            if in_tests_only && (!pasted || self.pastes_test_file(slot)) && modules.insert(slot) {
                pending.extend_from_slice(&self.file(slot).includes);
                modules.extend(
                    includers
                        .iter()
                        .copied()
                        .filter(|&includer| is_source(includer)),
                );
            }
        }
        modules
    }

    fn definition(&self, slot: u32, symbol: &StoredSymbol) -> Definition<'_> {
        let flags = self.flags_in(slot, symbol.flags);
        Definition {
            name: self.strings.resolve(symbol.name),
            qualified_name: self.strings.resolve(symbol.qualified_name),
            owner: symbol.owner.map(|owner| self.strings.resolve(owner)),
            kind: symbol.kind,
            file: self.strings.resolve(self.file(slot).path),
            line: symbol.line,
            end_line: symbol.end_line.max(symbol.line),
            signature: symbol
                .signature
                .map(|signature| self.strings.resolve(signature)),
            exported: symbol.exported(),
            flags,
        }
    }

    fn place_in_path_order(&mut self, slot: u32, path: &str) {
        let (files, strings) = (&self.files, &self.strings);
        self.order.insert(slot, |held| {
            files[held as usize]
                .as_ref()
                .is_some_and(|file| tree_order(strings.resolve(file.path), path) == Ordering::Less)
        });
    }

    // constraint: the files of one package arrive with one shared list, so a package's list is split once, and again
    // only when a file brings another
    fn note_built_with(&mut self, package: Str, dependencies: &Arc<str>) {
        if (self.built_with.get(&package))
            .is_some_and(|(read, _)| Arc::ptr_eq(read, dependencies) || read == dependencies)
        {
            return;
        }
        let mut reached: Vec<Str> = (dependencies.lines())
            .map(|manifest| self.strings.intern(manifest))
            .collect();
        reached.sort_unstable();
        (self.built_with).insert(package, (Arc::clone(dependencies), reached.into()));
    }

    pub(crate) fn build_constraint(&self, slot: u32) -> Option<&str> {
        let constraint = self.build_constraints.get(&slot)?;
        Some(self.strings.resolve(*constraint))
    }

    /// The packages the files of `package` are built with, sorted, where their extraction lists them.
    pub(crate) fn built_with(&self, package: Str) -> Option<&[Str]> {
        (self.built_with.get(&package)).map(|(_, reached)| &reached[..])
    }

    fn store(&mut self, path: Str, extraction: &Extraction) -> StoredFile {
        let dir = path::dir(&extraction.file);
        let symbols: Box<[StoredSymbol]> = extraction
            .symbols
            .iter()
            .map(|symbol| self.store_symbol(symbol))
            .collect();

        StoredFile {
            path,
            dir: self.strings.intern(dir),
            parent_dir: self.strings.intern(path::dir(dir)),
            package: extraction
                .package
                .as_deref()
                .map(|package| self.strings.intern(package)),
            parents: self.store_parents(&extraction.symbols),
            value_types: self.store_value_types(&extraction.symbols),
            locals: locals(extraction),
            callables: self.store_callables(extraction),
            folded: folded(extraction),
            calls: self.store_calls(extraction),
            file_scope_names: self.store_file_scope_names(extraction),
            local_calls: local_calls(extraction),
            uses: self.store_uses(extraction),
            includes: self.store_includes(path, extraction),
            re_exports: self.store_re_exports(extraction),
            bindings: self.store_bindings(extraction),
            assertions: self.store_assertions(extraction),
            symbols,
            words: OnceLock::new(),
        }
    }

    fn store_assertions(&mut self, extraction: &Extraction) -> Box<[StoredAssertion]> {
        extraction
            .assertions
            .iter()
            .map(|assertion| StoredAssertion {
                type_name: self.strings.intern(&assertion.type_name),
                interface: self.strings.intern(&assertion.interface),
                type_file: (assertion.type_file.as_deref()).map(|file| self.strings.intern(file)),
                interface_file: (assertion.interface_file.as_deref())
                    .map(|file| self.strings.intern(file)),
            })
            .collect()
    }

    fn store_symbol(&mut self, symbol: &Symbol) -> StoredSymbol {
        StoredSymbol {
            name: self.strings.intern(&symbol.name),
            qualified_name: self.strings.intern(&symbol.qualified_name),
            owner: symbol
                .owner
                .as_deref()
                .map(|owner| self.strings.intern(owner)),
            signature: symbol
                .signature
                .as_deref()
                .map(|signature| self.strings.intern(signature)),
            line: symbol.line,
            end_line: symbol.end_line,
            kind: symbol.kind,
            flags: symbol.flags,
            exported_and_fewest_arguments: StoredSymbol::with_export_mark(
                symbol.fewest_arguments,
                symbol.exported,
            ),
            most_arguments: symbol.most_arguments,
        }
    }

    fn written_from_root<'a>(&self, written: &'a str) -> Option<&'a str> {
        let separator = self.conventions.scope_separator?;
        self.conventions
            .owner_chain_separator
            .is_none()
            .then(|| written.strip_prefix(separator))
            .flatten()
    }

    /// The owner a receiver on a class spells before the language's class member separator, empty where it names
    /// none, and `None` for a receiver on anything else.
    pub(crate) fn class_named<'a>(&self, receiver: &'a str) -> Option<&'a str> {
        receiver.strip_suffix(self.conventions.class_member_separator?)
    }

    fn store_parents(&mut self, symbols: &[Symbol]) -> Box<[Parent]> {
        let all_from_root = self.conventions.parent_spelling == ParentSpelling::FromRoot;
        let mut parents = Vec::new();
        for (index, symbol) in symbols.iter().enumerate() {
            for written in &symbol.parents {
                let rooted = self.written_from_root(written);
                parents.push(Parent {
                    symbol: narrow(index),
                    written: self.strings.intern(rooted.unwrap_or(written)),
                    written_from_root: all_from_root || rooted.is_some(),
                });
            }
        }
        parents.into_boxed_slice()
    }

    fn store_value_types(&mut self, symbols: &[Symbol]) -> Box<[ValueTypes]> {
        symbols
            .iter()
            .enumerate()
            .filter(|(_, symbol)| !symbol.value_types.is_empty())
            .map(|(index, symbol)| ValueTypes {
                symbol: narrow(index),
                types: self
                    .strings
                    .intern(&symbol.value_types.join(VALUE_TYPE_SEPARATOR)),
            })
            .collect()
    }

    fn store_callables(&mut self, extraction: &Extraction) -> Box<[StoredCallable]> {
        let mut callables: Vec<StoredCallable> = extraction
            .callables
            .iter()
            .map(|callable| StoredCallable {
                symbol: callable.symbol.0,
                key: self.strings.intern(&callable.key),
                role: callable.role,
            })
            .collect();
        callables.sort_by_key(|callable| callable.symbol);
        callables.into_boxed_slice()
    }

    // constraint: a stored call keeps its caller in the bits its marks leave, so a call inside a definition past the
    // `StoredCall::CALLERS`th of its file is not kept
    fn store_calls(&mut self, extraction: &Extraction) -> Box<[StoredCall]> {
        let return_paths = self.conventions.receivers == Receivers::ReturnPaths;
        let inherited_fields = self.conventions.receivers == Receivers::InheritedFields;
        let mut calls: Vec<StoredCall> = extraction
            .calls
            .iter()
            .filter(|call| holds(extraction, call.site_of) && call.site_of.0 < StoredCall::CALLERS)
            .map(|call| {
                let rooted = self.written_from_root(&call.callee);
                let callee = rooted.unwrap_or(&call.callee);
                let receiver = call.receiver.as_deref();
                let class = receiver.and_then(|receiver| self.class_named(receiver));
                if return_paths
                    && let Some((_, rest)) = receiver.and_then(|receiver| receiver.split_once("()"))
                {
                    self.strings
                        .intern(rest.split_once('|').map_or("", |(_, value)| value));
                }
                if inherited_fields && receiver.is_some_and(|receiver| receiver.starts_with("->")) {
                    self.strings.intern("");
                }
                let mut stored = StoredCall::new(
                    call.site_of.0,
                    self.strings.intern(callee),
                    class
                        .or(receiver)
                        .map(|receiver| self.strings.intern(receiver)),
                    (call.line, call.arguments),
                );
                if rooted.is_some() {
                    stored = stored.written_from_root();
                }
                if callee.contains(QUALIFIER_SEPARATORS) {
                    stored = stored.qualified();
                }
                if class.is_some() {
                    stored = stored.on_a_class();
                }
                if call.binds_apart {
                    stored = stored.binding_apart();
                }
                if call.calls_a_method {
                    stored = stored.calling_a_method();
                }
                stored
            })
            .collect();
        calls.sort_by_key(|call| call.caller());
        calls.into_boxed_slice()
    }

    fn store_file_scope_names(&mut self, extraction: &Extraction) -> Box<[Str]> {
        let mut keys = Vec::new();
        for call in &extraction.calls {
            if holds(extraction, call.site_of) {
                continue;
            }
            let callee = self.written_from_root(&call.callee).unwrap_or(&call.callee);
            let callee = self.strings.intern(callee);
            keys.push(self.intern_listing_key(callee));
        }
        for site in &extraction.uses {
            if !holds(extraction, site.site_of) {
                keys.push(self.strings.intern(&site.name));
            }
        }
        distinct(keys).into_boxed_slice()
    }

    // constraint: a stored use keeps its site in the bits its marks leave, so a use inside a definition past the
    // `StoredUse::SITES`th of its file is not kept
    fn store_uses(&mut self, extraction: &Extraction) -> Box<[StoredUse]> {
        extraction
            .uses
            .iter()
            .filter(|site| holds(extraction, site.site_of) && site.site_of.0 < StoredUse::SITES)
            .map(|site| {
                let receiver = match &site.receiver {
                    Receiver::Bare => UseReceiver::Bare,
                    Receiver::Qualifier(qualifier) => {
                        UseReceiver::Qualifier(self.strings.intern(qualifier))
                    }
                    Receiver::Own => UseReceiver::Own,
                    Receiver::Path(path) => {
                        UseReceiver::Path((!path.is_empty()).then(|| self.strings.intern(path)))
                    }
                    _ => UseReceiver::Path(None),
                };
                let name = self.strings.intern(&site.name);
                StoredUse::new(site.site_of.0, (name, receiver), site.access, site.line)
            })
            .collect()
    }

    fn store_includes(&mut self, path: Str, extraction: &Extraction) -> Box<[Str]> {
        let includes = extraction
            .includes
            .iter()
            .map(|include| self.strings.intern(&include.target))
            .filter(|&target| target != path);
        distinct(includes).into_boxed_slice()
    }

    fn store_re_exports(&mut self, extraction: &Extraction) -> Box<[StoredReExport]> {
        extraction
            .re_exports
            .iter()
            .map(|re_export| StoredReExport {
                local_name: self.strings.intern(&re_export.local_name),
                source_name: self.strings.intern(&re_export.source_name),
                source_file: re_export
                    .source_file
                    .as_deref()
                    .map(|file| self.strings.intern(file)),
            })
            .collect()
    }

    fn store_bindings(&mut self, extraction: &Extraction) -> Box<[StoredBinding]> {
        extraction
            .bindings
            .iter()
            .map(|binding| StoredBinding {
                alias: binding
                    .alias
                    .as_deref()
                    .map(|alias| self.strings.intern(alias)),
                target_dir: binding
                    .target_dir
                    .as_deref()
                    .map(|dir| self.strings.intern(dir)),
                last_segment: self.strings.intern(&binding.last_segment),
            })
            .collect()
    }

    fn index(&mut self, slot: u32) {
        for index in 0..narrow(self.file(slot).symbols.len()) {
            let at = SymbolAt { file: slot, index };
            let name = self.sym(at).name;
            let key = self.order_of(at);
            let position = self.by_name.get(&name).map_or(0, |held| match held.last() {
                Some(&last) if self.order_of(last) > key => {
                    held.partition_point(|&other| self.order_of(other) < key)
                }
                _ => held.len(),
            });
            self.by_name.entry(name).or_default().insert(position, at);
        }
        for at in 0..self.file(slot).callables.len() {
            let callable = self.file(slot).callables[at];
            let at = SymbolAt::new(slot, callable.symbol);
            let occurrence = Occurrence {
                at,
                role: callable.role,
            };
            self.callables
                .entry(callable.key)
                .or_default()
                .push(occurrence);
        }
        for name in distinct(self.file(slot).uses.iter().map(|site| site.name)) {
            self.uses.entry(name).or_default().push(slot);
        }
        for at in 0..self.file(slot).re_exports.len() {
            let re_export = self.file(slot).re_exports[at];
            if re_export.local_name != re_export.source_name {
                let rename = Rename {
                    file: slot,
                    local_name: re_export.local_name,
                };
                let renames = self.renamed_from.entry(re_export.source_name).or_default();
                renames.push(rename);
            }
        }
        for at in 0..self.file(slot).bindings.len() {
            let binding = self.file(slot).bindings[at];
            if let Some(local_name) = self.renamed_item(binding) {
                let rename = Rename {
                    file: slot,
                    local_name,
                };
                let renames = self.renamed_from.entry(binding.last_segment).or_default();
                renames.push(rename);
            }
        }
        self.index_sites(slot);
        for key in self.parent_keys_of(slot) {
            self.parented.entry(key).or_default().push(slot);
        }
        for at in 0..self.file(slot).includes.len() {
            let target = self.file(slot).includes[at];
            self.included_by.entry(target).or_default().push(slot);
        }
        for at in 0..self.file(slot).file_scope_names.len() {
            let key = self.file(slot).file_scope_names[at];
            self.named_at_file_scope.entry(key).or_default().push(slot);
        }
        if self.conventions.directory_packages {
            self.index_package(slot);
        }
    }

    // constraint: a binding names an item under a name of its own only where its target is a module's file, as a Rust
    // `use … as` is; a directory package's alias renames a package, and `.` and `_` bind no name
    fn renamed_item(&self, binding: StoredBinding) -> Option<Str> {
        let alias = binding.alias?;
        (binding.target_dir.is_some()
            && !self.conventions.directory_packages
            && !matches!(self.strings.resolve(alias), "." | "_"))
        .then_some(alias)
    }

    fn index_sites(&mut self, slot: u32) {
        for key in self.site_keys_of(slot) {
            self.sites.entry(key).or_default().push(slot);
        }
    }

    fn site_keys_of(&mut self, slot: u32) -> Vec<Str> {
        let file = self.file(slot);
        let callees = distinct(
            file.calls.iter().map(|call| call.callee).chain(
                file.local_calls
                    .iter()
                    .map(|call| file.symbols[call.local as usize].name),
            ),
        );
        distinct(
            callees
                .into_iter()
                .map(|callee| self.intern_listing_key(callee)),
        )
    }

    fn parent_keys_of(&mut self, slot: u32) -> Vec<Str> {
        let mut keys = Vec::new();
        for at in 0..self.file(slot).parents.len() {
            let written = self.file(slot).parents[at].written;
            keys.push(self.intern_listing_key(written));
        }
        for at in 0..self.file(slot).assertions.len() {
            let interface = self.file(slot).assertions[at].interface;
            keys.push(self.intern_listing_key(interface));
        }
        distinct(keys)
    }

    fn intern_listing_key(&mut self, written: Str) -> Str {
        let Some(last) = qualified_tail(self.strings.resolve(written)) else {
            return written;
        };
        if let Some(key) = self.strings.get(last) {
            return key;
        }
        let last = last.to_owned();
        self.strings.intern(&last)
    }

    fn index_package(&mut self, slot: u32) {
        let dir = self.file(slot).dir;
        let rank = self.path_rank(slot);
        let position = self.package_files.get(&dir).map_or(0, |files| {
            files.partition_point(|&held| self.path_rank(held) < rank)
        });
        self.package_files
            .entry(dir)
            .or_default()
            .insert(position, slot);
    }

    fn forget(&mut self, slot: u32) {
        let site_keys = self.site_keys_of(slot);
        let parent_keys = self.parent_keys_of(slot);
        let Some(file) = self.files[slot as usize].take() else {
            return;
        };
        self.build_constraints.remove(&slot);
        for key in parent_keys {
            unlist(&mut self.parented, key, |&held| held != slot);
        }

        for name in distinct(file.symbols.iter().map(|symbol| symbol.name)) {
            unlist(&mut self.by_name, name, |at| at.file != slot);
        }
        for key in site_keys {
            unlist(&mut self.sites, key, |&held| held != slot);
        }
        for name in distinct(file.uses.iter().map(|site| site.name)) {
            unlist(&mut self.uses, name, |&held| held != slot);
        }
        for callable in &file.callables {
            unlist(&mut self.callables, callable.key, |occurrence| {
                occurrence.at.file != slot
            });
        }
        for re_export in &file.re_exports {
            unlist(&mut self.renamed_from, re_export.source_name, |rename| {
                rename.file != slot
            });
        }
        let renamed = file
            .bindings
            .iter()
            .filter(|binding| binding.alias.is_some());
        for name in distinct(renamed.map(|binding| binding.last_segment)) {
            unlist(&mut self.renamed_from, name, |rename| rename.file != slot);
        }
        unlist(&mut self.package_files, file.dir, |&held| held != slot);
        for &target in &file.includes {
            unlist(&mut self.included_by, target, |&held| held != slot);
            unlist(&mut self.test_included_by, target, |&held| held != slot);
        }
        for &key in &file.file_scope_names {
            unlist(&mut self.named_at_file_scope, key, |&held| held != slot);
        }
        self.symbols -= file.symbols.len();
    }
}

pub(crate) fn listed<'a, K: Eq + Hash, T>(
    index: &'a HashMap<K, Vec<T>, FixedState>,
    key: &K,
) -> &'a [T] {
    index.get(key).map_or(&[], Vec::as_slice)
}

pub(crate) fn distinct<T: Ord>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut items: Vec<T> = items.into_iter().collect();
    items.sort_unstable();
    items.dedup();
    items
}

pub(crate) fn agree<K: Eq + Hash, V: PartialEq>(
    held: &mut HashMap<K, Option<V>, FixedState>,
    key: K,
    value: Option<V>,
) {
    held.entry(key)
        .and_modify(|one| {
            if *one != value {
                *one = None;
            }
        })
        .or_insert(value);
}

pub(crate) fn keep_first<T>(
    items: &mut Vec<T>,
    limit: usize,
    mut order: impl FnMut(&T, &T) -> Ordering,
) {
    if limit < items.len() {
        if let Some(last) = limit.checked_sub(1) {
            items.select_nth_unstable_by(last, &mut order);
        }
        items.truncate(limit);
    }
}

pub(crate) fn last_segment(written: &str) -> &str {
    written
        .rsplit(QUALIFIER_SEPARATORS)
        .next()
        .unwrap_or(written)
}

/// The parts of `written` after each of its separators, the shortest first: the names a qualified name can end with.
pub(crate) fn tails(written: &str) -> impl Iterator<Item = &str> {
    written
        .rmatch_indices(QUALIFIER_SEPARATORS)
        .map(move |(at, separator)| &written[at + separator.len()..])
}

/// The items of `first`, or those of `otherwise` where `first` yields none.
pub(crate) fn or_if_none<T>(
    mut first: impl Iterator<Item = T>,
    mut otherwise: impl Iterator<Item = T>,
) -> impl Iterator<Item = T> {
    let mut found = false;
    std::iter::from_fn(move || {
        if let Some(item) = first.next() {
            found = true;
            return Some(item);
        }
        if found { None } else { otherwise.next() }
    })
}

fn qualified_tail(written: &str) -> Option<&str> {
    let last = last_segment(written);
    if !last.is_empty() {
        return (last.len() < written.len()).then_some(last);
    }
    let trimmed = written.trim_end_matches(QUALIFIER_SEPARATORS);
    let last = last_segment(trimmed);
    (!last.is_empty() && last.len() < trimmed.len()).then(|| &written[trimmed.len() - last.len()..])
}

fn unlist<T>(index: &mut HashMap<Str, Vec<T>, FixedState>, key: Str, keep: impl FnMut(&T) -> bool) {
    let Some(held) = index.get_mut(&key) else {
        return;
    };
    held.retain(keep);
    if held.is_empty() {
        index.remove(&key);
    }
}

fn locals(extraction: &Extraction) -> Box<[Local]> {
    let mut is_local = vec![false; extraction.symbols.len()];
    for local in &extraction.local_symbols {
        if let Some(slot) = is_local.get_mut(local.0 as usize) {
            *slot = true;
        }
    }
    let mut holder = None;
    let mut locals = Vec::new();
    for (index, &local) in is_local.iter().enumerate() {
        let index = narrow(index);
        if local {
            locals.push(Local {
                symbol: index,
                holder: holder.unwrap_or(index),
            });
        } else {
            holder = Some(index);
        }
    }
    locals.into_boxed_slice()
}

fn folded(extraction: &Extraction) -> Box<[Folded]> {
    let mut folded: Vec<Folded> = extraction
        .folded_declarations
        .iter()
        .map(|folded| Folded {
            definition: folded.definition.0,
            line: folded.line,
        })
        .collect();
    folded.sort_by_key(|folded| folded.definition);
    folded.into_boxed_slice()
}

fn local_calls(extraction: &Extraction) -> Box<[StoredLocalCall]> {
    let mut local_calls: Vec<StoredLocalCall> = extraction
        .local_calls
        .iter()
        .filter(|call| holds(extraction, call.site_of) && holds(extraction, call.local))
        .map(|call| StoredLocalCall {
            caller: call.site_of.0,
            local: call.local.0,
            line: call.line,
        })
        .collect();
    local_calls.sort_by_key(|call| call.caller);
    local_calls.into_boxed_slice()
}

// constraint: `FILE_SCOPE`, the `site_of` of a site at file scope, lies past every extraction's symbols
const fn holds(extraction: &Extraction, id: SymbolId) -> bool {
    (id.0 as usize) < extraction.symbols.len()
}

pub(crate) const fn is_call_target_symbol(symbol: &StoredSymbol) -> bool {
    !matches!(symbol.kind, SymbolKind::Field | SymbolKind::EnumVariant) && !symbol.flags.is_label()
}

pub(crate) const fn is_data_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Field | SymbolKind::Variable | SymbolKind::Constant
    )
}

/// Whether [`Graph::references`] selects a symbol of `kind`: every kind a use names, which is all but an enum's
/// variants.
#[must_use]
pub const fn is_referent(kind: SymbolKind) -> bool {
    !matches!(kind, SymbolKind::EnumVariant)
}

/// Whether [`Graph::implementations`] selects a symbol of `kind`: a class, a struct, an interface, a trait, an enum, a
/// namespace or a type alias.
#[must_use]
pub const fn is_parent(kind: SymbolKind) -> bool {
    names_as_parent(kind) || matches!(kind, SymbolKind::Enum)
}

/// Whether a parent can name a symbol of `kind`: every kind [`is_parent`] admits but an enum.
pub(crate) const fn names_as_parent(kind: SymbolKind) -> bool {
    derives(kind) || matches!(kind, SymbolKind::Namespace | SymbolKind::TypeAlias)
}

// constraint: the order of the file index, whose `/` ranks below every other byte, so the engine inserts files in this
// order and path order ties break the same way in the graph and in the index
pub(crate) fn tree_order(left: &str, right: &str) -> Ordering {
    let rank = |byte: u8| if byte == b'/' { 0 } else { u16::from(byte) + 1 };
    left.bytes().map(rank).cmp(right.bytes().map(rank))
}

const fn rank_key<'g>(
    &(at, found): &(usize, Found<'g>),
) -> (Nearness, bool, bool, &'g str, u32, usize) {
    (
        found.nearness,
        found.definition.flags.is_test(),
        found.definition.flags.is_vendored(),
        found.definition.file,
        found.definition.line,
        at,
    )
}

#[expect(
    clippy::expect_used,
    reason = "a graph holds fewer than two billion files and a file fewer than two billion definitions or calls"
)]
pub(crate) fn narrow(len: usize) -> u32 {
    u32::try_from(len)
        .ok()
        .filter(|&narrow| narrow < 1 << 31)
        .expect("fewer than two billion")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_comes_before_a_sibling_its_name_prefixes() {
        let mut paths = ["a-b/x.go", "a/z.go", "a/b/y.go", "a.go"];

        paths.sort_by(|left, right| tree_order(left, right));

        assert_eq!(paths, ["a/b/y.go", "a/z.go", "a-b/x.go", "a.go"]);
    }
}
