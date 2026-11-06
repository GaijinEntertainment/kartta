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
//! owner is the type's bare name, `Service`. An anonymous class is an unexported class named by the names that lead
//! to it, joined by `·` in braces: the callable that creates it and the variable or property it is assigned to,
//! `{testRun·job}` for `$job = new class {}` in `Suite::testRun`, or `{class}` where no name leads to it. A name the
//! file has given already takes a count, `{testRun·2}`. Its members are owned by that name,
//! `App\{testRun·job}::handle`, and its creation is a call of it. A class takes the class it
//! extends, the interfaces it implements and the traits it `use`s as parents, and an interface the interfaces it
//! extends, each under the full name PHP compiles it to: through the file's `use` map by its first segment, in any
//! case, else under the namespace, `Base` in `namespace App;` the parent `App\Base`, and `C` after `use A\B as C;` the
//! parent `A\B`. A member is exported unless it is `private`.
//!
//! A call records its receiver: the variable of a member call, `$this` or `$svc`, the scope of a static call, `Logger`,
//! `self` or `parent`, and the empty receiver where that is an expression, `$this->pool()->get()`. A call on a variable
//! that still holds what a `new` assigned it records the class: the anonymous class's name, `$job->handle()` the
//! receiver `{testRun·job}`, the name PHP compiles a written class to, from the root, `\App\Job`, or `self`, `static` or
//! `parent`. A call on a property of `$this` records the class the property's declared type names, or else the one
//! class the writes in its class's body give it, `\App\Pool` for `$this->pool->get()`; a property a class with parents
//! says nothing of records `->pool`, and a property with a class holds it from the root as its value type, for the graph
//! to read up the parents. A qualified scope is the name
//! PHP compiles it to, from the root: `Sub\Logger::log()` in the namespace `App` is the receiver `\App\Sub\Logger`. A
//! qualified function call,
//! `\App\run()`, and a construction, `new \App\User()`, call the last segment of the name. A construction of the alias
//! of a `use` calls `__construct` with the class's name from the root as its receiver, `\App\Model` for `new Eloquent`
//! after `use App\Model as Eloquent;`. `new self`, `new static` and `new parent`, in any case, call nothing. A closure
//! assigned to a variable, `$f = function () {}`, makes the calls
//! in its body; a closure passed as an argument leaves them to the callable around it. A call before the file's first
//! symbol records nothing.
//!
//! A callable named by a string or an array is a use of what it names. A string in the callable argument of a builtin
//! that takes one, `array_map('trim', $x)` or `usort($a, 'cmp')`, or of a hook function, `add_action('init', 'boot')`,
//! is a use of the function, and `'Cache::flush'` of the member `flush` of `Cache`. An array `[$this, 'render']`,
//! `[self::class, 'make']`, `[static::class, ...]` or `[parent::class, ...]` is a use of the method of the class around
//! it, and `[Cache::class, 'flush']` of the one of `Cache`, or of the class an alias of that name imports. PHP reads a
//! callable string as fully qualified, `'helper'` the global `helper` and `'App\helper'` the one of `App`, so a string
//! is a use only where the namespace it spells is the file's own. A bare use binds the function its own file declares.
//!
//! # Resolution
//!
//! An `include` or `require` whose path folds to a constant, from strings, `__DIR__`, `__FILE__`, the `.` operator and
//! `dirname`, includes the file it names: from the file's directory for `__DIR__` and a `./` or `../` path, and from
//! the root, or else from the one file whose path ends in it, for any other. A path built from a variable includes
//! nothing. A `use` includes the file that the PSR-4 autoload of the repository's `composer.json` files maps its name
//! to, or else, for a `use function` or a `use const`, the file it maps the name's namespace to; a `use` that maps to
//! no file of the repository is an import as written. A call on the alias of a `use` has the class's name from the
//! root as its receiver, `\App\Model` for `Eloquent::boot()` after `use App\Model as Eloquent;`, which reads the type of
//! exactly that qualified name, and no type of the graph where the class lies outside the tree.
//!
//! A file whose name ends in `Test.php`, or under a `test` or `tests` directory in any case, is test code. A file
//! named `*.generated.php` or `*_pb.php`, or whose leading comments carry `@generated` or `DO NOT EDIT`, is generated
//! code. No symbol is an entry point.
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

use kartta_lang::path::has_extension;
use kartta_lang::{
    BareReceivers, BlockComment, Conventions, DocComments, Language, OwnMembers, ParentSpelling,
    RawExtraction, ReceiverMembers, Receivers, Repository, Resolver, SymbolKind, TypeUses,
};

use crate::resolve::Autoload;

mod banner;
mod fold;
mod psr4;
mod resolve;
mod walk;

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

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::Unreached;
    // constraint: PHP looks a call's name up in a function table, never among constants; a closure a constant holds
    // is called as `(C::F)()`, whose callee is no name
    conventions.values_are_call_targets = false;
    conventions.type_uses = TypeUses::Nowhere;
    conventions.unrecorded_data = &[SymbolKind::Field, SymbolKind::Constant];
    conventions.bare_receivers = BareReceivers::Classes(&["parent", "self", "static"]);
    conventions.base_receivers = &["parent"];
    conventions.constructor = Some(walk::CONSTRUCTOR);
    conventions.parent_spelling = ParentSpelling::FromRoot;
    conventions.receiver_members = ReceiverMembers::Inherited;
    conventions.receivers = Receivers::InheritedFields;
    conventions.own_type_receivers = Some(&["$this", "self", "static"]);
    conventions.includers = Some(
        "the files whose include or require of a constant path names this file, and those whose use names a name the \
         PSR-4 autoload of a composer.json maps to it",
    );
    conventions
};

impl Language for Php {
    fn name(&self) -> &'static str {
        "php"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["php", "phtml"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_php::PHP_NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    // constraint: a bare call never reaches a method of its own class, which PHP calls only through `$this`, `self`
    // or `static`
    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(Autoload::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}
