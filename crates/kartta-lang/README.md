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
test — is recorded as a directive and applied by the `Resolver` the language builds over a read-only `Repository`.

The fingerprint must change whenever what `extract` computes from the same bytes can change: the grammar, the walk,
the shape of the result. A cache keys an extraction by the hash of the bytes, the language's `fingerprint`, this
crate's `FINGERPRINT` — the code of the helpers and the encoding — and `RawExtraction::CODEC`, the layout the
extraction is stored in, so a new version of a language or of this crate never reads an old one's.

## Call conventions

How a bare call resolves differs between languages — C++ looks in the caller's class first, Go never reaches a method
without a receiver — and the graph cannot infer it from the extractions. A language states it in `Conventions`.

## Walking tree-sitter trees

The `tree-sitter` feature adds `kartta_lang::tree`: a pre-order walk and the text and signature helpers a language
extracting through tree-sitter needs. The contract itself names no parser.

Status: the API is new and is not published yet.
