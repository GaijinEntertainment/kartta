# kartta-lang-blk

BLK for the kartta code graph: the includes of Dagor `DataBlock` text files.

`Blk` implements the `kartta_lang::Language` contract over the `tree-sitter-blk` grammar. It claims every file whose
extension is `blk`, in any case.

## What a file extracts to

A BLK file declares no symbols and makes no calls. Every `include` statement with a path is a directive, its path kept
as written without its quotes. A file the engine loads as binary extracts to nothing, and so does a text that can spell
neither the `include` keyword nor a `mountPoints` block name.

## Resolution

An include names the file the engine loads for it, with the path read as the engine reads it, and one that names no
file is an import, kept as written. A relative path names the file beside the including file, a `#` path the one
repository file that ends with it, and a `%name/rest` path `rest` below the directory a `mountPoints` block maps `name`
to. The mount table is read from the files named `settings.blk`, `application.blk`, `aot_config.blk` and
`libs_aot_config.blk`. An absolute path, a drive-letter one included, names no file. A relative or a `%name/rest` path
that names no file takes the one file that differs from it only in ASCII letter case, as the game's vromfs does; a `#`
path and a mount's name keep letter case.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
