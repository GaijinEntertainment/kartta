//! The receiver contract a language with owner receivers keeps: a member call on a value the caller declares records the
//! empty receiver, so the graph never binds it through an owner that the value's name happens to spell, while the
//! caller's own receiver, as the language spells it, and an import keep a name.
//!
//! C++ and JS/TS keep it, and C++ names the class a value is declared as where the walk sees the declaration, the class
//! the value holds and not an owner its name spells. Python, Ruby, Rust, PHP, Lua, Quirrel, Puppet and daslang record a
//! value's own name instead, and join these cases once they keep it.

use kartta_lang::CallRef;
use kartta_languages::all;

/// The first call of `callee` the language records in `source`, or `None` where it records none.
fn call_of(language: &str, source: &str, callee: &str) -> Option<CallRef> {
    all()
        .into_iter()
        .find(|candidate| candidate.name() == language)?
        .extract(source.as_bytes())
        .calls
        .into_iter()
        .find(|call| call.callee == callee)
}

#[test]
fn a_member_call_on_a_value_the_caller_declares_records_the_empty_receiver() {
    let cases = [
        (
            "cpp",
            "void run() { auto buf = make(); buf.resize(); }\n",
            "resize",
        ),
        (
            "cpp",
            "void run(Buffer** items) { items[0]->close(); }\n",
            "close",
        ),
        (
            "typescript",
            "function run() { const buf = make(); buf.resize(); }\n",
            "resize",
        ),
        (
            "typescript",
            "function run(items: Items) { items.close(); }\n",
            "close",
        ),
        (
            "javascript",
            "function run() { let row = next(); row.save(); }\n",
            "save",
        ),
    ];

    for (language, source, callee) in cases {
        assert_eq!(
            call_of(language, source, callee).map(|call| call.receiver),
            Some(Some(String::new())),
            "{language}: {source}"
        );
    }
}

#[test]
fn a_cpp_member_call_on_a_value_of_a_declared_class_names_the_class() {
    let cases = [
        (
            "void run() { Buffer buf = make(); buf.resize(); }\n",
            "resize",
        ),
        ("void run(Buffer* items) { items->close(); }\n", "close"),
    ];

    for (source, callee) in cases {
        assert_eq!(
            call_of("cpp", source, callee).map(|call| call.receiver),
            Some(Some("Buffer".to_owned())),
            "{source}"
        );
    }
}

#[test]
fn a_member_call_on_the_own_receiver_or_an_import_keeps_its_name() {
    let cases = [
        (
            "cpp",
            "struct Pool { void run() { this->drain(); } };\n",
            "drain",
            "Pool",
        ),
        (
            "typescript",
            "class Pool { run() { this.drain(); } }\n",
            "drain",
            "Pool",
        ),
        (
            "typescript",
            "import * as fs from \"fs\";\nfunction run() { fs.readFile(); }\n",
            "readFile",
            "fs",
        ),
    ];

    for (language, source, callee, receiver) in cases {
        assert_eq!(
            call_of(language, source, callee).map(|call| call.receiver),
            Some(Some(receiver.to_owned())),
            "{language}: {source}"
        );
    }
}
