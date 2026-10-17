//! C and C++ for the kartta code graph.
//!
//! [`Cpp`] implements [`kartta_lang::Language`] over the tree-sitter C++ grammar, which reads C as well. It claims a
//! file by its extension, in any letter case: `cpp`, `cc`, `cxx`, `h`, `hpp`, `hxx`, `hh`, `c`, `inl` and `inc`.
//! Objective-C++ (`m`, `mm`) and CUDA (`cu`, `cuh`) are not claimed, because the grammar fails on most such files.
//!
//! # Symbols
//!
//! A file yields its functions and methods, its classes, structs, unions (as structs) and enums that have a body, its
//! type aliases (`using` and `typedef`), the data members of its classes as fields, and its namespace-scope variables
//! and constants. A namespace yields no symbol of its own; it shows in the qualified names of what it holds:
//!
//! - A symbol outside every class is qualified by its enclosing named namespaces, `ns::f`. An anonymous namespace adds
//!   nothing, and `namespace a::b` adds both names.
//! - A class member, a type declared in a class body included, is qualified by its class alone, `Widget::area`, and
//!   is owned by it. An out-of-line definition takes its owner from the scope it writes: `int ns::Widget::area()`
//!   defines `Widget::area`.
//! - A class whose head writes a scope, `class ns::Widget {}`, is qualified by that scope after the enclosing
//!   namespaces, up to the innermost one named like the scope's first name, so both definitions of `ns::Widget` store
//!   one qualified name.
//!
//! A declaration without a body is emitted only where the file holds no definition of the same callable, and an
//! in-class declaration whose out-of-line definition the file holds becomes a
//! [`FoldedDeclaration`](kartta_lang::FoldedDeclaration) of that definition. A [`Callable`](kartta_lang::Callable) is
//! recorded for an exported function only where its identity is certain: the key is its scope — the enclosing
//! namespaces, then the enclosing classes — its name, its parameter types without their names, defaults and top-level
//! `const` or `volatile`, and its member qualifiers, as in `ns::Widget::run(const int &,int)const &`. A template, a
//! function inside a body or a lambda, a member of an unnamed class, a scope written with template arguments, a
//! relative scope that can name an enclosing namespace, and a parameter declared through anything but a name, a
//! pointer or a reference get no key.
//!
//! A symbol is unexported inside an anonymous namespace, as a `static` free function, and as a namespace-scope
//! constant that is neither `extern` nor `inline`. A file whose leading comments announce generated code —
//! `@generated`, `DO NOT EDIT` or `Code generated` outside a backtick span — flags every symbol generated, and a free
//! `main`, `wmain`, `WinMain`, `wWinMain` or `DagorWinMain` at global scope is an entry point, as is a constructor, a
//! destructor and an operator, which the language calls for an object it makes or ends and for an expression.
//!
//! A function-like macro is not a symbol: a definition without a return type outside a class, such as a test
//! harness's `TEST(Suite, Case) {}`, is dropped, as is one inside a class that is not a constructor, a destructor or an
//! operator, and a trailing annotation macro, `void load() LOCKS(mtx) {}`, never names the symbol it annotates. A
//! declaration that holds a parse error yields no field and no data symbol.
//!
//! # Calls
//!
//! A bare call `f()` names `f`; a member call names the field, its template arguments dropped, with `this` as its
//! receiver on `this` and the empty receiver on any other operand, a named one included, because the operand of `.` or
//! `->` is a value and its name names no type. A call through a named scope keeps the scope as written, `ns::f`, and
//! one through the root keeps the `::`, `::f`. `new ns::Widget<T>()` is a call of `Widget`. A cast, a call spelled by a
//! type keyword, and a call before the first symbol of the file are not recorded.
//!
//! # Includes
//!
//! The [`Resolver`] resolves each `#include` to a file of the repository, matching letter
//! case, and records a miss as an [`ImportRef`](kartta_lang::ImportRef) with the specifier as written; a computed
//! `#include MACRO` records nothing. A `./` or `../` specifier names a file from the including file's directory, and a
//! quoted one names a file in that directory first. A specifier then names the file at its own path, else the one file
//! whose path ends in it at a `/`, else the one file of its last component's name, else the first of the specifier
//! with `.h`, `.hpp`, `.c`, `.cpp` or `.cc` added. Two files that end in the specifier name neither: a missing include
//! is better than a wrong one.
//!
//! The resolver also flags a file's symbols by its path: as test code under a `test`, `tests`, `unittest` or
//! `unittests` directory or with a name ending in `Test`, `_test` or `_unittest` or starting with `test_`, and as
//! generated code with a `.pb` stem, a `_generated.h` suffix, or `.gen.`, `_gen.` or `_AOT.` in the name.
//!
//! # Parse repairs
//!
//! Two constructs the grammar cannot reduce take the declarations around them down with them, so the parser reads a
//! repaired copy of the file while every name, signature and text comes from the original bytes. An empty trailing
//! call argument, `CHECK(ptr, )`, has its comma blanked before the first parse. Where the first tree holds an error
//! that spills over three rows or more, the upper-case macro invocations that open its lines are blanked and the file
//! is parsed again; a `#define` body is left as written. A re-parse that fails keeps the first tree.
//!
//! # Fingerprint
//!
//! The fingerprint covers the crate's name and version, a digest of its own sources, and the grammar's version where
//! it declares one, its node types, ABI version and counts of node kinds, parse states and fields. It cannot see a
//! change to the grammar's parse tables or external scanner that keeps those, nor a tree-sitter release that recovers
//! from errors differently.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_cpp::Cpp;
//!
//! let raw = Cpp::new().extract(b"namespace gfx {\nint area(int w, int h) { return w * h; }\n}\n");
//!
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["gfx::area"]);
//! let keys: Vec<&str> = raw.callables.iter().map(|callable| callable.key.as_str()).collect();
//! assert_eq!(keys, ["gfx::area(int,int)"]);
//! ```

mod banner;
mod fingerprint;
mod fnv;
mod repair;
mod resolve;
mod walk;

use std::cell::RefCell;

use kartta_lang::{
    Conventions, DocComments, InheritedMembers, Language, OwnMembers, RawExtraction, Repository,
    Resolver,
};
use tree_sitter::Parser;

const EXTENSIONS: [&str; 10] = [
    "cpp", "cc", "cxx", "h", "hpp", "hxx", "hh", "c", "inl", "inc",
];
const BRACKET_INCLUDE: u32 = 0;
const QUOTED_INCLUDE: u32 = 1;

thread_local! {
    static PARSER: RefCell<Parser> = RefCell::new(cpp_parser());
}

/// C and C++, as one language of the code graph.
///
/// Its calls resolve under C++ name lookup: a bare call in a member reaches the member's own class first, then a
/// member a base class certainly declares, and a variable or a constant is never a call target. Each thread that
/// extracts keeps one parser for the files it extracts.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Cpp;

impl Cpp {
    /// The language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Language for Cpp {
    fn name(&self) -> &'static str {
        "cpp"
    }

    fn claims(&self, path: &str) -> bool {
        kartta_lang::path::base(path)
            .rsplit_once('.')
            .is_some_and(|(_, extension)| {
                EXTENSIONS
                    .iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            })
    }

    fn fingerprint(&self) -> &[u8] {
        fingerprint::fingerprint()
    }

    fn conventions(&self) -> Conventions {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::First;
        conventions.inherited_members = InheritedMembers::Found;
        conventions.values_are_call_targets = false;
        conventions.scope_separator = Some("::");
        conventions
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::C_LIKE.with_carried(&["template ", "template<"])
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        PARSER.with_borrow_mut(|parser| extract(parser, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(resolve::Includes::new(repository))
    }
}

#[expect(
    clippy::expect_used,
    reason = "the C++ grammar's ABI is fixed at build time by the pinned crates; a tree-sitter that cannot load it \
              fails every extraction alike, which is a build defect"
)]
fn cpp_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .expect("the pinned C++ grammar has an ABI this tree-sitter loads");
    parser
}

fn extract(parser: &mut Parser, source: &[u8]) -> RawExtraction {
    let repaired = repair::for_parse(source);
    let parsed = repaired.as_deref().unwrap_or(source);
    let Some(tree) = parser.parse(parsed, None) else {
        return RawExtraction::default();
    };

    let tree = repair::after_recovery(&tree, parsed)
        .and_then(|second| parser.parse(&second, None))
        .unwrap_or(tree);

    walk::raw_walk(&tree, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qualified_names(raw: &RawExtraction) -> Vec<&str> {
        raw.symbols
            .iter()
            .map(|symbol| symbol.qualified_name.as_str())
            .collect()
    }

    #[test]
    fn claims_c_and_cpp_files_by_extension_in_any_case() {
        let cpp = Cpp::new();

        for path in [
            "a.cpp",
            "a.cc",
            "a.cxx",
            "a.h",
            "a.hpp",
            "a.hxx",
            "a.hh",
            "a.c",
            "a.inl",
            "a.inc",
            "src/A.CPP",
            "src/b.H",
        ] {
            assert!(cpp.claims(path), "{path}");
        }
        for path in [
            "a.m",
            "a.mm",
            "a.cu",
            "a.cuh",
            "a.rs",
            "cpp",
            "src.cpp/readme",
            "a.cpp.orig",
        ] {
            assert!(!cpp.claims(path), "{path}");
        }
    }

    #[test]
    fn calls_resolve_under_cpp_name_lookup() {
        let conventions = Cpp::new().conventions();

        assert_eq!(conventions.own_members, OwnMembers::First);
        assert_eq!(conventions.inherited_members, InheritedMembers::Found);
        assert!(!conventions.values_are_call_targets);
        assert_eq!(conventions.owner_chain_separator, None);
        assert!(!conventions.bare_call_reaches_enclosing_owners);
        assert_eq!(conventions.scope_separator, Some("::"));
        assert!(!conventions.directory_packages);
    }

    #[test]
    fn the_language_is_named_cpp_in_its_own_family() {
        let cpp = Cpp::new();

        assert_eq!(cpp.name(), "cpp");
        assert_eq!(cpp.family(), "cpp");
    }

    #[test]
    fn the_fingerprint_names_the_crate_and_is_computed_once() {
        let (one, other) = (Cpp::new(), Cpp::new());
        let first = one.fingerprint();
        let second = other.fingerprint();

        assert!(first.starts_with(b"kartta-lang-cpp\0"));
        assert_eq!(
            first.len(),
            "kartta-lang-cpp\0".len() + env!("CARGO_PKG_VERSION").len() + 1 + 8 + 5 * 8
        );
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn empty_trailing_argument_keeps_both_functions() {
        let raw = Cpp::new().extract(
            br"
void run(int idx)
{
  CHECK(idx < items.size(), );
  CHECK(support != nullptr, );
}

void after_it()
{
  more();
}
",
        );

        assert_eq!(qualified_names(&raw), ["run", "after_it"]);
    }

    #[test]
    fn declaration_macro_shred_recovers_the_class_and_its_owners() {
        let raw = Cpp::new().extract(
            br"
class Wrap final : public Base
{
public:
  void render(ContextId inst_ctx, uint32_t mask, const Point3 &view_pos, bool cuttings = true,
    bool disable_cockpit = false)
#ifndef PROJECT_X
    DAG_TS_REQUIRES(render_lock)
#endif
      ;
  void render(ContextId inst_ctx, ContextId pilots_ctx, uint32_t mask, const Point3 &view_pos,
    bool cuttings = true, const Frustum *cull_frustum = nullptr)
#ifndef PROJECT_X
    DAG_TS_REQUIRES(render_lock)
#endif
      ;
  void renderWeapons(ContextId inst_ctx, uint32_t mask);
  void renderTrans(const TMatrix &vtm);
};

void free_fn() {}
",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "Wrap",
                "free_fn",
                "Wrap::render",
                "Wrap::render",
                "Wrap::renderWeapons",
                "Wrap::renderTrans"
            ]
        );
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "free_fn()",
                "Wrap::render(ContextId,uint32_t,const Point3 &,bool,bool)",
                "Wrap::render(ContextId,ContextId,uint32_t,const Point3 &,bool,const Frustum *)",
                "Wrap::renderWeapons(ContextId,uint32_t)",
                "Wrap::renderTrans(const TMatrix &)",
            ]
        );
    }

    #[test]
    fn extraction_is_the_same_on_every_thread() {
        let source = b"namespace ns {\nstruct Widget { void run(int value); };\n}\n\
                       void ns::Widget::run(int input) {}\n";
        let here = Cpp::new().extract(source);

        let there = std::thread::scope(|scope| scope.spawn(|| Cpp::new().extract(source)).join());

        assert!(matches!(there, Ok(ref raw) if *raw == here), "{there:?}");
    }
}
