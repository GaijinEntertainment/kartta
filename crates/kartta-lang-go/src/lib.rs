//! Go for the kartta code graph: the definitions, calls, uses and imports of Go source.
//!
//! [`Go`] implements [`Language`] over the `tree-sitter-golang` grammar. It claims every file whose extension is `go`,
//! in any case, unless its name starts with `_` or `.`, which no build of the go command reads: the graph holds no such
//! file, and the repository's file index does.
//!
//! # What a file extracts to
//!
//! Every function, method, type, interface member, struct field, constant and variable is a symbol, qualified by the
//! package clause: `api.Serve`, and `api.Server.Close` for a member of `Server`. A struct body gives one field symbol
//! per named field, an interface body one method symbol per member, whether the type is defined (`type T struct{}`)
//! or a literal alias (`type T = struct{}`); any other alias, `type X = Y`, is a type alias, and any other defined
//! type, `type H func()`, a defined type. An anonymous struct with a field that a struct field's or a variable's type
//! spells, behind pointers and containers too, or that a literal `:=` or `var` assigns to a name spells,
//! `tests := []struct{...}{...}`, is no symbol: the field or the variable it types owns its fields, `Limits` for the
//! field `Limits` of `Server`, and they are qualified by the names that lead to them, `api.Server.Limits.MaxConns`. A
//! `:=` name that owns such a struct is a variable of its body. The owner's value is the struct, spelled `{N}` for the
//! owner's index in the file, so a type path through it reaches the fields, and the owner holds the types the struct
//! embeds. A value a body declares qualifies its fields by the callable too, `api.TestRun.tests.name`. An embedded
//! field or interface is a parent of its type. A compile-time assertion (`var _ I = T{}`, `&T{}`, `new(T)`, `T(nil)` or
//! `(*T)(nil)`, on an interface `I` or an instantiated `I[A]`) embeds nothing, so it is recorded as an assertion, for
//! the graph to apply where it sees the whole package.
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
//! or method selected from a value without a call, `c.Limit`, a key of a struct literal, `Config{Limit: 5}`, and an
//! element of one without keys whose struct the file declares, `Config{5}`, are uses whose receiver is the value's
//! type path, as a member call's is; the keys and elements of a literal of an anonymous struct assigned to no name, an
//! argument or a returned value, name no field. A name a local binding covers is no use of
//! what it would otherwise name. The graph judges a type, a field and a package-level value dead through these uses
//! ([`TypeUses::Every`], [`DataUses::Every`]).
//!
//! A function bound to a package-level variable, `var Handler = func() {}`, is the variable alone, and the calls of a
//! package-level `var` or `const` initializer, a function literal's body among them, are calls of the name the value
//! initializes, or of the first name when the values do not line up with the names, as one call's results do.
//!
//! # Resolution
//!
//! [`Go::resolver`] reads every `go.mod` outside a `vendor` directory: its `module` path and its `replace` directives
//! onto local directories, which apply to the files of the module that declares them, as the go command applies only
//! the main module's. An import names the vendored copy of its package when the `vendor` directory of the module that
//! builds the importing file holds one, that file's own module or, where that one holds no `vendor/modules.txt`, the
//! module that replaces it onto its directory, and otherwise the directory under the module whose path is the longest
//! prefix of the import, the importing file's own module where several declare that path; an import that matches no
//! module, or whose
//! directory holds no files, is external. A resolved import includes every Go file of the package's
//! directory that is neither a test file nor one it leaves unclaimed, except the importing file itself.
//!
//! The resolver keeps only what it derived: the modules, their local replaces, and which of them vendor. A resolver
//! built again over an edit that changes none of them, such as a `require` bump or a `go.mod` added under `vendor`,
//! [resolves as](kartta_lang::Resolver::resolves_as) the one built before, so an engine that follows the tree
//! resolves no Go file again for it.
//!
//! A file's build constraint is what its `//go:build` line says, else its `// +build` lines, as go/build reads them, and
//! the tags its `_$GOOS`, `_$GOARCH` or `_$GOOS_$GOARCH` name requires. Two files are built apart where no build
//! satisfies both: a build has one `GOOS`, which sets `linux` under `android`, `solaris` under `illumos`, `darwin` under
//! `ios` and `unix` under a Unix one, one `GOARCH`, one compiler of `gc` and `gccgo`, one release, which sets every
//! `go1.N` up to its own, and any other tags.
//!
//! A `_test.go` file is test code, and a file named `*.pb.go` or carrying the `// Code generated ... DO NOT EDIT.`
//! banner is generated code. A file under a `vendor` directory is vendored code. A free `func main` in `package main`
//! is the program's entry point, and so is a free `func init()` in any package, which the runtime calls and no code
//! can name. A struct field whose tag hands it to a library by its name, as `json:"name"` does, is read by reflection
//! ([`SymbolFlags::is_reflected`](kartta_lang::SymbolFlags::is_reflected)). So is a field whose tag holds no
//! `key:"value"` pair, since a library may read such a tag whole. A tag whose every value is `-`, which makes
//! `encoding/json` and its kin skip the field, hands nothing over. An exported field is read by reflection too where
//! the file passes a value of its struct to a call named `Marshal`, `MarshalIndent`, `MarshalWrite`, `Unmarshal`,
//! `UnmarshalRead`, `Encode` or `Decode`, unless a tag skips the field, or to `ValueOf` or `TypeOf`, whatever its tags,
//! as `encoding/json`, `encoding/xml`, `reflect` and their kin name theirs: `json.Unmarshal(data, &info)` for
//! `var info Info` marks the exported fields of `Info`, and those of the structs they hold, when `Info` is a
//! package-level struct of the file, or an alias, a defined type or a container type of one, and the walk names the
//! value's type by its name. A func type hands its results' fields, and a channel its element's, to `ValueOf` and
//! `TypeOf` alone, since a serializing library refuses both. An exported function or package-level variable of a
//! `package main` file that declares no `func main`, as a plugin's files are, is read by name too: the plugin's host
//! looks it up through `plugin.Lookup`. The walk reads one file, so a file of a program's `package main` that leaves
//! `func main` to a sibling file is taken for a plugin's.
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
    BuiltApart, Conventions, DataUses, DocComments, Implements, Language, MemberCalls, OwnMembers,
    RawExtraction, Receivers, Repository, Resolver, SameMethod, Twins, TypeUses,
};

use crate::resolve::GoResolver;

mod build;
mod method_type;
mod resolve;
mod runtime;
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
    conventions.data_uses = DataUses::Every;
    conventions.twins = Twins::Package(BuiltApart(build::built_apart));
    conventions.library_interface_methods = runtime::INTERFACE_METHODS;
    conventions.implements = Implements::MethodSets(SameMethod(method_type::same_method));
    conventions.includers = Some(
        "the files whose import names this file's package, the same ones for every file of the package and none for \
         a _test.go file",
    );
    conventions
};

impl Language for Go {
    fn name(&self) -> &'static str {
        "go"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["go"]) && !build::is_skipped(path)
    }

    fn declines(&self, path: &str) -> Option<&str> {
        (has_extension(path, &["go"]) && build::is_skipped(path))
            .then_some("no Go build reads a file whose name starts with _ or .")
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
