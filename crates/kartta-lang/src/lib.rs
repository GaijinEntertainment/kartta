//! The contract a language implements to plug into the kartta code graph.
//!
//! The graph knows no language. A language is a value that implements [`Language`]: it says which files are its,
//! extracts the definitions, calls and imports of one file from its bytes, and resolves that extraction against the
//! file's path and the repository. kartta ships languages as crates of their own, and a language kartta does not ship
//! is one more crate implementing the same trait.
//!
//! Extraction is split at the point a cache can share. [`Language::extract`] reads only the bytes, so its
//! [`RawExtraction`] is the same for every file with the same content, wherever it sits; a cache keyed by the content's
//! hash and the language's [`fingerprint`](Language::fingerprint) computes it once for all of them. The
//! [`Resolver`] a language builds over the [`Repository`] then applies the path: resolves imports to the files they
//! name, and marks the test, generated or vendored code the file's path shows, as the bytes alone cannot.
//!
//! # Examples
//!
//! A language that knows one kind of definition, a line starting with `def `:
//!
//! ```
//! use kartta_lang::{Extraction, Language, RawExtraction, Repository, Resolver, Symbol, SymbolKind};
//!
//! #[derive(Debug)]
//! struct Defs;
//!
//! impl Language for Defs {
//!     fn name(&self) -> &str {
//!         "defs"
//!     }
//!
//!     fn claims(&self, path: &str) -> bool {
//!         path.ends_with(".defs")
//!     }
//!
//!     fn fingerprint(&self) -> &[u8] {
//!         b"defs 1"
//!     }
//!
//!     fn extract(&self, source: &[u8]) -> RawExtraction {
//!         let mut raw = RawExtraction::default();
//!         for (at, line) in String::from_utf8_lossy(source).lines().enumerate() {
//!             if let Some(name) = line.strip_prefix("def ") {
//!                 let line = u32::try_from(at + 1).unwrap_or(u32::MAX);
//!                 raw.symbols.push(Symbol::new(name, SymbolKind::Function, line, line));
//!             }
//!         }
//!         raw
//!     }
//!
//!     fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
//!         Box::new(Paths)
//!     }
//! }
//!
//! struct Paths;
//!
//! impl Resolver for Paths {
//!     fn resolve(&self, raw: RawExtraction, path: &str, _repository: &dyn Repository) -> Extraction {
//!         raw.into_extraction(path).0
//!     }
//! }
//!
//! let raw = Defs.extract(b"def parse\nother\ndef render\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.name.as_str()).collect();
//! assert_eq!(names, ["parse", "render"]);
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod banner;
mod codec;
mod doc;
mod extraction;
pub mod fingerprint;
mod language;
mod memory;
pub mod path;
mod symbol;
#[cfg(feature = "tree-sitter")]
pub mod tree;

pub use crate::banner::Banner;
pub use crate::doc::{BlockComment, DocComments};
pub use crate::extraction::{
    Access, CallRef, Callable, CallableRole, Extraction, FoldedDeclaration, ImplementsAssertion,
    ImportBinding, ImportRef, IncludeRef, LocalCall, RawDirective, RawExtraction, RawReExport,
    ReExport, Receiver, UseRef,
};
pub use crate::language::{
    BareReceivers, BuiltApart, CallsApart, CallsOnValues, Conventions, DataUses, Implements,
    IncludeReach, IncludedRuntimeMembers, InheritedMembers, Language, MemberCalls, NamespaceCalls,
    OwnMembers, ParentSpelling, ReceiverMembers, Receivers, Repository, Resolver, SameMethod,
    Twins, TypeUses, UnlistedReceivers, UntypedMembers, UntypedOwners, ValueUses,
};
pub use crate::memory::MemoryRepository;
pub use crate::symbol::{FILE_SCOPE, Symbol, SymbolFlags, SymbolId, SymbolKind};

/// Identifies this crate's code: its name, its version and a digest of its manifest, of the versions the build's
/// `Cargo.lock` resolves for its dependencies, and of its source, as
/// [`set_source_id`](crate::fingerprint::set_source_id) computes it.
///
/// The `tree` helpers a language extracts with and the encoding of a [`RawExtraction`] both shape what a cache holds
/// for the same bytes, and neither shows in a language's [`fingerprint`](Language::fingerprint). A cache keys an
/// extraction by both, so a language's fingerprint covers its own code and grammar only.
pub const FINGERPRINT: &[u8] = env!("SOURCE_ID").as_bytes();
