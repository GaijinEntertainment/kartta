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
//! The `kartta-languages` crate gives every language kartta ships, or the ones a list of names selects.
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
//!     fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
//!         Box::new(Paths)
//!     }
//! }
//!
//! struct Paths;
//!
//! impl Resolver for Paths {
//!     fn resolve(&self, raw: RawExtraction, path: &str, _repository: &dyn Repository) -> Extraction {
//!         raw.into_extraction(path).0
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let tree = tempfile::tempdir()?;
//! std::fs::write(tree.path().join("main.defs"), "def parse\n")?;
//!
//! let outcome = Engine::build(tree.path(), &BuildOptions::default(), vec![Box::new(Defs)], None)?;
//! assert!(outcome.errors.is_empty());
//! let engine = outcome.engine;
//!
//! let (_, graph) = engine.graph_of("main.defs").ok_or("defs claims the file")?;
//! assert_eq!(graph.definitions("parse", None, 10).total, 1);
//! # Ok(())
//! # }
//! ```
//!
//! # Status
//!
//! The engine builds the index and each family's graph of definitions, calls and uses, and each graph answers callers,
//! callees, references, call paths, the files that include a file, the implementors of a type, what a change to a range
//! of a file's lines reaches and searches by the words of names and paths; [`Engine::analyze_change`] answers what a
//! change to regions across families reaches, [`Engine::stats`] what the build made of each language's files,
//! [`Engine::resolution`] how every call site binds, and [`Engine::sources`] the text of definitions with their doc
//! comments; [`ask_reading`] asks the graphs about a qualified name spelled with another language's owner separator,
//! and [`unmatched`] what a name that selected no symbol names instead.
//! The first release with a public surface will be `1.0.0`.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod engine;
mod extracting;
#[cfg(feature = "watch")]
mod follow;
mod following;
mod reading;
mod repository;
mod resolving;
mod source;
mod stats;

pub use kartta_fs::{
    BinaryExtensions, BinaryExtensionsError, BuildError, BuildOptions, CancelToken, Capped, Case,
    ContentIndexStats, ContentIndexStorage, Cut, Entry, EntryKind, Failure, Filter, FilterError,
    FuzzyMatch, FuzzyMatches, FuzzyOptions, GlobOptions, GrepOptions, IgnoreSources, Index, Line,
    LineKind, ListError, ListOptions, Listed, Listing, MatchedFile, Matches, PathFault, Paths,
    PatternError, Patterns, PrunedNames, PrunedNamesError, RefreshOutcome, Stopped,
};
#[cfg(feature = "watch")]
pub use kartta_fs::{WatchError, Watcher};
pub use kartta_graph::{
    Blast, CallPath, Candidate, Change, Dead, DeadCode, Definition, Definitions, Dependent,
    Dependents, DirCount, DirectCalls, Excluded, FileScope, Found, Graph, Hit, Hotspot,
    Implementations, Implementor, Limits, Miss, Misses, Nearness, ReachBand, Reached, Reference,
    References, Region, Resolution, Search, Seed, Segment, SegmentSymbol, Selector, SelfCall,
    SiteCounts, SiteQuery, Stats, Unbound, Walk, confidence, is_parent, is_referent,
};
pub use kartta_lang::{
    Access, BlockComment, Conventions, DocComments, Extraction, Language, RawExtraction,
    Repository, Resolver, Symbol, SymbolFlags, SymbolKind, TypeUses,
};

pub use crate::engine::{Engine, EngineOutcome, EngineRefresh, Widened, replace_failures};
#[cfg(feature = "watch")]
pub use crate::follow::{Followed, Follower, Watch};
pub use crate::reading::{Unmatched, UnmatchedKind, ask_reading, selects_as_callers, unmatched};
pub use crate::source::{CutLine, Source, SourceOptions};
pub use crate::stats::{EngineStats, LanguageFiles};
