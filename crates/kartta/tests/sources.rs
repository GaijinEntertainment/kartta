//! What the engine answers for a definition's text: the doc comment above it, the caps a caller sets, and the file as
//! it is when asked.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::io;

use kartta::{
    BuildOptions, CutLine, Definition, DocComments, Engine, Extraction, Language, RawExtraction,
    Repository, Resolver, Source, SourceOptions, Symbol, SymbolKind,
};
use tempfile::TempDir;

/// Definitions that open on a `def <name>` line and close on the next `end` line, documented by `#` lines above.
#[derive(Debug)]
struct Blocks;

impl Language for Blocks {
    fn name(&self) -> &'static str {
        "blocks"
    }

    fn claims(&self, path: &str) -> bool {
        path.ends_with(".blocks")
    }

    fn fingerprint(&self) -> &[u8] {
        b"blocks 1"
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::lines(&["#"])
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        let mut raw = RawExtraction::default();
        for (at, line) in String::from_utf8_lossy(source).lines().enumerate() {
            let number = u32::try_from(at + 1).expect("a test file is short");
            if let Some(name) = line.strip_prefix("def ") {
                raw.symbols
                    .push(Symbol::new(name, SymbolKind::Function, number, number));
            } else if line == "end"
                && let Some(open) = raw.symbols.last_mut()
            {
                open.end_line = number;
            }
        }
        raw
    }

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(AtPath)
    }
}

struct AtPath;

impl Resolver for AtPath {
    fn resolve(&self, raw: RawExtraction, path: &str, _repository: &dyn Repository) -> Extraction {
        raw.into_extraction(path).0
    }
}

fn tree(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let dir = tempfile::tempdir().expect("create temp dir");
    for (path, content) in files {
        fs::write(dir.path().join(path), content).expect("write file");
    }
    let engine = Engine::build(
        dir.path(),
        &BuildOptions::default(),
        vec![Box::new(Blocks)],
        None,
    )
    .expect("build the engine")
    .engine;
    (dir, engine)
}

fn definitions<'e>(engine: &'e Engine, names: &[&str]) -> Vec<Definition<'e>> {
    let (_, graph) = engine.graphs().next().expect("the blocks graph");
    names
        .iter()
        .map(|name| {
            graph
                .definitions(*name, None, 1)
                .found
                .first()
                .expect("the name is defined")
                .definition
        })
        .collect()
}

fn source(engine: &Engine, name: &str, options: SourceOptions) -> Source {
    engine
        .sources(&definitions(engine, &[name]), options)
        .pop()
        .expect("one source per definition")
        .expect("the file reads")
}

const FILE: &str = "x = 1\n\n# Parses.\n# Fails on nothing.\ndef parse\n  body\nend\n# Renders.\ndef render\nend\n";

#[test]
fn a_source_runs_from_the_doc_comment_above_the_definition_through_its_last_line() {
    let (_dir, engine) = tree(&[("a.blocks", FILE)]);

    let parse = source(&engine, "parse", SourceOptions::default());
    assert_eq!((parse.doc_line, parse.first_line, parse.lines), (3, 3, 5));
    assert_eq!(
        parse.text,
        "# Parses.\n# Fails on nothing.\ndef parse\n  body\nend"
    );
    assert_eq!(parse.cut_lines, []);
    assert!(!parse.resized);

    let render = source(&engine, "render", SourceOptions::default());
    assert_eq!(render.text, "# Renders.\ndef render\nend");
}

#[test]
fn sources_answer_in_the_order_the_definitions_were_given_across_files() {
    let (_dir, engine) = tree(&[("a.blocks", FILE), ("b.blocks", "def other\nend\n")]);

    let given = definitions(&engine, &["render", "other", "parse"]);
    let texts: Vec<String> = engine
        .sources(&given, SourceOptions::default())
        .into_iter()
        .map(|source| source.expect("the file reads").text)
        .collect();
    assert_eq!(
        texts,
        [
            "# Renders.\ndef render\nend",
            "def other\nend",
            "# Parses.\n# Fails on nothing.\ndef parse\n  body\nend",
        ]
    );
}

#[test]
fn the_line_cap_keeps_the_head_of_a_long_definition() {
    let (_dir, engine) = tree(&[("a.blocks", FILE)]);

    let parse = source(&engine, "parse", SourceOptions::default().max_lines(4));
    assert_eq!((parse.first_line, parse.lines), (3, 4));
    assert_eq!(
        parse.text,
        "# Parses.\n# Fails on nothing.\ndef parse\n  body"
    );
}

#[test]
fn a_doc_comment_too_long_for_the_line_cap_loses_its_head() {
    let (_dir, engine) = tree(&[("a.blocks", FILE)]);

    let parse = source(&engine, "parse", SourceOptions::default().max_lines(2));
    assert_eq!((parse.doc_line, parse.first_line, parse.lines), (3, 4, 2));
    assert_eq!(parse.text, "# Fails on nothing.\ndef parse");
}

#[test]
fn the_widest_line_cap_answers_a_later_definition_whole() {
    let (_dir, engine) = tree(&[("a.blocks", FILE)]);

    let render = source(
        &engine,
        "render",
        SourceOptions::default().max_lines(usize::MAX),
    );
    assert_eq!((render.first_line, render.lines), (8, 3));
    assert_eq!(render.text, "# Renders.\ndef render\nend");
}

#[test]
fn bytes_that_are_not_utf8_read_as_the_replacement_character() {
    let (dir, engine) = tree(&[("a.blocks", "def odd\n  a?b\nend\n")]);
    fs::write(dir.path().join("a.blocks"), b"def odd\n  a\xffb\nend\n").expect("rewrite file");

    let odd = source(&engine, "odd", SourceOptions::default());
    assert_eq!(odd.text, "def odd\n  a\u{fffd}b\nend");
    assert!(!odd.resized);
}

#[test]
fn the_byte_cap_cuts_a_long_line_on_a_character_boundary_and_says_how_much() {
    let (_dir, engine) = tree(&[("a.blocks", "def wide\n  ab\u{e9}cdef\nend\n")]);

    let wide = source(&engine, "wide", SourceOptions::default().max_line_bytes(5));
    assert_eq!(wide.text, "def w\n  ab\nend");
    let cuts: Vec<(u32, usize)> = wide
        .cut_lines
        .iter()
        .map(|cut: &CutLine| (cut.line, cut.bytes))
        .collect();
    assert_eq!(cuts, [(1, 3), (2, 6)]);
}

#[test]
fn line_endings_are_left_out_of_the_text() {
    let (_dir, engine) = tree(&[("a.blocks", "# Parses.\r\ndef parse\r\nend\r\n")]);

    assert_eq!(
        source(&engine, "parse", SourceOptions::default()).text,
        "# Parses.\ndef parse\nend"
    );
}

#[test]
fn a_file_edited_since_the_build_answers_its_lines_now_and_says_its_size_changed() {
    let (dir, engine) = tree(&[("a.blocks", FILE)]);
    fs::write(dir.path().join("a.blocks"), "x = 1\n\n# Parses.\n").expect("rewrite the file");

    let parse = source(&engine, "parse", SourceOptions::default());
    assert!(parse.resized);
    assert_eq!((parse.lines, parse.text.as_str()), (0, ""));

    fs::write(dir.path().join("a.blocks"), FILE.replace("body", "BODY")).expect("rewrite the file");
    let parse = source(&engine, "parse", SourceOptions::default());
    assert!(!parse.resized, "an edit that keeps the size goes unseen");
    assert!(parse.text.contains("BODY"));
}

#[test]
fn a_file_deleted_since_the_build_answers_an_error_naming_it() {
    let (dir, engine) = tree(&[("a.blocks", FILE), ("b.blocks", "def other\nend\n")]);
    fs::remove_file(dir.path().join("a.blocks")).expect("delete the file");

    let sources = engine.sources(
        &definitions(&engine, &["parse", "other", "render"]),
        SourceOptions::default(),
    );
    let errors: Vec<Option<(io::ErrorKind, bool)>> = sources
        .iter()
        .map(|source| {
            source
                .as_ref()
                .err()
                .map(|error| (error.kind(), error.to_string().starts_with("a.blocks: ")))
        })
        .collect();
    assert_eq!(
        errors,
        [
            Some((io::ErrorKind::NotFound, true)),
            None,
            Some((io::ErrorKind::NotFound, true)),
        ]
    );
}
