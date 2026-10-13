//! The structure a walk discovered: an arena of recorded nodes, keyed by [`NodeId`].

use std::fmt;

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

/// The tree a walk discovered: an arena of nodes, each directory's children name-sorted.
///
/// This is the structure a consumer keeps. It carries no report about the run that produced it — for that, see
/// [`WalkOutcome`](crate::WalkOutcome), which hands this out via
/// [`into_tree`](crate::WalkOutcome::into_tree).
#[derive(Clone, Debug)]
pub struct Tree<N> {
    /// Every node's name, end to end in node order.
    pub(crate) names: String,
    /// Every node in depth-first order, which is id order.
    pub(crate) slots: Vec<Slot<N>>,
}

/// One node as the arena stores it.
#[derive(Clone, Debug)]
pub(crate) struct Slot<N> {
    /// Where the node's name begins in [`Tree::names`]. It ends where the next node's begins.
    pub(crate) name_start: u32,
    /// One past the last node of this node's subtree, which is the id of its next sibling when it has one.
    pub(crate) subtree_end: u32,
    pub(crate) value: N,
}

impl<N> Tree<N> {
    /// The entries directly inside the walk root, name-sorted.
    ///
    /// The root itself is not a node: a walk records what is *inside* the path it was given, so a caller that needs a
    /// root node synthesizes its own.
    #[must_use]
    pub const fn roots(&self) -> Children<'_, N> {
        Children {
            tree: self,
            next: 0,
            end: self.end(),
        }
    }

    /// The node this id identifies.
    ///
    /// # Panics
    ///
    /// Panics if the id did not come from this tree. Use [`Tree::get`] to handle a foreign id instead.
    #[must_use]
    pub fn node(&self, id: NodeId) -> Node<'_, N> {
        assert!(
            id.index() < self.slots.len(),
            "{id:?} is not a node of this tree of {} nodes",
            self.slots.len()
        );

        Node { tree: self, id }
    }

    /// The node this id identifies, or `None` if the id is not this tree's.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<Node<'_, N>> {
        (id.index() < self.slots.len()).then_some(Node { tree: self, id })
    }

    /// Every node in depth-first order: a directory is immediately followed by its subtree.
    ///
    /// The order matches a recursive descent from [`Tree::roots`], and it is id order.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = Node<'_, N>> {
        (0..self.end()).map(|id| Node {
            tree: self,
            id: NodeId(id),
        })
    }

    /// How many entries the tree holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the tree holds no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// One past the last node id.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the walk stores every subtree end as a u32, and the last root's subtree end is the node count"
    )]
    const fn end(&self) -> u32 {
        self.slots.len() as u32
    }
}

/// One recorded entry: its name, the value the [`Policy`](crate::Policy) produced for it, and its recorded children.
///
/// A borrowed view into its [`Tree`], cheap to copy.
pub struct Node<'t, N> {
    tree: &'t Tree<N>,
    id: NodeId,
}

impl<'t, N> Node<'t, N> {
    /// The node's id in its tree.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// The entry's own name, without any directory prefix. Lossily decoded, as during the walk.
    #[must_use]
    pub fn name(self) -> &'t str {
        let Tree { names, slots } = self.tree;
        let at = self.id.index();
        let end = slots
            .get(at + 1)
            .map_or(names.len(), |next| next.name_start as usize);

        &names[slots[at].name_start as usize..end]
    }

    /// The value the policy recorded for this entry.
    #[must_use]
    pub fn value(self) -> &'t N {
        &self.slot().value
    }

    /// The recorded children, name-sorted.
    ///
    /// Yields nothing for a file, for a directory the policy pruned, and for a directory whose every entry was skipped —
    /// the three are indistinguishable here by design, since only the policy knows which it meant.
    #[must_use]
    pub fn children(self) -> Children<'t, N> {
        Children {
            tree: self.tree,
            next: self.id.0 + 1,
            end: self.slot().subtree_end,
        }
    }

    fn slot(self) -> &'t Slot<N> {
        &self.tree.slots[self.id.index()]
    }
}

impl<N> Clone for Node<'_, N> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<N> Copy for Node<'_, N> {}

impl<N: fmt::Debug> fmt::Debug for Node<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("name", &self.name())
            .field("value", self.value())
            .finish()
    }
}

/// The children of one node, or the roots of a tree, in name order. See [`Node::children`] and [`Tree::roots`].
pub struct Children<'t, N> {
    tree: &'t Tree<N>,
    next: u32,
    end: u32,
}

impl<'t, N> Iterator for Children<'t, N> {
    type Item = Node<'t, N>;

    fn next(&mut self) -> Option<Node<'t, N>> {
        if self.next == self.end {
            return None;
        }

        let node = Node {
            tree: self.tree,
            id: NodeId(self.next),
        };
        self.next = node.slot().subtree_end;

        Some(node)
    }
}

impl<N> Clone for Children<'_, N> {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree,
            next: self.next,
            end: self.end,
        }
    }
}

impl<N: fmt::Debug> fmt::Debug for Children<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}
