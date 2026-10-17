//! Where the doc comment of a Ruby definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_ruby::Ruby;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Ruby::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Ruby::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "require 'json'

# Parses.
def parse
end

=begin
Renders.
#=end
=end
def render
end
";

#[test]
fn hash_comments_and_an_embedded_document_above_a_method_document_it() {
    assert_eq!(doc_line(SOURCE, "parse"), 3);
    assert_eq!(doc_line(SOURCE, "render"), 7);
}
