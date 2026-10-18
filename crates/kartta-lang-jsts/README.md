# kartta-lang-jsts

TypeScript, TSX and JavaScript for the kartta code graph.

The crate implements `kartta_lang::Language` three times — `TypeScript`, `Tsx` and `JavaScript` — one per tree-sitter
grammar. The three share one walk and one family, `jsts`, so the `.tsx` components of a frontend and the `.ts` modules
they import resolve against each other in one graph.

## What a file yields

Functions, classes, interfaces, type aliases, enums and namespaces, each member qualified by the chain of names around
it. A function bound to a name — an arrow in a `const`, a function wrapped in `memo` or `useCallback`, an object
property, a class field — owns the calls in its body, and so does a `describe`, `it` or `test` registration, named by
its description. A capitalised JSX tag is a call of its component, and `new Foo()` a call of `Foo`. A call on `this`
names the class around the method as its receiver, and a call on a value the file declares before it, a parameter or a
variable, names none inside the scope that declares it, because a value's name names no type; a `var` counts for the
block that declares it only, so a call on it after that block keeps the name. CommonJS export assignments are not
modelled.

Inside a definition, each read, write and update of a member or a module-level value is a data site. A member on
`this` binds the class around the method, or a base found without a guess; on a value whose declaration spells one
type, a parameter `p: Props`, a `const cfg: Config` or a `new Pool()`, a member of that type; on a class or an import
by its name. A key of an object literal writes a field where the literal's type is written beside it, as an annotation,
`as` or `satisfies`. A member of a value of no known type is recorded and binds nothing, because a value built from a
literal or an inline type makes the file's one field of the name a namesake. A use of a variable or a constant the file
imports or declares at module level, before its declaration too, binds that declaration, the one imported module that
declares it, or a re-export of it; a module-level function or class the file declares, taken as a value, is no use.

The conventions name the methods that ECMAScript's built-in objects and the DOM give a value, so a member call of one,
`line.trim()` or `node.addEventListener()`, binds a member of the name and no free function of the repository.

## How imports resolve

The resolver reads the repository's `tsconfig.json` and `package.json` files, outside `node_modules` and `vendor`, and
resolves a specifier the way the TypeScript compiler and bundlers do: relative paths with their extensions and `index`
files, the ESM `./m.js` spelling of a `.ts` source, `tsconfig` `paths`, `package.json` `imports` and the `exports` of
the repository's own packages, and Vite's `?query` suffixes. A re-export names the file its specifier resolves to.

Status: the API is new and is not published yet.
