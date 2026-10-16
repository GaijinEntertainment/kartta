//! Python for the kartta code graph: the functions, classes, fields, calls and imports of Python source.
//!
//! [`Python`] implements [`Language`] over the `tree-sitter-python` grammar. It claims every file whose extension is
//! `py` or `pyi`, in any case.
//!
//! # What a file extracts to
//!
//! Every function, method and class is a symbol, and so is a variable assigned to a bare name at module level and an
//! annotated attribute in a class body, `name: str = ""`, which is a field of its class. A symbol is qualified by the
//! module its path names and by the classes around it: `pkg.service.Service.greet` for a method of `Service` in
//! `pkg/service.py`, and `pkg.Service` for a class in `pkg/__init__.py`. A definition nested in a function is qualified
//! by the module alone, is no member of a class around that function, and is local to the function's body. A decorated
//! definition starts at its first decorator. A name that starts with `_` is not exported.
//!
//! A call records its callee and, for a member call, its receiver: `self.greet()` records `greet` on `self`, and a
//! member call on anything but a name, `items[0].close()`, records the empty receiver. A call is attributed to the
//! function around it; a class opens no scope. A call before the first definition of the file is dropped.
//!
//! # Resolution
//!
//! An `import` or `from ... import` that names a module of the repository includes its file, `a/b.py` or
//! `a/b/__init__.py`, or failing both its stub, `a/b.pyi` or `a/b/__init__.pyi`: an absolute module is probed from the
//! repository root and then from each directory around the importing file, nearest first, and a relative one from the
//! importing file's directory, one level up for each leading dot after the first. A from-import also includes each
//! imported name that is a submodule of the package, a namespace package without an `__init__.py` included, and probes
//! none below a plain module file. An import that includes no file, such as one of the standard library or a
//! `from __future__ import`, is recorded as written.
//!
//! A file named `test_*`, `*_test` or `conftest`, or below a `test` or `tests` directory, is test code. A `*_pb2` or
//! `*_pb2_grpc` file, or one with a `@generated` or `DO NOT EDIT` marker in the comments and module docstring that open
//! it, outside a backtick span, is generated code. A `.pyi` stub takes the module name and the role of the `.py` source
//! it describes. A free `def main` at module level is the entry point.
//!
//! # Fingerprint
//!
//! [`Python::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_python::Python;
//!
//! let python = Python::new();
//! assert!(python.claims("pkg/service.py"));
//!
//! let raw = python.extract(b"class Service:\n    def greet(self):\n        return helper()\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["Service", "Service.greet"]);
//! assert_eq!(raw.calls[0].callee, "helper");
//! ```

use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::path::base;
use kartta_lang::{Conventions, Language, OwnMembers, RawExtraction, Repository, Resolver};
use tree_sitter::{Parser, Tree};

use crate::fnv::{FNV_OFFSET, fnv};
use crate::resolve::Modules;

mod banner;
mod fnv;
mod resolve;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_python::NODE_TYPES.as_bytes());

/// The Python language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Python;

impl Python {
    /// The Python language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Language for Python {
    fn name(&self) -> &'static str {
        "python"
    }

    fn claims(&self, path: &str) -> bool {
        base(path).rsplit_once('.').is_some_and(|(_, extension)| {
            extension.eq_ignore_ascii_case("py") || extension.eq_ignore_ascii_case("pyi")
        })
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn conventions(&self) -> Conventions {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::Unreached;
        conventions.owner_chain_separator = Some(".");

        conventions
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        parse(source).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(Modules::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

fn parse(source: &[u8]) -> Option<Tree> {
    thread_local! {
        static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
    }

    PARSER.with_borrow_mut(|parser| {
        if parser.is_none() {
            let mut fresh = Parser::new();
            fresh.set_language(&grammar()).ok()?;
            *parser = Some(fresh);
        }
        parser.as_mut()?.parse(source, None)
    })
}

fn fingerprint() -> Vec<u8> {
    let grammar = grammar();
    let version = grammar.metadata().map_or([0; 3], |version| {
        [
            version.major_version,
            version.minor_version,
            version.patch_version,
        ]
    });
    let counts = [
        grammar.abi_version(),
        grammar.node_kind_count(),
        grammar.parse_state_count(),
        grammar.field_count(),
    ];

    let mut bytes = Vec::new();
    for part in [env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")] {
        bytes.extend_from_slice(part.as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(&SOURCE_DIGEST);
    bytes.extend_from_slice(&NODE_TYPES_DIGEST.to_le_bytes());
    bytes.extend_from_slice(&version);
    for count in counts {
        bytes.extend_from_slice(&u64::try_from(count).unwrap_or(u64::MAX).to_le_bytes());
    }

    bytes
}
