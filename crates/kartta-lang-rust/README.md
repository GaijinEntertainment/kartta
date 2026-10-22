# kartta-lang-rust

Rust for the kartta code graph: the items, members and calls of Rust source, and the files its `mod` declarations
pull in.

`Rust` implements the `kartta_lang::Language` contract over the `tree-sitter-rust` grammar. It claims every file whose
extension is `rs`, in any case.

## What a file extracts to

Every function, struct, union, enum, trait, type alias, `macro_rules!`, constant, static and inline module is a
symbol, and so is every named field and every enum variant. A member of a `trait`, or of an `impl` whose target names a
type, is owned by that type and qualified under it, `Service::new`; a member of an `impl` for a tuple, `dyn` or
function type has no owner. A function of an `extern` block that `#[wasm_bindgen(method)]` marks is owned by the type
its first parameter, `this`, refers to, since wasm-bindgen makes it that type's method. An item of an inline module is
qualified by the chain of its modules, `a::b::Worker`, except a `macro_rules!`, which keeps its bare name.
`impl Trait for Type` adds `Trait` to the parents of the `Type` the file declares under that name or path.

A call records its callee and, where the syntax spells one, its receiver: `self.greet()` and `Self::new()` record the
type of the `impl` around them, and `Widget::new()` records `Widget`. A method call on a lowercase value other than
`other` records the name after a `.`, `.socket` for `socket.connect()`, since alone it would read as the module that
a path call, `socket::connect()`, names; `Fish.write()` records `Fish`. A macro invocation is a call
of the macro. A call of an item that a function body declares is a local call of that item. The conventions name the
methods and associated functions of the standard library that code calls most, so a call of one through a receiver or
a path, `name.clone()` or `fs::write()`, binds a member of the name and no free function of the repository. They list
`other` alone as the receiver of a value of the caller's own type, since a call on `self` binds through the type it
records, so `input.parse()` inside a `parse` binds no member of the caller's own type. They say no call names a module,
so `tree.walk()` beside a `mod walk;` binds no call to the module, and `text()` in a file that declares `mod text;`
and `fn text` binds the function. They let a member call bind a free function only through a lowercase receiver, a
module's, as `walk::run()` names one, so `index.refresh_paths()`, `self.refresh_paths()` and
`Index::refresh_paths(self)` bind no free `fn refresh_paths`, and `Poll::Ready(x)` binds no struct `Ready`.

A `use` of a path that may start at another crate, `use kartta_lang::tree::{self, text}`, keeps each name it binds
with the path the name reaches, and a path call through such a crate, `kartta_walk::walk::run()`, keeps its receiver
with its path, read through the `use` of its first segment where the file has one. A name the file declares at its top
level, or binds to two paths anywhere, keeps none; a glob, `as _`, `crate::`, `self::`, `super::`, a capitalised first
segment and the standard library's crates keep nothing.

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

A package's library is named by its manifest's `[lib] name`, else by its `[package] name` with `-` read as `_`, and lies
at its `[lib] path`, else at `src/lib.rs`; each value is read only from a `key = "value"` on one line under its table's
header, the form Cargo writes, or a `key = 'value'`, and a value in any other form is not. A manifest without a
`[package] name` declares no library. A kept path starts at the library its first segment names, the caller's own
package's or else the nearest package's, and none where two are as near, and each segment after it names a module file
by the same layout. The path imports the file of the module it ends at, or of the module that holds the lowercase item
it ends at, so `text()` after `use kartta_lang::tree::text;` binds the `text` that `crates/kartta-lang/src/tree.rs`
exports at its top level.

Code gated by `#[test]`, `#[bench]` or a `cfg` that holds only under `test` is test code, and so is a file below a
`tests` directory outside `src`. A `*.gen.rs` or `*_generated.rs` file, or one with a `@generated` or `DO NOT EDIT`
marker in its leading comments, is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.

Status: the API is new and is not published yet.
