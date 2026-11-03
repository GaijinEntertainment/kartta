//! What a Python file extracts to from its bytes alone: its symbols qualified by their classes, the calls its
//! functions make, and the directives its imports leave for the resolver; and where the doc comment of a definition
//! starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Access, CallRef, FILE_SCOPE, Language, LocalCall, RawExtraction, Receiver, Symbol, SymbolId,
    SymbolKind,
};
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
fn the_runtime_methods_ascend_without_a_repeat_and_name_the_built_in_types() {
    let methods = Python::new().conventions().runtime_methods;

    assert!(methods.is_sorted_by(|earlier, later| earlier < later));
    for method in ["strip", "append", "items", "read_text"] {
        assert!(methods.binary_search(&method).is_ok(), "{method}");
    }
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
fn only_the_first_name_of_a_chained_assignment_and_no_tuple_is_a_symbol() {
    let raw = extract(b"a = b = 1\nc, d = 1, 2\nclass K:\n    state = 0\n    typed: int = 0\n");
    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(names, ["a", "K", "K.state", "K.typed"]);
    assert_eq!(qualified(&raw, "K.state").kind, SymbolKind::Field);
}

#[test]
fn a_field_a_class_annotates_without_a_value_is_marked_as_declared_alone() {
    let raw = extract(b"class K:\n    declared: int\n    typed: int = 0\n    state = 0\n");

    assert_eq!(qualified(&raw, "K.declared").value_types, [""]);
    assert_eq!(qualified(&raw, "K.typed").value_types, Vec::<String>::new());
    assert_eq!(qualified(&raw, "K.state").value_types, Vec::<String>::new());
}

#[test]
fn an_attribute_a_method_gives_its_instance_is_a_field_declared_once_after_the_class_members() {
    let raw = extract(
        b"class Pool:\n    size: int = 0\n\n    def __init__(this, size):\n        this.size = size\n        \
          this.free = []\n\n        def reset():\n            this.free = []\n            this.dirty = False\n\n\
          \x20   @staticmethod\n    def make(other):\n        other.made = True\n\n    def grow(self):\n        \
          self.free.append(1)\n        self.peak = 2\n        self.label = ''\n\n    @property\n    def label(self):\n\
          \x20       return ''\n\nTAIL = 1\n",
    );
    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(
        names,
        [
            "Pool",
            "Pool.size",
            "Pool.__init__",
            "reset",
            "Pool.make",
            "Pool.grow",
            "Pool.label",
            "Pool.free",
            "Pool.dirty",
            "Pool.peak",
            "TAIL",
        ]
    );
    let free = qualified(&raw, "Pool.free");
    assert_eq!(
        (free.kind, free.owner.as_deref(), free.line),
        (SymbolKind::Field, Some("Pool"), 6)
    );
    assert_eq!(qualified(&raw, "Pool.label").kind, SymbolKind::Method);
    assert_eq!(raw.local_symbols, [id_of(&raw, "reset")]);
    assert_eq!(
        raw.calls,
        [CallRef::new(
            id_of(&raw, "grow"),
            "append".to_owned(),
            Some(String::new()),
            17
        )]
    );
}

#[test]
fn a_method_that_binds_its_receiver_s_name_again_declares_no_field_after_it() {
    let raw = extract(
        b"class K:\n    def f(self):\n        self.early = 1\n        self = other\n        self.extra = 1\n\n    \
          def g(self):\n        for self in items:\n            self.looped = 1\n",
    );
    let fields: Vec<&str> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Field)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(fields, ["K.early"]);
}

#[test]
fn a_parameter_of_a_nested_function_that_takes_the_receiver_s_name_is_no_receiver() {
    let raw = extract(
        b"class K:\n    def f(self=None):\n        self.kept = 1\n        def g(self):\n            \
          self.temp = 1\n        def h():\n            self = other\n            self.gone = 1\n        \
          apply(lambda self: setattr(self, 'x', 1))\n        self.after = 2\n        def shared():\n            \
          nonlocal self\n            self.shared = 3\n\n    @builtins.staticmethod\n    \
          def make(obj):\n        obj.made = True\n",
    );
    let fields: Vec<&str> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Field)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(fields, ["K.kept", "K.after", "K.shared"]);
}

#[test]
fn the_attributes_of_a_class_local_to_a_function_are_local_too() {
    let raw = extract(
        b"def build():\n    class Node:\n        def __init__(self):\n            self.next = None\n    \
          return Node\n",
    );
    let locals: Vec<&str> = raw
        .local_symbols
        .iter()
        .map(|id| raw.symbols[id.0 as usize].qualified_name.as_str())
        .collect();

    assert_eq!(locals, ["Node", "Node.__init__", "Node.next"]);
}

#[test]
fn two_classes_of_one_name_local_to_two_functions_each_keep_their_fields() {
    let raw = extract(
        b"def first():\n    class Node:\n        size = 0\n        def __init__(self):\n            self.value = 1\n\
          \ndef second():\n    class Node:\n        size = 0\n        def __init__(self):\n            \
          self.value = 2\n",
    );
    let fields: Vec<(&str, u32)> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Field)
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.line))
        .collect();

    assert_eq!(
        fields,
        [
            ("Node.size", 3),
            ("Node.value", 5),
            ("Node.size", 9),
            ("Node.value", 11)
        ]
    );
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
        .map(|call| (name(call.site_of), name(call.local), call.line))
        .collect()
}

fn callers_of<'r>(raw: &'r RawExtraction, callee: &str) -> Vec<&'r str> {
    raw.calls
        .iter()
        .filter(|call| call.callee == callee)
        .map(|call| raw.symbols[call.site_of.0 as usize].name.as_str())
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
                raw.symbols[call.site_of.0 as usize].name.as_str(),
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
                raw.symbols[call.site_of.0 as usize].name.as_str(),
                raw.symbols[call.local.0 as usize].line,
            )
        })
        .collect();

    assert_eq!(calls, [("Local", 5), ("outer", 2)]);
}

#[test]
fn a_call_in_a_class_body_at_module_level_stays_a_call_of_its_method() {
    let raw = extract(b"class Table:\n    def column():\n        pass\n    made = column()\n");

    assert_eq!(raw.local_calls, Vec::<LocalCall>::new());
    assert_eq!(callers_of(&raw, "column"), ["Table"]);
}

#[test]
fn a_call_in_a_class_body_is_made_by_the_class() {
    let raw = extract(
        b"def make_default():\n    return 1\n\ndef register(cls):\n    return cls\n\nclass Settings:\n    \
          limit = make_default()\n    register(object)\n    sizes = [make_default() for _ in range(2)]\n\n    \
          def read(self):\n        return make_default()\n\nregister(Settings)\n",
    );
    let mut calls: Vec<(&str, &str, u32)> = raw
        .calls
        .iter()
        .map(|call| {
            let site = if call.site_of == FILE_SCOPE {
                ""
            } else {
                raw.symbols[call.site_of.0 as usize].name.as_str()
            };
            (site, call.callee.as_str(), call.line)
        })
        .collect();
    calls.sort_by_key(|&(_, callee, line)| (line, callee));

    assert_eq!(
        calls,
        [
            ("Settings", "make_default", 8),
            ("Settings", "register", 9),
            ("Settings", "make_default", 10),
            ("read", "make_default", 13),
            ("", "register", 15),
        ]
    );
}

#[test]
fn a_class_evaluates_its_bases_in_the_scope_around_it() {
    let raw = extract(
        b"def outer():\n    def mixin():\n        pass\n    class Local(mixin()):\n        \
          def mixin(self):\n            pass\n        made = mixin()\n    return Local\n",
    );
    let mut lines: Vec<(&str, u32)> = raw
        .local_calls
        .iter()
        .map(|call| {
            (
                raw.symbols[call.site_of.0 as usize].name.as_str(),
                raw.symbols[call.local.0 as usize].line,
            )
        })
        .collect();
    lines.sort_unstable();

    assert_eq!(lines, [("Local", 2), ("Local", 5)]);
}

#[test]
fn a_class_body_reads_the_module_values_it_names() {
    let raw = extract(
        b"import tools\nDEFAULT = 1\n\ndef on_update():\n    pass\n\nclass Panel(tools.Base):\n    \
          size = DEFAULT\n    hook = on_update\n",
    );
    let uses: Vec<(&str, &str, Receiver)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                raw.symbols[site.site_of.0 as usize].name.as_str(),
                site.name.as_str(),
                site.receiver.clone(),
            )
        })
        .collect();

    assert_eq!(
        uses,
        [
            ("Panel", "Base", Receiver::Path("tools".to_owned())),
            ("Panel", "DEFAULT", Receiver::Bare),
            ("Panel", "on_update", Receiver::Bare),
        ]
    );
}

#[test]
fn an_attribute_on_a_def_the_module_binds_names_the_def_as_its_owner() {
    let raw = extract(b"def tick():\n    pass\n\ndef run():\n    return tick.count\n");
    let uses: Vec<(&str, Receiver)> = raw
        .uses
        .iter()
        .map(|site| (site.name.as_str(), site.receiver.clone()))
        .collect();

    assert_eq!(
        uses,
        [
            ("count", Receiver::Path("tick".to_owned())),
            ("tick", Receiver::Bare),
        ]
    );
}

#[test]
fn a_class_body_reads_the_attributes_it_binds_and_none_of_its_targets() {
    let raw = extract(
        b"class Devices:\n    ONE = \"one\"\n    TWO = \"two\"\n    KNOWN = (ONE, TWO)\n    ONE += 1\n\n    \
          def size(self):\n        return 1\n\n    length = property(size)\n",
    );
    let uses: Vec<(&str, &str, Receiver, Access, u32)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                raw.symbols[site.site_of.0 as usize].name.as_str(),
                site.name.as_str(),
                site.receiver.clone(),
                site.access,
                site.line,
            )
        })
        .collect();
    let own = || Receiver::Path("Devices".to_owned());

    assert_eq!(
        uses,
        [
            ("Devices", "ONE", own(), Access::Read, 4),
            ("Devices", "TWO", own(), Access::Read, 4),
            ("Devices", "ONE", own(), Access::Update, 5),
            ("Devices", "size", own(), Access::Read, 10),
        ],
    );
}

#[test]
fn a_class_body_reads_the_object_of_an_attribute_and_gives_a_global_name_to_the_module() {
    let raw = extract(
        b"LIMIT = 1\n\nclass Devices:\n    global LIMIT\n    LIMIT = 2\n    ONE = LIMIT\n    TWO = ONE.real\n",
    );
    let uses: Vec<(&str, Receiver, Access, u32)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                site.name.as_str(),
                site.receiver.clone(),
                site.access,
                site.line,
            )
        })
        .collect();

    assert_eq!(
        uses,
        [
            ("real", Receiver::Path(String::new()), Access::Read, 7),
            ("ONE", Receiver::Path("Devices".to_owned()), Access::Read, 7),
            ("LIMIT", Receiver::Bare, Access::Write, 5),
            ("LIMIT", Receiver::Bare, Access::Read, 6),
        ]
    );
}

#[test]
fn a_class_body_s_names_reach_its_first_iterable_and_nothing_nested_in_it() {
    let raw = extract(
        b"ONE = 0\n\nclass Devices:\n    ONE = 1\n    FIRST = [item for item in ONE]\n    \
          INNER = [ONE for item in ()]\n    LATE = lambda: ONE\n\n    def hidden(self, ONE):\n        \
          return ONE\n\n    def method(self):\n        def nested():\n            return ONE\n        \
          return ONE\n\n    class Inner:\n        TWO = ONE\n",
    );
    let uses: Vec<(&str, Receiver, u32)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                raw.symbols[site.site_of.0 as usize].name.as_str(),
                site.receiver.clone(),
                site.line,
            )
        })
        .collect();

    assert_eq!(
        uses,
        [
            ("Devices", Receiver::Path("Devices".to_owned()), 5),
            ("Devices", Receiver::Bare, 6),
            ("Devices", Receiver::Bare, 7),
            ("nested", Receiver::Bare, 14),
            ("method", Receiver::Bare, 15),
            ("Inner", Receiver::Bare, 18),
        ],
        "a comprehension's first iterable reads the class; its inner part, a lambda, a method, a nested function and \
         a nested class read the module, and a parameter hides both"
    );
}

#[test]
fn module_level_code_reads_the_module_values_it_names() {
    let raw = extract(
        b"import config\nLIMIT = 3\nprint(LIMIT)\nLIMIT += 1\nfor step in range(LIMIT):\n    pass\n\
          shown, hidden = config.SIZE, LIMIT\n\ndef register(f):\n    return f\n\n@register\ndef run():\n    pass\n\n\
          DEFAULT: run = run\nprint(LIMIT := 5)\n",
    );
    let uses: Vec<(SymbolId, &str, Receiver, Access, u32)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                site.site_of,
                site.name.as_str(),
                site.receiver.clone(),
                site.access,
                site.line,
            )
        })
        .collect();

    assert_eq!(
        uses,
        [
            (FILE_SCOPE, "LIMIT", Receiver::Bare, Access::Read, 3),
            (FILE_SCOPE, "LIMIT", Receiver::Bare, Access::Update, 4),
            (FILE_SCOPE, "LIMIT", Receiver::Bare, Access::Read, 5),
            (
                FILE_SCOPE,
                "SIZE",
                Receiver::Path("config".to_owned()),
                Access::Read,
                7
            ),
            (FILE_SCOPE, "LIMIT", Receiver::Bare, Access::Read, 7),
            (FILE_SCOPE, "register", Receiver::Bare, Access::Read, 12),
            (FILE_SCOPE, "run", Receiver::Bare, Access::Type, 16),
            (FILE_SCOPE, "run", Receiver::Bare, Access::Read, 16),
        ],
        "code outside every definition reads the module's names at file scope, an annotation there names a type, and \
         binding a name is no use of it"
    );
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
          for h in items:\n        h()\n    with acquire() as i:\n        i()\n    try:\n        pass\n    \
          except Error as j:\n        j()\n    k = lambda m: m()\n    [n() for n in items]\n    (o := make)\n    \
          o()\n    def inner():\n        return a()\n",
    );
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(callees, ["acquire"]);
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
fn a_bare_call_of_a_builtin_the_module_does_not_bind_records_nothing() {
    let raw = extract(
        b"print(len(items))\ndef f():\n    return list(range(3)), __import__(name)\nclass Panel:\n    size = int()\n\
          def g():\n    global open\n    return open()\n",
    );

    assert_eq!(raw.calls, Vec::<CallRef>::new());
    assert_eq!(raw.local_calls, Vec::<LocalCall>::new());
}

#[test]
fn a_bare_call_of_a_builtin_the_module_or_its_scope_binds_is_still_a_call() {
    let raw = extract(
        b"from app import list\ndef f():\n    return list(), range(), open()\ndef range():\n    pass\n\
          len = make()\nlen()\ndef g():\n    from tools import open\n    return open()\n\
          class Table:\n    def int():\n        pass\n    size = int()\n",
    );
    let callees: Vec<(&str, &str)> = raw
        .calls
        .iter()
        .map(|call| {
            let site = raw
                .symbols
                .get(call.site_of.0 as usize)
                .map_or("", |symbol| symbol.name.as_str());
            (site, call.callee.as_str())
        })
        .collect();

    assert_eq!(
        callees,
        [
            ("f", "list"),
            ("f", "range"),
            ("", "make"),
            ("", "len"),
            ("g", "open"),
            ("Table", "int"),
        ]
    );
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
        .map(|call| call.site_of)
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
    assert_eq!(greet.site_of, id_of(&raw, "_private"));
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
fn a_call_before_the_first_definition_is_from_file_scope_and_a_computed_callee_is_dropped() {
    let raw = extract(b"setup()\ndef f():\n    handlers[0]()\n    run()\n");
    let calls: Vec<(&str, bool)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.site_of == FILE_SCOPE))
        .collect();

    assert_eq!(calls, [("setup", true), ("run", false)]);
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
            ("numpy", vec!["np"]),
            ("collections", vec!["OrderedDict", "defaultdict", "", ""]),
            (".", vec!["sibling", ""]),
        ]
    );
}

#[test]
fn an_aliased_name_is_recorded_by_the_name_it_imports_then_the_alias_it_binds() {
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
            ("a.b", vec!["ab"]),
            ("c", vec![]),
            (".", vec!["sibling", "name", "", "aliased"]),
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

    assert!(first.starts_with(b"kartta-lang-python "));
    assert_eq!(Python::new().fingerprint(), first.as_slice());
}

#[test]
fn a_use_names_its_receiver_s_class_and_reads_a_module_value_the_file_binds() {
    let raw = extract(
        b"from consts import LIMIT\nimport helpers\nVERSION = 1\n\nclass Pool(Base):\n    def grow(self, other):\n\
          \x20       self.size += 1\n        other.size = LIMIT\n        total = VERSION + helpers.STEP\n        \
          Config.DEBUG = True\n        def inner():\n            return self.free\n        \
          return self.items.append(1)\n\ndef bump():\n    global VERSION\n    VERSION += 1\n    VERSION = LATER\n\
          \x20   Config.inner.flag = VERSION\n\nLATER = 2\n\ndef shadow():\n    \
          print(VERSION, item.field, LATER.real, Pool, bump)\n    VERSION = item = 3\n",
    );
    let mut uses: Vec<(&str, &str, Receiver, Access, u32)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                raw.symbols[site.site_of.0 as usize].name.as_str(),
                site.name.as_str(),
                site.receiver.clone(),
                site.access,
                site.line,
            )
        })
        .collect();
    uses.sort_by_key(|&(_, name, _, _, line)| (line, name));
    let path = |written: &str| Receiver::Path(written.to_owned());

    assert_eq!(
        uses,
        [
            ("grow", "size", Receiver::Own, Access::Update, 7),
            ("grow", "LIMIT", Receiver::Bare, Access::Read, 8),
            ("grow", "size", path(""), Access::Write, 8),
            ("grow", "STEP", path("helpers"), Access::Read, 9),
            ("grow", "VERSION", Receiver::Bare, Access::Read, 9),
            ("grow", "DEBUG", path("Config"), Access::Write, 10),
            ("inner", "free", Receiver::Own, Access::Read, 12),
            ("grow", "items", Receiver::Own, Access::Read, 13),
            ("bump", "VERSION", Receiver::Bare, Access::Update, 17),
            ("bump", "LATER", Receiver::Bare, Access::Read, 18),
            ("bump", "VERSION", Receiver::Bare, Access::Write, 18),
            ("bump", "VERSION", Receiver::Bare, Access::Read, 19),
            ("bump", "flag", path(""), Access::Write, 19),
            ("bump", "inner", path("Config"), Access::Read, 19),
            ("shadow", "LATER", Receiver::Bare, Access::Read, 24),
            ("shadow", "Pool", Receiver::Bare, Access::Read, 24),
            ("shadow", "bump", Receiver::Bare, Access::Read, 24),
            ("shadow", "field", path(""), Access::Read, 24),
            ("shadow", "real", path(""), Access::Read, 24),
        ],
        "a name the function binds later is its own, and a module value owns no attribute"
    );
}

#[test]
fn an_annotation_or_a_base_names_a_type_as_one_and_a_member_taken_through_a_class_reads_it() {
    let raw = extract(
        b"import models\n\nclass Palette:\n    default: int = 1\n    shade: models.Shade\n\n\
          def pick(palette: Palette, *rest: list[Palette]) -> Optional[Palette]:\n    \
          chosen: Palette = Palette.default\n    return isinstance(chosen, Palette)\n\n\nclass Dark(Palette):\n    \
          pass\n",
    );
    let mut uses: Vec<(&str, &str, Access, u32)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                raw.symbols[site.site_of.0 as usize].name.as_str(),
                site.name.as_str(),
                site.access,
                site.line,
            )
        })
        .collect();
    uses.sort_by_key(|&(_, name, access, line)| (line, name, access));

    assert_eq!(
        uses,
        [
            ("Palette", "Shade", Access::Type, 5),
            ("pick", "Palette", Access::Type, 7),
            ("pick", "Palette", Access::Type, 7),
            ("pick", "Palette", Access::Type, 7),
            ("pick", "Palette", Access::Read, 8),
            ("pick", "Palette", Access::Type, 8),
            ("pick", "default", Access::Read, 8),
            ("pick", "Palette", Access::Read, 9),
            ("Dark", "Palette", Access::Type, 12),
        ],
        "a name inside an annotation, a parameter's, a return's or a variable's, or in a class's bases names a type, \
         and a name an expression takes a member of or passes reads it"
    );
}

#[test]
fn a_call_in_an_annotation_or_in_a_list_of_bases_reads_the_names_around_its_callee() {
    let raw = extract(
        br#"import bpy

rgb_options = []
other_options = []
LIMIT = 1

class Foo:
    pass

class Panel:
    rgb2: bpy.props.EnumProperty(items=rgb_options)
    plain = bpy.props.EnumProperty(items=other_options)
    bare: Foo
    many: list[Foo]
    quoted: "Foo"
    tagged: Annotated[Foo, measure(wrap(LIMIT), width=Foo)]
    chained: Registry[wrap(LIMIT)[Foo]]

def run(*sizes: Sized(LIMIT), size: Foo = LIMIT) -> Sized(Foo):
    pass

class Capped(capped(Foo, LIMIT), Foo, metaclass=Foo):
    pass
"#,
    );
    let mut uses: Vec<(&str, &str, Access, u32)> = raw
        .uses
        .iter()
        .map(|site| {
            (
                raw.symbols[site.site_of.0 as usize].name.as_str(),
                site.name.as_str(),
                site.access,
                site.line,
            )
        })
        .collect();
    uses.sort_by_key(|&(_, name, access, line)| (line, name, access));

    assert_eq!(
        uses,
        [
            ("Panel", "props", Access::Read, 11),
            ("Panel", "rgb_options", Access::Read, 11),
            ("Panel", "other_options", Access::Read, 12),
            ("Panel", "props", Access::Read, 12),
            ("Panel", "Foo", Access::Type, 13),
            ("Panel", "Foo", Access::Type, 14),
            ("Panel", "Foo", Access::Read, 16),
            ("Panel", "Foo", Access::Type, 16),
            ("Panel", "LIMIT", Access::Read, 16),
            ("Panel", "Foo", Access::Type, 17),
            ("Panel", "LIMIT", Access::Read, 17),
            ("run", "Foo", Access::Read, 19),
            ("run", "Foo", Access::Type, 19),
            ("run", "LIMIT", Access::Read, 19),
            ("run", "LIMIT", Access::Read, 19),
            ("Capped", "Foo", Access::Read, 22),
            ("Capped", "Foo", Access::Type, 22),
            ("Capped", "Foo", Access::Type, 22),
            ("Capped", "LIMIT", Access::Read, 22),
        ],
        "a call's arguments and the path to its callee are read wherever the call sits, and a name outside the call, \
         a subscript's included, still names a type; a string annotation records nothing"
    );
}

fn doc_line(source: &str, name: &str) -> u32 {
    let line = named(&extract(source.as_bytes()), name).line;
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Python::new().doc_comments().first_line(&lines, line)
}

#[test]
fn the_comments_above_the_first_decorator_document_a_definition() {
    let source = "import os

# Parses.
# Fails on nothing.
@cached
@traced
def parse():
    \"\"\"The docstring is in the body.\"\"\"


x = 1
def after():
    pass
";

    assert_eq!(doc_line(source, "parse"), 3);
    assert_eq!(doc_line(source, "after"), 12);
}

#[test]
fn a_signature_leaves_out_its_comments() {
    let raw = extract(b"def walk(a,  # the first\n         b):\n    pass\n");

    assert_eq!(
        named(&raw, "walk").signature.as_deref(),
        Some("def walk(a,\n         b):")
    );
}
