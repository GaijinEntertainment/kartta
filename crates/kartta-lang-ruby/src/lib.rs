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
//! A call inside a method is a call made by it, and a call in the body of a class or a module outside every method is
//! a call made by that type. A call records its receiver: a plain name as written, `self` as the caller's own type,
//! which in a type's body is the type itself, and any other expression as the empty receiver; a type's body runs with
//! `self` as the type, so a call there with no receiver, `track :size`, records the type as its receiver. A receiver
//! that is a class, `self` in a class method or a type's body, `self.class` in an instance method, or a constant, ends
//! in `.`, the separator of a class method's name: `Geo::Shape.` for `self`, and `.` alone for `self.class` and for a
//! constant, whose path the receiver does not keep. Of a class's methods of one name, a call on the class binds the
//! class method, `Geo::Shape.build`, before the instance method, `Geo::Shape#build`, and a call on any other receiver
//! the instance method first. A bare `new` that reads no local makes, in a class method, an instance of the owner, so
//! `new.getopts` there records `Geo::Shape`. A call outside every method
//! and type is a call from file scope, before the file's first symbol too.
//!
//! A bare name, `setup`, is a call with no receiver unless it reads a local variable, as Ruby decides: a binding of the
//! name earlier in the source, in the same `def` or body of a class, a module or a `class << x`, or in a block or a
//! lambda around it, makes it a local. A parameter of any kind, an assignment, a `for` target, a `rescue => e`, a
//! pattern capture and a named group of a regexp literal matched with `=~` bind a name. Inside a block or a lambda,
//! `it` and `_1` to `_9` read its implicit parameters; `__FILE__`, `__LINE__` and `__ENCODING__` are never calls.
//!
//! A method named by a symbol is a use of it: `method(:render)`, `public_method`, `singleton_method`, `send(:render)`,
//! `public_send` and `__send__` name a method of the object's own type where they have no receiver or `self`, and one
//! of a class where the receiver is a constant or a constant path, `Cache.send(:flush)` or `Cache::Store.send(:flush)`,
//! its class method of the name first, as a call on the class binds it. A top-level `def` is a private method of
//! `Object`, which every object reaches, so at a site no class owns, file scope, a top-level `def`, `def self.x` or
//! `def obj.x`, the symbol names a top-level method as a bare use. With no receiver or `self`, `singleton_method` names
//! a method of the object itself, which only a class body names, through its class; with a constant receiver it names
//! one of that class anywhere. Any other receiver names a type the walk cannot tell, and `instance_method` and
//! `respond_to?` most often only inspect, so they name nothing.
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

use kartta_lang::path::has_extension;
use kartta_lang::{
    BlockComment, Conventions, DocComments, InheritedMembers, Language, OwnMembers, RawExtraction,
    Repository, Resolver, SymbolKind, TypeUses,
};

use crate::resolve::RubyResolver;

mod bindings;
mod locals;
mod macros;
mod owner;
mod resolve;
mod walk;

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

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::First;
    conventions.inherited_members = InheritedMembers::Admitted;
    conventions.owner_chain_separator = Some("::");
    conventions.class_member_separator = Some(walk::CLASS_MEMBER_SEPARATOR);
    conventions.own_type_receivers = Some(&["other"]);
    conventions.type_uses = TypeUses::Nowhere;
    conventions.unrecorded_data = &[SymbolKind::Constant];
    conventions
};

impl Language for Ruby {
    fn name(&self) -> &'static str {
        "ruby"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["rb"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_ruby::NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    // constraint: a bare call inside a method has the implicit receiver `self`, so it reaches the methods of its own
    // type first; the parents of a type also name the modules it `extend`s, which its instance methods do not reach,
    // so an inherited method is one candidate among those of its name
    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(RubyResolver)
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_ruby::LANGUAGE.into()
}
