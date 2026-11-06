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
//! beside a module's `manifests/`, each in the lower case of the module layout, is test code, and one whose leading `#`
//! comments carry `@generated` or `DO NOT EDIT` is generated code. No symbol is an entry point.
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

use kartta_lang::path::has_extension;
use kartta_lang::{
    BlockComment, Conventions, DocComments, Language, RawExtraction, Repository, Resolver, TypeUses,
};

use crate::resolve::PuppetResolver;

mod class;
mod resolve;
mod walk;

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

const DOC_COMMENTS: DocComments =
    DocComments::lines(&["#"]).with_blocks(&[BlockComment::new("/*", "*/")]);

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.type_uses = TypeUses::Nowhere;
    conventions.includers = Some(
        "the manifests that reference a class or defined type the module autoloader loads from this file: an include, \
         require or contain of it, a resource of its type, or a class that inherits it",
    );
    conventions
};

impl Language for Puppet {
    fn name(&self) -> &'static str {
        "puppet"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["pp"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_puppet::NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(PuppetResolver)
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_puppet::LANGUAGE.into()
}
