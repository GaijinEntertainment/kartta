//! Lua for the kartta code graph: the functions and table methods of Lua source, the calls between them, and the
//! modules a file loads.
//!
//! [`Lua`] implements [`Language`] over the `tree-sitter-lua` grammar. It claims every file whose extension is `lua`,
//! in any case.
//!
//! # What a file extracts to
//!
//! A named function is a symbol: a declaration, `function f`, `local function f` or `function M.f` and `function M:f`,
//! and a function bound to a name, `M.f = function() end`, `local f = function() end`, or a field of a table bound to
//! a name, `local T = { f = function() end }`, whose owner is the variable in the table's own place. A function on a
//! table is a method of the table, `M.f`. A function declared or bound with `local`, `local function f` or
//! `local f = function() end`, is not exported; a method stays exported when its table is local, because a module
//! hands its local table out by `return M`. Every function opens a scope, so a call inside it is a call made by it; a
//! value beside a bound function in one assignment, `mk` in `f, g = mk(), function() end`, is not.
//!
//! A call records its callee and, through `.` or `:`, its receiver: the table when it is a plain name, `M` in
//! `M.build()` and `self` in `self:render()`, else the empty receiver. A call outside every function is a call from
//! file scope, before the file's first function too.
//!
//! A name read where no call takes it is a use of it: an argument, `table.sort(t, compare)`, a value an assignment, a
//! `return` or a loop takes, a field's value, `{ on_click = handler }`, an operand and a condition; an assignment's
//! target, a callee and a table indexed through `.` or `:` are no reads. A parameter hides its name in its function,
//! as `self` does in a method declared with `:` and a local function its own name, a local from the end of its
//! declaration to the end of its block, the chunk too, and a loop variable inside its loop; outside every function, a
//! local function, a local bound to a function and a local declared without a value, which `function f()` binds later,
//! hide nothing. A use binds the function of its name its own file declares outside every function.
//!
//! # Resolution
//!
//! A bare `require`, `dofile` or `loadfile` whose first argument is a string literal loads a module and records no
//! call. `require "a.b"` includes `a/b.lua` or the package `a/b/init.lua`, from the root and then from the requiring
//! file's directory; `dofile "x.lua"` and `loadfile` include the path as written, from the file's directory and then
//! from the root. A module the repository does not hold, such as one of the standard library, includes nothing. A
//! loader with any other argument is an ordinary call.
//!
//! A file named `*_spec.lua` or `*_test.lua`, or under a `spec`, `test` or `tests` directory in any case, is test code.
//! A file named `*.gen.lua` or `*_pb.lua`, or whose leading comments carry `@generated` or `DO NOT EDIT`, is generated
//! code. No symbol is an entry point.
//!
//! # Fingerprint
//!
//! [`Lua::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_lua::Lua;
//!
//! let lua = Lua::new();
//! assert!(lua.claims("lua/plugin/init.lua"));
//!
//! let raw = lua.extract(b"local M = {}\nfunction M.build(x)\n  return helper(x)\nend\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["M.build"]);
//! assert_eq!(raw.calls[0].callee, "helper");
//! ```

use kartta_lang::path::has_extension;
use kartta_lang::{
    BlockComment, Conventions, DocComments, Language, OwnMembers, RawExtraction, Repository,
    Resolver,
};

use crate::resolve::LuaResolver;

mod banner;
mod resolve;
mod walk;

/// The Lua language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Lua;

impl Lua {
    /// The Lua language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

// constraint: a long comment closes at a bracket of its own level, so each level is a comment of its own; a level past
// the third is not read as doc
const DOC_COMMENTS: DocComments = DocComments::lines(&["--"]).with_blocks(&[
    BlockComment::new("--[[", "]]").closed_alone(),
    BlockComment::new("--[=[", "]=]").closed_alone(),
    BlockComment::new("--[==[", "]==]").closed_alone(),
    BlockComment::new("--[===[", "]===]").closed_alone(),
]);

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::Unreached;
    conventions.includers = Some(
        "the files whose bare require, dofile or loadfile of a string literal loads this file",
    );
    conventions
};

impl Language for Lua {
    fn name(&self) -> &'static str {
        "lua"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["lua"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_lua::NODE_TYPES)
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

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(LuaResolver)
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_lua::LANGUAGE.into()
}
