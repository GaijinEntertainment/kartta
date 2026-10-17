//! Where the doc comment of a Python definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_python::Python;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Python::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Python::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "import os

# Parses.
# Fails on nothing.
@cached
@traced
def parse():
    \"\"\"The docstring is in the body.\"\"\"


x = 1
def after():
    pass
";

#[test]
fn the_comments_above_the_first_decorator_document_a_definition() {
    assert_eq!(doc_line(SOURCE, "parse"), 3);
    assert_eq!(doc_line(SOURCE, "after"), 12);
}
