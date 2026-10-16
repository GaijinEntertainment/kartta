//! A language-agnostic code graph: the definitions of one language family's files, found by name.
//!
//! A [`Graph`] takes in [`Extraction`](kartta_lang::Extraction)s, one per file, as the languages of `kartta-lang`
//! produce them, and answers what a file defines and where a name is defined. It names no language: what differs
//! between languages reaches it as the extractions and the [`Conventions`](kartta_lang::Conventions) it is built with.
//!
//! The graph is kept a file at a time. [`Graph::insert`] replaces whatever it held for a path and [`Graph::remove`]
//! forgets a path, so a graph follows a tree that changes without being built again, and inserting or removing one
//! file renumbers nothing else.
//!
//! # Examples
//!
//! ```
//! use kartta_graph::Graph;
//! use kartta_lang::{Conventions, Extraction, Symbol, SymbolKind};
//!
//! let mut extraction = Extraction::default();
//! extraction.file = "src/parse.go".to_owned();
//! extraction.symbols.push(Symbol::new("Parse", SymbolKind::Function, 3, 9));
//!
//! let mut graph = Graph::new(Conventions::default());
//! graph.insert(&extraction);
//!
//! let found = graph.definitions("Parse", None, 10);
//! assert_eq!(found.total, 1);
//! assert_eq!(found.found[0].definition.file, "src/parse.go");
//! ```

mod graph;
mod strings;

pub use crate::graph::{Definition, Definitions, Found, Graph, Nearness};
