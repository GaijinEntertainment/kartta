//! Rust for the kartta code graph: the items, members and calls of Rust source, and the files its `mod` declarations
//! pull in.
//!
//! [`Rust`] implements [`Language`] over the `tree-sitter-rust` grammar. It claims every file whose extension is `rs`,
//! in any case.
//!
//! # What a file extracts to
//!
//! Every function, struct, union, enum, trait, type alias, `macro_rules!`, constant, static and inline module is a
//! symbol, and so is every named field of a struct and every variant of an enum. A member of a `trait`, or of an `impl`
//! whose target names a type, is owned by that type and qualified under it, `Service::new`; a member of an `impl` for a
//! tuple, `dyn` or function type is owned by the type as written inside a function body, `(u8, u8)::show`, and has no
//! owner outside one. A function of an `extern` block that `#[wasm_bindgen(method)]` marks is
//! owned by the type its first parameter, `this`, refers to, one `constructor` marks by the type it returns, `T` of a
//! `Result<T, E>`, and one `static_method_of = T` marks by `T`, written as a name or a string. An item of an inline
//! module is qualified by the chain of its modules, `a::b::Worker`, so two modules of one file keep their same-named
//! items apart. A `macro_rules!` keeps its bare name, because a macro is in scope by text order rather than by path,
//! and only `#[macro_export]` exports one, at its crate's root path. A file's own module is named by its path, which no
//! qualified name carries. `impl Trait for Type` adds `Trait` to the parents of the `Type` the file declares under that
//! name or path, and of a `Type` it does not declare the file asserts that `Type` implements `Trait`, where its `use`s,
//! a glob or a path can lead each name into the tree. An impl's owner settles after the walk, and its members, the
//! items nested in them, its `self` receivers and its trait pair follow it. `pub` exports an item, but `pub(self)` and
//! `pub(in self)` do not; a trait's members take the trait's visibility and a variant its enum's.
//!
//! An invocation of a `macro_rules!` the file defines before it, in its block or module or one around it, by its bare
//! name, defines the items the first arm its tokens match names by a metavariable, `pub struct $name;` or
//! `fn $name()`, once for each identifier the invocation binds to it, qualified and owned where the invocation is.
//! `impl Trait for $name { .. }` adds `Trait` to the parents of the type it binds, and the `fn`s, `const`s and `type`s
//! of an impl for `$name`, with a trait or without, are that type's members. An item the arm spells by a literal name,
//! a member of a struct, an enum or a trait it makes, an arm that forwards to another invocation, an invocation no arm
//! matches, and a macro of another file or crate define nothing.
//!
//! A call records its callee and, where the syntax spells one, its receiver: `self.greet()` and `Self::new()` record
//! the type of the `impl` or `trait` around them, `Widget::new()` records `Widget`, and `a::Worker::run()` records
//! `a::Worker` when the file declares that module. A member call on anything but a name or a chain that names a type
//! records the empty receiver, and one on a lowercase name other than `other` records the name after a `.`, `.socket`
//! for `socket.connect()`, since alone it would read as the module that a path call, `socket::connect()`, names;
//! `Fish.write()` records `Fish`. A method call on a chain records what the chain holds: the type `T::f()` constructs,
//! what a declared `Option<T>` or `Result<T, E>` holds once it is unwrapped, or the path of up to eight calls to what a
//! method returns, `Sink::first()?`, which the graph follows through each method's return type. A
//! method call on a parameter its function declares `T`, `&T`, `&mut T`, `T<'a>`, `Box<T>`, `Rc<T>`, `Arc<T>`, `Self`,
//! `impl Trait` or `dyn Trait` records the type instead, `Ranking` for `ranking.offer()` in
//! `fn offer(ranking: &mut Ranking)`, where a `use` outside every function body brings it from outside the toolchain's
//! crates, a path names it through another crate or the file's own, the file declares it and no `use` brings it, or a
//! top-level glob from outside the toolchain's crates may bring it and the prelude does not name it, since a crate of
//! the tree that declares a namesake of a standard library type, `File` or `Vec`, holds another type. A type
//! parameter and a name a `let`, a closure or a pattern binds again keep the value's name. A macro invocation is a
//! call of the macro. A call of an item that a function body declares is a local call of that item. A bare call of a
//! name that a pattern binds, a parameter, a `let`, a closure's parameter, a match arm, a `for` or an `if let`, as
//! `helper()` in `fn f(helper: fn())` or `run()` after `let run = || {};`, calls the value the name holds and is no
//! call, whatever the file imports or declares under the name.
//!
//! A call written in a macro's arguments, `assert!(C.opens(x))`, is a call of the function around the macro, as one
//! outside it is, for the macros of any crate. The grammar reads the arguments as tokens, so the walk takes a name, a
//! path or a member before a `(`, `collect::<Vec<_>>()` among them, as a call, and a name before a `!` as a nested
//! macro invocation, whose arguments it reads the same way. A closure in the arguments binds its parameters to the end
//! of the token tree it is written in. Arguments that interpolate, `#name` or `#(..)*`, as `quote!` writes them, are a
//! template of generated code and hold no call of the site, and neither do an attribute nor the parameters and body
//! of a `fn` the arguments declare. A pattern there names constructors and holds no call either: the standard
//! `matches!`'s pattern, between its value and its guard, and a `(` tree before `=`, a guard's `if` or an unguarded
//! arm's `=>`, `if let Some(x) =`.
//!
//! A `use` of a path that may start at another crate or at the file's own crate or module,
//! `use kartta_lang::tree::{self, text as at}` or `use super::helper`, keeps each name it binds with the path the name
//! reaches, and so does a path call through such a path for its receiver, `kartta_walk::walk::run()` for `walk` and
//! `crate::m::f()` for `m`, or for its name where the path ends at `crate`, `self` or `super`, `super::f()` for `f`,
//! and a macro invocation through one for its macro, `kartta_lang::mac!()` for `mac`, each read through the `use` of
//! its first segment where the file has one. A name the file declares at its top level, or binds to two paths anywhere,
//! keeps none, though a `use` keeps a name the file declares only as a module, which Rust keeps apart from what the
//! `use` binds, as `use parse::parse;` beside `mod parse;`, and a path call through `crate` or `super` that ends at a
//! name the file declares, `super::new()` beside `fn new`, records the path's last keyword as its receiver and keeps
//! the keyword with the path, `super` for `super`; `as _`, a capitalised first segment, a path through
//! `self::` or `super::` inside an inline module and the standard library's crates keep nothing. A `use` or a glob
//! whose first segment is a module the file declares where it is written starts at the file's own module, so
//! `use m::helper;` beside `mod m;` keeps `self::m::helper`, and inside an inline module, as a path through `self::`
//! there, keeps nothing. A path call outside every inline module whose first segment is a module that a bodiless `mod`
//! of the file's top level declares starts there too, `alpha::run()` beside `mod alpha;` keeping `self::alpha`, and a
//! path call keeps such a module's name; a module a `#[path]` places, or an inline one, keeps nothing. A glob outside
//! every function body and inline module,
//! `use super::*;`, keeps the path of the module it ends at once, under those rules, and one inside a block, or of a
//! capitalised item, keeps nothing. A path that ends at a capitalised item, a type, is kept only where the file calls
//! its name bare, as `Wrapper(1)` calls a tuple struct's constructor, since a call through a type names its member. A
//! `use` inside a function body or an inline module is in scope in its block alone, not in a module nested in it:
//! outside the blocks of the `use`s that alone bind a name, a path call reads the name as the crate it spells, and a
//! bare call of the name leaves it no path. The paths are among the file's directives in source order. `extern crate`
//! adds no import.
//!
//! A name read where no call takes it is a use of it: an argument, `items.map(helper)`, an element, an operand, a
//! range's bound, a condition, an indexed value or an index, a returned, a broken-out, a block's or a closure's last
//! value, the value a `let`, a `let` condition, an assignment, a `const`, a `static`, a field, a reference, a cast, a
//! `match`, a match arm or a `for` takes, either side of `+=` and its kin, the base of a field access or a method
//! call, `SETTINGS` of `SETTINGS.enabled`, and an awaited or a `?` operand. The field of a field access, `enabled`
//! there, is recorded nowhere. `Self::make` read so is a use of the member `make` of the type around it, and
//! `Service::make` one of the member of `Service`. A name a pattern binds hides it from the end of what the binding
//! reads on: a parameter in its function or closure, a match arm's pattern in the arm, a `for`'s in its body, an
//! `if let`'s in the operands after it and its consequence, and a `let`'s to the end of its block. A bare read binds
//! only an item its own file declares outside every owner and body, so the walk keeps one only where its own inline
//! module declares the item, and a read through `Self` or a type only where the file declares a member of the name
//! under that type.
//!
//! # Resolution
//!
//! A bodiless `mod NAME;` includes the file it pulls in: its `#[path]` override, or `NAME.rs` or `NAME/mod.rs` beside
//! a crate root or a file named `lib.rs`, `main.rs` or `mod.rs`, and under the directory named for the file's stem
//! otherwise, one directory deeper for each inline module around the declaration. A package is a directory that holds
//! a `Cargo.toml`, and its crate roots are its library's root, read below, and the files of Cargo's conventional
//! layout: `src/lib.rs`, `src/main.rs`, `build.rs`, and each target under `src/bin/`, `examples/`, `tests/` and
//! `benches/`. A top-level `fn main` in a crate root of that layout other than `src/lib.rs` is the entry point. A
//! file's package is the nearest package around it, named by the path of its manifest, so of the types of one name that
//! a call's receiver names, the one the caller's own package declares binds before another's, as two versions of one
//! crate in a cargo registry are told apart. A package is built with every package the tree holds of each name its
//! manifest's dependency tables list, of every kind and target, by the `package` key that renames one, its own or that
//! of the entry it inherits from its nearest workspace's `[workspace.dependencies]`, else by its key, and with what
//! each of those is built with but its optional, dev- and build-dependencies, so a call that no import or path names
//! binds a definition of another package only where its package is built with that one.
//!
//! A package's library is named by its manifest's `[lib] name`, else by its `[package] name` with `-` read as `_`, and
//! lies at its `[lib] path`, else at `src/lib.rs`, each value read only from a `key = "value"` on one line under its
//! table's header, the form Cargo writes, or a `key = 'value'`; a manifest without a `[package] name` declares no
//! library. A path a file keeps starts at the library its first segment names: the caller's own package's, else the
//! package that shares the most leading directories with the caller's, and none where two share as many. A path through
//! `crate::` starts at the caller's crate root: the file itself where it is one, else the `main.rs` of the target
//! directory it lies in, else the nearer of the library whose directory holds it and, below `src/`, the main binary,
//! the library where both sit in one directory; a file of `tests/`, `examples/` or `benches/` outside a target
//! directory has none. A path through `self::` starts at the caller's own module, and each `super::` at the module
//! above, the crate root where the module's directory lies in the root's, else the `NAME.rs` or `NAME/mod.rs` the
//! module layout names for the directory above, and none past the crate root. Each segment after it names the module
//! file the module layout gives it, and the path imports the file of the module it ends at, or of the module that holds
//! the item it ends at; a longer rest imports nothing. A call
//! of the name, or through it, binds what that file exports at its top level under the name, a function before a type
//! of the name, since a call never names a type that Rust keeps apart from values, and from the file's module or a
//! module below it, a private item other than a macro too. A test item binds only a call from test code of its own
//! crate, whose test build alone compiles it, and an exported one from any module of the crate; a macro the file does
//! not export binds no call through it. A glob imports the file of the module it ends at, and none for a path that
//! ends past a module, so a bare call binds what that file shows it the same way, where the caller's own file declares
//! nothing of the name.
//!
//! The resolver keeps only what it derived: the packages, the name and root of each library, and the packages each is
//! built with. A resolver built again over an edit that changes none of them, such as a dependency bump in a
//! `Cargo.toml`,
//! [resolves as](kartta_lang::Resolver::resolves_as) the one built before, so an engine that follows the tree resolves
//! no Rust file again for it.
//!
//! Code gated by an attribute whose path ends in `test` or `bench`, as `#[test]` and `#[tokio::test]` do, or by a `cfg`
//! that holds only under `test` is test code, and so is every file below
//! a `tests` directory outside `src`, counted from the repository root or from a package, in the lower case of Cargo's
//! own target directory. A `cfg` predicate is evaluated over every assignment of the atoms it repeats, one truth per
//! distinct atom, leaving an atom written once unknown: `cfg(all(test, not(miri)))` gates, while
//! `cfg(any(test, feature = "x"))`, `cfg(not(test))` and the never-true `cfg(any())` do not; past 12 repeated atoms
//! the rest stay unknown. A file that a test-gated `mod` or a test file pulls in is a test include. A `*.gen.rs` or
//! `*_generated.rs` file, or one with a `@generated` or `DO NOT EDIT` marker in its leading comments, outside a
//! backtick span, is generated code.
//!
//! # Fingerprint
//!
//! [`Rust::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_rust::Rust;
//!
//! let rust = Rust::new();
//! assert!(rust.claims("src/lib.rs"));
//!
//! let raw = rust.extract(b"struct Service;\nimpl Service {\n    fn touch(&self) { self.greet(); }\n}\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["Service", "Service::touch"]);
//! assert_eq!(raw.calls[0].receiver.as_deref(), Some("Service"));
//! ```

use kartta_lang::path::has_extension;
use kartta_lang::{
    Conventions, DocComments, Language, NamespaceCalls, OwnMembers, RawExtraction, Repository,
    Resolver, SymbolKind, TypeUses,
};

use crate::resolve::Crates;

mod manifest;
mod resolve;
mod runtime;
mod walk;

/// The Rust language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Rust;

impl Rust {
    /// The Rust language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::Unreached;
    conventions.namespace_calls = NamespaceCalls::Unbound;
    conventions.owner_chain_separator = Some("::");
    conventions.runtime_methods = runtime::METHODS;
    conventions.library_methods = runtime::LIBRARY_METHODS;
    conventions.own_type_receivers = Some(&["other"]);
    conventions.type_uses = TypeUses::Nowhere;
    conventions.member_calls = kartta_lang::MemberCalls::ModulePaths;
    conventions.receivers = kartta_lang::Receivers::ReturnPaths;
    conventions.unrecorded_data = &[SymbolKind::Field];
    conventions.calls_apart = kartta_lang::CallsApart::Marked;
    conventions.implements = kartta_lang::Implements::Traits;
    conventions.twins = kartta_lang::Twins::Every;
    conventions.included_runtime_members =
        kartta_lang::IncludedRuntimeMembers::SpelledOwnOrExported;
    conventions
};

impl Language for Rust {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["rs"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_rust::NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::C_LIKE.with_carried(&["#["])
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(Crates::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}
