//! Where the doc comment of a Bash definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_bash::Bash;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Bash::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Bash::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "set -e

# Parses.
# Fails on nothing.
parse() {
  :
}
echo done
after() { :; }
";

#[test]
fn hash_comments_directly_above_a_function_document_it() {
    assert_eq!(doc_line(SOURCE, "parse"), 3);
    assert_eq!(doc_line(SOURCE, "after"), 9);
}
