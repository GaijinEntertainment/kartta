//! Where the doc comment of a PHP definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_php::Php;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Php::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Php::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "<?php
/**
 * Parses.
 */
#[Pure]
function parse() {}

# Renders.
// Twice.
function render() {}
";

#[test]
fn a_doc_block_and_the_attributes_below_it_document_a_function() {
    assert_eq!(doc_line(SOURCE, "parse"), 2);
    assert_eq!(doc_line(SOURCE, "render"), 8);
}
