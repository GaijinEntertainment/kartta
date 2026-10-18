//! The structure a walk discovered: an arena of recorded nodes in depth-first order.

use std::fmt;

/// The tree a walk discovered: an arena of nodes, each directory's children name-sorted.
///
/// This is the structure a consumer keeps. It carries no report about the run that produced it — for that, see the
/// [`WalkOutcome`](crate::WalkOutcome) this came in.
#[derive(Clone, Debug)]
pub struct Tree<V> {
    /// Every node's name, end to end in node order.
    pub(crate) names: String,
    /// Every node in depth-first order.
    pub(crate) slots: Vec<Slot<V>>,
}

/// One node as the arena stores it.
#[derive(Clone, Debug)]
pub(crate) struct Slot<V> {
    /// Where the node's name begins in [`Tree::names`]. It ends where the next node's begins.
    pub(crate) name_start: u32,
    /// One past the last node of this node's subtree, which is its next sibling's slot when it has one.
    pub(crate) subtree_end: u32,
    pub(crate) value: V,
}

impl<V> Tree<V> {
    /// The entries directly inside the walk root, name-sorted.
    ///
    /// The root itself is not a node: a walk records what is *inside* the path it was given, so a caller that needs a
    /// root node synthesizes its own.
    #[must_use]
    pub const fn children(&self) -> Children<'_, V> {
        Children {
            tree: self,
            next: 0,
            end: self.end(),
        }
    }

    /// Every node in depth-first order: a directory is immediately followed by its subtree.
    ///
    /// The order matches a recursive descent from [`Tree::children`].
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = Node<'_, V>> {
        (0..self.end()).map(|at| Node { tree: self, at })
    }

    /// How many nodes the tree holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the tree holds no nodes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// One past the last node's slot.
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
pub struct Node<'t, V> {
    tree: &'t Tree<V>,
    at: u32,
}

impl<'t, V> Node<'t, V> {
    /// The entry's own name, without any directory prefix. Lossily decoded, as during the walk.
    #[must_use]
    pub fn name(self) -> &'t str {
        let Tree { names, slots } = self.tree;
        let at = self.at as usize;
        let end = slots
            .get(at + 1)
            .map_or(names.len(), |next| next.name_start as usize);

        &names[slots[at].name_start as usize..end]
    }

    /// The value the policy recorded for this entry.
    #[must_use]
    pub fn value(self) -> &'t V {
        &self.slot().value
    }

    /// The recorded children, name-sorted.
    ///
    /// Yields nothing for a file, for a directory the policy pruned, and for a directory whose every entry was skipped
    /// — the three are indistinguishable here by design, since only the policy knows which it meant.
    #[must_use]
    pub fn children(self) -> Children<'t, V> {
        Children {
            tree: self.tree,
            next: self.at + 1,
            end: self.slot().subtree_end,
        }
    }

    fn slot(self) -> &'t Slot<V> {
        &self.tree.slots[self.at as usize]
    }
}

impl<V> Clone for Node<'_, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for Node<'_, V> {}

impl<V: fmt::Debug> fmt::Debug for Node<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("name", &self.name())
            .field("value", self.value())
            .finish()
    }
}

/// The children of one node, or of a tree's walk root, in name order. See [`Node::children`] and [`Tree::children`].
pub struct Children<'t, V> {
    tree: &'t Tree<V>,
    next: u32,
    end: u32,
}

impl<'t, V> Iterator for Children<'t, V> {
    type Item = Node<'t, V>;

    fn next(&mut self) -> Option<Node<'t, V>> {
        if self.next == self.end {
            return None;
        }

        let node = Node {
            tree: self.tree,
            at: self.next,
        };
        self.next = node.slot().subtree_end;

        Some(node)
    }
}

impl<V> Clone for Children<'_, V> {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree,
            next: self.next,
            end: self.end,
        }
    }
}

impl<V: fmt::Debug> fmt::Debug for Children<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}
