//! Where the doc comment of a daslang definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_daslang::Daslang;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Daslang::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Daslang::new()
        .doc_comments()
        .first_line(&lines, symbol.line)
}

const SOURCE: &str = "options gen2

// Parses.
// Fails on nothing.
[export]
def parse() {
  return
}

/* Renders. */
def render() {
  return
}
";

#[test]
fn the_comments_above_a_function_and_its_annotation_document_it() {
    assert_eq!(doc_line(SOURCE, "parse"), 3);
    assert_eq!(doc_line(SOURCE, "render"), 10);
}
