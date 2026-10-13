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
//! # Status
//!
//! This release carries no API. The engine is being ported into this repository crate by crate, starting with
//! `kartta-fs`, and the first release with a public surface will be `0.1.0`.
