# kartta-lang-rust

Rust for the kartta code graph: the items, members and calls of Rust source, and the files its `mod` declarations
pull in.

`Rust` implements the `kartta_lang::Language` contract over the `tree-sitter-rust` grammar. It claims every file whose
extension is `rs`, in any case.

## What a file extracts to

Every function, struct, union, enum, trait, type alias, `macro_rules!`, constant, static and inline module is a
symbol, and so is every named field and every enum variant. A member of a `trait`, or of an `impl` whose target names a
type, is owned by that type and qualified under it, `Service::new`; a member of an `impl` for a tuple, `dyn` or
function type is owned by the type as written inside a function body, `(u8, u8)::show`, and has no owner outside one.
A function of an `extern` block that `#[wasm_bindgen(method)]` marks is owned by the type
its first parameter, `this`, refers to, since wasm-bindgen makes it that type's method; one `constructor` marks is
owned by the type it returns, `T` of a `Result<T, E>`, and one `static_method_of = T` marks by `T`, since wasm-bindgen
makes them that type's members. An item of an inline module is qualified by the chain of its modules, `a::b::Worker`,
except a `macro_rules!`, which keeps its bare name and is exported only by `#[macro_export]`, at its crate's root path.
A function marked `#[proc_macro]` is a macro, since it is invoked as one.
`impl Trait for Type` adds `Trait` to the parents of the `Type` the file declares under that name or path, and of a
`Type` it does not declare the file asserts that `Type` implements `Trait`, where its `use`s, a glob or a path can lead
each name into the tree.

An invocation of a `macro_rules!` the file defines before it, in its block or module or one around it, by its bare
name, defines the items the first arm its tokens match names by a metavariable, `pub struct $name;` or `fn $name()`,
once for each identifier the invocation binds to it, qualified and owned where the invocation is.
`impl Trait for $name { .. }` adds `Trait` to the parents of the type it binds, and the `fn`s, `const`s and `type`s of
an impl for `$name`, with a trait or without, are that type's members. An item the arm spells by a literal name, a
member of a struct, an enum or a trait it makes, an arm that forwards to another invocation, an invocation no arm
matches, and a macro of another file or crate define nothing.

A call records its callee and, where the syntax spells one, its receiver: `self.greet()` and `Self::new()` record the
type of the `impl` around them, and `Widget::new()` records `Widget`. A method call on a lowercase value other than
`other` records the name after a `.`, `.socket` for `socket.connect()`, since alone it would read as the module that
a path call, `socket::connect()`, names; `Fish.write()` records `Fish`. A method call on a parameter its function
declares `T`, `&T`, `&mut T`, `T<'a>`, `Box<T>`, `Rc<T>`, `Arc<T>`, `Self`, `impl Trait` or `dyn Trait` records the
type instead, `Ranking` for `ranking.offer()` in `fn offer(ranking: &mut Ranking)`, where a `use` outside every function
body brings it from outside the toolchain's crates, a path names it through another crate or the file's own, the file
declares it and no `use` brings it, or a top-level glob from outside the toolchain's crates may bring it and the prelude
does not name it; a type parameter, a type of the standard library, as `File` or `Vec`, and a name a `let`, a closure
or a pattern binds again keep the value's name. A method call on a chain records what the chain holds: the type `T::f()`
constructs, what a declared `Option<T>` or `Result<T, E>` holds once it is unwrapped, or the path of up to eight calls
to what a method returns, `Sink::first()?`, which the graph follows through each method's return type. A macro
invocation is a call marked as one: it binds only a macro, and
any other call binds no macro, so `ready!(x)` beside `fn ready` and `derive(x)` beside `macro_rules! derive` bind
nothing. A call of an item that a
function body declares is a local call of that item. A bare call of a name that a pattern binds, a parameter, a `let`,
a closure's parameter, a match arm, a `for` or an `if let`, as `helper()` in `fn f(helper: fn())` or `run()` after
`let run = || {};`, calls the value the name holds and is no call, whatever the file imports or declares under the
name. A call written in a macro's arguments, `assert!(C.opens(x))`,
is a call of the function around the macro, for the macros of any crate: the grammar reads the arguments as tokens, so
a name, a path or a member before a `(` is a call, a name before a `!` a nested macro invocation, and a closure there
binds its parameters to the end of its token tree. Arguments that interpolate, `#name` or `#(..)*`, as `quote!` writes
them, are a template of generated code and hold no call, and neither do an attribute nor the parameters and body of a
`fn` the arguments declare, nor a pattern: the standard `matches!`'s, between its value and its guard, and a `(`
tree before `=`, a guard's `if` or an unguarded arm's `=>`, `if let Some(x) =`. The conventions name the
methods and associated functions of the standard library that code calls most, so a call of one through a receiver or
a path, `name.clone()` or `fs::write()`, binds a member of the name and no free function of the repository. They list
`other` alone as the receiver of a value of the caller's own type, since a call on `self` binds through the type it
records, so `input.parse()` inside a `parse` binds no member of the caller's own type. They say no call names a module,
so `tree.walk()` beside a `mod walk;` binds no call to the module, and `text()` in a file that declares `mod text;`
and `fn text` binds the function. They let a member call bind a free function only through a lowercase receiver, a
module's, as `walk::run()` names one, so `index.refresh_paths()`, `self.refresh_paths()` and
`Index::refresh_paths(self)` bind no free `fn refresh_paths`, and `Poll::Ready(x)` binds no struct `Ready`.

A `use` of a path that may start at another crate or at the file's own crate or module, `use kartta_lang::tree::{self,
text}` or `use super::helper`, keeps each name it binds with the path the name reaches, and a path call through such a
path, `kartta_walk::walk::run()` or `crate::m::f()`, keeps its receiver with its path, or its name where the path ends
at `crate`, `self` or `super`, as `super::f()` does, as a macro invocation through one, `kartta_lang::mac!()`, keeps
its macro's name, each read through the `use` of its first segment where the file has one. A name the file declares at
its top level, or binds to two paths anywhere, keeps none, though a `use` keeps a name the file declares only as a
module, which Rust keeps apart from what the `use` binds, as `use parse::parse;` beside `mod parse;`; `as _`, a
capitalised first segment, a path through `self::` or `super::` inside an inline module and the standard library's
crates keep nothing. A `use` or a glob whose first segment is a module the file declares where it is written starts at
the file's own module, so `use m::helper;` beside `mod m;` keeps `self::m::helper`, and inside an inline module, as a
path through `self::` there, keeps nothing. A glob outside every
function body and inline module, `use super::*;`, keeps the path of the module it ends at once, under those rules, and
one inside a block, or of a capitalised item, keeps nothing. A path that ends at a capitalised item, a type, is kept
only where the file calls its name bare, as `Wrapper(1)` calls a tuple struct's constructor, since a call through a
type names its member. A `use` inside a function body or an inline module is in scope in its block alone, not in a
module nested in it: outside the blocks of the `use`s that alone bind a name, a path call reads the name as the crate
it spells, and a bare call of the name leaves it no path.

A name read where no call takes it, `items.map(helper)`, `Handler { on_event: helper }` or the base `SETTINGS` of
`SETTINGS.enabled`, is a use of it, and so are `Self::make` and `Service::make` of the member `make`. The field of a
field access, `enabled` there, is recorded nowhere. A name a pattern binds hides it from the end of what the binding
reads on: a parameter in its function or closure, a match arm's pattern in the arm, a `for`'s in its body, an `if let`'s
in the operands after it and its consequence, and a `let`'s to the end of its block. A bare read is kept only where its
inline module declares an item of its name, since it binds only an item the file declares outside every owner and body,
and a read through a type only where the file declares that member.

## Resolution

A bodiless `mod NAME;` includes the file it pulls in, by its `#[path]` override or by Cargo's module layout: beside a
crate root or a file named `lib.rs`, `main.rs` or `mod.rs`, under the directory named for any other file's stem, and one
directory deeper for each inline module around it. A directory that holds a `Cargo.toml` is a package, and its crate
roots are its library's root, read below, `src/lib.rs`, `src/main.rs`, `build.rs` and each target under `src/bin/`,
`examples/`, `tests/` and `benches/`. A top-level `fn main` in a crate root of that layout other than `src/lib.rs` is
the entry point; a file that only the manifest's `[lib] path` makes a crate root is none. A package is built with every
package the tree holds of each name its manifest's dependency tables list, and with what every build of those takes in
turn, so a call that no import or path names binds a definition of another package only where its package is built with
that one: `Config::default()` in a crate built with no crate that declares `Config` binds nothing.

A package's library is named by its manifest's `[lib] name`, else by its `[package] name` with `-` read as `_`, and lies
at its `[lib] path`, else at `src/lib.rs`; each value is read only from a `key = "value"` on one line under its table's
header, the form Cargo writes, or a `key = 'value'`, and a value in any other form is not. A manifest without a
`[package] name` declares no library. A kept path starts at the library its first segment names, the caller's own
package's or else the nearest package's, and none where two are as near. A path through `crate::` starts at the
caller's crate root, the file itself or the target's `main.rs`, else the nearer of the library and the main binary
whose directories hold it; one through `self::` at the caller's module, and each `super::` at the module above. Each
segment after the first names a module file by the same layout. The path imports the file of the module it ends at, or
of the module that holds the item it ends at, so `text()` after `use kartta_lang::tree::text;` binds the `text` that
`crates/kartta-lang/src/tree.rs` exports at its top level, and `Wrapper(1)` after `use crate::m::Wrapper;` the struct
`src/m.rs` declares. A glob imports the file of the module it ends at, and none for a path that ends past a module,
so a bare call in a separate `src/tests.rs` that `use super::*;` opens binds what `src/lib.rs` shows it, its private
items included, where the caller's own file declares nothing of the name. A function there wins over a type of its name,
which Rust keeps apart from values, so `kevent()` binds the `extern` function `kevent` beside a `struct kevent`.

Code gated by an attribute whose path ends in `test` or `bench`, as `#[test]` and `#[tokio::test]` do, or by a `cfg`
that holds only under `test` is test code, and so is a file below a `tests` directory outside `src`, in the lower case
of Cargo's own target directory. A `*.gen.rs` or `*_generated.rs` file, or one with a `@generated` or `DO NOT EDIT`
marker in its leading comments, is generated code.

## Fingerprint

The fingerprint covers the crate's version and source and the grammar's version, node types and table sizes, so a cache
keeps the extractions of each build of this crate apart.
