use foldhash::fast::FixedState;
use hashbrown::HashMap;
use kartta_lang::{Conventions, Extraction, Symbol, SymbolFlags, SymbolKind, path};

use crate::strings::{Str, Strings};

const QUALIFIER_SEPARATORS: [char; 5] = ['.', ':', '\\', '#', '/'];

/// The definitions of one language family's files, kept a file at a time and found by name.
///
/// A file goes in as its [`Extraction`] and comes out whole: [`Graph::insert`] replaces whatever the graph held for
/// the same path, and [`Graph::remove`] forgets it, so a graph follows a tree that changes a file at a time without
/// being built again. The graph names no language; what differs between languages reaches it as the extractions and
/// the [`Conventions`] it is built with.
#[derive(Debug)]
pub struct Graph {
    conventions: Conventions,
    strings: Strings,
    files: Vec<Option<StoredFile>>,
    vacant: Vec<u32>,
    by_path: HashMap<Str, u32, FixedState>,
    by_name: HashMap<Str, Vec<SymbolAt>, FixedState>,
    symbols: usize,
}

#[derive(Debug)]
struct StoredFile {
    path: Str,
    symbols: Box<[StoredSymbol]>,
}

#[derive(Debug, Clone, Copy)]
struct StoredSymbol {
    name: Str,
    qualified_name: Str,
    owner: Option<Str>,
    signature: Option<Str>,
    line: u32,
    end_line: u32,
    kind: SymbolKind,
    flags: SymbolFlags,
    exported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SymbolAt {
    file: u32,
    index: u32,
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
    /// Test code, generated code, an entry point.
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

impl Graph {
    /// An empty graph for files whose calls resolve under `conventions`.
    #[must_use]
    pub fn new(conventions: Conventions) -> Self {
        Self {
            conventions,
            strings: Strings::default(),
            files: Vec::new(),
            vacant: Vec::new(),
            by_path: HashMap::with_hasher(FixedState::default()),
            by_name: HashMap::with_hasher(FixedState::default()),
            symbols: 0,
        }
    }

    /// The conventions the graph's calls resolve under.
    #[must_use]
    pub const fn conventions(&self) -> Conventions {
        self.conventions
    }

    /// How many files the graph holds.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.by_path.len()
    }

    /// How many definitions the graph holds.
    #[must_use]
    pub const fn symbol_count(&self) -> usize {
        self.symbols
    }

    /// Takes in one file's extraction, replacing whatever the graph held for the same path.
    pub fn insert(&mut self, extraction: &Extraction) {
        let path = self.strings.intern(&extraction.file);
        self.remove_at(path);

        let slot = self.vacant.pop().unwrap_or_else(|| {
            self.files.push(None);
            narrow(self.files.len() - 1)
        });
        let symbols: Box<[StoredSymbol]> = extraction
            .symbols
            .iter()
            .map(|symbol| self.store(symbol))
            .collect();
        for (index, symbol) in symbols.iter().enumerate() {
            let at = SymbolAt {
                file: slot,
                index: narrow(index),
            };
            self.by_name.entry(symbol.name).or_default().push(at);
        }

        self.symbols += symbols.len();
        self.files[slot as usize] = Some(StoredFile { path, symbols });
        self.by_path.insert(path, slot);
    }

    /// Forgets the file at `path`, and answers whether the graph held it.
    pub fn remove(&mut self, path: &str) -> bool {
        self.strings
            .get(path)
            .is_some_and(|path| self.remove_at(path))
    }

    /// The definitions in the file at `path`, ordered by the line each starts on, one that encloses others before
    /// them, or `None` when the graph does not hold the file. A file the graph holds with no definition answers an
    /// empty outline: its language found none, or could not parse it.
    #[must_use]
    pub fn outline(&self, path: &str) -> Option<Vec<Definition<'_>>> {
        let slot = *self.by_path.get(&self.strings.get(path)?)?;
        let file = self.files[slot as usize].as_ref()?;

        let mut outline: Vec<Definition<'_>> = file
            .symbols
            .iter()
            .map(|symbol| self.definition(file.path, symbol))
            .collect();
        outline.sort_by(|left, right| {
            left.line
                .cmp(&right.line)
                .then(right.end_line.cmp(&left.end_line))
        });
        Some(outline)
    }

    /// The definitions `name` selects, nearest to the file `from` first, then those outside test code, then by path
    /// and line, at most `limit` of them.
    ///
    /// A bare name selects the definitions of that name. A qualified one, `Owner.parse` or `ns::Owner::parse`,
    /// selects every definition whose qualified name equals it or ends with it at a separator, and every definition
    /// of its last segment whose own qualified name, of two segments or more, it ends with: `ns::Owner::parse` selects
    /// a member qualified as `Owner::parse`. The separators are those the languages qualify with: `.`, `:`, `\`, `#`
    /// and `/`.
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
            .map(|definition| Found {
                nearness: match from_dir {
                    Some(dir) if path::dir(definition.file) == dir => Nearness::SameDirectory,
                    _ => Nearness::Elsewhere,
                },
                definition,
            })
            .enumerate()
            .collect();
        let total = found.len();
        if limit < total {
            found.select_nth_unstable_by_key(limit, rank_key);
            found.truncate(limit);
        }
        found.sort_unstable_by_key(rank_key);

        Definitions {
            total,
            found: found.into_iter().map(|(_, found)| found).collect(),
        }
    }

    fn selected(&self, name: &str) -> impl Iterator<Item = Definition<'_>> {
        let last = name.rsplit(QUALIFIER_SEPARATORS).next().unwrap_or(name);
        let qualified = last != name;
        let at_separator =
            |head: Option<&str>| head.is_some_and(|head| head.ends_with(QUALIFIER_SEPARATORS));

        let suffixed = qualified.then(|| {
            self.named(last).filter(move |definition| {
                let own = definition.qualified_name;
                own == name
                    || at_separator(own.strip_suffix(name))
                    || (own != last && at_separator(name.strip_suffix(own)))
            })
        });
        self.named(name).chain(suffixed.into_iter().flatten())
    }

    fn named(&self, name: &str) -> impl Iterator<Item = Definition<'_>> {
        self.strings
            .get(name)
            .and_then(|name| self.by_name.get(&name))
            .into_iter()
            .flatten()
            .filter_map(|at| {
                let file = self.files[at.file as usize].as_ref()?;
                Some(self.definition(file.path, &file.symbols[at.index as usize]))
            })
    }

    fn definition(&self, path: Str, symbol: &StoredSymbol) -> Definition<'_> {
        Definition {
            name: self.strings.resolve(symbol.name),
            qualified_name: self.strings.resolve(symbol.qualified_name),
            owner: symbol.owner.map(|owner| self.strings.resolve(owner)),
            kind: symbol.kind,
            file: self.strings.resolve(path),
            line: symbol.line,
            end_line: symbol.end_line.max(symbol.line),
            signature: symbol
                .signature
                .map(|signature| self.strings.resolve(signature)),
            exported: symbol.exported,
            flags: symbol.flags,
        }
    }

    fn store(&mut self, symbol: &Symbol) -> StoredSymbol {
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
            exported: symbol.exported,
        }
    }

    fn remove_at(&mut self, path: Str) -> bool {
        let Some(slot) = self.by_path.remove(&path) else {
            return false;
        };
        let Some(file) = self.files[slot as usize].take() else {
            return false;
        };

        let mut names: Vec<Str> = file.symbols.iter().map(|symbol| symbol.name).collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            self.unlist(name, slot);
        }
        self.symbols -= file.symbols.len();
        self.vacant.push(slot);
        true
    }

    fn unlist(&mut self, name: Str, slot: u32) {
        let Some(holders) = self.by_name.get_mut(&name) else {
            return;
        };
        holders.retain(|at| at.file != slot);
        if holders.is_empty() {
            self.by_name.remove(&name);
        }
    }
}

const fn rank_key<'g>(&(at, found): &(usize, Found<'g>)) -> (Nearness, bool, &'g str, u32, usize) {
    (
        found.nearness,
        found.definition.flags.is_test(),
        found.definition.file,
        found.definition.line,
        at,
    )
}

#[expect(
    clippy::expect_used,
    reason = "a graph holds fewer than four billion files and a file fewer than four billion definitions"
)]
fn narrow(len: usize) -> u32 {
    u32::try_from(len).expect("fewer than four billion")
}
