# kartta-lang-lua

Lua for the kartta code graph: the functions and table methods of Lua source, the calls between them, and the modules a
file loads.

`Lua` implements the `kartta_lang::Language` contract over the `tree-sitter-lua` grammar. It claims every file whose
extension is `lua`, in any case.

## What a file extracts to

A named function is a symbol: a declaration such as `function M.f`, a function bound to a name such as
`local f = function() end`, and a function in a field of a table bound to a name, `local T = { f = function() end }`.
A function on a table is a method of the table, `M.f`, and a function declared or bound with `local` is not exported;
a method of a local table stays exported, because a module hands that table out by `return M`.
A call inside a function is a call made by it, and a call through `.` or `:` records the table as its receiver when the
table is a plain name. A name read where no call takes it, `table.sort(t, compare)` or `{ on_click = handler }`, is a
use of it, which binds the function of its name the file declares outside every function; a parameter hides its name
in its function, a local from the end of its declaration to the end of its block, the chunk too, and a loop variable
inside its loop. Outside every function, a local function, a local bound to a function and a local declared without a
value, which `function f()` binds later, hide nothing.

## Resolution

A bare `require`, `dofile` or `loadfile` with a string literal loads a module and records no call. `require "a.b"`
includes `a/b.lua` or the package `a/b/init.lua`, from the root and then from the requiring file's directory;
`dofile` and `loadfile` include the path as written, from the file's directory and then from the root. A module the
repository does not hold includes nothing. A `*_spec.lua` or `*_test.lua` file, or one under `spec`, `test` or `tests`
in any case, is test code; a `*.gen.lua` or `*_pb.lua` file, or one whose leading comments carry `@generated` or
`DO NOT EDIT`, is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
