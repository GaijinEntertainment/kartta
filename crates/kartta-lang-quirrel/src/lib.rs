//! Quirrel for the kartta code graph: the definitions, calls and imports of Quirrel scripts.
//!
//! [`Quirrel`] implements [`Language`] over the `tree-sitter-quirrel` grammar. It claims every file whose extension
//! is `nut`, in any case.
//!
//! # What a file extracts to
//!
//! Every named function, class, enum, enum member and constant is a symbol, and so is a function, a lambda or a class
//! bound to a name. A function value takes the name of its binding, the name its callers use: a table slot, a `let`
//! or `local`, or the target of an `=` or `<-`. A member of a named or bound class is a method of the class, and an
//! assignment to a field access binds a method of the whole receiver expression, `gui.Widget.build` one of
//! `gui.Widget`. Any other function is free; a function value with no binding and no name of its own is no symbol,
//! and its calls and uses belong to the function around it, or to the file at its top level. A data slot with a plain
//! name, `count = 0`, of a class or of a table that a file-scope `let` or `local` binds, is a field of that class or
//! binding, with the slot as its signature when the value is a literal, a name or a unary expression; a computed
//! `[k] = v` or quoted `"k": v` slot is none. `class C(Base)` names `Base` as a parent. A callable's signature is its
//! header, from its declaration or binding to its body.
//!
//! A call records its callee: a bare name, a root-table `::f`, or the field of a member call, whose receiver is the
//! whole receiver expression as written, but for a field of `this`, below. A computed callee, `a[k]()` or `f()()`,
//! records nothing, and neither does a bare call of a parameter or a local of an enclosing function, `cb()` in
//! `function run(cb)`, which calls whatever value the name holds, unless the file defines a free function or a class of
//! the name, which is most often the value it passes. A local that destructures a table, `let { build } = cfg`, takes
//! the slot of its name and keeps its call.
//!
//! A name read where no call takes it, `subscribe("evt", onEvent)`, `{ onClick = handler, handler }` or `return
//! handler`, and that no parameter or local of an enclosing function declares, is a bare use of it; so is a root-table
//! `::name` read the same way. A name being declared, a slot's key, a member's field, an assignment's target and the
//! receiver of a member access are not. A bare use binds the definition of its name its own file declares outside every
//! owner and body, a field of the enclosing class first: a `from` import or a destructured `require` names the module
//! a name comes from, which the graph does not keep, so a name another module exports binds nothing.
//!
//! A `this.f()` starts at the class around the caller, and a call on a name of a class at that class. A member the class
//! does not hold binds where the nearest class above it holds one, since a class copies its base's members and then
//! applies its own body over them. `this` in a function value inside a member reads as the member's class, though the
//! language takes it from whatever calls the value, which the walk does not see.
//!
//! A receiver that starts `this?.` records as `this.`, so `this?.slot.f()` is `this.slot.f()`. A `this.slot.f()`
//! records the name a slot's value is built from as its receiver, when every write the body of the caller's owner
//! makes to the slot, a default or a `this.slot =` in any member, is either `null` or a call of that one bare name, and
//! the file binds the name to anything but a function: `slot = Part()` makes it `Part.f()`. A slot written anything
//! else, or from two names, keeps `this.slot`, and so does one its owner's body never writes, whatever a base, a
//! derived class or another file's code writes there.
//!
//! # Resolution
//!
//! Every `import`, `from ... import`, and `require("spec")` or `require_optional("spec")` with one string literal
//! names a module. A script resolves through the nearest `.sqconfig` in its directory or above it, and that file
//! alone: `%name/rest` names `rest` below the directory the file mounts as `name`, and any other specifier that ends
//! in `.nut` names the file beside the script, else the file below the first base path that holds it. A `.sqconfig`
//! that cannot be read mounts nothing and adds no base path. A script that no `.sqconfig` governs resolves by the
//! specifier's shape: `%root/rest` and any other slashed or `.nut` specifier name the file under the nearest directory
//! of the script that holds them, `./` and `../` name the file relative to the script, and `.nut` is appended when the
//! path as written names none. A bare name is a native module and names no file. Under a `.sqconfig`, a path that names
//! no file takes the one file that differs from it only in ASCII letter case, as the game's vromfs does; a mount's
//! name, a specifier's `.nut` suffix, and every match without a `.sqconfig`, keep letter case. The file a specifier
//! names is an include; a specifier that names no file is an import, kept as written, with the names a `from` import
//! takes.
//!
//! The resolver keeps only what it derived: the mounts and base paths of each `.sqconfig`. A resolver built again over
//! an edit that changes none of them, such as a comment or another key in a `.sqconfig`,
//! [resolves as](kartta_lang::Resolver::resolves_as) the one built before, so an engine that follows the tree resolves
//! no script again for it.
//!
//! A file with a `test`, `tests` or `testData` directory in its path, in any case, the last where the Quirrel compiler
//! keeps its test scripts, or whose stem ends in `_test` or starts with `test_`, is test code. A file whose name holds
//! `.gen.` or `_gen.`, or whose leading comments carry `@generated`, `DO NOT EDIT` or `Code generated`, is generated
//! code.
//!
//! # Fingerprint
//!
//! [`Quirrel::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to any of them
//! changes the fingerprint. A change to `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys
//! beside it. It cannot see a grammar built from changed sources that keep its version and tables — a patched external
//! scanner — nor a change to the tree-sitter runtime that keeps its version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_quirrel::Quirrel;
//!
//! let quirrel = Quirrel::new();
//! assert!(quirrel.claims("ui/hud/main.nut"));
//!
//! let raw = quirrel.extract(b"class Hud {\n  function draw() {}\n}\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["Hud", "Hud.draw"]);
//! ```

use kartta_lang::path::has_extension;
use kartta_lang::{
    BlockComment, Conventions, DocComments, Exports, IncludedRuntimeMembers, Language, OwnMembers,
    RawExtraction, ReceiverMembers, Repository, Resolver, UnlistedReceivers, UntypedOwners,
};

use crate::resolve::QuirrelResolver;

mod resolve;
mod runtime;
mod sqconfig;
mod walk;

/// The Quirrel language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Quirrel;

impl Quirrel {
    /// The Quirrel language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const DOC_COMMENTS: DocComments =
    DocComments::lines(&["//", "#"]).with_blocks(&[BlockComment::new("/*", "*/")]);

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::First;
    conventions.runtime_methods = runtime::METHODS;
    conventions.included_runtime_members = IncludedRuntimeMembers::SpelledOrOwn;
    conventions.untyped_owners = UntypedOwners::Tables;
    conventions.base_receivers = &["base"];
    conventions.receiver_members = ReceiverMembers::InheritedUpToFields;
    conventions.own_type_receivers = Some(&["this"]);
    conventions.unlisted_receivers = UnlistedReceivers::OwnMemberFieldsAsGuess;
    conventions.includers = Some(
        "the scripts whose import, from ... import, require or require_optional of a string literal names this file",
    );
    // constraint: a function, a class, a `let` or a `local` at file scope is local to its file, which hands out only
    // what it returns, yet the walk marks every definition exported
    conventions.exports = Exports::Unrecorded;
    conventions
};

impl Language for Quirrel {
    fn name(&self) -> &'static str {
        "quirrel"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["nut"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_quirrel::NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(QuirrelResolver::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_quirrel::LANGUAGE.into()
}
