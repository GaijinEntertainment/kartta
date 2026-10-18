use kartta::{Definition, Limits, Reached, SymbolFlags, Walk};
use serde::{Deserialize, Serialize, Serializer};

/// How many calls away `callers` walks when the params name no depth.
const CALLERS_DEPTH: u32 = 3;
/// How many symbols a walk over calls reaches at most when the params name no cap.
const WALK_MAX_SYMBOLS: usize = 500;

/// The params of a walk over calls, `callers` and `callees`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WalkParams {
    /// The symbol to start from, as `Graph::callers` selects one.
    pub(crate) name: String,
    /// The graph to ask, by a language's name or its family's; every graph when left out.
    #[serde(default)]
    pub(crate) lang: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    min_confidence: u8,
    #[serde(default)]
    max_symbols: Option<usize>,
}

impl WalkParams {
    pub(crate) fn limits(&self) -> Limits {
        Limits::new(self.depth.unwrap_or(CALLERS_DEPTH))
            .with_min_confidence(self.min_confidence)
            .with_max_symbols(self.max_symbols.unwrap_or(WALK_MAX_SYMBOLS))
    }
}

/// A definition as every answer spells it. The flags appear only when one is set.
#[derive(Debug, Serialize)]
pub(crate) struct DefinitionView<'g> {
    name: &'g str,
    qualified_name: &'g str,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<&'g str>,
    kind: &'static str,
    file: &'g str,
    line: u32,
    end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<&'g str>,
    exported: bool,
    #[serde(skip_serializing_if = "Flags::is_empty")]
    flags: Flags,
}

/// A definition's flags, spelled as the list of those set: `test`, `entry`, `generated`, `vendored`.
#[derive(Debug)]
struct Flags(SymbolFlags);

impl Flags {
    const fn set(&self) -> [(&'static str, bool); 4] {
        [
            ("test", self.0.is_test()),
            ("entry", self.0.is_entry()),
            ("generated", self.0.is_generated()),
            ("vendored", self.0.is_vendored()),
        ]
    }

    fn is_empty(&self) -> bool {
        self.set().iter().all(|&(_, set)| !set)
    }
}

impl Serialize for Flags {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(
            self.set()
                .into_iter()
                .filter_map(|(name, set)| set.then_some(name)),
        )
    }
}

impl<'g> From<&Definition<'g>> for DefinitionView<'g> {
    fn from(definition: &Definition<'g>) -> Self {
        Self {
            name: definition.name,
            qualified_name: definition.qualified_name,
            owner: definition.owner,
            kind: definition.kind.label(),
            file: definition.file,
            line: definition.line,
            end_line: definition.end_line,
            signature: definition.signature,
            exported: definition.exported,
            flags: Flags(definition.flags),
        }
    }
}

/// A symbol a walk over calls reached, and the call that reached it.
#[derive(Debug, Serialize)]
pub(crate) struct ReachedView<'g> {
    symbol: DefinitionView<'g>,
    depth: u32,
    confidence: u8,
    line: u32,
}

impl<'g> From<&Reached<'g>> for ReachedView<'g> {
    fn from(reached: &Reached<'g>) -> Self {
        Self {
            symbol: DefinitionView::from(&reached.symbol),
            depth: reached.depth,
            confidence: reached.confidence,
            line: reached.line,
        }
    }
}

/// The answer of a walk over calls: one match for each graph the name selects a symbol in, in the order of the
/// engine's graphs.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Walks<'g> {
    matches: Vec<WalkView<'g>>,
}

#[derive(Debug, Serialize)]
struct WalkView<'g> {
    graph: &'g str,
    root: DefinitionView<'g>,
    passed_over: usize,
    reached: Vec<ReachedView<'g>>,
    truncated: bool,
    turned_away: usize,
    unexpanded: usize,
    below_confidence: usize,
    unresolved: usize,
}

impl<'g> Walks<'g> {
    pub(crate) fn push(&mut self, graph: &'g str, walk: &Walk<'g>) {
        self.matches.push(WalkView {
            graph,
            root: DefinitionView::from(&walk.root),
            passed_over: walk.passed_over,
            reached: walk.reached.iter().map(ReachedView::from).collect(),
            truncated: walk.truncated,
            turned_away: walk.turned_away,
            unexpanded: walk.unexpanded,
            below_confidence: walk.below_confidence,
            unresolved: walk.unresolved,
        });
    }
}
