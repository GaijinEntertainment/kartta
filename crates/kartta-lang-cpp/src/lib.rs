//! C and C++ for the kartta code graph.
//!
//! [`Cpp`] implements [`kartta_lang::Language`] over the tree-sitter C++ grammar, which reads C as well. It claims a
//! file by its extension, in any letter case: `cpp`, `cc`, `cxx`, `h`, `hpp`, `hxx`, `hh`, `c`, `inl` and `inc`.
//! Objective-C (`m`), Objective-C++ (`mm`) and CUDA (`cu`, `cuh`) are not claimed, because the grammar fails on most
//! such files.
//!
//! # Symbols
//!
//! A file yields its functions and methods, its classes, structs, unions (as structs) and enums that have a body, its
//! type aliases (`using` and `typedef`), the data members of its classes as fields, and its namespace-scope variables
//! and constants. A field declared as a value of a class, a reference to one, or a pointer or an array of those carries
//! that class as its [value type](kartta_lang::Symbol::value_types), spelled as a member call's receiver spells it but
//! without its template arguments, after a `*` for each pointer or array dimension, and a field of a class template
//! also the class its first template argument names, after one `*` more, or two for an argument written as a pointer,
//! which `->` reaches through the template's `operator->`, as `eastl::unique_ptr<Lock>` does, and `[]` through its
//! `operator[]`, as `Tab<Lock>` does; a field of a builtin type or of a template parameter, and a type behind more than
//! two pointers, carry none. A type alias of a class carries the class the same way, `List` for `typedef List<Node>
//! NodeList;`; an alias of a pointer, a reference or a builtin type, of a template parameter, or of a class whose
//! written scope holds template arguments or starts at a template parameter, `std::vector<T>::iterator` or
//! `T::iterator`, carries none. A class's parents are its base
//! classes as written, without template arguments, but for a parameter of an enclosing template, `: public Compare`,
//! which names no type. A namespace yields no symbol of its own; it shows in the qualified names of what it holds:
//!
//! - A symbol outside every class is qualified by its enclosing named namespaces, `ns::f`. An anonymous namespace adds
//!   nothing, and `namespace a::b` adds both names.
//! - A class member is qualified by its class alone, `Widget::area`, and is owned by it. A type declared in a class
//!   body is owned by the class too, and qualified by the class's own qualified name, `ns::Widget::Kind`, so that two
//!   namesake classes of different namespaces keep their nested types apart. An out-of-line definition takes its owner
//!   from the scope it writes: `int ns::Widget::area()` defines `Widget::area`, unless the file opens or uses that
//!   scope as a namespace and declares no type of its name: `void ImGui::EndGroup() {}` beside a `namespace ImGui`
//!   defines the free function `ImGui::EndGroup`.
//! - A class whose head writes a scope, `class ns::Widget {}`, is qualified by that scope after the enclosing
//!   namespaces, up to the innermost one named like the scope's first name, so both definitions of `ns::Widget` store
//!   one qualified name.
//!
//! A declaration without a body is emitted once for each callable key, and only where the file holds no definition
//! of that key; a declaration with no key is always emitted. An in-class declaration whose out-of-line definition the
//! file holds becomes a
//! [`FoldedDeclaration`](kartta_lang::FoldedDeclaration) of that definition. A [`Callable`](kartta_lang::Callable) is
//! recorded for an exported function only where its identity is certain: the key is its scope — the enclosing
//! namespaces, then the enclosing classes — its name, its parameter types without their names, defaults and top-level
//! `const` or `volatile`, and its member qualifiers, as in `ns::Widget::run(const int &,int)const &`. A template, a
//! function inside a body or a lambda, a member of an unnamed class, a conversion operator, `operator bool()`, a scope
//! written with template arguments, a relative scope that can name an enclosing namespace, and a parameter declared
//! through anything but a name, a pointer or a reference get no key. An exported namespace-scope value is keyed by
//! its qualified name, which holds no parameter list: `extern` without an initializer declares it, and any other
//! declaration defines it.
//!
//! A symbol is unexported inside an anonymous namespace, as a `static` free function, and as a namespace-scope
//! constant that is neither `extern` nor `inline`, but for a file-scope constant of a `.c` file, which C links outside
//! its file unless it is `static`. A file whose leading comments, after a byte order mark if it has one,
//! announce generated code — `@generated`, `DO NOT EDIT` or `Code generated` outside a backtick span — flags every
//! symbol generated, and a free `main`, `wmain`, `WinMain`, `wWinMain` or `DagorWinMain` at global scope is an entry
//! point, as is a constructor, a destructor and an operator, which the language calls for an object it makes or ends
//! and for an expression.
//!
//! A declaration's signature keeps the macros after its parameters, `void raise(const char* format, ...)
//! PRINTF_ATTR(2, 3);`, which the parser reads as a declaration of their own, and a definition keeps the body the
//! parser reads apart from it: that of an out-of-line constructor after a macro whose initializer list follows its
//! parameters, `PX_INLINE Sc::Shape::Shape(bool b)` `: mFlags(0)`, and that of a function with a scoped parameter type
//! and a macro call after its parameters, `void lock(ns::id_t token) ACQUIRE()`. A class specialization's signature is
//! its head, `template <> struct Box<bool> : Bits`, which tells it from its primary template under their one qualified
//! name; no other class keeps one.
//!
//! A function-like macro is not a symbol: a definition without a return type outside a class, such as a test
//! harness's `TEST(Suite, Case) {}`, is dropped, as is one inside a class that is not a constructor, a destructor or an
//! operator, and a trailing annotation macro, `void load() LOCKS(mtx) {}`, never names the symbol it annotates. A macro
//! before the return type, `JPH_INLINE float Tan(float x)`, keeps the function it annotates, pointer return types
//! included, `IMGUI_API ImGuiTabItem* FindTab()`, as does an upper-case return type before a calling convention,
//! `BOOL WINAPI DllMain()`. A macro between the return type and the name, `XMMATRIX XM_CALLCONV XMMatrixRotationZ()`,
//! and a return type written with a scope after a macro, `INLINE std::string to_string()`, both read by the parser as
//! a scope of the name, name no owner. The scope a definition writes after them, `LRESULT CALLBACK
//! Window::wndProc()`, names its owner. A name a space or a line break sets apart from its parameters after a macro
//! and a return type, `GLM_FUNC_QUALIFIER typename genType::row_type row (`, which the parser evicts into an error,
//! still names the function, and a conversion operator after a macro, `ML_INLINE double4::operator int4() const`, is
//! `operator int4` of `double4`. A macro before a namespace-scope variable's type on its line, `DAS_API uint32_t
//! flags;` or `extern KRNLIMP Map *map;`, keeps the variable. Its signature starts past a macro that opens the
//! declaration, `uint32_t flags`, and keeps one after a specifier, `extern KRNLIMP Map *map`. A declaration that
//! holds a parse error past that yields no field and no data symbol.
//!
//! # Calls
//!
//! A bare call `f()` names `f`; a member call names the field, its template arguments dropped, with the class its
//! operand holds as its receiver: the bare name of the caller's own class on `this`, and on a local or a parameter
//! declared as a value of a class or a reference to it for `.`, or a pointer to it for `->`, that class as its
//! declaration spells it: with the scopes and the template arguments it writes, `b::Widget`, `::b::Widget` or
//! `ns::Box<int>`, the arguments as written, and by its bare name where the spelling holds a space outside them.
//! In a member, a name the body does not declare, `lock` or `this->lock`, names a field of the member's class or of a
//! base, or a namespace's value, which the walk cannot tell apart, so the call records it after the operator that takes
//! the member, `.lock` or `->lock`, for the graph to bind through the field it finds, and an element of it, `locks[i]`,
//! before a `[]`, `.locks[]` or `->locks[]`. `this` outside a member keeps `this`, and any other operand records the
//! empty receiver, because the operand of `.` or `->` is a value whose name names no type. A call through a named
//! scope keeps the scope as written, `ns::f`, and one through the root keeps the `::`, `::f`.
//! `new ns::Widget<T>()` is a call of `Widget`. A cast, a call spelled by a type keyword, and a call or a `new` of a
//! parameter of an enclosing template, `Compare()` in a template over `Compare`, are not recorded. A call outside every
//! function is a call from file scope, before the file's first symbol too. A name in a constructor's initializer list
//! is a call of its bare name where it spells a scope or a template's arguments, or where the class around the
//! constructor derives from a class of that name, unless it names the caller's own class.
//!
//! # Uses
//!
//! A function's initializer list and body yield uses, and so does an initializer outside every body and lambda, where
//! a name, a table of handlers' most often, is a use at [`FILE_SCOPE`](kartta_lang::FILE_SCOPE) that keeps only the
//! name it spells. `x.f` and `x->f` outside a call's callee use `f` on
//! [`Receiver::Path`](kartta_lang::Receiver::Path): the class as a member call's receiver spells it where `x` is a
//! local or a parameter declared as a value of it or a reference to it for `.`, or a pointer to it for `->`, else
//! empty. `this->f` and a member a constructor's initializer list sets use `f` on
//! [`Receiver::Own`](kartta_lang::Receiver::Own), a designated initializer `.f = v` on the path of the type its
//! declaration or compound literal spells, spelled so too. A bare name that no local,
//! parameter, capture or enclosing template parameter declares before it is [`Receiver::Bare`](kartta_lang::Receiver),
//! a member of the enclosing class or a namespace's value alike, an enumerator or a macro included. A qualified name
//! that names a value, `Config::kLimit` or `&Widget::draw`, uses its last name on the path of its scope, spelled as a
//! member call's receiver is, `ns::Box<int>` for `ns::Box<int>::empty` and `Outer<int>::Inner` for
//! `Outer<int>::Inner::kMin`, and one written from the root alone, `::limit`, is bare.
//! A callee and a label are no use. The left side of `=` writes, of a compound assignment, `++` or `--` updates, and
//! an initializer list or a designator writes; `a.b` in `a.b.c = v` reads. Past the class, a bare name binds as
//! [`ValueUses::Included`] says and a member on the empty path as [`UntypedMembers::IncludedField`] says.
//!
//! # Includes
//!
//! The [`Resolver`] resolves each `#include` to a file of the repository, matching letter
//! case, and records a miss as an [`ImportRef`](kartta_lang::ImportRef) with the specifier as written; a computed
//! `#include MACRO` records nothing. A `./` or `../` specifier names a file from the including file's directory, and a
//! quoted one names a file in that directory first. A specifier then names the file at its own path, else the one file
//! whose path ends in it at a `/`, else the one file of its last component's name, else the first of the specifier
//! with `.h`, `.hpp`, `.c`, `.cpp` or `.cc` added. Of several files that end in the specifier, the one whose root, its
//! path less the specifier, holds the including file is named, the deepest root where roots nest, as a file of one
//! vendored copy of a library reaches its own copy. Where no root holds the including file, the one of those files
//! whose directory shares the most leading directories with the including file's is named, as the build of the code
//! beside a copy sets that copy's include path; none of them is named where two share as many, or for a bare name in
//! angle brackets, which spells a system header such as `<time.h>`, since a missing include is better than a wrong
//! one. A file sees what the files it includes include in turn, as the preprocessor pastes them in, so its includes
//! are [`IncludeReach::Transitive`].
//!
//! The resolver also flags a file's symbols by its path: as test code under a `test`, `tests`, `unittest` or
//! `unittests` directory in any case, or with a name ending in `Test`, `_test` or `_unittest` or starting with
//! `test_`, and as generated code with a `.pb` stem, a `_generated.h` suffix, or `.gen.`, `_gen.` or `_AOT.` in the
//! name.
//!
//! # Parse repairs
//!
//! Four constructs the grammar cannot reduce take the declarations around them down with them, so the parser reads a
//! repaired copy of the file while every name, signature and text comes from the original bytes. An empty trailing
//! call argument, `CHECK(ptr, )`, has its comma blanked before the first parse, and so do the upper-case words around
//! the name after `class`, `struct` or `union` of a class that goes on to a base list or a body, the export macros of
//! `class TORRENT_EXPORT torrent : public base {}`, `class Builder FINAL_CLASS {}` and the specialization `struct
//! EXPORT hash<Key> {}`, and so does the call around the key of `ATTRIBUTE_ALIGNED16(class) btX {}`, while `struct
//! STAT st;` is left as written. So does a macro alone
//! on its line above a template, a class or a namespace head, past comments and other such lines, Jolt's
//! `JPH_NAMESPACE_BEGIN` or rtm's `RTM_IMPL_FILE_PRAGMA_PUSH`; above a function it stays, read as the function's return
//! type, and a macro call alone on its line stays too. So do the macros that open and close a namespace inside a
//! namespace, alone on their lines at its edges: `ABSL_NAMESPACE_BEGIN` right after `namespace absl {` and set apart
//! from the code below it by a blank or a comment line, and `ABSL_NAMESPACE_END` right before its `}`. A conditional
//! whose branches each write one head of one name before the `{` after it, `#ifdef A` `void f(int a)` `#else` `void
//! f(int a, int b)` `#endif` `{`, is read with its first branch taken, and so is one whose branches each write a type
//! alone before a function's name, `#if A` `auto` `#else` `std::tuple<int>` `#endif` `reflect() const`, one that holds
//! the `if` of a statement it leaves open, `#ifdef A` `if (x)` `#endif` `f();`, one whose code opens with an `else`, and
//! one that opens right after a `(`. Where the first tree holds an error that spills over three rows or more, the upper-case macro invocations that open its lines are blanked
//! with the macros between one and a body's brace, `TEST(Suite, Case) NO_ANALYSIS {`, and the file is parsed again,
//! and an error that opens at a class head reaches the brace that closes the class; a
//! `#define` body is left as written. Where it reads an `#else` or `#elif` as a
//! directive inside a block the first branch opened, `#ifdef A` `if (x) {` `#else` `if (y) {` `#endif`, every
//! conditional of the file whose first branch leaves a brace open, or closes one it did not open, is read with that
//! branch taken, whatever its condition: its directives and its later branches are blanked. Where it holds the `#else`
//! as a bare token of an error instead, the conditionals whose first branch leaves a brace open are read so. Where it
//! reads an `#endif` as the statement of the label right before it, `skip:`, the label is blanked. Where it reads the
//! macro that opens a class member as the member's type and cannot read on,
//! `PX_FORCE_INLINE const Gu::GeometryUnion& get()` or `PX_INLINE Shape() : mFlags(0) {}`, a misreading that can close
//! the class at that member, the class's members up to the brace that closes it lose the macro that opens them before
//! a type and a declarator, a destructor, a conversion operator or a constructor's initializer list, but where the
//! first tree reads the member as written; the signature of such a member starts past its macro. A re-parse that fails
//! keeps the first tree.
//!
//! A table's data is blanked too: a brace group of 4 KiB or more that holds nothing but numeric literals, signs,
//! commas, comments and nested groups of the same, the initializer of `static const float weights[] = { 0.1, ... }`,
//! keeps its braces and newlines and loses the rest before the first parse. tree-sitter holds a node for every literal,
//! which is far more memory than the file, and such a table after a syntax error can take every definition of the file
//! down; nothing a graph holds is written among the literals. In a parser tree-sitter generates for a grammar, a file
//! whose text before its first brace includes `tree_sitter/parser.h` and defines `LANGUAGE_VERSION`, the group may also
//! hold names, macro calls, designators, strings and character literals: its parse tables spell the parser's macros and
//! the grammar's enumerators, and would parse into twenty times the file's bytes. Its definitions, and the calls and
//! uses of its lexer and its language function, are read as written.
//!
//! # Fingerprint
//!
//! The fingerprint covers the crate's name and version, a digest of its own sources, and the grammar's version, its
//! node types, ABI version and counts of node kinds, parse states and fields. It cannot see a
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

mod overrides;
mod repair;
mod resolve;
mod walk;

use kartta_lang::tree::parse;
use kartta_lang::{
    CallsOnValues, Conventions, DocComments, Implements, IncludeReach, InheritedMembers, Language,
    OwnMembers, RawExtraction, Repository, Resolver, SameMethod, TypeUses, UntypedMembers,
    ValueUses,
};

const EXTENSIONS: [&str; 10] = [
    "cpp", "cc", "cxx", "h", "hpp", "hxx", "hh", "c", "inl", "inc",
];
const BRACKET_INCLUDE: u32 = 0;
const QUOTED_INCLUDE: u32 = 1;

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

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::First;
    conventions.inherited_members = InheritedMembers::Found;
    conventions.values_are_call_targets = false;
    conventions.scope_separator = Some("::");
    conventions.include_reach = IncludeReach::Transitive;
    conventions.calls_on_values = CallsOnValues::NoNearestGuess;
    conventions.value_uses = ValueUses::Included;
    conventions.untyped_members = UntypedMembers::IncludedField;
    conventions.type_uses = TypeUses::Nowhere;
    conventions.implements = Implements::Overrides(SameMethod(overrides::overrides));
    conventions
};

impl Language for Cpp {
    fn name(&self) -> &'static str {
        "cpp"
    }

    fn claims(&self, path: &str) -> bool {
        kartta_lang::path::has_extension(path, &EXTENSIONS)
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_cpp::NODE_TYPES)
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::C_LIKE.with_carried(&["template ", "template<"])
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        extract(source)
    }

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(resolve::CppResolver)
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_cpp::LANGUAGE.into()
}

fn extract(source: &[u8]) -> RawExtraction {
    let grammar = grammar();
    let repaired = repair::for_parse(source);
    let parsed = repaired.as_deref().unwrap_or(source);
    let Some(tree) = parse(&grammar, parsed) else {
        return RawExtraction::default();
    };

    let Some(second) = repair::after_recovery(&tree, parsed) else {
        return walk::raw_walk(&tree, source);
    };
    drop(tree);
    parse(&grammar, &second)
        .or_else(|| parse(&grammar, parsed))
        .map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
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
        assert_eq!(conventions.include_reach, IncludeReach::Transitive);
        assert_eq!(conventions.calls_on_values, CallsOnValues::NoNearestGuess);
    }

    #[test]
    fn uses_past_the_class_bind_through_the_includes() {
        let conventions = Cpp::new().conventions();

        assert_eq!(conventions.value_uses, ValueUses::Included);
        assert_eq!(conventions.untyped_members, UntypedMembers::IncludedField);
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

        assert!(first.starts_with(b"kartta-lang-cpp "));
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
