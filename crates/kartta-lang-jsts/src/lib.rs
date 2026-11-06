//! TypeScript, TSX and JavaScript for the kartta code graph.
//!
//! Three [`Language`] values, [`TypeScript`], [`Tsx`] and [`JavaScript`], each parse their files with their own
//! tree-sitter grammar and share one walk and one family, `jsts`, so a `.tsx` component and the `.ts` module it
//! imports resolve against each other in one graph.
//!
//! # What a file yields
//!
//! - Functions, classes, interfaces, type aliases, enums, namespaces and ambient `declare module "m"` blocks, a member
//!   owned by the chain of names that encloses it: `run` in `namespace A { class Worker { run() {} } }` is
//!   `A.Worker.run`, and a variable or a constant a namespace declares is its member too: `limit` in
//!   `namespace A { export const limit = 1 }` is `A.limit`. A body never owns members, a class's static block
//!   included, so a class declared inside a function starts a new chain. A class with no name goes by the name that
//!   binds it, as JavaScript names it: `default` for an anonymous `export default class`, `Box` for
//!   `const Box = class {}`, `A.Box` inside `namespace A`, and flat, as an object key is, `Box` for `ns.Box = class {}`
//!   and `{ Box: class {} }`; the methods of one that nothing names are free functions. A type alias owns the members
//!   its type spells, as an interface does: `open` in `type Api = { open(): void }` is `Api.open`.
//! - A function bound to a name is a function that owns the calls in its body: `const f = () => {}`, a function
//!   wrapped in `memo`, `forwardRef`, `useCallback`, `useMemo`, `observer` or `debounce`, an object property
//!   `{ onClick: () => {} }`, named by its key and owned by nothing, a class field `handler = () => {}`, a method of
//!   the class, and an anonymous `export default` function, named
//!   `default`. An anonymous callback such as an inline JSX prop opens no scope: its calls belong to the function
//!   around it. A declared class, an anonymous `export default class` included, owns the calls of its body outside
//!   every such function, a field's initializer, a decorator and a static block, and `this` there names the class; a
//!   class expression, `const Box = class {}`, `ns.Box = class {}` or `{ Box: class {} }`, has them made by the name
//!   that binds it, and `this` there names the class too.
//! - A test registration — `describe`, `it` and `test` with their `x` and `f` forms, `suite`, `bench`, and their
//!   `.skip`, `.only`, `.todo`, `.concurrent`, `.sequential` and `.failing` modifiers — is a function named by its
//!   description, and a bare lifecycle hook, `beforeEach`, `afterEach`, `beforeAll` or `afterAll`, one named by the
//!   hook. Either name is a label, so no call or use of the name binds the registration. A registration whose
//!   description is not a plain string opens no scope.
//! - Calls, with a capitalised JSX tag as a call of the component and `new Foo()` as a call of `Foo`. A bare call of a
//!   name a parameter holds binds nothing and counts as a site, unless the file, not a minified one, defines a function
//!   or a class of the name that no type or namespace owns, other than the definition that makes the call, which is
//!   most often the value it passes. A call on
//!   `this` names the class as its receiver in a method, a function a class field holds and a class body outside
//!   them, an instance field's initializer and a static one's alike, and an arrow function keeps the `this` of the
//!   scope it is written in. Any other function, a function expression or a declaration inside a method, an object
//!   literal's method or a function outside every class, gives a `this` of no known type, and a call on it records
//!   the empty receiver. A call on a value the file declares before
//!   it, inside the scope that declares it — a parameter, a `let`, `const` or `var`, a destructured name, a catch
//!   binding, a loop variable, a function declared inside a function body, the name of a function expression inside
//!   it — records the empty receiver, because a value's name names no type, unless it is a parameter written as one
//!   name whose annotation or, with no annotation, whose function's context spells its one type, which it records, or
//!   a variable written as one name whose annotation or `new` spells it, which it records, or that a call initializes,
//!   which records the call as a path the graph follows through return types
//!   ([`Receivers::ReturnPaths`]); an import binding, a class, a namespace and a class declared in a nearer scope keep
//!   their name.
//! - A variable or a constant a function body declares by one name, unless a function or a class is its value, is
//!   local to that body: no use binds it, a call of its name inside the scope that declares it calls it, and a read or
//!   a call of a namesake the module declares never binds it.
//! - Data sites inside a definition, each a read, a write (`=`, a key of an object literal whose type is written
//!   beside it) or an update (`+=`, `++`): a member taken without a call, on `this` as a member of the class around
//!   the method, on a value whose declaration spells one type, `p: Props` or `new Pool()`, as a member of that type,
//!   on a class or an import by its name, and on anything else as a member of a value of no known type, which binds
//!   nothing; and a use of a module-level variable or constant the file imports or declares, or of a function or
//!   class it declares at its top level taken as a value, before its declaration too, which binds the declaration in
//!   the file, in the one module it imports that declares it, or through a re-export. A `var` that a block outside
//!   every body declares, `if (ready) { var root = window; }`, is a module-level variable, and so is a function such
//!   a block declares in a script that is not strict code, one with no `import`, no `export` and no `"use strict"`,
//!   as ECMAScript's Annex B.3.3 scopes it; a `let`, a `const` and a class stay their block's. Outside every
//!   definition, at file scope, the walk records the same uses of those names, and no member.
//! - Overload signatures joined to the implementation that follows them, as one callable.
//! - Imports, `require` and dynamic `import()` of a string, and `export … from` re-exports, a type-only one included.
//!
//! Export assignments, `module.exports = …` and `exports.f = …`, are not modelled: a function that exists only as the
//! right side of one yields no symbol. No symbol is an entry point.
//!
//! A file is test code when its name holds `.test.` or `.spec.`, or a directory on its path is `__tests__` or
//! `tests` in any case; a directory named `test` is not enough. A file is generated code, its text read after a UTF-8
//! byte order mark, when a `@generated` or `DO NOT EDIT` banner, outside a backtick span, stands in the comments before
//! its first line of code, when that first line of code runs past 500 bytes, as a minified bundle's does, or when its
//! name holds `.gen.` or `.generated.` or its stem ends in `_pb`, `.pb` or `.min`. A hand-written `.d.ts` file stays
//! ordinary. A file under a `node_modules` or `vendor` directory is vendored code.
//!
//! # How imports resolve
//!
//! The resolver reads every `tsconfig.json`, `jsconfig.json` and `package.json` of the repository, except those under
//! a `node_modules` or `vendor` directory, when it is built, and a `tsconfig*.json` or `jsconfig*.json` that one of
//! them extends or references when it compiles that one. A specifier then resolves to a file of the repository:
//!
//! - `./m` and `../m` probe the specifier as written, with each script extension, and as a directory with an
//!   `index` file; a miss retries an ESM `./m.js` as the TypeScript source beside it.
//! - `#m` takes the `imports` of the nearest `package.json` above the importing file.
//! - Any other specifier takes the `paths` of the nearest `tsconfig.json` or `jsconfig.json` above the importing
//!   file, the `tsconfig.json` first where a directory holds both, with its `baseUrl` and one `extends` hop applied,
//!   or those of the sibling configs a solution-style config references. A root-relative `/scripts/m.js` takes the
//!   `paths` alone, so a `"/*": ["./*"]` pattern roots it at the config's directory, the one a web server serves.
//!   Failing that, a specifier that names a package of the repository takes its `exports`, its `module` or `main`,
//!   or the file under its directory. Two packages of one name resolve to the one nearer the importing file, and
//!   two equally near resolve to neither.
//! - A specifier with a `?query`, as Vite writes them, resolves without the query when it does not resolve with it.
//!
//! A resolved import is an include of its file; one that names no file of the repository, such as an external
//! package, is kept as an import as written. A re-export names the file its specifier resolves to by the same rules,
//! or none. Each name that an `import` of a bare specifier binds, or a declarator that takes a `require` of one,
//! `const fs = require("fs")` and `const { readFileSync } = require("fs")`, is an import binding outside the
//! repository where the specifier resolves to no file, so a member call through it, `<Dialog.Footer>` with `Dialog`
//! from `@chakra-ui/react`, and a call of the name itself, `useState()` with `useState` from `react`, bind no
//! definition of the repository outside the caller's file; a specifier that starts with `.`, `/` or `#` names a file
//! of the repository whether it resolves or not.
//!
//! The resolver keeps only what it derived: the aliases of each `tsconfig.json` and `jsconfig.json`, the `imports` of
//! each `package.json`, and each package's name, directory, `exports` and entry. A resolver built again over an edit
//! that changes none of them, such as a `version` or `dependencies` bump in a `package.json`,
//! [resolves as](kartta_lang::Resolver::resolves_as) the one built before, so an engine that follows the tree resolves
//! no script again for it.
//!
//! # Fingerprint
//!
//! Each language's fingerprint holds this crate's name, version and a digest of its sources, and its grammar's
//! declared version, node types, ABI version and counts of node kinds, parse states and fields. A change in
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::{Language, SymbolKind};
//! use kartta_lang_jsts::TypeScript;
//!
//! let raw = TypeScript::new().extract(b"export const greet = (name: string) => hello(name);\n");
//!
//! let greet = &raw.symbols[0];
//! assert_eq!((greet.name.as_str(), greet.kind, greet.exported), ("greet", SymbolKind::Function, true));
//! assert_eq!(raw.calls[0].callee, "hello");
//! ```

mod config;
mod flavor;
mod generated;
mod modules;
mod package_json;
mod resolve;
mod runtime;
mod syntax;
#[cfg(test)]
mod tests;
mod tsconfig;
mod walk;

use kartta_lang::{
    CallsApart, Conventions, DocComments, IncludedRuntimeMembers, Language, MemberCalls,
    OwnMembers, RawExtraction, Receivers, Repository, Resolver, TypeUses, UntypedMembers,
    ValueUses,
};

use crate::flavor::Flavor;
use crate::resolve::JstsResolver;

const FAMILY: &str = "jsts";

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::Unreached;
    conventions.owner_chain_separator = Some(".");
    conventions.bare_call_reaches_enclosing_owners = true;
    conventions.member_calls = MemberCalls::NearbyFreeFunctionsUnlessTyped;
    conventions.receivers = Receivers::ReturnPaths;
    conventions.calls_apart = CallsApart::Marked;
    conventions.runtime_methods = runtime::METHODS;
    conventions.included_runtime_members = IncludedRuntimeMembers::SpelledOrOwn;
    conventions.value_uses = ValueUses::Imported;
    conventions.untyped_members = UntypedMembers::Unbound;
    conventions.type_uses = TypeUses::Nowhere;
    conventions.includers = Some(
        "the files whose import, export ... from, require or import() of a string resolves to this file",
    );
    conventions
};

macro_rules! flavor_language {
    ($(#[$doc:meta])* $language:ident => $flavor:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub struct $language;

        impl $language {
            /// The language.
            #[must_use]
            pub const fn new() -> Self {
                Self
            }
        }

        impl Language for $language {
            fn name(&self) -> &str {
                $flavor.name()
            }

            fn family(&self) -> &str {
                FAMILY
            }

            fn claims(&self, path: &str) -> bool {
                $flavor.claims(path)
            }

            fn fingerprint(&self) -> &[u8] {
                $flavor.fingerprint()
            }

            fn conventions(&self) -> Conventions {
                CONVENTIONS
            }

            fn doc_comments(&self) -> DocComments {
                DocComments::C_LIKE
            }

            fn extract(&self, source: &[u8]) -> RawExtraction {
                $flavor.extract(source)
            }

            fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver> {
                Box::new(JstsResolver::new(repository))
            }
        }
    };
}

flavor_language! {
    /// TypeScript, named `typescript`: the files ending in `.ts`, `.mts` or `.cts`, in any case.
    TypeScript => Flavor::TypeScript
}

flavor_language! {
    /// TypeScript with JSX, named `tsx`: the files ending in `.tsx`, in any case.
    Tsx => Flavor::Tsx
}

flavor_language! {
    /// JavaScript, named `javascript`: the files ending in `.js`, `.jsx`, `.mjs` or `.cjs`, in any case.
    JavaScript => Flavor::JavaScript
}
