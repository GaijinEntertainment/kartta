//! What a Python file extracts to from its bytes alone: its symbols qualified by their classes, the calls its
//! functions make, and the directives its imports leave for the resolver.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{CallRef, Language, LocalCall, RawExtraction, Symbol, SymbolId, SymbolKind};
use kartta_lang_python::Python;

const SAMPLE: &[u8] = br#"
import os
import numpy as np
from collections import OrderedDict, defaultdict
from . import sibling

VERSION = "1.0"

def free_function(x):
    return helper(x)


def helper(v):
    return v


@lru_cache
def decorated(x):
    return x


class Service(Base, mixins.Loggable):
    name: str = ""
    count: int

    def greet(self):
        return helper(self.name)

    def _private(self):
        outer = self.greet()

        def nested():
            return outer

        return nested
"#;

fn extract(source: &[u8]) -> RawExtraction {
    Python::new().extract(source)
}

fn named<'r>(raw: &'r RawExtraction, name: &str) -> &'r Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn qualified<'r>(raw: &'r RawExtraction, name: &str) -> &'r Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.qualified_name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn id_of(raw: &RawExtraction, name: &str) -> SymbolId {
    let at = raw
        .symbols
        .iter()
        .position(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    SymbolId(u32::try_from(at).unwrap_or(u32::MAX))
}

fn generated(source: &[u8]) -> Vec<bool> {
    extract(source)
        .symbols
        .iter()
        .map(|symbol| symbol.flags.is_generated())
        .collect()
}

#[test]
fn symbols_are_qualified_by_their_classes_in_source_order() {
    let raw = extract(SAMPLE);
    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(
        names,
        [
            "VERSION",
            "free_function",
            "helper",
            "decorated",
            "Service",
            "Service.name",
            "Service.count",
            "Service.greet",
            "Service._private",
            "nested",
        ]
    );
    assert_eq!(named(&raw, "VERSION").kind, SymbolKind::Variable);
    assert!(named(&raw, "VERSION").exported);
}

#[test]
fn a_decorated_definition_starts_at_its_first_decorator() {
    let raw = extract(SAMPLE);
    let decorator = SAMPLE
        .split(|&byte| byte == b'\n')
        .position(|line| line.starts_with(b"@lru_cache"))
        .expect("decorator line");

    assert_eq!(
        named(&raw, "decorated").line,
        u32::try_from(decorator + 1).unwrap_or(u32::MAX)
    );
}

#[test]
fn an_annotated_class_attribute_is_a_field_and_a_method_local_is_nothing() {
    let raw = extract(SAMPLE);
    let field = qualified(&raw, "Service.name");

    assert_eq!(field.kind, SymbolKind::Field);
    assert_eq!(field.owner.as_deref(), Some("Service"));
    assert!(raw.symbols.iter().all(|symbol| symbol.name != "outer"));
}

#[test]
fn only_the_first_name_of_a_chained_assignment_and_no_tuple_or_unannotated_attribute_is_a_symbol() {
    let raw = extract(b"a = b = 1\nc, d = 1, 2\nclass K:\n    state = 0\n    typed: int = 0\n");
    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(names, ["a", "K", "K.typed"]);
}

#[test]
fn a_function_carries_its_declaration_as_its_signature() {
    let raw = extract(b"def greet(name, count=1):\n    return name\n");

    assert_eq!(
        named(&raw, "greet").signature.as_deref(),
        Some("def greet(name, count=1):")
    );
}

#[test]
fn a_method_is_owned_by_its_class_and_an_underscore_hides_it() {
    let raw = extract(SAMPLE);
    let service = named(&raw, "Service");
    let greet = named(&raw, "greet");

    assert_eq!((service.kind, service.exported), (SymbolKind::Class, true));
    assert_eq!(greet.kind, SymbolKind::Method);
    assert_eq!(greet.owner.as_deref(), Some("Service"));
    assert!(!named(&raw, "_private").exported);
}

#[test]
fn a_function_nested_in_a_method_is_no_member_of_the_class() {
    let raw = extract(SAMPLE);
    let nested = named(&raw, "nested");

    assert_eq!(nested.kind, SymbolKind::Function);
    assert_eq!(nested.owner, None);
    assert_eq!(nested.qualified_name, "nested");
}

#[test]
fn a_definition_inside_a_function_body_is_local_to_it() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    class Local:\n        tag: str = \"\"\n        \
          def run(self):\n            pass\n    return helper()\n\nclass Top:\n    def method(self):\n        pass\n",
    );
    let locals: Vec<SymbolId> = ["helper", "Local", "tag", "run"]
        .into_iter()
        .map(|name| id_of(&raw, name))
        .collect();

    assert_eq!(raw.local_symbols, locals);
}

fn local_calls(raw: &RawExtraction) -> Vec<(&str, &str, u32)> {
    let name = |id: SymbolId| raw.symbols[id.0 as usize].name.as_str();
    raw.local_calls
        .iter()
        .map(|call| (name(call.caller_id), name(call.local), call.line))
        .collect()
}

fn callers_of<'r>(raw: &'r RawExtraction, callee: &str) -> Vec<&'r str> {
    raw.calls
        .iter()
        .filter(|call| call.callee == callee)
        .map(|call| raw.symbols[call.caller_id.0 as usize].name.as_str())
        .collect()
}

#[test]
fn a_bare_call_of_a_definition_a_function_around_it_holds_is_a_local_call() {
    let raw = extract(
        b"def outer():\n    def first():\n        return second()\n    def second():\n        pass\n    \
          class Local:\n        pass\n    return first(), Local()\n",
    );

    assert_eq!(
        local_calls(&raw),
        [
            ("first", "second", 3),
            ("outer", "first", 8),
            ("outer", "Local", 8)
        ]
    );
    assert_eq!(raw.calls, Vec::<CallRef>::new());
}

#[test]
fn a_call_binds_the_definition_its_own_function_holds_beside_a_namesake_in_another() {
    let raw = extract(
        b"def first():\n    def helper():\n        pass\n    return helper()\ndef second():\n    \
          def helper():\n        pass\n    return helper()\n",
    );
    let calls: Vec<(&str, u32)> = raw
        .local_calls
        .iter()
        .map(|call| {
            (
                raw.symbols[call.caller_id.0 as usize].name.as_str(),
                raw.symbols[call.local.0 as usize].line,
            )
        })
        .collect();

    assert_eq!(calls, [("first", 2), ("second", 6)]);
}

#[test]
fn a_name_bound_nearer_the_call_hides_a_definition_around_it() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    def by_parameter(helper):\n        return helper()\n    \
          def by_assignment():\n        helper = make()\n        return helper()\n    def by_target():\n        \
          with open() as helper:\n            helper()\n        try:\n            pass\n        \
          except Error as helper:\n            helper()\n    def by_case(subject):\n        \
          match subject:\n            case helper:\n                helper()\n            \
          case [*helper] as other:\n                helper()\n    \
          def by_lambda():\n        return (lambda helper: helper())(make)\n    def by_import():\n        \
          from tools import helper\n        return helper()\n    def by_global():\n        global helper\n        \
          return helper()\n    return [helper() for helper in handlers]\n",
    );

    assert_eq!(raw.local_calls, Vec::<LocalCall>::new());
    assert_eq!(callers_of(&raw, "helper"), ["by_import", "by_global"]);
}

#[test]
fn the_first_iterable_of_a_comprehension_sees_the_scope_around_it() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    return [item for helper in helper()]\n",
    );

    assert_eq!(local_calls(&raw), [("outer", "helper", 4)]);
}

#[test]
fn an_assignment_expression_in_a_comprehension_binds_in_the_function_around_it() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    def inner():\n        \
          [(helper := x) for x in xs]\n        return helper()\n",
    );

    assert_eq!(raw.local_calls, Vec::<LocalCall>::new());
    assert_eq!(callers_of(&raw, "helper"), Vec::<&str>::new());
}

#[test]
fn a_definition_is_the_target_of_a_name_its_scope_also_assigns() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    helper = wrap(helper)\n    return helper()\n",
    );

    assert_eq!(local_calls(&raw), [("outer", "helper", 5)]);
    assert_eq!(callers_of(&raw, "wrap"), ["outer"]);
}

#[test]
fn a_name_defined_twice_binds_its_later_definition() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    def helper():\n        pass\n    return helper()\n",
    );
    let lines: Vec<u32> = raw
        .local_calls
        .iter()
        .map(|call| raw.symbols[call.local.0 as usize].line)
        .collect();

    assert_eq!(lines, [4]);
}

#[test]
fn an_annotation_of_a_parameter_binds_no_name() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    def inner(value: helper, *rest: helper):\n        \
          return helper()\n",
    );

    assert_eq!(local_calls(&raw), [("inner", "helper", 5)]);
}

#[test]
fn a_call_in_a_class_body_sees_the_names_that_body_binds() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    class Local:\n        def helper():\n            \
          pass\n        made = helper()\n    return helper()\n",
    );
    let calls: Vec<(&str, u32)> = raw
        .local_calls
        .iter()
        .map(|call| {
            (
                raw.symbols[call.caller_id.0 as usize].name.as_str(),
                raw.symbols[call.local.0 as usize].line,
            )
        })
        .collect();

    assert_eq!(calls, [("outer", 5), ("outer", 2)]);
}

#[test]
fn a_call_in_a_class_body_at_module_level_stays_a_call_of_its_method() {
    let raw = extract(b"class Table:\n    def column():\n        pass\n    made = column()\n");
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(raw.local_calls, Vec::<LocalCall>::new());
    assert_eq!(callees, ["column"]);
}

#[test]
fn a_nonlocal_name_reaches_the_definition_around_it() {
    let raw = extract(
        b"def outer():\n    def helper():\n        pass\n    def inner():\n        nonlocal helper\n        \
          helper = wrap(helper)\n        return helper()\n",
    );

    assert_eq!(local_calls(&raw), [("inner", "helper", 7)]);
    assert_eq!(callers_of(&raw, "wrap"), ["inner"]);
}

#[test]
fn a_bare_call_of_a_value_a_function_around_it_binds_records_nothing() {
    let raw = extract(
        b"def f(a, b=1, *c, d: int = 2, **e):\n    a(); b(); c(); d(); e()\n    g = make\n    g()\n    \
          for h in items:\n        h()\n    with open() as i:\n        i()\n    try:\n        pass\n    \
          except Error as j:\n        j()\n    k = lambda m: m()\n    [n() for n in items]\n    (o := make)\n    \
          o()\n    def inner():\n        return a()\n",
    );
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(callees, ["open"]);
    assert_eq!(raw.local_calls, Vec::<LocalCall>::new());
}

#[test]
fn a_bare_call_of_a_module_variable_is_still_a_call() {
    let raw = extract(b"handler = make()\ndef f():\n    return handler()\n");

    assert_eq!(callers_of(&raw, "handler"), ["f"]);
}

#[test]
fn a_bare_call_of_a_name_its_scope_imports_and_assigns_is_still_a_call() {
    let raw = extract(
        b"def f():\n    from tools import helper\n    helper = wrap(helper)\n    return helper()\n",
    );

    assert_eq!(callers_of(&raw, "helper"), ["f"]);
}

#[test]
fn a_definition_in_a_class_body_is_hidden_from_its_methods_and_the_function_around_it() {
    let raw = extract(
        b"def outer():\n    class Local:\n        def run(self):\n            return run()\n    return run()\n",
    );

    assert_eq!(raw.local_calls, Vec::<LocalCall>::new());
    assert_eq!(callers_of(&raw, "run"), ["run", "outer"]);
}

#[test]
fn a_nested_class_is_qualified_by_the_classes_around_it() {
    let raw = extract(b"class A:\n    class Worker:\n        def run(self):\n            pass\n");
    let run = named(&raw, "run");

    assert_eq!(qualified(&raw, "A.Worker").kind, SymbolKind::Class);
    assert_eq!(run.owner.as_deref(), Some("A.Worker"));
    assert_eq!(run.qualified_name, "A.Worker.run");
}

#[test]
fn superclasses_are_parents_as_written() {
    let raw = extract(SAMPLE);

    assert_eq!(named(&raw, "Service").parents, ["Base", "mixins.Loggable"]);
}

#[test]
fn a_call_belongs_to_the_function_around_it() {
    let raw = extract(SAMPLE);

    let callers: Vec<SymbolId> = raw
        .calls
        .iter()
        .filter(|call| call.callee == "helper" && call.receiver.is_none())
        .map(|call| call.caller_id)
        .collect();
    assert_eq!(
        callers,
        [id_of(&raw, "free_function"), id_of(&raw, "greet")]
    );
}

#[test]
fn a_member_call_on_a_name_records_the_name_as_its_receiver() {
    let raw = extract(SAMPLE);
    let greet = raw
        .calls
        .iter()
        .find(|call| call.callee == "greet")
        .expect("greet call");

    assert_eq!(greet.receiver.as_deref(), Some("self"));
    assert_eq!(greet.caller_id, id_of(&raw, "_private"));
}

#[test]
fn a_member_call_on_an_unnamed_expression_keeps_an_empty_receiver() {
    let raw = extract(b"def f():\n    items[0].close()\n    self.pool.get()\n    make().run()\n");
    let calls: Vec<(&str, Option<&str>)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect();

    assert_eq!(
        calls,
        [
            ("close", Some("")),
            ("get", Some("")),
            ("run", Some("")),
            ("make", None)
        ]
    );
}

#[test]
fn a_call_before_the_first_definition_or_of_a_computed_callee_is_dropped() {
    let raw = extract(b"setup()\ndef f():\n    handlers[0]()\n    run()\n");
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(callees, ["run"]);
}

#[test]
fn every_import_leaves_one_directive_with_its_names() {
    let raw = extract(SAMPLE);
    let directives: Vec<(&str, Vec<&str>)> = raw
        .directives
        .iter()
        .map(|directive| {
            (
                directive.spec.as_str(),
                directive.names.iter().map(String::as_str).collect(),
            )
        })
        .collect();

    assert_eq!(
        directives,
        [
            ("os", vec![]),
            ("numpy", vec![]),
            ("collections", vec!["OrderedDict", "defaultdict"]),
            (".", vec!["sibling"]),
        ]
    );
}

#[test]
fn an_aliased_name_is_recorded_by_the_name_it_imports() {
    let raw =
        extract(b"import a.b as ab, c\nfrom . import sibling, name as aliased\nfrom x import *\n");
    let directives: Vec<(&str, Vec<&str>)> = raw
        .directives
        .iter()
        .map(|directive| {
            (
                directive.spec.as_str(),
                directive.names.iter().map(String::as_str).collect(),
            )
        })
        .collect();

    assert_eq!(
        directives,
        [
            ("a.b", vec![]),
            ("c", vec![]),
            (".", vec!["sibling", "name"]),
            ("x", vec![])
        ]
    );
}

#[test]
fn a_future_import_names_the_future_module_and_its_features() {
    let raw = extract(b"from __future__ import annotations, division\n");

    assert_eq!(raw.directives.len(), 1);
    assert_eq!(raw.directives[0].spec, "__future__");
    assert_eq!(raw.directives[0].names, ["annotations", "division"]);
    assert_ne!(
        raw.directives[0].form,
        extract(b"import os\n").directives[0].form
    );
}

#[test]
fn only_a_free_module_level_main_is_the_entry_point() {
    let raw = extract(
        b"def main():\n    run()\n\n\ndef helper():\n    pass\n\n\n\
          class App:\n    def main(self):\n        pass\n\n\n\
          def outer():\n    def main():\n        pass\n    return main\n",
    );

    assert!(qualified(&raw, "main").flags.is_entry());
    assert!(!qualified(&raw, "App.main").flags.is_entry());
    assert!(!named(&raw, "helper").flags.is_entry());
    let nested = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "main")
        .nth(1)
        .expect("nested main");
    assert!(!nested.flags.is_entry());
}

#[test]
fn a_decorated_main_is_still_the_entry_point() {
    let raw = extract(b"@click.command()\ndef main():\n    pass\n");

    assert!(named(&raw, "main").flags.is_entry());
}

#[test]
fn a_special_method_or_a_property_is_an_entry_point() {
    let raw = extract(
        br"
import functools

class Box:
    def __init__(self):
        pass

    def __eq__(self, other):
        pass

    @property
    def size(self):
        pass

    @size.setter
    def size(self, value):
        pass

    @size.getter
    def size(self):
        pass

    @size.deleter
    def size(self):
        pass

    @registry.setter
    def store(self, value):
        pass

    @functools.cached_property
    def area(self):
        pass

    @staticmethod
    def make():
        pass

    def _private(self):
        pass

    def __(self):
        pass

def __custom__():
    pass
",
    );

    for name in ["Box.__init__", "Box.__eq__", "Box.size", "Box.area"] {
        assert!(qualified(&raw, name).flags.is_entry(), "{name}");
    }
    let accessors: Vec<bool> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "Box.size")
        .map(|symbol| symbol.flags.is_entry())
        .collect();
    assert_eq!(accessors, [true; 4]);
    for name in [
        "Box.store",
        "Box.make",
        "Box._private",
        "Box.__",
        "__custom__",
    ] {
        assert!(!qualified(&raw, name).flags.is_entry(), "{name}");
    }
}

#[test]
fn a_marker_in_a_header_comment_or_the_module_docstring_marks_the_file_generated() {
    for source in [
        b"# Generated by codegen. DO NOT EDIT.\n\ndef emitted():\n    pass\n".as_slice(),
        b"#!/usr/bin/env python\n\"\"\"\nModule summary.\n\n@generated by a tool.\n\"\"\"\n\n\
          def emitted():\n    pass\n",
        b"r\"\"\"\n@generated by a tool.\n\"\"\"\n\ndef emitted():\n    pass\n",
        b"u\"\"\"DO NOT EDIT.\"\"\"\n\ndef emitted():\n    pass\n",
        b"\"\"\"\nsummary\n\"\"\"\n# @generated\ndef emitted():\n    pass\n",
        b"\"\"\"Summary.\"\"\"  # @generated\ndef emitted():\n    pass\n",
        b"\"\"\"Quotes \\\"\"\" stay inside.\n@generated\n\"\"\"\ndef emitted():\n    pass\n",
        b"r'''Raw \\''' too. @generated'''\ndef emitted():\n    pass\n",
        b"\xEF\xBB\xBF# @generated\ndef emitted():\n    pass\n",
    ] {
        assert_eq!(
            generated(source),
            [true],
            "{}",
            String::from_utf8_lossy(source)
        );
    }
}

#[test]
fn a_plain_header_a_mention_or_a_marker_after_code_marks_nothing() {
    for source in [
        b"def from_proto():\n    pass\n".as_slice(),
        b"\"\"\"Module summary, hand written.\"\"\"\n\ndef f():\n    pass\n",
        b"\"\"\"Codegen helpers.\n\nEmitted files carry a `@generated` / `DO NOT EDIT` header.\n\"\"\"\n\n\
          def f():\n    pass\n",
        b"\"\"\"\nmodule summary\n\"\"\" ; CONST = 1\n# DO NOT EDIT\ndef real():\n    pass\n",
        b"\"\"\"summary\"\"\"; S = \"DO NOT EDIT\"\n\ndef coded():\n    pass\n",
        b"\"\"\"summary\"\"\"; S = 1\n# @generated\ndef coded():\n    pass\n",
        b"r\"\"\"Raw module summary.\"\"\"\n\ndef f():\n    pass\n",
        b"# a `@generated` / `DO NOT EDIT` marker\ndef f():\n    pass\n",
        b"import os\n# @generated\ndef f():\n    pass\n",
    ] {
        let flags = generated(source);

        assert_ne!(flags.len(), 0, "{}", String::from_utf8_lossy(source));
        assert!(!flags.contains(&true), "{}", String::from_utf8_lossy(source));
    }
}

#[test]
fn python_claims_its_extensions_in_any_case() {
    let python = Python::new();

    assert!(python.claims("pkg/service.py"));
    assert!(python.claims("pkg/stub.PYI"));
    assert!(!python.claims("notes.md"));
    assert!(!python.claims("py"));
}

#[test]
fn the_fingerprint_names_the_crate_and_stays_put() {
    let first = Python::new().fingerprint().to_vec();

    assert!(first.starts_with(b"kartta-lang-python\0"));
    assert_eq!(Python::new().fingerprint(), first.as_slice());
}
