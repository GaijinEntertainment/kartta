//! A language-agnostic code graph: the definitions, calls and uses of one language family's files.
//!
//! A [`Graph`] takes in [`Extraction`](kartta_lang::Extraction)s, one per file, as the languages of `kartta-lang`
//! produce them, and answers what a file defines, where a name is defined, and who calls what. It names no language:
//! what differs between languages reaches it as the extractions and the [`Conventions`](kartta_lang::Conventions) it
//! is built with.
//!
//! The graph is kept a file at a time. [`Graph::insert`] replaces whatever it held for a path and [`Graph::remove`]
//! forgets a path, so a graph follows a tree that changes without being built again, and inserting or removing one
//! file renumbers nothing else.
//!
//! A file keeps its call sites and uses as its language wrote them, and [`Graph::callers`], [`Graph::callees`],
//! [`Graph::references`], [`Graph::call_path`], [`Graph::analyze_segment`] and [`Graph::analyze_change`] resolve the
//! sites of the names they reach when they are asked, so no answer goes stale when another file changes.
//! [`Graph::resolution`] resolves every site, to answer how the graph's calls bind.
//!
//! # Examples
//!
//! ```
//! use kartta_graph::{Graph, Limits};
//! use kartta_lang::{CallRef, Conventions, Extraction, Symbol, SymbolId, SymbolKind};
//!
//! let mut parse = Extraction::default();
//! parse.file = "src/parse.go".to_owned();
//! parse.symbols.push(Symbol::new("Parse", SymbolKind::Function, 3, 9));
//!
//! let mut main = Extraction::default();
//! main.file = "src/main.go".to_owned();
//! main.symbols.push(Symbol::new("main", SymbolKind::Function, 1, 4));
//! main.calls.push(CallRef::new(SymbolId(0), "Parse".to_owned(), None, 2));
//!
//! let mut graph = Graph::new(Conventions::default());
//! graph.insert(&parse);
//! graph.insert(&main);
//!
//! let found = graph.definitions("Parse", None, 10);
//! assert_eq!(found.total, 1);
//! assert_eq!(found.found[0].definition.file, "src/parse.go");
//!
//! let callers = graph.callers("Parse", Limits::new(1)).ok_or("Parse is defined")?;
//! assert_eq!(callers.reached[0].symbol.name, "main");
//! assert_eq!(callers.reached[0].line, 2);
//! # Ok::<(), &str>(())
//! ```

mod aliases;
mod analyze;
mod calls;
mod candidates;
mod change;
pub mod confidence;
mod dead;
mod dependents;
mod dispatch;
mod graph;
mod implementations;
mod members;
mod one_edit;
mod order;
mod resolution;
mod resolve;
mod scope;
mod search;
mod stats;
mod stored;
mod strings;
mod twins;
mod typed;
mod words;

pub use crate::analyze::{Blast, DirectCalls, ReachBand, Segment, SegmentSymbol};
pub use crate::calls::{
    CallPath, FileScope, Limits, Reached, Reference, References, SelfCall, SiteCounts, SiteQuery,
    Walk,
};
pub use crate::change::{Change, DirCount, Hotspot, Region, Seed};
pub use crate::dead::{Dead, DeadCode, Excluded};
pub use crate::dependents::{Dependent, Dependents};
pub use crate::graph::{
    Candidate, Definition, Definitions, Found, Graph, Nearness, Selector, is_parent, is_referent,
};
pub use crate::implementations::{Implementations, Implementor, OutsideImplementations};
pub use crate::resolution::{Miss, Misses, Resolution, Unbound};
pub use crate::search::{Hit, Search};
pub use crate::stats::Stats;
