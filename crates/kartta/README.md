# kartta

Multi-language code graph engine: filesystem index, tree-sitter extraction, graph queries.

kartta walks a repository once, extracts symbols and references per language on tree-sitter grammars, and answers
queries over the resulting code graph — symbol lookup, call and reference edges, path and content search.

This crate is the entry point. It owns the engine and re-exports the types that cross its API boundary, so ordinary use
needs no direct dependency on the `kartta-*` building blocks. An MCP server over the same engine ships separately as
`kartta-mcp`, and Go and Node bindings are published from the engine's C ABI.

Status: this release carries no API. The engine is being ported into this repository crate by crate, starting with
[`kartta-fs`](https://crates.io/crates/kartta-fs); the first release with a public surface will be `0.1.0`.
