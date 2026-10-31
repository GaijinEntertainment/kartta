use std::borrow::Cow;
use std::ops::Deref;
use std::rc::Rc;

use foldhash::fast::FixedState;
use hashbrown::HashMap;

use crate::graph::{Graph, narrow};
use crate::stored::SymbolAt;
use crate::strings::Str;

#[derive(Debug)]
pub(crate) struct Candidates<'g> {
    pub(crate) named: &'g [SymbolAt],
    pub(crate) targets: Vec<SymbolAt>,
    pub(crate) locals: Vec<SymbolAt>,
    // constraint: whether a gate or the call's arguments turned away a call target of the name, which makes the name no
    // project's own
    pub(crate) turned_away: bool,
    index: Option<Rc<Index>>,
    gate: Option<Gate<'g>>,
}

#[derive(Debug)]
struct Index {
    in_file: HashMap<u32, (u32, u32), FixedState>,
    by_owner: HashMap<Str, Vec<SymbolAt>, FixedState>,
    by_dir: HashMap<Str, Vec<SymbolAt>, FixedState>,
    by_parent_dir: HashMap<Str, Vec<SymbolAt>, FixedState>,
    first_party: Vec<SymbolAt>,
}

/// The packages a caller's package can bind a definition of by its name alone: its own, and outside their test code
/// those it is built with, sorted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Gate<'g> {
    pub(crate) package: Str,
    pub(crate) reached: &'g [Str],
}

impl Gate<'_> {
    // constraint: another package's test code is built with its own tests alone
    pub(crate) fn admits_at(&self, graph: &Graph, at: SymbolAt) -> bool {
        graph.file(at.file).package.is_none_or(|package| {
            package == self.package
                || (self.reached.binary_search(&package).is_ok() && !graph.flags(at).is_test())
        })
    }
}

pub(crate) const INDEXED_FROM: usize = 64;

impl<'g> Candidates<'g> {
    pub(crate) const fn scanned(
        named: &'g [SymbolAt],
        targets: Vec<SymbolAt>,
        turned_away: bool,
    ) -> Self {
        Self {
            named,
            targets,
            locals: Vec::new(),
            turned_away,
            index: None,
            gate: None,
        }
    }

    pub(crate) fn indexed(
        graph: &Graph,
        named: &'g [SymbolAt],
        targets: Vec<SymbolAt>,
        locals: Vec<SymbolAt>,
    ) -> Self {
        let mut in_file: HashMap<u32, (u32, u32), FixedState> = HashMap::default();
        for (position, at) in named.iter().enumerate() {
            let range = in_file.entry(at.file).or_insert((narrow(position), 0));
            range.1 += 1;
        }
        let mut by_owner: HashMap<Str, Vec<SymbolAt>, FixedState> = HashMap::default();
        let mut by_dir: HashMap<Str, Vec<SymbolAt>, FixedState> = HashMap::default();
        let mut by_parent_dir: HashMap<Str, Vec<SymbolAt>, FixedState> = HashMap::default();
        let mut first_party = Vec::new();
        for &at in &targets {
            let file = graph.file(at.file);
            let symbol = &file.symbols[at.index as usize];
            if let Some(owner) = symbol.owner {
                by_owner.entry(owner).or_default().push(at);
            }
            by_dir.entry(file.dir).or_default().push(at);
            by_parent_dir.entry(file.parent_dir).or_default().push(at);
            if !symbol.flags.is_vendored() {
                first_party.push(at);
            }
        }
        Self {
            named,
            targets,
            locals,
            turned_away: false,
            index: Some(Rc::new(Index {
                in_file,
                by_owner,
                by_dir,
                by_parent_dir,
                first_party,
            })),
            gate: None,
        }
    }

    /// These candidates narrowed to the ones `gate` admits, `targets` among them, sharing this set's index.
    pub(crate) fn gated(&self, targets: Vec<SymbolAt>, gate: Gate<'g>) -> Self {
        Self {
            named: self.named,
            turned_away: self.turned_away || targets.len() < self.targets.len(),
            targets,
            locals: self.locals.clone(),
            index: self.index.clone(),
            gate: Some(gate),
        }
    }

    /// Whether [`locals`](Self::locals) holds every candidate local to a body, as an indexed set does; a scanned one
    /// leaves them among [`named`](Self::named).
    pub(crate) const fn holds_locals_apart(&self) -> bool {
        self.index.is_some()
    }

    pub(crate) fn in_file(&self, graph: &Graph, slot: u32) -> &'g [SymbolAt] {
        self.index.as_ref().map_or_else(
            || graph.in_file(self.named, slot),
            |index| {
                index.in_file.get(&slot).map_or(&[], |&(start, len)| {
                    &self.named[start as usize..(start + len) as usize]
                })
            },
        )
    }

    pub(crate) fn with_owner(&self, graph: &Graph, owner: Str) -> Cow<'_, [SymbolAt]> {
        self.subset(
            graph,
            |index| index.by_owner.get(&owner),
            |at| graph.sym(at).owner == Some(owner),
        )
    }

    pub(crate) fn in_dir(&self, graph: &Graph, dir: Str) -> Cow<'_, [SymbolAt]> {
        self.subset(
            graph,
            |index| index.by_dir.get(&dir),
            |at| graph.file(at.file).dir == dir,
        )
    }

    pub(crate) fn in_parent_dir(&self, graph: &Graph, parent_dir: Str) -> Cow<'_, [SymbolAt]> {
        self.subset(
            graph,
            |index| index.by_parent_dir.get(&parent_dir),
            |at| graph.file(at.file).parent_dir == parent_dir,
        )
    }

    pub(crate) fn first_party(&self, graph: &Graph) -> Cow<'_, [SymbolAt]> {
        self.subset(
            graph,
            |index| Some(&index.first_party),
            |at| !graph.sym(at).flags.is_vendored(),
        )
    }

    fn subset<'s>(
        &'s self,
        graph: &Graph,
        indexed: impl FnOnce(&'s Index) -> Option<&'s Vec<SymbolAt>>,
        keeps: impl Fn(SymbolAt) -> bool,
    ) -> Cow<'s, [SymbolAt]> {
        let Some(index) = &self.index else {
            return Cow::Owned(
                self.targets
                    .iter()
                    .copied()
                    .filter(|&at| keeps(at))
                    .collect(),
            );
        };
        let indexed = indexed(index).map_or(&[][..], Vec::as_slice);
        self.gate.as_ref().map_or(Cow::Borrowed(indexed), |gate| {
            Cow::Owned(
                (indexed.iter().copied())
                    .filter(|&at| gate.admits_at(graph, at))
                    .collect(),
            )
        })
    }
}

#[derive(Debug)]
pub(crate) enum CandidateSet<'g> {
    Shared(Rc<Candidates<'g>>),
    Own(Candidates<'g>),
}

impl<'g> Deref for CandidateSet<'g> {
    type Target = Candidates<'g>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Shared(candidates) => candidates,
            Self::Own(candidates) => candidates,
        }
    }
}
