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
//! `M.build()` and `self` in `self:render()`, else the empty receiver. A call before the file's first function records
//! nothing.
//!
//! # Resolution
//!
//! A bare `require`, `dofile` or `loadfile` whose first argument is a string literal loads a module and records no
//! call. `require "a.b"` includes `a/b.lua` or the package `a/b/init.lua`, from the root and then from the requiring
//! file's directory; `dofile "x.lua"` and `loadfile` include the path as written, from the file's directory and then
//! from the root. A module the repository does not hold, such as one of the standard library, includes nothing. A
//! loader with any other argument is an ordinary call.
//!
//! A file named `*_spec.lua` or `*_test.lua`, or under a `spec`, `test` or `tests` directory, is test code. A file
//! named `*.gen.lua` or `*_pb.lua`, or whose leading comments carry `@generated` or `DO NOT EDIT`, is generated code.
//! No symbol is an entry point.
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

use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::path::base;
use kartta_lang::{
    BlockComment, Conventions, DocComments, Language, OwnMembers, RawExtraction, Repository,
    Resolver,
};
use tree_sitter::{Parser, Tree};

use crate::fnv::{FNV_OFFSET, fnv};
use crate::resolve::Loaders;

mod banner;
mod fnv;
mod resolve;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_lua::NODE_TYPES.as_bytes());

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

impl Language for Lua {
    fn name(&self) -> &'static str {
        "lua"
    }

    fn claims(&self, path: &str) -> bool {
        base(path)
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("lua"))
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn doc_comments(&self) -> DocComments {
        DOC_COMMENTS
    }

    fn conventions(&self) -> Conventions {
        let mut conventions = Conventions::default();
        conventions.own_members = OwnMembers::Unreached;

        conventions
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        parse(source).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, source))
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(Loaders::new(repository))
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_lua::LANGUAGE.into()
}

fn parse(source: &[u8]) -> Option<Tree> {
    thread_local! {
        static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
    }

    PARSER.with_borrow_mut(|parser| {
        if parser.is_none() {
            let mut fresh = Parser::new();
            fresh.set_language(&grammar()).ok()?;
            *parser = Some(fresh);
        }
        parser.as_mut()?.parse(source, None)
    })
}

fn fingerprint() -> Vec<u8> {
    let grammar = grammar();
    let version = grammar.metadata().map_or([0; 3], |version| {
        [
            version.major_version,
            version.minor_version,
            version.patch_version,
        ]
    });
    let counts = [
        grammar.abi_version(),
        grammar.node_kind_count(),
        grammar.parse_state_count(),
        grammar.field_count(),
    ];

    let mut bytes = Vec::new();
    for part in [env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")] {
        bytes.extend_from_slice(part.as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(&SOURCE_DIGEST);
    bytes.extend_from_slice(&NODE_TYPES_DIGEST.to_le_bytes());
    bytes.extend_from_slice(&version);
    for count in counts {
        bytes.extend_from_slice(&u64::try_from(count).unwrap_or(u64::MAX).to_le_bytes());
    }

    bytes
}
