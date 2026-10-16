//! Puppet for the kartta code graph: the classes, defined types, nodes and functions of Puppet manifests, and the
//! class references between them.
//!
//! [`Puppet`] implements [`Language`] over the `tree-sitter-puppet` grammar. It claims every file whose extension is
//! `pp`, in any case.
//!
//! # What a file extracts to
//!
//! A class, a defined type, a node definition, a function and a type alias are symbols, named as written —
//! `webserver::apache`, a node by its quoted name — and exported; a class takes the class it `inherits` as its parent.
//! A class, a defined type, a node and a function open a scope, so a statement inside one is a call made by it, and a
//! statement outside them all is a call made at file scope.
//!
//! A resource declaration is a call of its type, and a function call a call of its name; a call on a value,
//! `$config.lookup('key')` or `$ports.each |$p| {}`, records the method and the value it is called on. A class
//! reference — an `include`, `require` or `contain` statement with a bare or a quoted name, a `contain(...)` call, a
//! resource of a qualified type, an autoloaded defined type, and the class a class `inherits`, which Puppet declares
//! first — is a call of the class or defined type itself, so the graph binds it to that definition's symbol. A
//! reference that names no class, an uppercase type reference such as `Class['foo']` or a `$variable`, records
//! nothing. The grammar reads `include` and `require` as keywords and parses no call of them, `include('foo')`, nor a
//! resource-form class declaration, `class { 'foo': }`, so those forms record nothing either.
//!
//! # Resolution
//!
//! A class reference also includes the manifest the module autoloader loads for it: `foo` names
//! `foo/manifests/init.pp` and `foo::bar::baz` names `foo/manifests/bar/baz.pp`, at the root or under `modules/`,
//! `site/`, `site-modules/` or `dist/`, else the one file anywhere whose path ends that way. A built-in resource type,
//! such as `file` or `package`, names no manifest. A file under a `spec`, `tests`, `test` or `examples` directory
//! beside a module's `manifests/` is test code, and one whose leading `#` comments carry `@generated` or `DO NOT EDIT`
//! is generated code. No symbol is an entry point.
//!
//! # Fingerprint
//!
//! [`Puppet::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_puppet::Puppet;
//!
//! let puppet = Puppet::new();
//! assert!(puppet.claims("modules/apache/manifests/init.pp"));
//!
//! let raw = puppet.extract(b"class apache {\n  include stdlib\n}\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.name.as_str()).collect();
//! assert_eq!(names, ["apache"]);
//! assert_eq!(raw.calls[0].callee, "stdlib");
//! ```

use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::path::base;
use kartta_lang::{Language, RawExtraction, Repository, Resolver};
use tree_sitter::{Parser, Tree};

use crate::fnv::{FNV_OFFSET, fnv};
use crate::resolve::Manifests;

mod banner;
mod class;
mod fnv;
mod resolve;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_puppet::NODE_TYPES.as_bytes());

/// The Puppet language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Puppet;

impl Puppet {
    /// The Puppet language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Language for Puppet {
    fn name(&self) -> &'static str {
        "puppet"
    }

    fn claims(&self, path: &str) -> bool {
        base(path)
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("pp"))
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        parse(source).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(Manifests::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_puppet::LANGUAGE.into()
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
