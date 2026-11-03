//! What a Lua file extracts to, how its module loads resolve against the repository, and where the doc comment above
//! a definition starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Access, CallRef, Extraction, FILE_SCOPE, Language, MemoryRepository, OwnMembers, RawExtraction,
    Receiver, Symbol, SymbolId, SymbolKind,
};
use kartta_lang_lua::Lua;

const SAMPLE: &[u8] = br#"
local M = {}

function greet(name)
    return "hi " .. name
end

local function helper(v)
    return greet(v)
end

function M.build(x)
    return helper(x)
end

function M:render()
    return self.build(1)
end

M.attach = function(x)
    return helper(x)
end

local hidden = function()
    return greet("h")
end

local T = {
    setup = function()
        return helper(0)
    end,
}

local result = greet("world")
"#;

// One symbol of each form that sets flags: a declaration, a bound function and a table field.
const EVERY_FORM: &[u8] = br"
function run()
    return 1
end

M.f = function()
    return 2
end

local handlers = {
    setup = function()
        return 3
    end,
}
";

fn extract(source: &[u8]) -> RawExtraction {
    Lua::new().extract(source)
}

fn resolve(source: &[u8], path: &str, files: &[&str]) -> Extraction {
    let lua = Lua::new();
    let repository = MemoryRepository::of_paths(files);
    let resolver = lua.resolver(&repository);
    resolver.resolve(lua.extract(source), path, &repository)
}

fn symbol<'r>(symbols: &'r [Symbol], name: &str) -> &'r Symbol {
    symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn id_of(symbols: &[Symbol], name: &str) -> SymbolId {
    let at = symbols
        .iter()
        .position(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    SymbolId(u32::try_from(at).unwrap_or(u32::MAX))
}

fn call<'r>(calls: &'r [CallRef], callee: &str) -> &'r CallRef {
    calls
        .iter()
        .find(|call| call.callee == callee)
        .unwrap_or_else(|| panic!("no call of {callee}"))
}

fn callees(calls: &[CallRef]) -> Vec<&str> {
    calls.iter().map(|call| call.callee.as_str()).collect()
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

fn names(symbols: &[Symbol]) -> Vec<&str> {
    symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect()
}

#[test]
fn named_functions_take_their_owner_into_the_qualified_name() {
    let raw = extract(SAMPLE);

    assert_eq!(
        names(&raw.symbols),
        [
            "greet", "helper", "M.build", "M.render", "M.attach", "hidden", "T.setup"
        ]
    );
    assert_eq!(symbol(&raw.symbols, "greet").kind, SymbolKind::Function);
    assert_eq!(symbol(&raw.symbols, "greet").owner, None);
    assert_eq!(symbol(&raw.symbols, "helper").kind, SymbolKind::Function);
    for method in ["build", "render", "attach", "setup"] {
        assert_eq!(
            symbol(&raw.symbols, method).kind,
            SymbolKind::Method,
            "{method}"
        );
    }
    assert_eq!(symbol(&raw.symbols, "render").owner.as_deref(), Some("M"));
    assert_eq!(symbol(&raw.symbols, "setup").owner.as_deref(), Some("T"));
}

#[test]
fn the_local_keyword_makes_a_function_unexported() {
    let raw = extract(SAMPLE);

    assert!(symbol(&raw.symbols, "greet").exported);
    assert!(!symbol(&raw.symbols, "helper").exported);
    assert!(symbol(&raw.symbols, "build").exported);
    assert!(symbol(&raw.symbols, "attach").exported);
    assert!(!symbol(&raw.symbols, "hidden").exported);
    assert!(symbol(&raw.symbols, "setup").exported);
}

#[test]
fn a_call_is_made_by_the_function_around_it() {
    let raw = extract(SAMPLE);

    let greet = raw
        .calls
        .iter()
        .find(|call| call.callee == "greet")
        .unwrap_or_else(|| panic!("no call of greet"));
    assert_eq!(greet.site_of, id_of(&raw.symbols, "helper"));
    assert_eq!(greet.receiver, None);

    let helper_callers: Vec<SymbolId> = raw
        .calls
        .iter()
        .filter(|call| call.callee == "helper")
        .map(|call| call.site_of)
        .collect();
    for caller in ["build", "attach", "setup"] {
        assert!(
            helper_callers.contains(&id_of(&raw.symbols, caller)),
            "{caller}"
        );
    }
}

#[test]
fn a_call_after_every_function_is_made_from_file_scope() {
    let raw = extract(SAMPLE);

    let last = raw.calls.last().unwrap_or_else(|| panic!("no calls"));
    assert_eq!(last.callee, "greet");
    assert_eq!(last.site_of, FILE_SCOPE);
}

#[test]
fn a_call_before_the_first_function_is_a_call_from_file_scope() {
    let raw = extract(b"setup()\nfunction run() end\nteardown()\n");

    assert_eq!(callees(&raw.calls), ["setup", "teardown"]);
    assert!(raw.calls.iter().all(|call| call.site_of == FILE_SCOPE));
}

#[test]
fn a_call_through_a_table_name_records_it_as_the_receiver() {
    let raw = extract(SAMPLE);

    let build = call(&raw.calls, "build");
    assert_eq!(build.receiver.as_deref(), Some("self"));
    assert_eq!(build.site_of, id_of(&raw.symbols, "render"));
}

#[test]
fn a_chained_call_head_keeps_the_empty_receiver() {
    let resolved = resolve(
        b"local anchor = 1\nfunction use()\n  a.b.c()\n  a.b:m()\n  self:build()\n  a.b.require(\"mod\")\nend\n",
        "a.lua",
        &["mod.lua"],
    );

    assert_eq!(call(&resolved.calls, "c").receiver.as_deref(), Some(""));
    assert_eq!(call(&resolved.calls, "m").receiver.as_deref(), Some(""));
    assert_eq!(
        call(&resolved.calls, "build").receiver.as_deref(),
        Some("self")
    );
    assert_eq!(
        call(&resolved.calls, "require").receiver.as_deref(),
        Some("")
    );
    assert_eq!(includes(&resolved), Vec::<&str>::new());
}

#[test]
fn a_declaration_signature_is_its_header() {
    let raw = extract(b"function M.f(a, b)\n  return a\nend\n");

    assert_eq!(
        symbol(&raw.symbols, "f").signature.as_deref(),
        Some("function M.f(a, b)")
    );
}

#[test]
fn a_bound_function_signature_keeps_its_binding() {
    let raw = extract(
        b"M.f = function(a)\n  return a\nend\n\nlocal T = {\n  g = function(b) return b end,\n}\n",
    );

    assert_eq!(
        symbol(&raw.symbols, "f").signature.as_deref(),
        Some("M.f = function(a)")
    );
    assert_eq!(
        symbol(&raw.symbols, "g").signature.as_deref(),
        Some("g = function(b)")
    );
}

#[test]
fn each_target_of_an_assignment_keeps_its_own_signature_and_calls() {
    let raw = extract(b"local f, g = function(x) fcall(x) end, function(y) gcall(y) end\n");

    assert_eq!(
        symbol(&raw.symbols, "f").signature.as_deref(),
        Some("f = function(x)")
    );
    assert_eq!(
        symbol(&raw.symbols, "g").signature.as_deref(),
        Some("g = function(y)")
    );
    assert_eq!(call(&raw.calls, "fcall").site_of, id_of(&raw.symbols, "f"));
    assert_eq!(call(&raw.calls, "gcall").site_of, id_of(&raw.symbols, "g"));
}

#[test]
fn a_value_beside_a_bound_function_is_no_call_of_it() {
    let raw = extract(
        concat!(
            "local anchor = 1\nf, g = mk(), function() body() end\n",
            "function outer()\n  h, i = mk2(), function() body2() end\nend\n",
        )
        .as_bytes(),
    );

    assert_eq!(call(&raw.calls, "mk").site_of, FILE_SCOPE);
    assert_eq!(call(&raw.calls, "body").site_of, id_of(&raw.symbols, "g"));
    assert_eq!(
        call(&raw.calls, "mk2").site_of,
        id_of(&raw.symbols, "outer")
    );
    assert_eq!(call(&raw.calls, "body2").site_of, id_of(&raw.symbols, "i"));
}

#[test]
fn an_attribute_between_variables_pairs_each_with_its_value() {
    let raw = extract(b"local a <const>, b = function() end, function() end\n");

    assert_eq!(names(&raw.symbols), ["a", "b"]);
    assert!(
        raw.symbols
            .iter()
            .all(|symbol| symbol.kind == SymbolKind::Function)
    );
}

#[test]
fn only_an_identifier_key_names_a_table_function() {
    let raw = extract(
        b"local T = {\n  [\"setup\"] = function() end,\n  [k] = function() end,\n  plain = function() end,\n}\n",
    );

    assert_eq!(names(&raw.symbols), ["T.plain"]);
}

#[test]
fn each_table_of_an_assignment_owns_its_own_fields() {
    let raw = extract(
        b"local T, U = {\n  f = function() end,\n}, {\n  g = function() end,\n}\nlocal V = 1, { h = function() end }\n",
    );

    assert_eq!(names(&raw.symbols), ["T.f", "U.g", "h"]);
    assert!(raw.symbols.iter().all(|symbol| symbol.exported));
}

#[test]
fn bound_functions_open_their_scopes_in_order_around_nested_bindings() {
    let raw = extract(
        b"local anchor = 1\nf, g = function()\n  h = function() hcall() end\n  fcall()\nend, function() gcall() end\n",
    );

    assert_eq!(call(&raw.calls, "fcall").site_of, id_of(&raw.symbols, "f"));
    assert_eq!(call(&raw.calls, "hcall").site_of, id_of(&raw.symbols, "h"));
    assert_eq!(call(&raw.calls, "gcall").site_of, id_of(&raw.symbols, "g"));
}

#[test]
fn a_nested_table_function_has_no_owner() {
    let raw = extract(b"local T = {\n  inner = {\n    f = function() end,\n  },\n}\n");

    assert_eq!(names(&raw.symbols), ["f"]);
    assert_eq!(symbol(&raw.symbols, "f").owner, None);
}

#[test]
fn a_function_inside_another_value_binds_nothing() {
    let raw = extract(b"x = ready() and function() end\n");

    assert_eq!(raw.symbols.len(), 0);
}

fn uses(raw: &RawExtraction) -> Vec<(&str, SymbolId, u32)> {
    raw.uses
        .iter()
        .map(|used| {
            assert_eq!(
                (&used.receiver, used.access),
                (&Receiver::Bare, Access::Read),
                "{used:?}"
            );
            (used.name.as_str(), used.site_of, used.line)
        })
        .collect()
}

#[test]
fn a_name_taken_as_a_value_is_a_use_of_it() {
    let raw = extract(
        b"local function handler(e) print(e) end
function setup()
  subscribe(\"evt\", handler)
  local cb = handler
  return { on_click = handler, handler }, handler or fallback
end
table.sort(items, compare)
if handler then go() end
function after()
  for handler = 1, 2 do keep(handler) end
  return handler
end
function before()
  register(handler)
  local handler = 1
end
function initializer()
  local handler = handler
  do local compare = 1 end
  return compare
end
do
  local function grouped() end
  register(grouped)
end
",
    );

    let (setup, after) = (id_of(&raw.symbols, "setup"), id_of(&raw.symbols, "after"));
    let (before, initializer) = (
        id_of(&raw.symbols, "before"),
        id_of(&raw.symbols, "initializer"),
    );
    assert_eq!(
        uses(&raw),
        [
            ("handler", setup, 3),
            ("handler", setup, 4),
            ("handler", setup, 5),
            ("handler", setup, 5),
            ("handler", setup, 5),
            ("fallback", setup, 5),
            ("items", FILE_SCOPE, 7),
            ("compare", FILE_SCOPE, 7),
            ("handler", FILE_SCOPE, 8),
            ("handler", after, 11),
            ("handler", before, 14),
            ("handler", initializer, 18),
            ("compare", initializer, 20),
            ("grouped", FILE_SCOPE, 24),
        ]
    );
}

#[test]
fn a_parameter_a_local_or_a_name_being_declared_is_no_use() {
    let raw = extract(
        b"function run(handler) register(handler) end
function other()
  local handler = 1
  register(handler)
  for i = 1, n do keep(i) end
  for k, v in pairs(list) do keep(k, v) end
  local function inner() end
  register(inner)
  keep(object.field)
  local f = function(x) return x end
end
handler = 2
M.handler = 3
function M:method() register(self) end
local function recurse() return recurse end
",
    );

    let other = id_of(&raw.symbols, "other");
    assert_eq!(uses(&raw), [("n", other, 5), ("list", other, 6)]);
}

#[test]
fn a_chunk_level_local_hides_its_name_from_the_end_of_its_declaration() {
    let raw = extract(
        b"function shared() return 1 end
local function early() return shared end
local shared = 2
local function reader() return shared end
",
    );

    let early = id_of(&raw.symbols, "early");
    assert_eq!(uses(&raw), [("shared", early, 2)]);
}

#[test]
fn a_chunk_level_local_function_or_function_value_hides_nothing() {
    let raw = extract(
        b"local function f() end
local g = function() end
local function reader() return f, g end
",
    );

    let reader = id_of(&raw.symbols, "reader");
    assert_eq!(uses(&raw), [("f", reader, 3), ("g", reader, 3)]);
}

#[test]
fn a_chunk_level_local_declared_without_a_value_hides_nothing() {
    let raw = extract(
        b"local handler, count
local function subscribe() register(handler, count) end
function handler() end
",
    );

    let subscribe = id_of(&raw.symbols, "subscribe");
    assert_eq!(
        uses(&raw),
        [("handler", subscribe, 2), ("count", subscribe, 2)]
    );
}

#[test]
fn a_file_without_loaders_includes_nothing() {
    let resolved = resolve(SAMPLE, "sample.lua", &[]);

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert_eq!(resolved.re_exports.len(), 0);
    assert_eq!(resolved.file, "sample.lua");
}

#[test]
fn a_require_at_the_top_of_a_file_includes_the_module_and_records_no_call() {
    let resolved = resolve(
        b"\nlocal util = require(\"app.util\")\n\nfunction run()\n    return util.go()\nend\n",
        "app/main.lua",
        &["app/util.lua", "app/main.lua"],
    );

    assert_eq!(includes(&resolved), ["app/util.lua"]);
    assert!(!callees(&resolved.calls).contains(&"require"));
}

#[test]
fn every_string_form_names_a_module() {
    let resolved = resolve(
        b"local a = require(\"a\")\nlocal b = require \"b\"\nlocal c = require[[c]]\n",
        "main.lua",
        &["a.lua", "b.lua", "c.lua", "main.lua"],
    );

    assert_eq!(includes(&resolved), ["a.lua", "b.lua", "c.lua"]);
}

#[test]
fn a_dotted_module_names_a_path() {
    let resolved = resolve(
        br#"local m = require("foo.bar.baz")"#,
        "main.lua",
        &["foo/bar/baz.lua", "main.lua"],
    );

    assert_eq!(includes(&resolved), ["foo/bar/baz.lua"]);
}

#[test]
fn a_module_falls_back_to_its_package_init() {
    let resolved = resolve(
        br#"local m = require("foo.bar")"#,
        "main.lua",
        &["foo/bar/init.lua", "main.lua"],
    );

    assert_eq!(includes(&resolved), ["foo/bar/init.lua"]);
}

#[test]
fn a_module_resolves_beside_the_requiring_file() {
    let resolved = resolve(
        br#"local h = require("helpers")"#,
        "pkg/main.lua",
        &["pkg/helpers.lua", "pkg/main.lua"],
    );

    assert_eq!(includes(&resolved), ["pkg/helpers.lua"]);
}

#[test]
fn a_module_at_the_root_wins_over_one_beside_the_file() {
    let resolved = resolve(
        br#"local h = require("helpers")"#,
        "pkg/main.lua",
        &["helpers.lua", "pkg/helpers.lua", "pkg/main.lua"],
    );

    assert_eq!(includes(&resolved), ["helpers.lua"]);
}

#[test]
fn dofile_takes_a_path_whose_dots_are_no_separators() {
    let resolved = resolve(
        br#"dofile("setup.lua")"#,
        "scripts/run.lua",
        &["scripts/setup.lua", "scripts/run.lua"],
    );

    assert_eq!(includes(&resolved), ["scripts/setup.lua"]);
    assert!(!callees(&resolved.calls).contains(&"dofile"));
}

#[test]
fn a_path_beside_the_file_wins_over_one_at_the_root() {
    let resolved = resolve(
        br#"dofile("setup.lua")"#,
        "scripts/run.lua",
        &["setup.lua", "scripts/setup.lua", "scripts/run.lua"],
    );

    assert_eq!(includes(&resolved), ["scripts/setup.lua"]);
}

#[test]
fn loadfile_resolves_from_the_root() {
    let resolved = resolve(
        br#"local chunk = loadfile("config/defaults.lua")"#,
        "main.lua",
        &["config/defaults.lua", "main.lua"],
    );

    assert_eq!(includes(&resolved), ["config/defaults.lua"]);
}

#[test]
fn a_module_outside_the_repository_includes_nothing_and_records_no_call() {
    let resolved = resolve(
        br#"local socket = require("socket")"#,
        "main.lua",
        &["main.lua"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert!(!callees(&resolved.calls).contains(&"require"));
}

#[test]
fn an_empty_module_name_loads_nothing() {
    let resolved = resolve(
        b"function run() end\nrequire \"\"\ndofile \"\"\n",
        "pkg/main.lua",
        &["pkg.lua", "pkg/init.lua", "pkg/main.lua"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert_eq!(resolved.calls.len(), 0);
}

#[test]
fn a_loader_with_a_computed_argument_is_an_ordinary_call() {
    let resolved = resolve(
        concat!(
            "\nlocal name = \"dynamic\"\n\nfunction load_it()\n    return require(name)\nend\n\n",
            "function load_concat()\n    return require(\"a\" .. \"b\")\nend\n",
        )
        .as_bytes(),
        "main.lua",
        &["main.lua"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert_eq!(callees(&resolved.calls), ["require", "require"]);
}

#[test]
fn a_loader_on_a_table_is_a_method_call() {
    let resolved = resolve(
        b"\nlocal M = {}\n\nfunction M.boot()\n    return M.require(\"x\")\nend\n",
        "main.lua",
        &["x.lua", "main.lua"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert_eq!(
        call(&resolved.calls, "require").receiver.as_deref(),
        Some("M")
    );
}

#[test]
fn a_test_path_marks_every_form_as_test_code() {
    for path in [
        "foo_spec.lua",
        "foo_test.lua",
        "proj/spec/util.lua",
        "proj/test/util.lua",
        "proj/tests/helper.lua",
        "proj/Tests/helper.lua",
    ] {
        let resolved = resolve(EVERY_FORM, path, &[]);
        assert_eq!(
            names(&resolved.symbols),
            ["run", "M.f", "handlers.setup"],
            "{path}"
        );
        assert!(
            resolved.symbols.iter().all(|symbol| symbol.flags.is_test()),
            "{path}"
        );
    }

    let production = resolve(EVERY_FORM, "proj/src/foo.lua", &[]);
    assert!(
        production
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_test())
    );
}

#[test]
fn a_generated_name_marks_every_form_as_generated() {
    for path in ["proto/widget.gen.lua", "proto/widget_pb.lua"] {
        let resolved = resolve(EVERY_FORM, path, &[]);
        assert_eq!(resolved.symbols.len(), 3, "{path}");
        assert!(
            resolved
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_generated()),
            "{path}"
        );
    }

    let plain = resolve(EVERY_FORM, "proto/widget.lua", &[]);
    assert!(
        plain
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_generated())
    );
}

fn is_generated(source: &[u8]) -> bool {
    let raw = extract(source);
    assert_eq!(raw.symbols.len(), 1, "{}", String::from_utf8_lossy(source));
    raw.symbols[0].flags.is_generated()
}

#[test]
fn a_marker_in_the_leading_comments_marks_generated_code() {
    assert!(is_generated(
        b"#!/usr/bin/env lua\n-- @generated by tooling - DO NOT EDIT\n\nfunction run()\n    return 1\nend\n"
    ));
    assert!(!is_generated(
        b"-- A plain module.\nfunction run()\n    return 1\nend\n"
    ));
    assert!(!is_generated(
        b"\nfunction run()\n    -- @generated\n    return 1\nend\n"
    ));
}

#[test]
fn a_long_comment_carries_a_marker_until_it_closes() {
    assert!(is_generated(
        b"--[[\n@generated by tooling\n]]\nfunction run()\n    return 1\nend\n"
    ));
    assert!(is_generated(
        b"--[[ @generated ]]\nfunction run()\n    return 1\nend\n"
    ));
    assert!(is_generated(
        b"--[[ license ]] -- @generated by tooling\nfunction run()\n    return 1\nend\n"
    ));
    assert!(!is_generated(
        b"--[[ header ]] local x = 1\n-- @generated\nfunction run()\n    return 1\nend\n"
    ));
    assert!(!is_generated(
        b"--[[\nheader text\n]] local x = \"DO NOT EDIT\"\nfunction run()\n    return 1\nend\n"
    ));
}

#[test]
fn no_symbol_is_an_entry_point() {
    let script = resolve(
        b"\nfunction main()\n    return run()\nend\n\nmain()\n",
        "bin/run.lua",
        &[],
    );

    assert_eq!(script.symbols.len(), 1);
    assert!(script.symbols.iter().all(|symbol| !symbol.flags.is_entry()));
    assert!(
        extract(SAMPLE)
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_entry())
    );
}

#[test]
fn a_bare_call_reaches_no_member_of_its_own_table() {
    assert_eq!(Lua::new().conventions().own_members, OwnMembers::Unreached);
}

#[test]
fn a_file_is_claimed_by_its_extension_in_any_case() {
    let lua = Lua::new();

    assert!(lua.claims("init.lua"));
    assert!(lua.claims("lua/plugin/INIT.LUA"));
    assert!(!lua.claims("init.luac"));
    assert!(!lua.claims("lua"));
    assert_eq!(lua.name(), "lua");
}

#[test]
fn the_fingerprint_names_the_crate_and_is_computed_once() {
    let (one, other) = (Lua::new(), Lua::new());

    assert!(one.fingerprint().starts_with(b"kartta-lang-lua "));
    assert!(std::ptr::eq(one.fingerprint(), other.fingerprint()));
}

const DOCUMENTED: &str = "local t = {}

--- Parses.
-- Fails on nothing.
function parse() end

--[[
Renders.
]]
function render() end
local v = t[t[1]]
function after() end

--[==[
Leveled, so ]] may stand inside.
]==]
function leveled() end

--[===[
The highest level read.
]===]
function highest() end
";

fn doc_line(name: &str) -> u32 {
    let lines: Vec<&[u8]> = DOCUMENTED.lines().map(str::as_bytes).collect();
    let line = symbol(&extract(DOCUMENTED.as_bytes()).symbols, name).line;
    Lua::new().doc_comments().first_line(&lines, line)
}

#[test]
fn line_comments_and_a_long_comment_above_a_function_document_it() {
    assert_eq!(doc_line("parse"), 3);
    assert_eq!(doc_line("render"), 7);
    assert_eq!(doc_line("leveled"), 14);
    assert_eq!(doc_line("highest"), 19);
}

#[test]
fn code_that_ends_with_the_long_comment_s_closer_documents_nothing() {
    assert_eq!(doc_line("after"), 12);
}

#[test]
fn a_signature_leaves_out_its_comments() {
    let raw = extract(b"function walk(a, -- the first\n  b)\n  return a\nend\n");

    assert_eq!(
        symbol(&raw.symbols, "walk").signature.as_deref(),
        Some("function walk(a,\n  b)")
    );
}
