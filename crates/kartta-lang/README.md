# kartta-lang

The contract a language implements to plug into the kartta code graph.

The graph knows no language. A language is a value implementing `Language`: it claims the files that are its, extracts
the definitions, calls and imports of one file from the file's bytes, and resolves that extraction against the file's
path and the repository. kartta's own languages are crates of their own, and a language kartta does not ship is one more
crate implementing the same trait — registered with the engine, never added to it.

## Two halves

`Language::extract` reads only the bytes. Its `RawExtraction` is the same for every file with the same content,
wherever it sits, so a cache keyed by the content's hash and the language's `fingerprint` computes it once for every
tree on a machine. Whatever depends on the path or the repository — which file an import names, whether the file is a
test — is recorded as a directive and applied by the `Resolver` the language builds over a read-only `Repository`. A
`MemoryRepository` holds one in memory, for a test of a resolver.

The fingerprint must change whenever what `extract` computes from the same bytes can change: the grammar, the walk,
the shape of the result. A cache keys an extraction by the hash of the bytes, the language's `fingerprint` and this
crate's `FINGERPRINT` — the code of the helpers and of the encoding the extraction is stored in — so a new version of a
language or of this crate never reads an old one's. A language crate's build script calls
`kartta_lang::fingerprint::set_source_id`, which hands the crate its name, version and a digest of its `Cargo.toml`, of
the versions the build's `Cargo.lock` resolves for its `[dependencies]` and of the files under its `src` as
`SOURCE_ID`, so the fingerprint follows the crate's code with no version kept by hand, and a lock-only bump of the
grammar or of tree-sitter invalidates what a cache holds. A build that finds no lock, as a registry dependency built
into a target directory outside the consumer's workspace can, follows its manifest's version requirements alone.

## Call conventions

How a call resolves differs between languages — C++ looks in the caller's class first, sees what its includes include
and guesses no nearby member for a call on a value, Go never reaches a method without a receiver — and the graph cannot
infer it from the extractions. A language states it in `Conventions`, as a `const` that sets the fields it changes on
`Conventions::DEFAULT`.

## Doc comments

A definition's doc comment sits on the lines above it, in the comment forms of its language, beside what a declaration
carries there, a Rust attribute or a C++ template header. A language states those forms in
`DocComments`, and a query that answers a definition's text reads its doc comment from the file when asked, so no
extraction records it.

## Generated-code banners

A tool that generates a file says so in the comments that open it, with a marker such as `@generated` or `DO NOT EDIT`.
A language states its markers and the comment forms of its header as a `Banner`, and flags the symbols of a file the
banner opens as generated code. A marker inside a backtick span is a mention, and one below the first code is no
banner. `Banner::opens` also carries a code fence, as the example of a doc comment opens one, across the comments of
the header, while `Banner::marks` reads one comment on its own.

## Parsing and walking tree-sitter trees

The `tree-sitter` feature adds `kartta_lang::tree`: a parse on the parser each thread keeps per grammar, a pre-order
walk, the text and signature helpers a language extracting through tree-sitter needs, and the fingerprint of such a
language from its `SOURCE_ID` and its grammar. The contract itself names no parser.
