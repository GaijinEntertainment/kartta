# kartta-lang-cpp

C and C++ for the kartta code graph: definitions, calls and includes extracted through tree-sitter.

`Cpp` implements the `kartta_lang::Language` contract over the tree-sitter C++ grammar, which reads C as well. It claims
`.cpp`, `.cc`, `.cxx`, `.h`, `.hpp`, `.hxx`, `.hh`, `.c`, `.inl` and `.inc` files, in any letter case.

## What a file yields

From the bytes alone: functions and methods, classes, structs, unions and enums, type aliases, class data members, and
namespace-scope variables and constants, each qualified the way C++ names it — `ns::f` for a free function in a
namespace, `Widget::area` for a member. A member call carries the class its operand holds as its receiver: the
caller's own class on `this`, and on a local or a parameter the class its declaration spells, with the scopes it writes,
`b::Widget`, and without its template arguments; one on any other operand carries the empty receiver, because a value's
name names no type. A bare call of a name a local, a parameter or a capture of the body declares calls through the
value it holds, a function pointer or a callable, and is not recorded. Callables carry an identity of scope, parameter
types and member qualifiers wherever the walk can establish one without guessing, so a declaration and its definition
meet in the graph and two overloads stay apart. Function-like macros, a test harness's `TEST(...)` blocks and trailing
annotation macros included, are never mistaken for definitions; a declaration's signature keeps the macros after its
parameters, and a definition keeps the body the parser reads apart from it after such a macro.

Inside a function's body and its initializer list, each read, write and update of a value is a use: `x.f` and `x->f`
outside a call's callee, `this->f`, a member a constructor's initializer list or a designated initializer sets, and a
bare name that no local, parameter, capture or enclosing template parameter of the body declares before it. A bare name
stays bare, since C++ looks it up in the enclosing class before the namespaces. A member on `this` or in an initializer
list is on the own receiver, and one on a local or a parameter names the type its declaration spells, a pointer's for
`->`, as a member call's receiver names it; any other operand names no type. A name the walk cannot place — an
enumerator, a macro, a value of another file — is kept, for the graph to bind or leave unbound.

From the path and the repository: each `#include` resolved to the file of the repository it names — the path relative to
the including file, the exact path, the one file whose path ends in it — with no compiler search path, or kept as an
unresolved import when no file matches or two match equally well; and test or generated code marked by the file's name.

## Parse repairs

Four constructs the grammar cannot reduce take the declarations around them down with them: an empty trailing macro
argument (`CHECK(ptr, )`), an export macro between a class key and the class name (`class TORRENT_EXPORT torrent :
base`), a macro alone on its line above a class, a template or a namespace (`JPH_NAMESPACE_BEGIN`), and a
function-like macro where a declaration belongs. So does a conditional whose first branch leaves a
brace open or closes one it did not open, `#ifdef A` `if (x) {` `#else` `if (y) {` `#endif`. Where the parser reads such
an `#else` inside the block the first branch opened, every conditional of that shape in the file is read with its first
branch taken, whatever its condition. A macro that opens a class member, `PX_FORCE_INLINE const Gu::GeometryUnion&
get()`, reads as the member's type and can close the class at that member; where the parser misreads one, the members
of that class lose the macros the parser cannot read past. The entries of a large table, its numbers or, in a parser
tree-sitter generates, its macro calls and enumerators, cost far more tree than the file, so a table's data is blanked
too. A file is read as such a parser when its text before its first brace includes `tree_sitter/parser.h` and defines
`LANGUAGE_VERSION`. The parser reads a repaired copy of the file, the same length as the original, while every name and
signature comes from the original bytes.
`design-docs/adr/3.3-repair-what-the-c-plus-plus-grammar-cannot-parse.md` records what each repair was measured to
recover, the grammar gap left open, and why the extensions claimed are the ones they are;
`design-docs/adr/3.31-parse-a-c-or-c-plus-plus-table-without-its-numbers.md` records the tables.

Status: the API is new and is not published yet.
