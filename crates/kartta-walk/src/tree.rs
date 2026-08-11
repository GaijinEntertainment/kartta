//! The structure a walk discovered: an arena of recorded nodes, keyed by [`NodeId`].

/// Identifies one node of a [`Tree`] arena. Only valid for the tree that produced it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub(crate) u32);

impl NodeId {
    /// The id as an index, for a caller keeping its own array alongside the arena.
    ///
    /// Ids are assigned densely from zero in the order [`Tree::nodes`] reports, so a `Vec` of the same length indexes
    /// one-to-one with the tree.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One recorded entry: its name, the value the [`Policy`](crate::Policy) produced for it, and its recorded children.
#[derive(Clone, Debug)]
pub struct Node<N> {
    pub(crate) name: Box<str>,
    pub(crate) value: N,
    pub(crate) children: Vec<NodeId>,
}

impl<N> Node<N> {
    /// The entry's own name, without any directory prefix. Lossily decoded, as during the walk.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The value the policy recorded for this entry.
    #[must_use]
    pub const fn value(&self) -> &N {
        &self.value
    }

    /// The recorded children, name-sorted.
    ///
    /// Empty for a file, for a directory the policy pruned, and for a directory whose every entry was skipped — the
    /// three are indistinguishable here by design, since only the policy knows which it meant.
    #[must_use]
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }
}

/// The tree a walk discovered: an arena of nodes, each directory's children name-sorted.
///
/// This is the structure a consumer keeps. It carries no report about the run that produced it — for that, see
/// [`WalkOutcome`](crate::WalkOutcome), which hands this out via
/// [`into_tree`](crate::WalkOutcome::into_tree).
#[derive(Clone, Debug)]
pub struct Tree<N> {
    pub(crate) nodes: Vec<Node<N>>,
    pub(crate) roots: Vec<NodeId>,
}

impl<N> Tree<N> {
    /// The entries directly inside the walk root, name-sorted.
    ///
    /// The root itself is not a node: a walk records what is *inside* the path it was given, so a caller that needs a
    /// root node synthesizes its own.
    #[must_use]
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// The node this id identifies.
    ///
    /// # Panics
    ///
    /// Panics if the id did not come from this tree. Use [`Tree::get`] to handle a foreign id instead.
    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node<N> {
        &self.nodes[id.index()]
    }

    /// The node this id identifies, or `None` if the id is not this tree's.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node<N>> {
        self.nodes.get(id.index())
    }

    /// Every node in depth-first order: a directory is immediately followed by its subtree.
    ///
    /// Iterating this is how a caller rebuilds full paths in one pass — the order matches a recursive descent from
    /// [`Tree::roots`], so no sort is needed.
    #[must_use]
    pub fn nodes(&self) -> &[Node<N>] {
        &self.nodes
    }

    /// How many entries the tree holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the tree holds no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
