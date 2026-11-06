//! Daslang for the kartta code graph: the definitions, calls and requires of daslang source.
//!
//! [`Daslang`] implements [`Language`] over the `tree-sitter-daslang` grammar. It claims every file whose extension is
//! `das`, in any case.
//!
//! # What a file extracts to
//!
//! The grammar reads only the second-generation syntax, which a file opts into with a top-level `options gen2`. A file
//! without it, or with an explicit `options gen2 = false` anywhere, extracts to nothing: its first-generation syntax
//! would parse to errors. The option is read from the tree, so an `options gen2` in a comment or a string does not
//! count.
//!
//! Every `def`, `struct`, `class`, field, `enum`, `bitfield`, `variant`, `tuple`, `typedef` and global `let` or `var`
//! is a symbol, qualified by the file's `module` clause: `ui.draw`, and `ui.Button.press` for a member of `Button`. The
//! clause itself is a namespace symbol and the file's package; `module $`, the compiler's builtin module, is neither.
//! A `def` inside a type is a method of the type. A `struct` or `class` names its `: Parent` as its parent. An `enum`
//! or a `bitfield` holds one variant per entry, a `variant` one variant per alternative, and a `tuple` one field per
//! named entry. A global `let` is a constant and a global `var` a variable. A declaration marked `private` is not
//! exported. A function's signature is its header, without the `=>` of an expression body.
//!
//! A call records its callee and, for a member call `obj.m()` or `obj->m()`, its receiver: the object when it is a bare
//! name, and the empty receiver otherwise. A module-scoped call `mod::f()` keeps `mod` as its receiver. A construction
//! `T(...)` or `new T(...)` is a call of `T`; `default<T>`, a `new` over a container and a macro splice in call
//! position record nothing. A block applied to a field, `obj.handler { ... }` or `obj.handler <| ${ ... }`, is a call
//! of `handler` on `obj`.
//!
//! A call also counts the arguments it passes, and a `def` the parameters it takes, so a call binds no `def` that takes
//! fewer: a native overload of a script function's name passes what the script function cannot take. A value a pipe or
//! a trailing block gives a call counts, a member call's object counts, and a `def` under `[class_method]` or
//! `[explicit_const_class_method]` takes the `self` the macro gives it. A construction counts nothing, so it binds by
//! its name alone, and a count stops at `u8::MAX`.
//!
//! A call of `query` that passes a block, written or piped and through a module or not, binds no `def`: the entity
//! system's native `query` or the `call_macro` of `daslib/decs_boost` takes it, so it records a macro invocation.
//!
//! A block applied in place — the argument of a call or a pipe — runs in its enclosing `def`, so its calls are that
//! `def`'s. A block or a local function stored in a name runs when the value is called, so its calls belong to no
//! symbol, and the graph drops them.
//!
//! Code that a macro takes as data runs where its result is compiled, not where it is written, so a call or a
//! construction in it is no call of the enclosing `def`: `quote(foo())` calls `quote` and uses `foo`, which the
//! compiled code names. That code is the argument of `quote`, `qmacro`, `qmacro_block`, `qmacro_block_to_array`,
//! `qmacro_expr` and `qmacro_type`; the argument after the name of `qmacro_function` and `qmacro_variable`, or
//! after the name and the class of `qmacro_method`; and the pattern of `qmatch`, `qmatch_block` and
//! `qmatch_function`, whose matched expression runs. A block after the call is its last argument,
//! `qmacro_block() { ... }`. The expression of a `qmacro` splice, `$e`, `$i`, `$v`, `$b`, `$a`, `$t`, `$c` or
//! `$f`, runs, so its calls belong to the enclosing `def`; the arguments of a `$c(name)(...)` splice are data.
//! `_::f`, the compiler's name for the scope that compiles the code, names what a bare `f` does. A macro called
//! through a module, `shell::quote(path)`, is an ordinary call.
//!
//! A function's address, `@@f`, is a bare use of `f` and `@@mod::f` one qualified by `mod`, the one form daslang takes
//! a function as a value in. A bare use binds the one function of its name its own file declares, else the one a file
//! it requires exports, else the one such a file declares `private`, as a call of the name binds it. A name that binds
//! none of those binds the one definition the files requiring its file bind it to, because quoted code compiles in the
//! file that expands the macro, which is the one that requires what the code names.
//!
//! # Resolution
//!
//! A `require` names a module, which the compiler finds by the `module` name a file declares; a module almost always
//! declares its own file stem, so the resolver reads the repository's file names as the module index. A specifier that
//! starts with `./` or `../` names the file relative to the importer. Any other specifier, `profile`, `daslib/strings`
//! or `engine.core.mesh`, reads its dots as slashes and names the first file it reaches under the importer's
//! directories, nearest first, the first of them the compiler's default lookup, then the one file anywhere whose path
//! ends with it, then the one file whose path ends with it below its first component, which the compiler reads as a
//! mount point, then the one file named for its last component. A specifier two files could satisfy at one step names
//! neither, and every match is case-sensitive, as the compiler's is. The file a `require` or an `include` names is an
//! include; one that names no file is an import, kept as written. A module no file is named for, most often one
//! compiled into the runtime, lies outside the repository, so a call that a `require` of it qualifies lies outside the
//! graph.
//!
//! Only a `require` or an `include` puts another file's script definition in a file's scope, and `module builtin` is
//! in every file's, so a call with no receiver binds by its name alone only a definition of a file its own file reaches
//! or of that module: the name is otherwise a native function's, of which the tree holds a namesake.
//!
//! A class starts as a copy of its parent's fields, and a `def` is a field, so a member call on a class the graph holds
//! that holds no member of the name binds the one its nearest parent holds. A bare call in a method binds the same
//! member once no global function of the name takes it, as the compiler reads such a call on `self`.
//!
//! A file with a `test` or `tests` directory in its path, in any case, or whose stem ends in `_test` or starts with
//! `test_`, is test code. A file whose name holds `.gen.` or `_gen.`, or whose leading comments carry `@generated`,
//! `DO NOT EDIT` or `Code generated`, is generated code. A `def` that carries an annotation other than one of the
//! compile hints the runtime defines, `[unused_argument]`, `[no_aot]`, `[hint]` and the like, is an entry point: the
//! annotation registers it with what calls it, as `[export]`, `[init]` or an entity system's `[es]` does.
//!
//! # Fingerprint
//!
//! [`Daslang::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to any of them
//! changes the fingerprint. A change to `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys
//! beside it. It cannot see a grammar built from changed sources that keep its version and tables — a patched external
//! scanner — nor a change to the tree-sitter runtime that keeps its version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_daslang::Daslang;
//!
//! let daslang = Daslang::new();
//! assert!(daslang.claims("daslib/strings.das"));
//!
//! let raw = daslang.extract(b"options gen2\nmodule ui\n\ndef draw() {\n}\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["ui", "ui.draw"]);
//! ```

use kartta_lang::path::has_extension;
use kartta_lang::{
    BareReceivers, CallsApart, Conventions, DocComments, InheritedMembers, Language, RawExtraction,
    ReceiverMembers, Repository, Resolver, SymbolKind, TypeUses, ValueUses,
};

use crate::resolve::DaslangResolver;

mod resolve;
mod walk;

/// The daslang language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Daslang;

impl Daslang {
    /// The daslang language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.bare_receivers = BareReceivers::Modules;
    conventions.inherited_members = InheritedMembers::Found;
    conventions.calls_apart = CallsApart::Marked;
    conventions.module_seen_everywhere = Some("builtin");
    conventions.value_uses = ValueUses::Expanded;
    conventions.native_names = &[
        "length", "float2", "float3", "float4", "int2", "int3", "int4", "uint2", "uint3", "uint4",
    ];
    conventions.own_type_receivers = Some(&["self"]);
    conventions.base_receivers = &["super"];
    conventions.receiver_members = ReceiverMembers::Inherited;
    conventions.type_uses = TypeUses::Nowhere;
    conventions.unrecorded_data = &[
        SymbolKind::Field,
        SymbolKind::Variable,
        SymbolKind::Constant,
    ];
    conventions.includers = Some("the files whose require or include names this file");
    conventions
};

impl Language for Daslang {
    fn name(&self) -> &'static str {
        "daslang"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["das"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_daslang::NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::C_LIKE
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(DaslangResolver)
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_daslang::LANGUAGE.into()
}
