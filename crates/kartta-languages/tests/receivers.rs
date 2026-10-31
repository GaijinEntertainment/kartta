//! The receiver contract a language with owner receivers keeps: a member call on a value the caller declares records
//! the empty receiver, so the graph never binds it through an owner that the value's name happens to spell, while the
//! caller's own receiver, as the language spells it, and an import keep a name.
//!
//! C++ and JS/TS keep it, and C++ names the class a value is declared as where the walk sees the declaration, the class
//! the value holds and not an owner its name spells, as JS/TS names the type a parameter with no annotation takes from
//! its function's context (ADR 3.101). Python, Ruby, Rust, PHP, Lua, Quirrel, Puppet and daslang record a value's
//! own name instead, which the graph's receiver tiers read (ADR 3.171); Rust writes a lowercase one after a `.`,
//! `.buf`, since alone it is the module that a path call names, and records the type instead where a parameter, a field
//! of `self`, a local or a chain names one (ADR 3.55). Python, Ruby, Rust, daslang and PHP list the receivers they
//! write for a value of the caller's own type in `Conventions::own_type_receivers`, so in each no other value's name is
//! taken for one of the caller's type, past a guess in PHP. Ruby and Rust list only `other`, since each records its own
//! receiver as the name of the caller's owner.

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

/// The receivers the language lists for a value of the caller's own type, empty where it lists none.
fn own_type_receivers(language: &str) -> &'static [&'static str] {
    all()
        .into_iter()
        .find(|candidate| candidate.name() == language)
        .and_then(|language| language.conventions().own_type_receivers)
        .unwrap_or_default()
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
            "function run(bufs: Buffer[]) { const buf = bufs[0]; buf.resize(); }\n",
            "resize",
        ),
        (
            "typescript",
            "function run(items: Items[]) { items.close(); }\n",
            "close",
        ),
        (
            "javascript",
            "function run(rows) { let row = rows[0]; row.save(); }\n",
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
            "Buffer",
        ),
        (
            "void run(Buffer* items) { items->close(); }\n",
            "close",
            "Buffer",
        ),
        (
            "void run(io::Buffer* items) { items->close(); }\n",
            "close",
            "io::Buffer",
        ),
    ];

    for (source, callee, class) in cases {
        assert_eq!(
            call_of("cpp", source, callee).map(|call| call.receiver),
            Some(Some(class.to_owned())),
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
        (
            "rust",
            "struct Pool;\nimpl Pool {\n    fn run(&self) {\n        self.drain();\n    }\n}\n",
            "drain",
            "Pool",
        ),
        (
            "rust",
            "struct Pool;\nimpl Pool {\n    fn run() {\n        Self::drain();\n    }\n}\n",
            "drain",
            "Pool",
        ),
        (
            "ruby",
            "class Pool\n  def run\n    self.drain\n  end\nend\n",
            "drain",
            "Pool",
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

#[test]
fn a_member_call_on_a_value_of_the_caller_s_type_records_a_receiver_its_language_lists() {
    let cases = [
        (
            "python",
            "class Pool:\n    def __eq__(self, other):\n        return other.drain()\n",
        ),
        (
            "rust",
            "struct Pool;\nimpl Pool {\n    fn eq(&self, others: &[Self]) -> bool {\n        \
             others.iter().all(|other| other.drain())\n    }\n}\n",
        ),
        (
            "ruby",
            "class Pool\n  def ==(other)\n    other.drain\n  end\nend\n",
        ),
        (
            "python",
            "class Pool:\n    def run(self):\n        self.drain()\n",
        ),
        (
            "python",
            "class Pool:\n    @classmethod\n    def run(cls):\n        cls.drain()\n",
        ),
        (
            "daslang",
            "options gen2\nclass Pool {\n    def run() {\n        self->drain()\n    }\n}\n",
        ),
        (
            "daslang",
            "options gen2\nclass Pool {\n    def run() {\n        self.drain()\n    }\n}\n",
        ),
        (
            "php",
            "<?php\nclass Pool {\n    function run() { $this->drain(); }\n}\n",
        ),
        (
            "php",
            "<?php\nclass Pool {\n    function run() { self::drain(); }\n}\n",
        ),
        (
            "php",
            "<?php\nclass Pool {\n    function run() { static::drain(); }\n}\n",
        ),
    ];

    for (language, source) in cases {
        let receiver = call_of(language, source, "drain").and_then(|call| call.receiver);
        let own_type = own_type_receivers(language);
        assert!(
            receiver
                .as_deref()
                .is_some_and(|receiver| own_type.contains(&receiver)),
            "{language} records {receiver:?}, not one of {own_type:?}: {source}"
        );
    }
}
