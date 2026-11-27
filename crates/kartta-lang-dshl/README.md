# kartta-lang-dshl

DSHL for the kartta code graph: the shaders, blocks, macros and variables of Dagor Engine shader source, the macro
invocations between them and the files they include.

`Dshl` implements the `kartta_lang::Language` contract over the `tree-sitter-dshl` grammar. It claims every file whose
extension is `dshl`, in any case.

## What a file extracts to

Each name of a `shader` is a class, a `block` an interface, a `macro` a macro, and a render pass, a render pass group
and a framebuffer layout each a class. A variable or a sampler at file scope is a variable; inside a shader, a block or
a macro it is a field of that declaration, and a `local` variable is no symbol. A preshader binding is a field, a `bool`
a constant, and an `interval` an enum whose ranges are its variants. DSHL has one namespace per compilation, so a
declaration at file scope is qualified by its own name: `frame_tex`, and `water.foam_tex` for a field of `water`. A
`define_macro_if_not_defined` is a macro flagged as a fallback: the build skips it where a game defines the macro
first, so a call bound to it may run the game's own.

A `supports` statement makes each block it names a parent of the shader or block around it, of every name of a shader
with several. A macro invocation is a call of the macro, made by the declaration around it, or from file scope outside
every declaration. A name that a condition, an `assume`, a `##if`, `##bool` or `##declare` directive, a preshader value
or an initial value reads is a use; keyword-like values, intrinsics and hardware features are not. The text of an
`hlsl { }` block is HLSL: no HLSL declaration is a symbol, and only its `#include` lines are read, as includes.

## Resolution

The shader compiler looks an include up under the include paths of the build configuration that compiles the file,
and the repository does not record which configuration that is. The resolver matches a spelling against the files whose
path ends in it, and of several takes the one that shares the most directories with the including file; two files
equally near resolve to neither, and the include is kept as an import. A spelling that climbs, `../common.dshl`, is
relative to the including file.

A use binds what its own file declares, else what the one file it includes directly declares, else what the one file
those include in turn declares; a name two files of one such step declare binds neither.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
