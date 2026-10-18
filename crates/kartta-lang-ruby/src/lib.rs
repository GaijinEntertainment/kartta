//! Ruby for the kartta code graph: the classes, modules, methods and constants of Ruby source, the calls between them,
//! and the files a file requires.
//!
//! [`Ruby`] implements [`Language`] over the `tree-sitter-ruby` grammar. It claims every file whose extension is `rb`,
//! in any case.
//!
//! # What a file extracts to
//!
//! A class, a module, a method, a constant, an accessor of `attr_reader`, `attr_writer` or `attr_accessor`, and an
//! alias of `alias` or `alias_method` are symbols. A member is qualified by the whole namespace chain of its type: an
//! instance method by `#`, `Geo::Shape#area`, a class method by `.`, `Geo::Shape.build`, and a constant or a nested
//! type by `::`. A `def` inside `class << self` is a class method. A constant bound to `Struct.new` or `Data.define` is
//! a class.
//!
//! A class takes its superclass as a parent, and so the modules it `include`s, `extend`s or `prepend`s. A method is
//! exported unless a `private` modifier covers it: a bare `private` for the methods after it in the body of a class, a
//! module or a `class << self`, `private :x` for a method already declared, `private def x` for one method, and
//! `private_class_method :x` for a class method; `protected` and `public` keep a method exported.
//! An alias of a method that the file declares before it in the same type is one callable with the body the original
//! names there.
//!
//! A call inside a method is a call made by it, and records its receiver: a plain name as written, `self` as the
//! caller's own type, and any other expression as the empty receiver. A call before the file's first symbol records
//! nothing. A call with no receiver and no arguments, `setup`, parses as a name, so it records nothing either.
//!
//! # Resolution
//!
//! A `require` or `require_relative` with no receiver and a string argument of plain text names a file, with or without
//! its `.rb`; a string with an interpolation or an escape names nothing.
//! `require_relative` resolves from the requiring file's directory, and `require` from the first of `lib`, the root,
//! `test`, `spec` and `app` that holds it. A required file of the repository is an include, and any other require, of
//! a gem or of the standard library, an import as written.
//!
//! A file named `*_test.rb` or `*_spec.rb`, or under a `test`, `tests` or `spec` directory, is test code. A file named
//! `*_pb.rb`, or whose leading `#` comments, after any byte order mark, carry `@generated`, `DO NOT EDIT` or
//! `This file is auto-generated`, is generated code. No symbol is an entry point.
//!
//! # Fingerprint
//!
//! [`Ruby::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_ruby::Ruby;
//!
//! let ruby = Ruby::new();
//! assert!(ruby.claims("lib/geometry/shape.rb"));
//!
//! let raw = ruby.extract(b"module Geo\n  class Shape\n    def area\n      compute(1)\n    end\n  end\nend\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["Geo", "Geo::Shape", "Geo::Shape#area"]);
//! assert_eq!(raw.calls[0].callee, "compute");
//! ```

use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::path::base;
use kartta_lang::{
    BlockComment, Conventions, DocComments, InheritedMembers, Language, OwnMembers, RawExtraction,
    Repository, Resolver,
};
use tree_sitter::{Parser, Tree};

use crate::fnv::{FNV_OFFSET, fnv};
use crate::resolve::Requires;

mod bindings;
mod fnv;
mod macros;
mod owner;
mod resolve;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_ruby::NODE_TYPES.as_bytes());

/// The Ruby language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Ruby;

impl Ruby {
    /// The Ruby language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const DOC_COMMENTS: DocComments =
    DocComments::lines(&["#"]).with_blocks(&[BlockComment::new("=begin", "=end").closed_alone()]);

impl Language for Ruby {
    fn name(&self) -> &'static str {
        "ruby"
    }

    fn claims(&self, path: &str) -> bool {
        base(path)
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("rb"))
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    // constraint: a bare call inside a method has the implicit receiver `self`, so it reaches the methods of its own
    // type first; the parents of a type also name the modules it `extend`s, which its instance methods do not reach,
    // so an inherited method is one candidate among those of its name
    fn conventions(&self) -> Conventions {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::First;
        conventions.inherited_members = InheritedMembers::Admitted;
        conventions.owner_chain_separator = Some("::");

        conventions
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        parse(source).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(Requires::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_ruby::LANGUAGE.into()
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
