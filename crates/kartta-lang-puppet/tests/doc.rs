//! Where the doc comment of a Puppet definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_puppet::Puppet;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Puppet::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Puppet::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "# Installs nginx.
# Twice.
class nginx {
}

/*
 * Configures a site.
 */
define nginx::site() {
}
";

#[test]
fn hash_and_block_comments_above_a_class_or_a_define_document_it() {
    assert_eq!(doc_line(SOURCE, "nginx"), 1);
    assert_eq!(doc_line(SOURCE, "nginx::site"), 6);
}
