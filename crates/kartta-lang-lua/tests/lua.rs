//! What a Lua file extracts to, and how its module loads resolve against the repository.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use std::io;

use kartta_lang::path::{base, dir};
use kartta_lang::{
    CallRef, Extraction, Language, NO_CALLER_ID, OwnMembers, RawExtraction, Repository, Symbol,
    SymbolId, SymbolKind,
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

struct Files(Vec<String>);

impl Repository for Files {
    fn contains(&self, path: &str) -> bool {
        self.0.iter().any(|file| file == path)
    }

    fn files_in_dir<'a>(&'a self, in_dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let in_dir = in_dir.to_owned();
        Box::new(
            self.0
                .iter()
                .filter(move |file| dir(file) == in_dir)
                .cloned(),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        Box::new(
            self.0
                .iter()
                .filter(move |file| base(file) == name)
                .cloned(),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::NotFound, path.to_owned()))
    }
}

fn extract(source: &[u8]) -> RawExtraction {
    Lua::new().extract(source)
}

fn resolve(source: &[u8], path: &str, files: &[&str]) -> Extraction {
    let lua = Lua::new();
    let repository = Files(files.iter().map(|file| (*file).to_owned()).collect());
    let resolver = lua.resolver(&repository);
    resolver.resolve(lua.extract(source), path)
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
    assert_eq!(greet.caller_id, id_of(&raw.symbols, "helper"));
    assert_eq!(greet.receiver, None);

    let helper_callers: Vec<SymbolId> = raw
        .calls
        .iter()
        .filter(|call| call.callee == "helper")
        .map(|call| call.caller_id)
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
    assert_eq!(last.caller_id, NO_CALLER_ID);
}

#[test]
fn a_call_before_the_first_function_records_nothing() {
    let raw = extract(b"setup()\nfunction run() end\nteardown()\n");

    assert_eq!(callees(&raw.calls), ["teardown"]);
}

#[test]
fn a_call_through_a_table_name_records_it_as_the_receiver() {
    let raw = extract(SAMPLE);

    let build = call(&raw.calls, "build");
    assert_eq!(build.receiver.as_deref(), Some("self"));
    assert_eq!(build.caller_id, id_of(&raw.symbols, "render"));
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
    assert_eq!(
        call(&raw.calls, "fcall").caller_id,
        id_of(&raw.symbols, "f")
    );
    assert_eq!(
        call(&raw.calls, "gcall").caller_id,
        id_of(&raw.symbols, "g")
    );
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

    assert_eq!(call(&raw.calls, "mk").caller_id, NO_CALLER_ID);
    assert_eq!(call(&raw.calls, "body").caller_id, id_of(&raw.symbols, "g"));
    assert_eq!(
        call(&raw.calls, "mk2").caller_id,
        id_of(&raw.symbols, "outer")
    );
    assert_eq!(
        call(&raw.calls, "body2").caller_id,
        id_of(&raw.symbols, "i")
    );
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

    assert_eq!(
        call(&raw.calls, "fcall").caller_id,
        id_of(&raw.symbols, "f")
    );
    assert_eq!(
        call(&raw.calls, "hcall").caller_id,
        id_of(&raw.symbols, "h")
    );
    assert_eq!(
        call(&raw.calls, "gcall").caller_id,
        id_of(&raw.symbols, "g")
    );
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
