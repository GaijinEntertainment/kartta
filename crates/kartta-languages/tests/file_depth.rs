//! A file picks how deep its chains and nestings run, and no depth ends the process: a language reads each source below
//! to its end on a stack that holds a small part of the frames a recursion over it would take.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;
use std::thread;

use kartta::{BuildOptions, Engine};
use kartta_lang::RawExtraction;

const DEPTH: usize = 4_000;
const STACK_BYTES: usize = 64 * 1024;

/// One source: `statement` with `@` in place of `open` repeated [`DEPTH`] times, `inner`, and `close` as often.
type Row<'s> = (&'s str, (&'s str, &'s str, &'s str));

fn nested((statement, (open, inner, close)): Row<'_>, depth: usize) -> String {
    let nesting = format!("{}{inner}{}", open.repeat(depth), close.repeat(depth));
    statement.replace('@', &nesting)
}

fn extraction(language: &str, source: String) -> RawExtraction {
    let languages = kartta_languages::select([language]).expect("the language is known");
    thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || languages[0].extract(source.as_bytes()))
        .expect("the thread starts")
        .join()
        .expect("the extraction ends")
}

/// `rows` each read to the `after` that `declaration` declares past them.
fn assert_read_past(language: &str, declaration: &str, rows: &[Row<'_>]) {
    for &row in rows {
        let source = format!("{}\n{declaration}\n", nested(row, DEPTH));
        let raw = extraction(language, source);
        assert!(
            raw.symbols.iter().any(|symbol| symbol.name == "after"),
            "{row:?}"
        );
    }
}

#[test]
fn a_chain_of_calls_builds_through_the_walk() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let source = nested(("const t = @;", ("", "a", ".m()")), 100_000);
    fs::write(tree.path().join("a.js"), source).expect("the file is written");
    let languages = kartta_languages::select(["javascript"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.engine.graph_of("a.js").is_some());
}

#[test]
fn javascript_binds_the_name_inside_a_pattern() {
    for nesting in [("[", "bound", "]"), ("{a:", "bound", "}")] {
        let statement = "function f() { const @ = y; bound.m(); }";
        let raw = extraction("javascript", nested((statement, nesting), DEPTH));
        let call = raw.calls.iter().find(|call| call.callee == "m");
        let receiver = call.and_then(|call| call.receiver.as_deref());
        assert_eq!(receiver, Some(""), "{nesting:?}");
    }
}

#[test]
fn javascript_reads_past_a_chain_or_a_pattern() {
    assert_read_past(
        "javascript",
        "function after() {}",
        &[
            ("const t = @;", ("", "a", ".m()")),
            ("const t = @;", ("", "a", "?.m()")),
            ("const t = @;", ("", "require('m')", ".m()")),
            ("const @ = y;", ("[", "x", "]")),
            ("const @ = y;", ("{a:", "x", "}")),
            ("function f(@) {}", ("[", "x", "]")),
            ("const @ = require('m');", ("[", "x", "]")),
        ],
    );
}

#[test]
fn go_keeps_a_path_whose_steps_shorten_it_before_they_lengthen_it() {
    let (containers, indices, fields) = ("[]".repeat(79), "[0]".repeat(79), ".b".repeat(79));
    let source = format!("package x\nfunc f(v {containers}T) {{\n\tv{indices}{fields}.m()\n}}\n");
    let raw = extraction("go", source);
    let call = raw.calls.iter().find(|call| call.callee == "m");
    let receiver = call.and_then(|call| call.receiver.as_deref());
    assert_eq!(receiver, Some(format!("T{fields}").as_str()));
}

#[test]
fn go_reads_past_a_value_a_type_or_a_literal() {
    assert_read_past(
        "go",
        "func after() {}",
        &[
            ("package x\nvar t = @", ("", "a", ".b")),
            ("package x\nvar t = @", ("", "a", ".b()")),
            ("package x\nvar t = @", ("", "a", "[0]")),
            ("package x\nvar t = @", ("(", "a", ")")),
            ("package x\nvar t = @", ("*", "a", "")),
            ("package x\nvar t = @", ("<-", "a", "")),
            ("package x\nvar t = @", ("new(", "a", ").b")),
            ("package x\nvar t @", ("[]", "int", "")),
            ("package x\nvar t @", ("*", "int", "")),
            ("package x\nvar t @", ("chan ", "int", "")),
            ("package x\nvar t @", ("map[int]", "int", "")),
            ("package x\nvar t @", ("(", "int", ")")),
            ("package x\nvar t @", ("struct{a ", "int", "}")),
            ("package x\ntype T @", ("struct{a ", "int", "}")),
            ("package x\nfunc f() @ { return nil }", ("[]", "int", "")),
            ("package x\nfunc (t @) m() {}", ("(", "*T", ")")),
            ("package x\nvar t = a.(@)", ("(", "T", ")")),
            ("package x\nvar t = T{@}", ("a:{", "", "}")),
            ("package x\ntype T []T\nvar t = T{@}", ("{", "", "}")),
            ("package x\nfunc f() { g[@](1) }", ("(", "T", ")")),
            ("package x\nfunc f() { g[@](1) }", ("", "T", "[T]")),
            ("package x\nfunc f() { g[@](1) }", ("g[", "T", "]")),
        ],
    );
}

#[test]
fn php_reads_past_a_folded_path_or_a_list() {
    assert_read_past(
        "php",
        "function after() {}",
        &[
            ("<?php\ninclude @ . '/b.php';", ("dirname(", "__DIR__", ")")),
            ("<?php\ninclude @;", ("", "__DIR__", " . '/b'")),
            ("<?php\ninclude @;", ("(", "__DIR__", ")")),
            ("<?php\nfunction f() { @ = $b; }", ("[", "$a", "]")),
            ("<?php\nfunction f() { @ = $b; }", ("list(", "$a", ")")),
            (
                "<?php\nfunction f() { foreach ($x as @) {} }",
                ("[", "$a", "]"),
            ),
        ],
    );
}
