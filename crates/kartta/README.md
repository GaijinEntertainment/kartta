# kartta

Multi-language code graph engine: repository file index, tree-sitter extraction, graph queries.

kartta walks a repository once, extracts symbols and references per language on tree-sitter grammars, and answers
queries over the resulting code graph — symbol lookup, call and reference edges, path and content search.

This crate is the entry point. It owns the engine and re-exports the types that cross its API boundary, so ordinary use
needs no direct dependency on the `kartta-*` building blocks. An MCP server over the same engine ships separately as
`kartta-mcp`, and the Go and Node bindings call it through the C ABI crate `kartta-ffi`.

The engine names no language of its own: it is handed the languages to build with, each a value implementing
[`kartta-lang`](https://crates.io/crates/kartta-lang)'s `Language` trait, so a language kartta does not ship plugs in as
one more crate. [`kartta-languages`](https://crates.io/crates/kartta-languages) gives every language kartta ships, or
the ones a list of names selects.

Status: the engine builds the file index and each language family's graph of definitions, calls and uses from one walk
that reads each claimed text file once — twice only for a file whose extraction the shared cache holds in a form the
build cannot read, or one a language's resolver asks for again, as configuration it reads. Each graph answers callers,
callees, references and call paths, resolving calls when a query asks, the files that include a file, the implementors
of a type and what a change to a range of a file's lines reaches, and searches definitions by the words of their names
and paths; the engine answers what a change to regions across families reaches, how many files each language claimed,
how many of them the walk read, the cache held and the graph holds, how every call site of each family binds,
resolved on the index's threads, and the text of definitions with the doc comments above them, read from their files
when asked. The first release with a public surface will be `0.1.0`.
