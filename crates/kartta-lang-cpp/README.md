# kartta-lang-cpp

C and C++ for the kartta code graph: definitions, calls and includes extracted through tree-sitter.

`Cpp` implements the `kartta_lang::Language` contract over the tree-sitter C++ grammar, which reads C as well. It claims
`.cpp`, `.cc`, `.cxx`, `.h`, `.hpp`, `.hxx`, `.hh`, `.c`, `.inl` and `.inc` files, in any letter case.

## What a file yields

From the bytes alone: functions and methods, classes, structs, unions and enums, type aliases, class data members, and
namespace-scope variables and constants, each qualified the way C++ names it — `ns::f` for a free function in a
namespace, `Widget::area` for a member. A member call on `this` carries `this` as its receiver and one on any other
operand the empty receiver, because a value's name names no type. Callables carry an identity of scope, parameter types
and member qualifiers wherever the walk can establish one without guessing, so a declaration and its definition meet in
the graph and two overloads stay apart. Function-like macros, a test harness's `TEST(...)` blocks and trailing
annotation macros included, are never mistaken for definitions.

From the path and the repository: each `#include` resolved to the file of the repository it names — the path relative to
the including file, the exact path, the one file whose path ends in it — with no compiler search path, or kept as an
unresolved import when no file matches or two match equally well; and test or generated code marked by the file's name.

## Parse repairs

Two constructs the grammar cannot reduce — an empty trailing macro argument, `CHECK(ptr, )`, and a function-like macro
where a declaration belongs — take the declarations around them down with them. The parser reads a repaired copy of the
file, the same length as the original, while every name and signature comes from the original bytes.
`design-docs/adr/3.3-repair-what-the-c-plus-plus-grammar-cannot-parse.md` records what each repair was measured to
recover, the grammar gap left open, and why the extensions claimed are the ones they are.

Status: the API is new and is not published yet.
