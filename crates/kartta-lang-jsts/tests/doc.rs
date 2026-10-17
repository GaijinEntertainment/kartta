//! Where the doc comment of a TypeScript or JavaScript definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_jsts::{JavaScript, TypeScript};

fn doc_line(language: &dyn Language, source: &str, name: &str) -> u32 {
    let raw = language.extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    language.doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "let x = 1;
/** Parses. */
export function parse() {}

class Widget {
  @Input() first: string;
  @Input() second: string;
}
";

#[test]
fn a_comment_directly_above_a_definition_documents_it() {
    assert_eq!(doc_line(&TypeScript::new(), SOURCE, "parse"), 2);
}

#[test]
fn a_decorated_field_above_documents_nothing_of_the_field_below_it() {
    assert_eq!(doc_line(&TypeScript::new(), SOURCE, "second"), 7);
}

#[test]
fn javascript_reads_the_same_comments() {
    let source = "/**\n * Parses.\n */\nfunction parse() {}\n";
    assert_eq!(doc_line(&JavaScript::new(), source, "parse"), 1);
}
