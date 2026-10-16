//! Multi-language code graph engine: repository file index, tree-sitter extraction, graph queries.
//!
//! kartta walks a repository once, extracts symbols and references per language on tree-sitter grammars, and answers
//! queries over the resulting code graph — symbol lookup, call and reference edges, path and content search. The engine
//! is the single source of truth for those semantics; the Go and Node bindings published alongside it translate and
//! decide nothing.
//!
//! This crate is the entry point: it owns the engine and re-exports the types that cross its API boundary, so ordinary
//! use needs no direct dependency on the `kartta-*` building blocks.
//!
//! The engine names no language of its own. [`Engine::build`] is handed the languages to build with, each a value
//! implementing [`Language`], so a language kartta does not ship plugs in as one more crate implementing that trait.
//!
//! # Examples
//!
//! ```
//! use kartta::{BuildOptions, Engine, Extraction, Language, RawExtraction, Repository, Resolver, Symbol, SymbolKind};
//!
//! #[derive(Debug)]
//! struct Defs;
//!
//! impl Language for Defs {
//!     fn name(&self) -> &str {
//!         "defs"
//!     }
//!
//!     fn claims(&self, path: &str) -> bool {
//!         path.ends_with(".defs")
//!     }
//!
//!     fn fingerprint(&self) -> &[u8] {
//!         b"defs 1"
//!     }
//!
//!     fn extract(&self, source: &[u8]) -> RawExtraction {
//!         let mut raw = RawExtraction::default();
//!         for (at, line) in String::from_utf8_lossy(source).lines().enumerate() {
//!             if let Some(name) = line.strip_prefix("def ") {
//!                 let line = u32::try_from(at + 1).unwrap_or(u32::MAX);
//!                 raw.symbols.push(Symbol::new(name, SymbolKind::Function, line, line));
//!             }
//!         }
//!         raw
//!     }
//!
//!     fn resolver<'r>(&'r self, _repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
//!         Box::new(Paths)
//!     }
//! }
//!
//! struct Paths;
//!
//! impl Resolver for Paths {
//!     fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
//!         raw.into_extraction(path).0
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let tree = tempfile::tempdir()?;
//! std::fs::write(tree.path().join("main.defs"), "def parse\n")?;
//!
//! let outcome = Engine::build(tree.path(), &BuildOptions::default(), vec![Box::new(Defs)], None)?;
//! assert!(outcome.errors().is_empty());
//! let engine = outcome.into_engine();
//!
//! let graph = engine.graph_of("main.defs").ok_or("defs claims the file")?;
//! assert_eq!(graph.definitions("parse", None, 10).total, 1);
//! # Ok(())
//! # }
//! ```
//!
//! # Status
//!
//! The engine builds the index and each family's graph of definitions, calls and uses, and each graph answers callers,
//! callees, references and call paths. The first release with a public surface will be `0.1.0`.

mod engine;
mod extracting;
mod repository;

pub use kartta_fs::{BuildError, BuildOptions, CancelToken, Index};
pub use kartta_graph::{
    CallPath, Definition, Definitions, Found, Graph, Limits, Nearness, Reached, Reference,
    References, Walk, confidence,
};
pub use kartta_lang::{
    Conventions, Extraction, Language, RawExtraction, Repository, Resolver, Symbol, SymbolFlags,
    SymbolKind,
};

pub use crate::engine::{Engine, EngineOutcome};
