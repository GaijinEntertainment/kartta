//! Go for the kartta code graph: the definitions, calls, uses and imports of Go source.
//!
//! [`Go`] implements [`Language`] over the `tree-sitter-golang` grammar. It claims every file whose extension is `go`,
//! in any case.
//!
//! # What a file extracts to
//!
//! Every function, method, type, interface member, struct field, constant and variable is a symbol, qualified by the
//! package clause: `api.Serve`, and `api.Server.Close` for a member of `Server`. A struct body gives one field symbol
//! per named field, an interface body one method symbol per member, whether the type is defined (`type T struct{}`)
//! or a literal alias (`type T = struct{}`); any other type, `type X = Y` or `type H func()`, is a type alias. An
//! embedded field or interface is a parent of its type, and so is the interface of a compile-time assertion
//! (`var _ I = T{}`, `&T{}`, `new(T)` or `(*T)(nil)`). An assertion whose type the file does not declare is recorded as
//! an assertion, for the graph to apply where it sees the whole package.
//!
//! A call records its callee and, for a member call, its receiver. Go spells an owner as a bare type name, so only an
//! identifier operand is a receiver: `s.client.Do()` and `f().g()` record the empty receiver, a member call on a value
//! of no known type, and so does a call on a value the caller declares, a parameter or a variable, whose name names no
//! type. The method's own receiver records the owner its declaration spells: `w.draw()` in `func (w *Widget) run()`
//! records `Widget`. A member call calls a method or a field of its value and reaches a function only through an
//! import, so no tier binds it to a function of the caller's file, package or neighbors. A chained call records every
//! call in the chain. A call of a symbol that a body declares, a local `var`, `const` or `type`, is a local call of
//! that symbol; a call of a local value with no symbol — a parameter, a `:=` variable — and a computed callee, `(*p)()`
//! or `fns[i]()`, record nothing. An index call names the generic function or method it instantiates, `f[T](x)` and
//! `v.F[T](x)`, when its indices are type arguments; an index by a value of the file, or of a package-level variable,
//! calls an element and records nothing.
//!
//! A name outside a call that names a package-level type or a member of an imported package — a type in a signature or
//! a declaration, a conversion, `pkg.Member` — is a use, attributed to the symbol whose declaration holds it. A field
//! or method selected from a value, `c.Limit`, is not: which type declares it is the value's type, which extraction
//! does not know. A name a local binding covers is no use of what it would otherwise name.
//!
//! A function bound to a package-level variable, `var Handler = func() {}`, is the variable alone: its body is no
//! caller, so the calls in it are made from file scope, and the graph drops them.
//!
//! # Resolution
//!
//! [`Go::resolver`] reads every `go.mod` outside a `vendor` directory: its `module` path and its `replace` directives
//! onto local directories, which apply to the files of the module that declares them, as the go command applies only
//! the main module's. An import names the vendored copy of its package when the `vendor` directory of the importing
//! file's module holds one, and otherwise the directory under the module whose path is the longest prefix of the
//! import, the importing file's own module where several declare that path; an import that matches no module, or whose
//! directory holds no files, is external. A resolved import includes every Go file of the package's
//! directory that is not a test file, except the importing file itself.
//!
//! A `_test.go` file is test code, and a file named `*.pb.go` or carrying the `// Code generated ... DO NOT EDIT.`
//! banner is generated code. A file under a `vendor` directory is vendored code. A free `func main` in `package main`
//! is the program's entry point.
//!
//! # Fingerprint
//!
//! [`Go::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its version,
//! ABI version, node types, and the counts of its node kinds, parse states and fields. A change to any of them changes
//! the fingerprint. A change to `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it.
//! It cannot see a grammar built from changed sources that keep its version and tables — a patched external scanner —
//! nor a change to the tree-sitter runtime that keeps its version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_go::Go;
//!
//! let go = Go::new();
//! assert!(go.claims("cmd/server/main.go"));
//!
//! let raw = go.extract(b"package api\n\nfunc Serve() {}\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["api.Serve"]);
//! ```

use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::path::base;
use kartta_lang::{
    Conventions, Language, MemberCalls, OwnMembers, RawExtraction, Repository, Resolver, TypeUses,
};
use tree_sitter::{Parser, Tree};

use crate::fnv::{FNV_OFFSET, fnv};
use crate::resolve::GoResolver;

mod fnv;
mod resolve;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_golang::NODE_TYPES.as_bytes());

/// The Go language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Go;

impl Go {
    /// The Go language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Language for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn claims(&self, path: &str) -> bool {
        base(path)
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("go"))
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn conventions(&self) -> Conventions {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::Unreached;
        conventions.directory_packages = true;
        conventions.member_calls = MemberCalls::Members;
        conventions.type_uses = TypeUses::Every;

        conventions
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        parse(source).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(GoResolver::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_golang::LANGUAGE.into()
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
