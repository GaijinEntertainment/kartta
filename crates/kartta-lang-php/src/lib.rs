//! PHP for the kartta code graph: the classes, interfaces, traits, enums, functions and methods of PHP source, the
//! calls between them, and the files a file includes or names in a `use`.
//!
//! [`Php`] implements [`Language`] over the `tree-sitter-php` grammar. It claims every file whose extension is `php`
//! or `phtml`, in any case.
//!
//! # What a file extracts to
//!
//! A function, a method, a class, an interface, a trait, an enum, a property, a constant, a property that a
//! constructor parameter promotes, an enum case and a closure assigned to a variable are symbols. A declaration is
//! qualified by the namespace in effect, `App\Service`, and a member by its type as well, `App\Service::greet`; its
//! owner is the type's bare name, `Service`. A member of an anonymous class has no owner. A class takes the class it
//! extends, the interfaces it implements and the traits it `use`s as parents, and an interface the interfaces it
//! extends. A member is exported unless it is `private`.
//!
//! A call records its receiver: the variable of a member call, `$this` or `$svc`, the scope of a static call, `Logger`,
//! `self` or `parent`, and the empty receiver where that is an expression, `$this->pool->get()`. A qualified scope,
//! `\App\Logger::log()`, is the receiver `Logger`, the bare name that owns the member. A qualified function call,
//! `\App\run()`, and a construction, `new \App\User()`, call the last segment of the name. `new self`,
//! `new static` and `new parent` call nothing. A closure assigned to a variable, `$f = function () {}`, makes the calls
//! in its body; a closure passed as an argument leaves them to the callable around it. A call before the file's first
//! symbol records nothing.
//!
//! # Resolution
//!
//! An `include` or `require` whose path folds to a constant, from strings, `__DIR__`, `__FILE__`, the `.` operator and
//! `dirname`, includes the file it names: from the file's directory for `__DIR__` and a `./` or `../` path, and from
//! the root, or else from the one file whose path ends in it, for any other. A path built from a variable includes
//! nothing. A `use` includes the file that the PSR-4 autoload of the repository's `composer.json` files maps its name
//! to, or else, for a `use function` or a `use const`, the file it maps the name's namespace to; a `use` that maps to
//! no file of the repository is an import as written.
//!
//! A file whose name ends in `Test.php`, or under a `test` or `tests` directory, is test code. A file named
//! `*.generated.php` or `*_pb.php`, or whose leading comments carry `@generated` or `DO NOT EDIT`, is generated code.
//! No symbol is an entry point.
//!
//! # Fingerprint
//!
//! [`Php::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its version,
//! ABI version, node types, and the counts of its node kinds, parse states and fields. A change to `kartta-lang` is
//! covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built from changed
//! sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_php::Php;
//!
//! let php = Php::new();
//! assert!(php.claims("src/Service.php"));
//!
//! let raw = php.extract(b"<?php\nnamespace App;\nclass Service {\n    function run() { helper(); }\n}\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["App\\Service", "App\\Service::run"]);
//! assert_eq!(raw.calls[0].callee, "helper");
//! ```

use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::path::base;
use kartta_lang::{
    BlockComment, Conventions, DocComments, Language, OwnMembers, RawExtraction, Repository,
    Resolver,
};
use tree_sitter::{Parser, Tree};

use crate::fnv::{FNV_OFFSET, fnv};
use crate::resolve::Autoload;

mod banner;
mod fnv;
mod fold;
mod psr4;
mod resolve;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_php::PHP_NODE_TYPES.as_bytes());

/// The PHP language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Php;

impl Php {
    /// The PHP language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const DOC_COMMENTS: DocComments =
    DocComments::lines(&["//", "#"]).with_blocks(&[BlockComment::new("/*", "*/")]);

impl Language for Php {
    fn name(&self) -> &'static str {
        "php"
    }

    fn claims(&self, path: &str) -> bool {
        base(path).rsplit_once('.').is_some_and(|(_, extension)| {
            extension.eq_ignore_ascii_case("php") || extension.eq_ignore_ascii_case("phtml")
        })
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    // constraint: a bare call never reaches a method of its own class, which PHP calls only through `$this`, `self`
    // or `static`
    fn conventions(&self) -> Conventions {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::Unreached;

        conventions
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        parse(source).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(Autoload::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_php::LANGUAGE_PHP.into()
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
