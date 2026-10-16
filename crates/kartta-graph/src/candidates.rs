use crate::graph::Graph;
use crate::stored::SymbolAt;
use crate::strings::Str;

#[derive(Debug)]
pub(crate) struct Candidates<'g> {
    named: &'g [SymbolAt],
    targets: Vec<SymbolAt>,
}

impl<'g> Candidates<'g> {
    pub(crate) const fn scanned(named: &'g [SymbolAt], targets: Vec<SymbolAt>) -> Self {
        Self { named, targets }
    }

    pub(crate) const fn named(&self) -> &'g [SymbolAt] {
        self.named
    }

    pub(crate) fn targets(&self) -> &[SymbolAt] {
        &self.targets
    }

    pub(crate) fn in_file(&self, graph: &Graph, slot: u32) -> &'g [SymbolAt] {
        graph.in_file(self.named, slot)
    }

    pub(crate) fn with_owner(&self, graph: &Graph, owner: Str) -> Vec<SymbolAt> {
        self.subset(|at| graph.sym(at).owner == Some(owner))
    }

    pub(crate) fn in_dir(&self, graph: &Graph, dir: Str) -> Vec<SymbolAt> {
        self.subset(|at| graph.file(at.file).dir == dir)
    }

    pub(crate) fn in_parent_dir(&self, graph: &Graph, parent_dir: Str) -> Vec<SymbolAt> {
        self.subset(|at| graph.file(at.file).parent_dir == parent_dir)
    }

    pub(crate) fn first_party(&self, graph: &Graph) -> Vec<SymbolAt> {
        self.subset(|at| !graph.sym(at).flags.is_vendored())
    }

    fn subset(&self, keeps: impl Fn(SymbolAt) -> bool) -> Vec<SymbolAt> {
        self.targets
            .iter()
            .copied()
            .filter(|&at| keeps(at))
            .collect()
    }
}
