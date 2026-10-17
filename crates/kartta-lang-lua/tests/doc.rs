//! Where the doc comment of a Lua definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_lua::Lua;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Lua::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Lua::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "local t = {}

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

#[test]
fn line_comments_and_a_long_comment_above_a_function_document_it() {
    assert_eq!(doc_line(SOURCE, "parse"), 3);
    assert_eq!(doc_line(SOURCE, "render"), 7);
    assert_eq!(doc_line(SOURCE, "leveled"), 14);
    assert_eq!(doc_line(SOURCE, "highest"), 19);
}

#[test]
fn code_that_ends_with_the_long_comment_s_closer_documents_nothing() {
    assert_eq!(doc_line(SOURCE, "after"), 12);
}
