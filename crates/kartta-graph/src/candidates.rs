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
    index: Option<Index>,
}

#[derive(Debug)]
struct Index {
    in_file: HashMap<u32, (u32, u32), FixedState>,
    by_owner: HashMap<Str, Vec<SymbolAt>, FixedState>,
    by_dir: HashMap<Str, Vec<SymbolAt>, FixedState>,
    by_parent_dir: HashMap<Str, Vec<SymbolAt>, FixedState>,
    first_party: Vec<SymbolAt>,
}

pub(crate) const INDEXED_FROM: usize = 64;

impl<'g> Candidates<'g> {
    pub(crate) const fn scanned(named: &'g [SymbolAt], targets: Vec<SymbolAt>) -> Self {
        Self {
            named,
            targets,
            locals: Vec::new(),
            index: None,
        }
    }

    pub(crate) fn indexed(
        graph: &Graph,
        named: &'g [SymbolAt],
        targets: Vec<SymbolAt>,
        locals: Vec<SymbolAt>,
    ) -> Self {
        let mut in_file: HashMap<u32, (u32, u32), FixedState> =
            HashMap::with_hasher(FixedState::default());
        for (position, at) in named.iter().enumerate() {
            let range = in_file.entry(at.file).or_insert((narrow(position), 0));
            range.1 += 1;
        }
        let mut by_owner: HashMap<Str, Vec<SymbolAt>, FixedState> =
            HashMap::with_hasher(FixedState::default());
        let mut by_dir: HashMap<Str, Vec<SymbolAt>, FixedState> =
            HashMap::with_hasher(FixedState::default());
        let mut by_parent_dir: HashMap<Str, Vec<SymbolAt>, FixedState> =
            HashMap::with_hasher(FixedState::default());
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
            index: Some(Index {
                in_file,
                by_owner,
                by_dir,
                by_parent_dir,
                first_party,
            }),
        }
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
            |index| index.by_owner.get(&owner),
            |at| graph.sym(at).owner == Some(owner),
        )
    }

    pub(crate) fn in_dir(&self, graph: &Graph, dir: Str) -> Cow<'_, [SymbolAt]> {
        self.subset(
            |index| index.by_dir.get(&dir),
            |at| graph.file(at.file).dir == dir,
        )
    }

    pub(crate) fn in_parent_dir(&self, graph: &Graph, parent_dir: Str) -> Cow<'_, [SymbolAt]> {
        self.subset(
            |index| index.by_parent_dir.get(&parent_dir),
            |at| graph.file(at.file).parent_dir == parent_dir,
        )
    }

    pub(crate) fn first_party(&self, graph: &Graph) -> Cow<'_, [SymbolAt]> {
        self.subset(
            |index| Some(&index.first_party),
            |at| !graph.sym(at).flags.is_vendored(),
        )
    }

    fn subset<'s>(
        &'s self,
        indexed: impl FnOnce(&'s Index) -> Option<&'s Vec<SymbolAt>>,
        keeps: impl Fn(SymbolAt) -> bool,
    ) -> Cow<'s, [SymbolAt]> {
        self.index.as_ref().map_or_else(
            || {
                Cow::Owned(
                    self.targets
                        .iter()
                        .copied()
                        .filter(|&at| keeps(at))
                        .collect(),
                )
            },
            |index| Cow::Borrowed(indexed(index).map_or(&[][..], Vec::as_slice)),
        )
    }
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a candidate set lives on the stack for one call, and boxing a call's own candidates would allocate once \
              per call site"
)]
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
