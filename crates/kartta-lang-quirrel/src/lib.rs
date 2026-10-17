//! Quirrel for the kartta code graph: the definitions, calls and imports of Quirrel scripts.
//!
//! [`Quirrel`] implements [`Language`] over the `tree-sitter-quirrel` grammar. It claims every file whose extension
//! is `nut`, in any case.
//!
//! # What a file extracts to
//!
//! Every named function, class, enum, enum member and constant is a symbol, and so is a function, a lambda or a class
//! bound to a name. A function value takes the name of its binding, the name its callers use: a table slot, a `let`
//! or `local`, or the target of an `=` or `<-`. A member of a named or bound class is a method of the class, and an
//! assignment to a field access binds a method of the whole receiver expression, `gui.Widget.build` one of
//! `gui.Widget`. Any other function is free; a function value with no binding and no name of its own belongs to no
//! symbol, and its calls have no caller. A data slot with a plain name, `count = 0`, of a class or of a table that a
//! file-scope `let` or `local` binds, is a field of that class or binding, with the slot as its signature when the
//! value is a literal, a name or a unary expression; a computed `[k] = v` or quoted `"k": v` slot is none.
//! `class C(Base)` names `Base` as a parent. A callable's signature is its header, from its declaration or binding to
//! its body.
//!
//! A call records its callee: a bare name, a root-table `::f`, or the field of a member call, whose receiver is the
//! whole receiver expression as written. A computed callee, `a[k]()` or `f()()`, records nothing.
//!
//! # Resolution
//!
//! Every `import`, `from ... import`, and `require("spec")` or `require_optional("spec")` with one string literal
//! names a module. A script resolves through the nearest `.sqconfig` in its directory or above it, and that file
//! alone: `%name/rest` names `rest` below the directory the file mounts as `name`, and any other specifier that ends
//! in `.nut` names the file beside the script, else the file below the first base path that holds it. A `.sqconfig`
//! that cannot be read mounts nothing and adds no base path. A script that no `.sqconfig` governs resolves by the
//! specifier's shape: `%root/rest` and any other slashed or `.nut` specifier name the file under the nearest directory
//! of the script that holds them, `./` and `../` name the file relative to the script, and `.nut` is appended when the
//! path as written names none. A bare name is a native module and names no file. Every match is case-sensitive. The
//! file a specifier names is an include; a specifier that names no file is an import, kept as written, with the names
//! a `from` import takes.
//!
//! A file with a `test` or `tests` directory in its path, or whose stem ends in `_test` or starts with `test_`, is test
//! code. A file whose name holds `.gen.` or `_gen.`, or whose leading comments carry `@generated`, `DO NOT EDIT` or
//! `Code generated`, is generated code.
//!
//! # Fingerprint
//!
//! [`Quirrel::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to any of them
//! changes the fingerprint. A change to `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys
//! beside it. It cannot see a grammar built from changed sources that keep its version and tables — a patched external
//! scanner — nor a change to the tree-sitter runtime that keeps its version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_quirrel::Quirrel;
//!
//! let quirrel = Quirrel::new();
//! assert!(quirrel.claims("ui/hud/main.nut"));
//!
//! let raw = quirrel.extract(b"class Hud {\n  function draw() {}\n}\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["Hud", "Hud.draw"]);
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
use crate::resolve::QuirrelResolver;

mod banner;
mod fnv;
mod resolve;
mod runtime;
mod sqconfig;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_quirrel::NODE_TYPES.as_bytes());

/// The Quirrel language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Quirrel;

impl Quirrel {
    /// The Quirrel language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const DOC_COMMENTS: DocComments =
    DocComments::lines(&["//", "#"]).with_blocks(&[BlockComment::new("/*", "*/")]);

impl Language for Quirrel {
    fn name(&self) -> &'static str {
        "quirrel"
    }

    fn claims(&self, path: &str) -> bool {
        base(path)
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("nut"))
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    fn conventions(&self) -> Conventions {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::First;
        conventions.runtime_methods = runtime::METHODS;

        conventions
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        parse(source).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(QuirrelResolver::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_quirrel::LANGUAGE.into()
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
