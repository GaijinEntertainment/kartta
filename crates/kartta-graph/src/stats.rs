use kartta_lang::{Access, SymbolKind};

use crate::graph::Graph;

/// What a graph holds, counted when asked.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct Stats {
    /// The files the graph holds.
    pub files: usize,
    /// The files it holds with no definition: their language found none, or could not parse them.
    pub files_without_symbols: usize,
    /// The definitions it holds.
    pub symbols: usize,
    /// The definitions of each kind, in the order of [`SymbolKind`], every kind the graph holds none of left out.
    pub kinds: Vec<(SymbolKind, usize)>,
    /// The calls written by name inside a definition, which a query resolves by the name.
    pub calls: usize,
    /// The calls of a definition local to the caller's body, which bind it without a lookup.
    pub local_calls: usize,
    /// The uses of a name outside a call inside a definition: the reads, writes and updates together.
    pub uses: usize,
    /// The uses that read the value their name binds, or name it as a type.
    pub reads: usize,
    /// The uses that assign the value without reading it.
    pub writes: usize,
    /// The uses that read and assign the value.
    pub updates: usize,
    /// The files the held files include, once for each file and each file it includes, whether the graph holds the
    /// included file or not.
    pub includes: usize,
}

impl Graph {
    /// Counts the files, definitions, call sites, uses and includes the graph holds.
    ///
    /// This reads every file and definition the graph holds, so its cost grows with them. A call or a use made outside every definition is not a site: the graph
    /// keeps only the name it spells, and no count here holds it.
    #[must_use]
    pub fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        for file in self.files.iter().flatten() {
            stats.files += 1;
            stats.symbols += file.symbols.len();
            stats.files_without_symbols += usize::from(file.symbols.is_empty());
            stats.calls += file.calls.len();
            stats.local_calls += file.local_calls.len();
            stats.uses += file.uses.len();
            for site in &file.uses {
                match site.access() {
                    Access::Read => stats.reads += 1,
                    Access::Write => stats.writes += 1,
                    Access::Update => stats.updates += 1,
                }
            }
            stats.includes += file.includes.len();
            for symbol in &file.symbols {
                count_kind(&mut stats.kinds, symbol.kind);
            }
        }
        stats.kinds.sort_unstable();
        stats
    }
}

fn count_kind(kinds: &mut Vec<(SymbolKind, usize)>, kind: SymbolKind) {
    match kinds.iter_mut().find(|(held, _)| *held == kind) {
        Some((_, count)) => *count += 1,
        None => kinds.push((kind, 1)),
    }
}
