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
//! tuple, `dyn` or function type has no owner. An item of an inline module is qualified by the chain of its modules,
//! `a::b::Worker`, so two modules of one file keep their same-named items apart. A `macro_rules!` keeps
//! its bare name, because a macro is in scope by text order rather than by path. A file's own module is named by its
//! path, which no qualified name carries. `impl Trait for Type` adds `Trait` to the parents of the `Type` the file
//! declares under that name or path.
//!
//! A call records its callee and, where the syntax spells one, its receiver: `self.greet()` and `Self::new()` record
//! the type of the `impl` or `trait` around them, `Widget::new()` records `Widget`, and `a::Worker::run()` records
//! `a::Worker` when the file declares that module. A member call on anything but a name records the empty receiver,
//! and a macro invocation is a call of the macro. A call of an item that a function body declares is a local call of
//! that item. A `use` declaration and `extern crate` add no import: a module path names no file.
//!
//! A name read where no call takes it is a use of it: an argument, `items.map(helper)`, an element, an operand, a
//! range's bound, a condition, an indexed value or an index, a returned, a broken-out, a block's or a closure's last
//! value, the value a `let`, a `let` condition, an assignment, a `const`, a `static`, a field, a reference, a cast, a
//! `match`, a match arm or a `for` takes, either side of `+=` and its kin, the base of a field access or a method
//! call, `SETTINGS.enabled`, and an awaited or a `?` operand. `Self::make` read so is a use of the member `make` of the
//! type around it, and `Service::make` one of the member of `Service`. A name a pattern binds hides it from the end of
//! what the binding reads on: a parameter in its function or closure, a match arm's pattern in the arm, a `for`'s in
//! its body, an `if let`'s in the operands after it and its consequence, and a `let`'s to the end of its block. A bare
//! read binds only an item its own file declares outside every owner and body, so the walk keeps one only where its own
//! inline module declares the item, and a read through `Self` or a type only where the file declares a member of the
//! name under that type.
//!
//! # Resolution
//!
//! A bodiless `mod NAME;` includes the file it pulls in: its `#[path]` override, or `NAME.rs` or `NAME/mod.rs` beside
//! a crate root or a file named `lib.rs`, `main.rs` or `mod.rs`, and under the directory named for the file's stem
//! otherwise, one directory deeper for each inline module around the declaration. A package is a directory that holds
//! a `Cargo.toml`, and its crate roots are the files of Cargo's conventional layout: `src/lib.rs`, `src/main.rs`,
//! `build.rs`, and each target under `src/bin/`, `examples/`, `tests/` and `benches/`. A top-level `fn main` in a crate
//! root other than the library is the entry point.
//!
//! Code gated by `#[test]`, `#[bench]` or a `cfg` that holds only under `test` is test code, and so is every file below
//! a `tests` directory outside `src`, counted from the repository root or from a package. A file that a test-gated
//! `mod` or a test file pulls in is a test include. A `*.gen.rs` or `*_generated.rs` file, or one with a `@generated`
//! or `DO NOT EDIT` marker in its leading comments, outside a backtick span, is generated code.
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
    Resolver,
};

use crate::resolve::Crates;

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
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::Unreached;
        conventions.namespace_calls = NamespaceCalls::Unbound;
        conventions.owner_chain_separator = Some("::");
        conventions.runtime_methods = runtime::METHODS;
        conventions.own_type_receivers = Some(&["other"]);

        conventions
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(Crates::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}
