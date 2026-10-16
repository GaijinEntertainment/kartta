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
//! [`Graph::references`] and [`Graph::call_path`] resolve the sites of the names they reach when they are asked, so no
//! answer goes stale when another file changes.
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
//! main.calls.push(CallRef {
//!     caller_id: SymbolId(0),
//!     callee: "Parse".to_owned(),
//!     receiver: None,
//!     line: 2,
//! });
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

mod calls;
mod candidates;
pub mod confidence;
mod graph;
mod order;
mod resolve;
mod scope;
mod stored;
mod strings;

pub use crate::calls::{CallPath, Limits, Reached, Reference, References, Walk};
pub use crate::graph::{Definition, Definitions, Found, Graph, Nearness};
