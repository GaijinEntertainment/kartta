# kartta-lang-go

Go for the kartta code graph: the definitions, calls, uses and imports of Go source.

`Go` implements the `kartta_lang::Language` contract over the `tree-sitter-golang` grammar. It claims every file whose
extension is `go`, in any case, unless its name starts with `_` or `.`, which no build of the go command reads: the
graph holds no such file, and the repository's file index does.

## What a file extracts to

Every function, method, type, interface member, struct field, constant and `var`-declared variable is a symbol,
qualified by the package clause: `api.Serve`, and `api.Server.Close` for a member of `Server`; a `:=` declaration is a
local binding, not a symbol, unless the name it declares takes a literal of an anonymous struct with a field. An
anonymous struct with a field that a struct field's or a variable's type spells, behind pointers and containers too, or
that a literal `:=` or `var` assigns to a name spells, `tests := []struct{...}{...}`, is no symbol: the field or the
variable it types owns its fields, `Limits` for the field `Limits` of `Server` and `tests` for a body's `tests`, and
holds the types the struct embeds; the fields are qualified by the names that lead to them, `api.Server.Limits.MaxConns`
and `api.TestRun.tests.name`. An embedded type is a parent of the type that embeds it. A compile-time assertion such as
`var _ io.Reader = (*File)(nil)` embeds nothing and is recorded as an assertion.

A call records its callee and, for a member call, its receiver as a type path: the type of the operand's value, as far
as the file's declarations spell it. `s.client.Do()` in `func f(s *Server)` records `Server.client`, and `w.draw()` in
`func (w *Widget) run()` records `Widget`; the graph follows the path through the value types each definition records,
a field's type or a function's results, to the method the call reaches. A name outside a call — a
type in a signature, a field, a conversion, a package-level value or function — is a use of what it names, unless a
local binding covers it. A member taken on a value without a call, `s.client` or the method value `w.draw`, is a use
whose receiver is the same type path, and so is each key of a struct literal, `Config{Timeout: 5}`, and each element of
one without keys whose struct the file declares, `Config{5, 3}`; a literal of an anonymous struct assigned to no name
keeps none. A key of a map, slice or array literal whose type is written so or declared so in the file reads the value
it names, and so does an unexported key of another package's type; a key of a sibling file's type, or an exported key
of another package's, writes a field. Each use is marked a read, a write — the left side of `=`, a struct literal's key
or element — or an update, `+=` or `++`.

## Resolution

The resolver reads every `go.mod` outside a `vendor` directory. An import names the vendored copy of its package when
the `vendor` directory of the module that builds the importing file holds one, that file's own module or, where that
one holds no `vendor/modules.txt`, the module that replaces it onto its directory, and otherwise the directory under
the module whose path is its longest prefix, `replace` directives onto local directories included. A resolved import
includes every Go file of the package that is neither a test file nor one it leaves unclaimed, except the importing
file itself. A `_test.go` file is test code, and a `*.pb.go` file or one with the `// Code generated ... DO NOT EDIT.`
banner is generated code.

A file's build constraint is what its `//go:build` line says, else its `// +build` lines, as go/build reads them, and
the tags its `_$GOOS`, `_$GOARCH` or `_$GOOS_$GOARCH` name requires. Two files are built apart where no build satisfies
both: a build has one `GOOS`, which sets `linux` under `android`, `solaris` under `illumos`, `darwin` under `ios` and
`unix` under a Unix one, one `GOARCH`, one compiler of `gc` and `gccgo`, one release, which sets every `go1.N` up to its
own, and any other tags.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
