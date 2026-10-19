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
//!   `A.Worker.run`. A body never owns members, so a class declared inside a function starts a new chain. A class with
//!   no name goes by the name that binds it, as JavaScript names it: `default` for an anonymous `export default class`,
//!   `Box` for `const Box = class {}`; the methods of one that nothing names are free functions. A type alias owns the
//!   members its type spells, as an interface does: `open` in `type Api = { open(): void }` is `Api.open`.
//! - A function bound to a name is a function that owns the calls in its body: `const f = () => {}`, a function
//!   wrapped in `memo`, `forwardRef`, `useCallback`, `useMemo`, `observer` or `debounce`, an object property
//!   `{ onClick: () => {} }`, a class field `handler = () => {}`, and an anonymous `export default` function, named
//!   `default`. An anonymous callback such as an inline JSX prop opens no scope: its calls belong to the function
//!   around it.
//! - A test registration — `describe`, `it` and `test` with their `x` and `f` forms, `suite`, `bench`, and their
//!   `.skip`, `.only`, `.todo`, `.concurrent`, `.sequential` and `.failing` modifiers — is a function named by its
//!   description, and a lifecycle hook one named by the hook. A registration whose description is not a plain string
//!   opens no scope.
//! - Calls, with a capitalised JSX tag as a call of the component and `new Foo()` as a call of `Foo`. A call on
//!   `this` names the type that owns the enclosing method as its receiver. A call on a value the file declares before
//!   it, inside the scope that declares it — a parameter, a `let`, `const` or `var`, a destructured name, a catch
//!   binding, a loop variable, a function declared inside a function body, the name of a function expression inside
//!   it — records the empty receiver, because a value's name names no type; an import binding, a class, a namespace
//!   and a class declared in a nearer scope keep their name.
//! - Data sites inside a definition, each a read, a write (`=`, a key of an object literal whose type is written
//!   beside it) or an update (`+=`, `++`): a member taken without a call, on `this` as a member of the class around
//!   the method, on a value whose declaration spells one type, `p: Props` or `new Pool()`, as a member of that type,
//!   on a class or an import by its name, and on anything else as a member of a value of no known type, which binds
//!   nothing; and a use of a module-level variable or constant the file imports or declares, or of a function or
//!   class it declares at its top level taken as a value, before its declaration too, which binds the declaration in
//!   the file, in the one module it imports that declares it, or through a re-export.
//! - Overload signatures joined to the implementation that follows them, as one callable.
//! - Imports, `require` and dynamic `import()` of a string, and `export … from` re-exports, a type-only one included.
//!
//! Export assignments, `module.exports = …` and `exports.f = …`, are not modelled: a function that exists only as the
//! right side of one yields no symbol. No symbol is an entry point.
//!
//! A file is test code when its name holds `.test.` or `.spec.`, or a directory on its path is `__tests__` or
//! `tests`; a directory named `test` is not enough. A file is generated code when a `@generated` or `DO NOT EDIT`
//! banner, outside a backtick span, stands in the comments before its first line of code, when that first line of
//! code runs past 500 bytes, as a minified bundle's does, or when its name holds `.gen.` or `.generated.` or its stem
//! ends in `_pb`, `.pb` or `.min`. A hand-written `.d.ts` file stays ordinary. A file under a `node_modules` or
//! `vendor` directory is vendored code.
//!
//! # How imports resolve
//!
//! The resolver reads every `tsconfig.json` and `package.json` of the repository, except those under a
//! `node_modules` or `vendor` directory, when it is built, and a `tsconfig*.json` that one of them extends or
//! references when it compiles that one. A specifier then resolves to a file of the repository:
//!
//! - `./m` and `../m` probe the specifier as written, with each script extension, and as a directory with an
//!   `index` file; a miss retries an ESM `./m.js` as the TypeScript source beside it.
//! - `#m` takes the `imports` of the nearest `package.json` above the importing file.
//! - Any other specifier takes the `paths` of the nearest `tsconfig.json` above the importing file, with its
//!   `baseUrl` and one `extends` hop applied, or those of the sibling configs a solution-style config references.
//!   Failing that, a specifier that names a package of the repository takes its `exports`, its `module` or `main`,
//!   or the file under its directory. Two packages of one name resolve to the one nearer the importing file, and
//!   two equally near resolve to neither.
//! - A specifier with a `?query`, as Vite writes them, resolves without the query when it does not resolve with it.
//!
//! A resolved import is an include of its file; one that names no file of the repository, such as an external
//! package, is kept as an import as written. A re-export names the file its specifier resolves to by the same rules,
//! or none.
//!
//! # Fingerprint
//!
//! Each language's fingerprint holds this crate's name, version and a digest of its sources, and its grammar's
//! declared version, node types, ABI version and counts of node kinds, parse states and fields. A change in
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a change in the
//! tree-sitter runtime, or in a grammar that keeps all of those equal.
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
    Conventions, DocComments, Language, OwnMembers, RawExtraction, Repository, Resolver,
    UntypedMembers, ValueUses,
};

use crate::flavor::Flavor;
use crate::resolve::JstsResolver;

const FAMILY: &str = "jsts";

fn conventions() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.owner_chain_separator = Some(".");
    conventions.bare_call_reaches_enclosing_owners = true;
    conventions.runtime_methods = runtime::METHODS;
    conventions.value_uses = ValueUses::Imported;
    conventions.untyped_members = UntypedMembers::Unbound;
    conventions
}

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
                conventions()
            }

            fn doc_comments(&self) -> DocComments {
                DocComments::C_LIKE
            }

            fn extract(&self, source: &[u8]) -> RawExtraction {
                $flavor.extract(source)
            }

            fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
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
