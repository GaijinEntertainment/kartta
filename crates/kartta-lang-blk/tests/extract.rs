//! What one BLK file extracts to from its bytes alone.

use kartta_lang::{Language, RawExtraction};
use kartta_lang_blk::Blk;

fn extract(source: &[u8]) -> RawExtraction {
    Blk::new().extract(source)
}

fn paths(raw: &RawExtraction) -> Vec<(&str, u32)> {
    raw.directives
        .iter()
        .map(|directive| (directive.spec.as_str(), directive.line))
        .collect()
}

#[test]
fn blk_claims_files_with_the_blk_extension_in_any_case() {
    let blk = Blk::new();

    assert!(blk.claims("gameData/units/tank.blk"));
    assert!(blk.claims("CONFIG/SETTINGS.BLK"));
    assert!(!blk.claims("config/settings.blkx"));
    assert!(!blk.claims("config/blk"));
    assert_eq!(blk.name(), "blk");
    assert_eq!(blk.family(), "blk");
}

#[test]
fn the_fingerprint_stays_put_and_names_the_crate() {
    let blk = Blk::new();
    let fingerprint = blk.fingerprint();

    assert!(fingerprint.starts_with(b"kartta-lang-blk\0"));
    assert_eq!(Blk::new().fingerprint(), fingerprint);
}

#[test]
fn every_include_form_records_its_path_as_written() {
    let source = br##"include "#/develop/gameBase/_shared.blk"
include '%dngSceneBlk/base.blk';
include shaders_game_internal.blk
INCLUDE /abs/path.blk // comment
block{
  include
    "a.blk"
  Include
  b.blk
}
"include" "c.blk"
'INCLUDE';d.blk
"##;
    let raw = extract(source);

    assert_eq!(
        paths(&raw),
        [
            ("#/develop/gameBase/_shared.blk", 1),
            ("%dngSceneBlk/base.blk", 2),
            ("shaders_game_internal.blk", 3),
            ("/abs/path.blk", 4),
            ("a.blk", 6),
            ("b.blk", 8),
            ("c.blk", 11),
            ("d.blk", 12),
        ]
    );
    assert!(raw.symbols.is_empty() && raw.calls.is_empty());
}

#[test]
fn a_path_the_engine_reads_differently_keeps_both_spellings() {
    let source = b"include \"gyro~~ps4.blk\"\ninclude a/*x*/b.blk\ninclude plain.blk\n";
    let raw = extract(source);

    let spellings: Vec<(&str, Vec<&str>)> = raw
        .directives
        .iter()
        .map(|directive| {
            let read = directive.names.iter().map(String::as_str).collect();
            (directive.spec.as_str(), read)
        })
        .collect();
    assert_eq!(
        spellings,
        [
            ("gyro~~ps4.blk", vec!["gyro~ps4.blk"]),
            ("a/*x*/b.blk", vec!["ab.blk"]),
            ("plain.blk", vec![]),
        ]
    );
}

#[test]
fn an_include_without_a_readable_path_records_nothing() {
    let sources: &[&[u8]] = &[
        b"a{\n  include\n}\ninclude;\n",
        b"a:i=1\ninclude",
        b"a:i=1\ninclude\n",
        b"include",
        b"include\n",
        b"include \"a.blk",
        b"include \"a.blk\n",
        b"include 'a.blk\nb:i=1\n",
        b"include \"\"\"a.blk",
        b"x{ include \"",
    ];

    for source in sources {
        let raw = extract(source);
        assert!(
            raw.directives.is_empty(),
            "{:?}: {:?}",
            String::from_utf8_lossy(source),
            paths(&raw)
        );
    }
}

#[test]
fn a_binary_file_records_nothing() {
    let mut source = b"\x01".to_vec();
    source.extend_from_slice(b"include \"x.blk\"\n");

    let binary = extract(&source);

    assert!(binary.directives.is_empty(), "{:?}", paths(&binary));
    assert_eq!(paths(&extract(&source[1..])), [("x.blk", 1)]);
}
