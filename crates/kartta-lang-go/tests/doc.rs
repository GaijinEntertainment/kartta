//! Where the doc comment of a Go definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_go::Go;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Go::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Go::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "package a

// Parse reads.
// It fails on nothing.
func Parse() {}

/*
Render draws.
*/
func Render() {}

var x = 1
func After() {}
";

#[test]
fn line_and_block_comments_directly_above_a_definition_document_it() {
    assert_eq!(doc_line(SOURCE, "Parse"), 3);
    assert_eq!(doc_line(SOURCE, "Render"), 7);
    assert_eq!(doc_line(SOURCE, "After"), 13);
}
