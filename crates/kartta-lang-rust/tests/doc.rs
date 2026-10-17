//! Where the doc comment of a Rust definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_rust::Rust;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Rust::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Rust::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "use std::fmt;

/// Parses.
#[must_use]
#[inline]
pub fn parse() -> u32 {
    1
}

/* Renders. */
fn render() {}
";

#[test]
fn the_attributes_between_a_doc_comment_and_its_item_belong_to_the_item() {
    assert_eq!(doc_line(SOURCE, "parse"), 3);
    assert_eq!(doc_line(SOURCE, "render"), 10);
}
