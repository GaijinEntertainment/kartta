//! Where the doc comment of a Quirrel definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_quirrel::Quirrel;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Quirrel::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Quirrel::new()
        .doc_comments()
        .first_line(&lines, symbol.line)
}

const SOURCE: &str = "let x = 1
// Parses.
# Fails on nothing.
function parse() {}

/* Renders. */
function render() {}
";

#[test]
fn every_comment_form_above_a_function_documents_it() {
    assert_eq!(doc_line(SOURCE, "parse"), 2);
    assert_eq!(doc_line(SOURCE, "render"), 6);
}
