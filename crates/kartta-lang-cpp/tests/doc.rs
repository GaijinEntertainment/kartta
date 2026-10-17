//! Where the doc comment of a C++ definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_cpp::Cpp;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Cpp::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Cpp::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "int x;
/// Parses.
template <typename T>
T parse(T value) { return value; }

/**
 * Renders.
 */
void render() {}
int y; // trailing
void after() {}

// Wraps.
template<typename T>
T wrap(T value) { return value; }
";

#[test]
fn a_template_header_belongs_to_the_definition_below_its_doc_comment() {
    assert_eq!(doc_line(SOURCE, "parse"), 2);
    assert_eq!(doc_line(SOURCE, "wrap"), 13);
}

#[test]
fn a_block_comment_counts_whole_and_code_above_documents_nothing() {
    assert_eq!(doc_line(SOURCE, "render"), 6);
    assert_eq!(doc_line(SOURCE, "after"), 11);
}
