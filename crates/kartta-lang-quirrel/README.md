# kartta-lang-quirrel

Quirrel for the kartta code graph: the definitions, calls and imports of Quirrel scripts.

`Quirrel` implements the `kartta_lang::Language` contract over the `tree-sitter-quirrel` grammar. It claims every file
whose extension is `nut`, in any case.

## What a file extracts to

Every named function, class, enum, enum member and constant is a symbol, and so is a function, a lambda or a class
bound to a name: a function value takes the name its callers use, from a table slot, a `let` or `local`, or the target
of an `=` or `<-`. A member of a named or bound class is a method of the class, and `gui.Widget.build <- function() {}`
is a method of `gui.Widget`. A data slot with a plain name, of a class or of a table a file-scope `let` or `local`
binds, is a field. A function value with no binding and no name of its own gives its calls to no symbol.

A call records its callee and, for a member call, the whole receiver expression as written. The conventions name the
methods of Quirrel's built-in types, so a member call of one, `list.len()` or `t.rawin()`, binds a member of the name
and no free function of the repository.

A name read where no call takes it, `subscribe("evt", onEvent)` or `{ onClick = handler }`, and that no parameter or
local of an enclosing function declares, is a use of it. It binds a definition of its own file only: the graph does not
keep the module that a `from` import or a destructured `require` takes a name from.

## Resolution

Every `import`, `from ... import`, and `require("spec")` or `require_optional("spec")` with one string literal names a
module: an include of the file it resolves to, or an import kept as written when it names no file. A script resolves
through the nearest `.sqconfig` in its directory or above it: `%name/rest` below the directory the file mounts as
`name`, any other `.nut` specifier beside the script or below the first base path that holds it. A script that no
`.sqconfig` governs resolves `%root/rest` and slashed or `.nut` specifiers under its nearest directory that holds them.
A bare name is a native module. Every match is case-sensitive. A file under a
`test` or `tests` directory, or named `*_test.nut` or `test_*.nut`, is test code; a file named `*.gen.*` or `*_gen.*`,
or whose leading comments carry `@generated`, `DO NOT EDIT` or `Code generated`, is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

Status: the API is new and is not published yet.
