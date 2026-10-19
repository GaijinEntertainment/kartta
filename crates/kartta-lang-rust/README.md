# kartta-lang-rust

Rust for the kartta code graph: the items, members and calls of Rust source, and the files its `mod` declarations
pull in.

`Rust` implements the `kartta_lang::Language` contract over the `tree-sitter-rust` grammar. It claims every file whose
extension is `rs`, in any case.

## What a file extracts to

Every function, struct, union, enum, trait, type alias, `macro_rules!`, constant, static and inline module is a
symbol, and so is every named field and every enum variant. A member of a `trait`, or of an `impl` whose target names a
type, is owned by that type and qualified under it, `Service::new`; a member of an `impl` for a tuple, `dyn` or
function type has no owner. An item of an inline module is qualified by the chain of its modules,
`a::b::Worker`, except a `macro_rules!`, which keeps its bare name. `impl Trait for Type` adds `Trait` to the parents
of the `Type` the file declares under that name or path.

A call records its callee and, where the syntax spells one, its receiver: `self.greet()` and `Self::new()` record the
type of the `impl` around them, and `Widget::new()` records `Widget`. A macro invocation is a call of the macro. A call
of an item that a function body declares is a local call of that item. The conventions name the methods and associated
functions of the standard library that code calls most, so a call of one through a receiver or a path, `name.clone()`
or `fs::write()`, binds a member of the name and no free function of the repository. They list `other` alone as the
receiver of a value of the caller's own type, since a call on `self` binds through the type it records, so
`input.parse()` inside a `parse` binds no member of the caller's own type.

A name read where no call takes it, `items.map(helper)`, `Handler { on_event: helper }` or `SETTINGS.enabled`, is a use
of it, and so are `Self::make` and `Service::make` of the member `make`. A name a pattern binds, a parameter, a `let`,
a closure's parameter, a match arm or a `for`, hides it from the end of its binding to the end of its block, arm or
loop. A bare read is kept only where its inline module declares an item of its name, since it binds only an item the
file declares outside every owner and body, and a read through a type only where the file declares that member.

## Resolution

A bodiless `mod NAME;` includes the file it pulls in, by its `#[path]` override or by Cargo's module layout: beside a
crate root or a file named `lib.rs`, `main.rs` or `mod.rs`, under the directory named for any other file's stem, and
one directory deeper for each inline module around it. A directory that holds a `Cargo.toml` is a package, and its
crate roots are `src/lib.rs`, `src/main.rs`, `build.rs` and each target under `src/bin/`, `examples/`, `tests/` and
`benches/`. A top-level `fn main` in a crate root other than the library is the entry point.

Code gated by `#[test]`, `#[bench]` or a `cfg` that holds only under `test` is test code, and so is a file below a
`tests` directory outside `src`. A `*.gen.rs` or `*_generated.rs` file, or one with a `@generated` or `DO NOT EDIT`
marker in its leading comments, is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

Status: the API is new and is not published yet.
