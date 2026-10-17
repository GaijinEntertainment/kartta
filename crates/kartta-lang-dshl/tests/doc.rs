//! Where the doc comment of a DSHL definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_dshl::Dshl;

fn doc_line(source: &str, name: &str) -> u32 {
    let raw = Dshl::new().extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Dshl::new().doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "include \"shader_global.dshl\"

// The fog texture.
texture fog_tex;

/* Draws the fog. */
shader fog_shader
{
}
";

#[test]
fn line_and_block_comments_above_a_definition_document_it() {
    assert_eq!(doc_line(SOURCE, "fog_tex"), 3);
    assert_eq!(doc_line(SOURCE, "fog_shader"), 6);
}
