# kartta-lang-jsts

TypeScript, TSX and JavaScript for the kartta code graph.

The crate implements `kartta_lang::Language` three times — `TypeScript`, `Tsx` and `JavaScript` — one per tree-sitter
grammar. The three share one walk and one family, `jsts`, so the `.tsx` components of a frontend and the `.ts` modules
they import resolve against each other in one graph.

## What a file yields

Functions, classes, interfaces, type aliases, enums and namespaces, each member qualified by the chain of names around
it. A function bound to a name — an arrow in a `const`, a function wrapped in `memo` or `useCallback`, an object
property, a class field — owns the calls in its body, and so does a `describe`, `it` or `test` registration, named by
its description. A capitalised JSX tag is a call of its component, and `new Foo()` a call of `Foo`. CommonJS export
assignments are not modelled.

## How imports resolve

The resolver reads the repository's `tsconfig.json` and `package.json` files, outside `node_modules` and `vendor`, and
resolves a specifier the way the TypeScript compiler and bundlers do: relative paths with their extensions and `index`
files, the ESM `./m.js` spelling of a `.ts` source, `tsconfig` `paths`, `package.json` `imports` and the `exports` of
the repository's own packages, and Vite's `?query` suffixes. A re-export names the file its specifier resolves to.

Status: the API is new and is not published yet.
