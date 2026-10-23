# kartta-lang-daslang

Daslang for the kartta code graph: the definitions, calls and requires of daslang source.

`Daslang` implements the `kartta_lang::Language` contract over the `tree-sitter-daslang` grammar. It claims every file
whose extension is `das`, in any case.

## What a file extracts to

The grammar reads only the second-generation syntax, which a file opts into with a top-level `options gen2`. A file
without it, or with `options gen2 = false`, extracts to nothing.

Every `def`, `struct`, `class`, field, `enum`, `bitfield`, `variant`, `tuple`, `typedef` and global `let` or `var` is a
symbol, qualified by the file's `module` clause: `ui.draw`, and `ui.Button.press` for a member of `Button`. The clause
also names the file's package. A `def` inside a type is a method, and a `struct` or `class` names its `: Parent` as its
parent.

A call records its callee and, for a member call, its receiver: the object when it is a bare name, and the empty
receiver otherwise. A module-scoped call `mod::f()` keeps `mod` as its receiver, and a construction `T(...)` or
`new T(...)` is a call of `T`. A block applied in place keeps its calls in the enclosing `def`; a block stored in a name
gives its calls to no symbol. A function's address, `@@f` or `@@mod::f`, the one form daslang passes a function as a
value in, is a use of `f`, which binds the function of the name its own file declares, else the one a file it requires
exports, else the one such a file declares `private`, as a call of the name binds it. The conventions list `self` as
the receiver of a value of the caller's own type, so `super.m()` and a member call on another name, `child.init()`
inside an `init`, bind no member of the caller's own class.

Code that a macro takes as data runs where its result is compiled, not where it is written, so a call or a construction
in it is no call of the enclosing `def`: `quote(foo())` calls `quote` and uses `foo`, which the compiled code names.
That code is the argument of `quote`, `qmacro`, `qmacro_block`, `qmacro_block_to_array`, `qmacro_expr` and
`qmacro_type`; the argument after the name of `qmacro_function` and `qmacro_variable`, or after the name and the class
of `qmacro_method`; and the pattern of `qmatch`, `qmatch_block` and `qmatch_function`, whose matched expression runs. A
block after the call is its last argument. The expression of a `qmacro` splice, `$e`, `$i`, `$v`, `$b`, `$a`, `$t`, `$c`
or `$f`, runs, so its calls belong to the enclosing `def`; the arguments of a `$c(name)(...)` splice are data. `_::f`,
the compiler's name for the scope that compiles the code, names what a bare `f` does.

## Resolution

A `require` or an `include` that names a file is an include of it; one that names no file is an import, kept as written.
The resolver reads the repository's file names as the module index, because a module almost always declares its own
file stem: a relative specifier names the file beside the importer, a dotted or slashed one the first file it reaches
under the importer's directories, then the one file whose path ends with it, then the one file named for its last
component, and a bare name the one file named for it. A specifier two files could satisfy names neither, and every
match is case-sensitive. A file under a `test` or `tests`
directory, or named `*_test.das` or `test_*.das`, is test code; a file named `*.gen.*` or `*_gen.*`, or whose leading
comments carry `@generated`, `DO NOT EDIT` or `Code generated`, is generated code. A `def` that carries an annotation
other than a compile hint of the runtime, such as `[unused_argument]` or `[no_aot]`, is an entry point, because the
annotation registers it with what calls it.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

## Licence

The crate is MIT. The `tree-sitter-daslang` grammar it links is BSD-3-Clause.

Status: the API is new and is not published yet.
