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
//! or a literal alias (`type T = struct{}`); any other alias, `type X = Y`, is a type alias, and any other defined
//! type, `type H func()`, a defined type. An
//! embedded field or interface is a parent of its type, and so is the interface of a compile-time assertion
//! (`var _ I = T{}`, `&T{}`, `new(T)` or `(*T)(nil)`). An assertion whose type the file does not declare is recorded as
//! an assertion, for the graph to apply where it sees the whole package.
//!
//! A call records its callee and, for a member call, its receiver as a type path
//! ([`Receivers::TypePaths`]): the type of the operand's value, as far as the
//! file's declarations spell it. `s.client.Do()` in `func f(s *Server)` records `Server.client`, `w.draw()` in
//! `func (w *Widget) run()` records `Widget`, and a variable records the path of what its declaration gives it: a
//! type it names, a composite literal's type, a call's result, `new(T)`, an element of a container, a type switch's
//! one type. A definition records the types of the values it yields: a variable's or a field's type, a function's
//! results, an alias's target. A member call on a value the walk cannot type, `(a + b).f()`, records the empty
//! receiver. A member call calls a method or a field of its value and reaches a function only through an
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
//! A function bound to a package-level variable, `var Handler = func() {}`, is the variable alone, and the calls of a
//! package-level `var` or `const` initializer, a function literal's body among them, are calls of the name the value
//! initializes, or of the first name when the values do not line up with the names, as one call's results do.
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
//! is the program's entry point, and so is a free `func init()` in any package, which the runtime calls and no code
//! can name.
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

use kartta_lang::path::has_extension;
use kartta_lang::{
    Conventions, DocComments, Language, MemberCalls, OwnMembers, RawExtraction, Receivers,
    Repository, Resolver, TypeUses,
};

use crate::resolve::GoResolver;

mod resolve;
mod walk;

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

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::Unreached;
    conventions.directory_packages = true;
    conventions.member_calls = MemberCalls::Members;
    conventions.receivers = Receivers::TypePaths;
    conventions.type_uses = TypeUses::Every;
    conventions
};

impl Language for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["go"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_golang::NODE_TYPES)
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::C_LIKE
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(GoResolver::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_golang::LANGUAGE.into()
}
