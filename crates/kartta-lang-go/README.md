# kartta-lang-go

Go for the kartta code graph: the definitions, calls, uses and imports of Go source.

`Go` implements the `kartta_lang::Language` contract over the `tree-sitter-golang` grammar. It claims every file whose
extension is `go`, in any case.

## What a file extracts to

Every function, method, type, interface member, struct field, constant and `var`-declared variable is a symbol,
qualified by the package clause: `api.Serve`, and `api.Server.Close` for a member of `Server`; a `:=` declaration is a
local binding, not a symbol. An embedded type is a parent of the type
that embeds it, and so is the interface of a compile-time assertion such as `var _ io.Reader = (*File)(nil)`.

A call records its callee and, for a member call, its receiver. Go spells an owner as a bare type name, so only an
identifier operand is a receiver: `s.client.Do()` and `f().g()` record the empty receiver, a member call on a value of
no known type, and so does a call on a parameter or a variable the caller declares. The method's own receiver records
the owner its declaration spells: `w.draw()` in `func (w *Widget) run()` records `Widget`. A name outside a call — a
type in a signature, a field, a conversion — is a use of what it names, unless a local binding covers it.

## Resolution

The resolver reads every `go.mod` outside a `vendor` directory. An import names the vendored copy of its package when
the repository holds one, and otherwise the directory under the module whose path is its longest prefix, `replace`
directives onto local directories included. A resolved import includes every Go file of the package that is not a test
file. A `_test.go` file is test code, and a `*.pb.go` file or one with the `// Code generated ... DO NOT EDIT.` banner
is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

Status: the API is new and is not published yet.
