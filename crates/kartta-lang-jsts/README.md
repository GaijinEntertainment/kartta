# kartta-lang-jsts

TypeScript, TSX and JavaScript for the kartta code graph.

The crate implements `kartta_lang::Language` three times — `TypeScript`, `Tsx` and `JavaScript` — one per tree-sitter
grammar. The three share one walk and one family, `jsts`, so the `.tsx` components of a frontend and the `.ts` modules
they import resolve against each other in one graph.

## What a file yields

Functions, classes, interfaces, type aliases, enums and namespaces, each member qualified by the chain of names around
it, a variable or a constant a namespace declares included. A function bound to a name — an arrow in a `const`, a
function wrapped in `memo` or `useCallback`, an object property, a class field — owns the calls in its body, and so does
a `describe`, `it` or `test` registration, named by its description, and a bare lifecycle hook such as `beforeEach`,
named by the hook; no call or use of either name binds them. A declared
class owns the calls of its body outside those functions, in a field's initializer, a decorator or a static block, where
`this` names the class, and a class expression's body makes them as the name that binds it, `Box` in
`const Box = class {}`. A capitalised JSX tag is a call of its component, and `new Foo()` a call of `Foo`. A call on
`this` names the class as its receiver in a method, a function a class field holds and a class body outside them, and
an arrow function keeps the `this` of the scope it is written in; any other function gives a `this` of no known type, so
a call on it names none. A call on a value the file declares before it, a parameter
or a variable, names none inside the scope that declares it, because a value's name names no type, unless it is a
parameter written as one name whose annotation or, with no annotation, whose function's context, an annotation the file
writes, spells its one type, a type parameter aside, or a variable written as one name whose annotation or `new` spells
it, or that a call initializes, which names the type the call's return annotation spells, where the call's definition
has one; as a receiver a
`var` counts for the block that declares it only, so a call on it after that block keeps the name, though a use of it is
a use of a module-level value as below. A variable or a constant a function body declares by one name, unless a function
or a class is its value, is local to that body: no use binds it, a call of its name inside the scope that declares it
calls it, and a read or a call of a namesake the module declares never binds it. CommonJS export assignments are not
modelled.

A class records the base its `extends` names, in JavaScript as in TypeScript: `Base` for `extends Base` and for
`extends ns.Base`. An expression that names no class, `mixin(Base)` or `null`, records nothing, and a class expression a
declarator binds records no base.

Inside a definition, each read, write and update of a member or a module-level value is a data site. A member on
`this` binds the class around the method, or a base found without a guess; on a value whose declaration spells one
type, a parameter `p: Props`, a `const cfg: Config` or a `new Pool()`, or a parameter its context types, a member of
that type; on a class or an import by its name. A key of an object literal writes a field where the literal's type is
written beside it, as an annotation,
`as` or `satisfies`. A member of a value of no known type is recorded and binds nothing, because a value built from a
literal or an inline type makes the file's one field of the name a namesake. A use of a variable or a constant the file
imports or declares at module level, before its declaration too, binds that declaration, the one imported module that
declares it, or a re-export of it, and so does a function or a class the file declares at its top level, taken as a
value. A `var` that a block outside every body declares is a module-level value, as ECMAScript scopes it, and so is a
function such a block declares in a script that is not strict code, one with no `import`, no `export` and no
`"use strict"`. Outside every definition, at file scope, a use of those names is recorded too, keeping its name alone,
and a member is not.

The conventions name the methods that ECMAScript's built-in objects and the DOM give a value, so a member call of one,
`line.trim()` or `node.addEventListener()`, binds a member of the name and no free function of the repository.

## How imports resolve

The resolver reads the repository's `tsconfig.json`, `jsconfig.json` and `package.json` files, outside `node_modules`
and `vendor`, and resolves a specifier the way the TypeScript compiler and bundlers do: relative paths with their
extensions and `index` files, the ESM `./m.js` spelling of a `.ts` source, `paths` and `baseUrl`, a root-relative
`/scripts/m.js` that a config's `paths` roots at the directory a web server serves, `package.json` `imports` and
the `exports` of the repository's own packages, Vue CLI's `@` alias, and Vite's `?query` suffixes. A re-export names
the file its specifier resolves to. A name that an `import` of a package outside the repository binds holds that
package's code, so a member call through it, `<Dialog.Footer>` with `Dialog` from `@chakra-ui/react` or
`path.extname()` with `path` from `node:path`, and a call of the name itself, `useState()` with `useState` from
`react`, bind no definition of the repository outside the caller's file; a specifier that starts with `.`, `/` or `#`
names a file of the repository whether it resolves or not.
