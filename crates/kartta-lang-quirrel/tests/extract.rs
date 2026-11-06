//! What one Quirrel file extracts to from its bytes alone, and where the doc comment above a definition starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Access, Conventions, FILE_SCOPE, IncludedRuntimeMembers, Language, OwnMembers, RawExtraction,
    Receiver, ReceiverMembers, Symbol, SymbolId, SymbolKind, UnlistedReceivers, UntypedOwners,
};
use kartta_lang_quirrel::Quirrel;

const SAMPLE: &str = r#"
import "eventbus"
from "darg" import hdpx

function greet(name) {
  print(name)
}

class Foo {
  constructor(x) {
    this.x = x
  }
  function method(a) {
    return helper(a)
  }
}
"#;

fn extract(source: &str) -> RawExtraction {
    Quirrel::new().extract(source.as_bytes())
}

fn named<'a>(raw: &'a RawExtraction, name: &str) -> &'a Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name} in {:?}", raw.symbols))
}

fn id_of(raw: &RawExtraction, qualified_name: &str) -> SymbolId {
    let position = raw
        .symbols
        .iter()
        .position(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("no symbol {qualified_name} in {:?}", raw.symbols));
    SymbolId(
        u32::try_from(position).unwrap_or_else(|_| panic!("{qualified_name} is past u32::MAX")),
    )
}

fn caller_of(raw: &RawExtraction, callee: &str) -> SymbolId {
    raw.calls
        .iter()
        .find(|call| call.callee == callee)
        .unwrap_or_else(|| panic!("no call of {callee} in {:?}", raw.calls))
        .site_of
}

fn kinds(raw: &RawExtraction) -> Vec<(&str, SymbolKind)> {
    raw.symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect()
}

fn callers(raw: &RawExtraction) -> Vec<(&str, SymbolId)> {
    raw.calls
        .iter()
        .map(|call| (call.callee.as_str(), call.site_of))
        .collect()
}

fn receivers(raw: &RawExtraction) -> Vec<(&str, Option<&str>)> {
    raw.calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect()
}

fn uses(raw: &RawExtraction) -> Vec<(&str, SymbolId, u32)> {
    raw.uses
        .iter()
        .map(|used| {
            assert_eq!(
                (&used.receiver, used.access),
                (&Receiver::Bare, Access::Read),
                "{used:?}"
            );
            (used.name.as_str(), used.site_of, used.line)
        })
        .collect()
}

fn imports(raw: &RawExtraction) -> Vec<(&str, Vec<&str>)> {
    raw.directives
        .iter()
        .map(|directive| {
            let names = directive.names.iter().map(String::as_str).collect();
            (directive.spec.as_str(), names)
        })
        .collect()
}

#[test]
fn quirrel_claims_files_with_the_nut_extension_in_any_case() {
    let quirrel = Quirrel::new();

    assert!(quirrel.claims("ui/hud/main.nut"));
    assert!(quirrel.claims("UI/MAIN.NUT"));
    assert!(!quirrel.claims("ui/.sqconfig"));
    assert!(!quirrel.claims("ui/main.nuts"));
    assert_eq!(quirrel.name(), "quirrel");
    assert_eq!(quirrel.family(), "quirrel");
}

#[test]
fn a_bare_call_in_a_member_reaches_the_class_first() {
    let conventions = Quirrel::new().conventions();
    let mut expected = Conventions::default();
    expected.own_members = OwnMembers::First;
    expected.runtime_methods = conventions.runtime_methods;
    expected.included_runtime_members = IncludedRuntimeMembers::SpelledOrOwn;
    expected.untyped_owners = UntypedOwners::Tables;
    expected.base_receivers = &["base"];
    expected.receiver_members = ReceiverMembers::InheritedUpToFields;
    expected.own_type_receivers = Some(&["this"]);
    expected.unlisted_receivers = UnlistedReceivers::OwnMemberFieldsAsGuess;
    expected.includers = Some(
        "the scripts whose import, from ... import, require or require_optional of a string literal names this file",
    );

    assert_eq!(conventions, expected);
}

#[test]
fn a_name_taken_as_a_value_is_a_use_of_it() {
    let raw = extract(
        "function handler(e) { print(e) }
function setup() {
  subscribe(\"evt\", handler)
  local cb = handler
  return { onClick = handler, handler }
}
subscribe(\"ready\", ::handler)
",
    );

    let setup = id_of(&raw, "setup");
    assert_eq!(
        uses(&raw),
        [
            ("handler", setup, 3),
            ("handler", setup, 4),
            ("handler", setup, 5),
            ("handler", setup, 5),
            ("handler", FILE_SCOPE, 7),
        ]
    );
}

#[test]
fn a_parameter_a_local_or_a_name_being_declared_is_no_use() {
    let raw = extract(
        "function handler() {}
function run(handler) { register(handler) }
function other() {
  register(handler)
  local handler = 1
  register(handler)
  foreach (k, v in list) { keep(k, v) }
  try { go() } catch (err) { report(err) }
  let { a, b = 2 } = pair
  use(a, b)
  keep(object.field)
}
let m = { handler = 1 }
handler = 2
::handler = 3
::handler <- 4
",
    );

    assert_eq!(
        uses(&raw),
        [
            ("list", id_of(&raw, "other"), 7),
            ("pair", id_of(&raw, "other"), 9)
        ]
    );
}

#[test]
fn a_call_through_a_parameter_or_a_local_is_not_a_callee() {
    let raw = extract(
        "function each(visit, list) { foreach (item in list) { visit(item); item() } }
function run(cb = null) {
  let step = make()
  step()
  cb?(1)
  try { go() } catch (err) { err() }
  later(@() cb())
}
function after() { visit(1); step(); cb(); item() }
",
    );

    let (run, after) = (id_of(&raw, "run"), id_of(&raw, "after"));
    assert_eq!(
        callers(&raw),
        [
            ("make", run),
            ("go", run),
            ("later", run),
            ("visit", after),
            ("step", after),
            ("cb", after),
            ("item", after),
        ]
    );
}

#[test]
fn a_call_through_a_value_keeps_a_function_or_a_class_its_file_defines_or_the_slot_it_destructures()
{
    let raw = extract(
        "function onDone() {}
const onFail = 1
class Step {}
function finish(onDone, onFail, Step) { onDone(); onFail(); Step() }
function setup(cfg) {
  let { build } = cfg
  build()
}
",
    );

    assert_eq!(
        callers(&raw),
        [
            ("onDone", id_of(&raw, "finish")),
            ("Step", id_of(&raw, "finish")),
            ("build", id_of(&raw, "setup"))
        ]
    );
}

#[test]
fn the_squirrel_extends_keyword_is_no_use_and_its_base_is_one() {
    let raw = extract(
        "class Circle extends Shape {}\nlet C = class extends Base {}\nlocal list = [1 2 extends]\n",
    );

    assert_eq!(
        uses(&raw),
        [
            ("Shape", FILE_SCOPE, 1),
            ("Base", FILE_SCOPE, 2),
            ("extends", FILE_SCOPE, 3)
        ]
    );
}

#[test]
fn the_runtime_methods_ascend_without_a_repeat_and_name_the_built_in_delegates() {
    let methods = Quirrel::new().conventions().runtime_methods;

    assert!(methods.is_sorted_by(|earlier, later| earlier < later));
    for method in ["len", "rawin", "tostring", "each", "bindenv"] {
        assert!(methods.binary_search(&method).is_ok(), "{method}");
    }
}

#[test]
fn the_runtime_methods_name_the_engine_s_native_values_and_not_the_scripts_helpers() {
    let methods = Quirrel::new().conventions().runtime_methods;

    for method in [
        "get",
        "set",
        "mutate",
        "subscribe",
        "getBlockByName",
        "setFrom",
        "getScene",
        "getTemplateByName",
    ] {
        assert!(methods.binary_search(&method).is_ok(), "{method}");
    }
    for method in [
        "isEmpty",
        "isEqual",
        "isValid",
        "isVisible",
        "isEnabled",
        "getName",
        "getTypeName",
        "size",
    ] {
        assert!(methods.binary_search(&method).is_err(), "{method}");
    }
}

#[test]
fn the_fingerprint_stays_put_and_names_the_crate() {
    let quirrel = Quirrel::new();
    let fingerprint = quirrel.fingerprint();

    assert!(fingerprint.starts_with(b"kartta-lang-quirrel "));
    assert_eq!(Quirrel::new().fingerprint(), fingerprint);
}

#[test]
fn extracts_functions_classes_methods_and_the_constructor() {
    let raw = extract(SAMPLE);

    let symbols: Vec<(&str, SymbolKind, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.kind,
                symbol.owner.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        symbols,
        [
            ("greet", SymbolKind::Function, None),
            ("Foo", SymbolKind::Class, None),
            ("Foo.constructor", SymbolKind::Method, Some("Foo")),
            ("Foo.method", SymbolKind::Method, Some("Foo")),
        ]
    );
    assert!(raw.symbols.iter().all(|symbol| symbol.exported));
    assert_eq!(caller_of(&raw, "print"), id_of(&raw, "greet"));
    assert_eq!(caller_of(&raw, "helper"), id_of(&raw, "Foo.method"));
}

#[test]
fn global_bindings_class_slots_enums_and_consts_extract() {
    let raw = extract(
        "::x <- function() { gx() }\nclass C { m = function() { helper() } }\nenum Color { Red, Green = 2 }\n\
         const MAX = 10\nlet Anon = class { am = function() { ahelper() } }\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("x", SymbolKind::Function),
            ("C", SymbolKind::Class),
            ("C.m", SymbolKind::Method),
            ("Color", SymbolKind::Enum),
            ("Color.Red", SymbolKind::EnumVariant),
            ("Color.Green", SymbolKind::EnumVariant),
            ("MAX", SymbolKind::Constant),
            ("Anon", SymbolKind::Class),
            ("Anon.am", SymbolKind::Method),
        ]
    );
    assert_eq!(caller_of(&raw, "gx"), SymbolId(0));
    assert_eq!(caller_of(&raw, "helper"), id_of(&raw, "C.m"));
    assert_eq!(caller_of(&raw, "ahelper"), id_of(&raw, "Anon.am"));
}

#[test]
fn local_function_and_class_sugar_declare_symbols() {
    let raw = extract(
        "function a() { return 1 }\nlocal function lf() { return 2 }\nlet function lf2() { tuna2() }\n\
         local class LC {\n  function lm() { inner() }\n}\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("a", SymbolKind::Function),
            ("lf", SymbolKind::Function),
            ("lf2", SymbolKind::Function),
            ("LC", SymbolKind::Class),
            ("LC.lm", SymbolKind::Method),
        ]
    );
    assert_eq!(
        callers(&raw),
        [("tuna2", SymbolId(2)), ("inner", SymbolId(4))]
    );
}

#[test]
fn a_const_function_is_a_function_that_owns_its_calls() {
    let raw = extract("const function cf() { helper() }\nconst MAX = 10\n");

    assert_eq!(
        kinds(&raw),
        [("cf", SymbolKind::Function), ("MAX", SymbolKind::Constant)]
    );
    assert_eq!(callers(&raw), [("helper", SymbolId(0))]);
}

#[test]
fn a_module_table_owns_its_declared_functions_and_no_other_table_does() {
    let raw = extract(
        "let t = {\n  key = 1, function tm() { body() }\n  slot = function() {}\n}\n\
         function f() {\n  let inner = { function im() {} }\n}\nreturn { function rm() {} }\n",
    );

    let owned: Vec<(&str, SymbolKind, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.kind,
                symbol.owner.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        owned,
        [
            ("t.key", SymbolKind::Field, Some("t")),
            ("t.tm", SymbolKind::Method, Some("t")),
            ("slot", SymbolKind::Function, None),
            ("f", SymbolKind::Function, None),
            ("im", SymbolKind::Function, None),
            ("rm", SymbolKind::Function, None),
        ]
    );
    assert_eq!(callers(&raw), [("body", id_of(&raw, "t.tm"))]);
}

#[test]
fn a_function_value_takes_its_binding_name_before_its_own() {
    let raw = extract(
        "let f = function inner() { a() }\nlet g = @ lam() b()\nobj.h <- function own() { c() }\nclass C {\n  \
         m = function own() { d() }\n}\nrun(function onDone() { e() })\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("f", SymbolKind::Function),
            ("g", SymbolKind::Function),
            ("obj.h", SymbolKind::Method),
            ("C", SymbolKind::Class),
            ("C.m", SymbolKind::Method),
            ("onDone", SymbolKind::Function),
        ]
    );
    assert_eq!(
        callers(&raw),
        [
            ("a", SymbolId(0)),
            ("b", SymbolId(1)),
            ("c", SymbolId(2)),
            ("d", SymbolId(4)),
            ("run", FILE_SCOPE),
            ("e", SymbolId(5)),
        ]
    );
}

#[test]
fn computed_and_string_keys_bind_no_name() {
    let raw = extract(
        "function first() {}\nlet handlers = {\n  [EventA] = function() { onA() },\n  \
         [\"k\"] = function() { onK() },\n  \"q\": @() onQ(),\n  [EventB] = function onB() { handleB() },\n}\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("first", SymbolKind::Function),
            ("onB", SymbolKind::Function)
        ]
    );
    assert_eq!(
        callers(&raw),
        [
            ("onA", FILE_SCOPE),
            ("onK", FILE_SCOPE),
            ("onQ", FILE_SCOPE),
            ("handleB", SymbolId(1)),
        ]
    );
}

#[test]
fn let_and_local_bindings_name_their_function_values() {
    let raw = extract(
        "let f = function(a) { helper(a) }\nlocal h = function() { inner() }\nlet g = @() g2()\n\
         let t: int = function() { thelper() }\nlet t2: int = @() t2body()\n",
    );

    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert_eq!(names, ["f", "h", "g", "t", "t2"]);
    assert_eq!(
        callers(&raw),
        [
            ("helper", SymbolId(0)),
            ("inner", SymbolId(1)),
            ("g2", SymbolId(2)),
            ("thelper", SymbolId(3)),
            ("t2body", SymbolId(4)),
        ]
    );
}

#[test]
fn exponent_floats_docstrings_and_trailing_commas_keep_their_callables() {
    let raw = extract(
        "function scale(a, eps = 1e-6) {\n  @@\"Scales a.\n  The text spans lines.\"\n  return mul(a, eps,)\n}\n\
         let f = @(a, b,) add(a, b)\nfunction after() { done() }\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("scale", SymbolKind::Function),
            ("f", SymbolKind::Function),
            ("after", SymbolKind::Function),
        ]
    );
    assert_eq!(
        callers(&raw),
        [
            ("mul", SymbolId(0)),
            ("add", SymbolId(1)),
            ("done", SymbolId(2))
        ]
    );
}

#[test]
fn from_and_import_are_names_after_a_declaration() {
    let raw = extract(
        "function pick(from, to) { return rnd(from, to) }\nlocal from = 0\nfrom = pick(from, 5)\n\
         local import = from\nimport = 1\n",
    );

    assert_eq!(imports(&raw), []);
    assert_eq!(kinds(&raw), [("pick", SymbolKind::Function)]);
    assert_eq!(callers(&raw), [("rnd", SymbolId(0)), ("pick", FILE_SCOPE)]);
}

#[test]
fn async_functions_own_their_awaited_calls() {
    let raw = extract("async function fetchAll() {\n  let r = await load()\n  use(r)\n}\n");

    assert_eq!(callers(&raw), [("load", SymbolId(0)), ("use", SymbolId(0))]);
}

#[test]
fn foreach_body_calls_keep_the_enclosing_callable() {
    let raw = extract("function f() {\n  foreach ([a, b] in t) {\n    helper(a)\n  }\n}\n");

    assert_eq!(callers(&raw), [("helper", SymbolId(0))]);
}

#[test]
fn a_table_call_before_a_function_slot_is_not_captured_by_that_slot() {
    let raw = extract(
        "function outer() {\n  local t = {\n    x = make()\n    f = function() { inner() }\n  }\n}\n",
    );

    assert_eq!(caller_of(&raw, "make"), id_of(&raw, "outer"));
    assert_eq!(caller_of(&raw, "inner"), id_of(&raw, "f"));
}

#[test]
fn a_newslot_keeps_a_nested_receiver_as_owner() {
    let raw = extract("gui.Widget.build <- function() { paint() }\n");

    let build = named(&raw, "build");
    assert_eq!(build.kind, SymbolKind::Method);
    assert_eq!(build.owner.as_deref(), Some("gui.Widget"));
    assert_eq!(build.qualified_name, "gui.Widget.build");
    assert_eq!(caller_of(&raw, "paint"), id_of(&raw, "gui.Widget.build"));
}

#[test]
fn a_newslot_receiver_call_is_not_captured_by_the_assigned_function() {
    let raw = extract("function outer() {\n  factory().f <- function() { y() }\n}\n");

    assert_eq!(caller_of(&raw, "factory"), id_of(&raw, "outer"));
    assert_eq!(caller_of(&raw, "y"), id_of(&raw, "factory().f"));
}

#[test]
fn member_calls_keep_the_whole_receiver_expression() {
    let raw = extract(
        "function f() {\n  obj.doThing()\n  ::globalFn()\n  obj?.maybe()\n  callable?()\n  recv.tail?()\n  \
         host.$slot()\n  peer?.$nslot()\n  a.b.c()\n}\n",
    );

    assert_eq!(
        receivers(&raw),
        [
            ("doThing", Some("obj")),
            ("globalFn", None),
            ("maybe", Some("obj")),
            ("callable", None),
            ("tail", Some("recv")),
            ("slot", Some("host")),
            ("nslot", Some("peer")),
            ("c", Some("a.b")),
        ]
    );
}

#[test]
fn callables_carry_their_header_as_signature() {
    let raw = extract(
        "function named(a, b = 1): int { return a }\nlet f = function(a, b) { x() }\n\
         local g = @(x) x + 1\nlet t = {\n  build = function(p) {}\n  onClick = @() null\n}\n\
         obj.f <- function(a) {}\nh = function(q) {}\n::r <- function(s) {}\nclass C {\n  \
         constructor(a, b) {}\n  function m(v: int): int { return v }\n  static sm = function(z) {}\n  \
         lam = @(w) w\n  static function sf(q) {}\n}\nlet a = 1, later = function(k) {}\n\
         function multi(a,\n    b) {}\nasync function af(a) {}\nlet al = async @(x) x\n\
         let function lf(n) {}\nconst function cf(c) {}\nrun(function onDone(e) {})\n\
         enum E { X }\nconst K = 1\n",
    );

    let signatures: Vec<(&str, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.signature.as_deref()))
        .collect();
    assert_eq!(
        signatures,
        [
            ("named", Some("function named(a, b = 1): int")),
            ("f", Some("let f = function(a, b)")),
            ("g", Some("local g = @(x)")),
            ("build", Some("build = function(p)")),
            ("onClick", Some("onClick = @()")),
            ("obj.f", Some("obj.f <- function(a)")),
            ("h", Some("h = function(q)")),
            ("r", Some("::r <- function(s)")),
            ("C", None),
            ("C.constructor", Some("constructor(a, b)")),
            ("C.m", Some("function m(v: int): int")),
            ("C.sm", Some("static sm = function(z)")),
            ("C.lam", Some("lam = @(w)")),
            ("C.sf", Some("static function sf(q)")),
            ("later", Some("later = function(k)")),
            ("multi", Some("function multi(a,\n    b)")),
            ("af", Some("async function af(a)")),
            ("al", Some("let al = async @(x)")),
            ("lf", Some("let function lf(n)")),
            ("cf", Some("const function cf(c)")),
            ("onDone", Some("function onDone(e)")),
            ("E", None),
            ("E.X", None),
            ("K", None),
        ]
    );
}

#[test]
fn class_data_slots_emit_fields_owned_by_the_class() {
    let raw = extract(
        "class C(B) {\n  [k] = 2\n  count = 0\n  static TAG = \"c\"\n  t = {\n    a = 1\n  }\n  neg =\n    -1\n  \
         doc = @\"a\nb\"\n  arr = [1]\n  call = make()\n  flag = true\n  off = false\n  ref = other\n  \
         ratio = 0.5\n  sep = ','\n  function m() {}\n  slot = function() {}\n}\n\
         let D = class {\n  held = null\n}\nregister(class {\n  hidden = 1\n})\n",
    );

    let fields: Vec<(&str, SymbolKind, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.kind,
                symbol.signature.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("C", SymbolKind::Class, None),
            ("C.count", SymbolKind::Field, Some("count = 0")),
            ("C.TAG", SymbolKind::Field, Some("static TAG = \"c\"")),
            ("C.t", SymbolKind::Field, None),
            ("C.neg", SymbolKind::Field, Some("neg =\n    -1")),
            ("C.doc", SymbolKind::Field, Some("doc = @\"a\nb\"")),
            ("C.arr", SymbolKind::Field, None),
            ("C.call", SymbolKind::Field, None),
            ("C.flag", SymbolKind::Field, Some("flag = true")),
            ("C.off", SymbolKind::Field, Some("off = false")),
            ("C.ref", SymbolKind::Field, Some("ref = other")),
            ("C.ratio", SymbolKind::Field, Some("ratio = 0.5")),
            ("C.sep", SymbolKind::Field, Some("sep = ','")),
            ("C.m", SymbolKind::Method, Some("function m()")),
            ("C.slot", SymbolKind::Method, Some("slot = function()")),
            ("D", SymbolKind::Class, None),
            ("D.held", SymbolKind::Field, Some("held = null")),
        ]
    );

    let two_line_field = named(&raw, "neg");
    assert_eq!(two_line_field.owner.as_deref(), Some("C"));
    assert_eq!((two_line_field.line, two_line_field.end_line), (8, 9));
    assert_eq!(named(&raw, "C").parents, ["B"]);
    assert_eq!(
        callers(&raw),
        [("make", FILE_SCOPE), ("register", FILE_SCOPE)]
    );
}

#[test]
fn fields_of_a_nested_class_value_emit_in_source_order() {
    let raw = extract("class Outer {\n  inner = class {\n    b = 2\n  }\n  c = 3\n}\n");

    assert_eq!(
        kinds(&raw),
        [
            ("Outer", SymbolKind::Class),
            ("inner", SymbolKind::Class),
            ("inner.b", SymbolKind::Field),
            ("Outer.c", SymbolKind::Field),
        ]
    );
}

#[test]
fn module_table_data_slots_emit_fields_owned_by_the_binding() {
    let raw = extract(
        "let cfg = {\n  [k] = 1,\n  \"s\": 2\n  size = 10\n  name = \"x\"\n  nested = { inner = 1 }\n  \
         build = function() {}\n  onClick = @() go()\n  Kind = class {}\n}\n\
         local a = 1, b: table = { c = 2 }\nlet x = make({ arg = 5 })\n\
         if (on) {\n  let hidden = { d = 3 }\n}\nfunction f() {\n  let local_t = { e = 4 }\n}\n\
         return { ret = 6 }\n",
    );

    let owned: Vec<(&str, SymbolKind, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.kind,
                symbol.owner.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        owned,
        [
            ("cfg.size", SymbolKind::Field, Some("cfg")),
            ("cfg.name", SymbolKind::Field, Some("cfg")),
            ("cfg.nested", SymbolKind::Field, Some("cfg")),
            ("build", SymbolKind::Function, None),
            ("onClick", SymbolKind::Function, None),
            ("Kind", SymbolKind::Class, None),
            ("b.c", SymbolKind::Field, Some("b")),
            ("f", SymbolKind::Function, None),
        ]
    );
    let signatures: Vec<Option<&str>> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.signature.as_deref())
        .collect();
    assert_eq!(
        signatures,
        [
            Some("size = 10"),
            Some("name = \"x\""),
            None,
            Some("build = function()"),
            Some("onClick = @()"),
            None,
            Some("c = 2"),
            Some("function f()"),
        ]
    );
    assert_eq!(callers(&raw), [("go", SymbolId(4)), ("make", FILE_SCOPE)]);
}

#[test]
fn a_computed_callee_records_no_call() {
    let raw = extract("function run() {\n  table[key]()\n  make()()\n}\n");

    assert_eq!(callers(&raw), [("make", SymbolId(0))]);
}

#[test]
fn a_function_nested_in_a_method_body_is_a_free_function() {
    let raw = extract("class Foo {\n  function method() {\n    function helper() {}\n  }\n}\n");

    assert_eq!(
        kinds(&raw),
        [
            ("Foo", SymbolKind::Class),
            ("Foo.method", SymbolKind::Method),
            ("helper", SymbolKind::Function),
        ]
    );
}

#[test]
fn only_a_marker_in_the_leading_comments_marks_generated_code() {
    let generated = |source: &str| {
        extract(source)
            .symbols
            .iter()
            .any(|symbol| symbol.flags.is_generated())
    };

    assert!(generated("// @generated\nfunction f() {}\n"));
    assert!(generated("/* DO NOT EDIT */\nfunction f() {}\n"));
    assert!(generated("// Code generated by tool.\nfunction f() {}\n"));
    assert!(!generated("function f() {\n  print(\"@generated\")\n}\n"));
    assert!(!generated("function f() {}\n// @generated\n"));
}

#[test]
fn import_directives_record_their_module_and_the_names_they_take() {
    let raw = extract(
        "from \"darg\" import hdpx\nfrom \"math\" import sin, cos\nfrom \"string\" import *\nimport \"eventbus\"\n\
         import \"datacache\" as cache\nfrom \"m\" import a as b\nfrom \"n\" import c, d as e\n\
         import @\"verbatim\"\nfrom \"x\" import f, g,\n  h\n\nrun()\n",
    );

    assert_eq!(
        imports(&raw),
        [
            ("darg", vec!["hdpx"]),
            ("math", vec!["sin", "cos"]),
            ("string", vec![]),
            ("eventbus", vec![]),
            ("datacache", vec![]),
            ("m", vec!["a"]),
            ("n", vec!["c", "d"]),
            ("verbatim", vec![]),
            ("x", vec!["f", "g", "h"]),
        ]
    );
    let lines: Vec<u32> = raw
        .directives
        .iter()
        .map(|directive| directive.line)
        .collect();
    assert_eq!(lines, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn a_loader_call_with_a_string_literal_is_an_import_and_no_call() {
    let raw = extract(
        "require(\"%ui/style.nut\")\nlocal m = require(\"scripts/m.nut\")\nrequire_optional(@\"maybe.nut\")\n\n\
         function f() {\n  require(pathVar)\n  require($\"{dir}/late.nut\")\n  require(\"\")\n}\n",
    );

    assert_eq!(
        imports(&raw),
        [
            ("%ui/style.nut", vec![]),
            ("scripts/m.nut", vec![]),
            ("maybe.nut", vec![])
        ]
    );
    assert_eq!(
        callers(&raw),
        [("require", SymbolId(0)), ("require", SymbolId(0))]
    );

    let member = extract("function g() {\n  obj.require(\"x.nut\")\n}\n");
    assert_eq!(imports(&member), []);
    assert_eq!(receivers(&member), [("require", Some("obj"))]);
}

#[test]
fn a_bound_class_value_takes_its_binding_name_and_owns_its_members() {
    let raw = extract(
        "let Widget = class(Base) {\n  bar = 1\n  constructor() { init() }\n  function build() { paint() }\n  \
         onDraw = @() draw()\n}\nlocal Plain = class(Other) {}\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("Widget", SymbolKind::Class),
            ("Widget.bar", SymbolKind::Field),
            ("Widget.constructor", SymbolKind::Method),
            ("Widget.build", SymbolKind::Method),
            ("Widget.onDraw", SymbolKind::Method),
            ("Plain", SymbolKind::Class),
        ]
    );
    assert_eq!(named(&raw, "Widget").parents, ["Base"]);
    assert_eq!(named(&raw, "Plain").parents, ["Other"]);
    assert_eq!(
        callers(&raw),
        [
            ("init", SymbolId(2)),
            ("paint", SymbolId(3)),
            ("draw", SymbolId(4))
        ]
    );
}

#[test]
fn class_values_bind_in_slots_and_assignments_and_stay_anonymous_elsewhere() {
    let raw = extract(
        "let t = {\n  [k] = class {},\n  Slot = class { function m() {} },\n}\nAssigned = class { function m() {} }\n\
         ::Root <- class {}\nobj.Member <- class {}\nfunction f() {\n  return class(Base) { function inner() {} }\n}\n\
         class Outer {\n  Nested = class {}\n}\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("Slot", SymbolKind::Class),
            ("Slot.m", SymbolKind::Method),
            ("Assigned", SymbolKind::Class),
            ("Assigned.m", SymbolKind::Method),
            ("Root", SymbolKind::Class),
            ("f", SymbolKind::Function),
            ("inner", SymbolKind::Function),
            ("Outer", SymbolKind::Class),
            ("Nested", SymbolKind::Class),
        ]
    );
}

#[test]
fn assigned_function_values_bind_by_their_target() {
    let raw = extract(
        "function outer() {\n  cb = function() { a() }\n  obj.m = @() b()\n  ::g = function() { c() }\n  \
         ob <- function() { o() }\n  obj[k] = function() { d() }\n}\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("outer", SymbolKind::Function),
            ("cb", SymbolKind::Function),
            ("obj.m", SymbolKind::Method),
            ("g", SymbolKind::Function),
            ("ob", SymbolKind::Function),
        ]
    );
    assert_eq!(
        callers(&raw),
        [
            ("a", SymbolId(1)),
            ("b", SymbolId(2)),
            ("c", SymbolId(3)),
            ("o", SymbolId(4)),
            ("d", SymbolId(0)),
        ]
    );
}

#[test]
fn a_call_before_the_first_function_is_a_call_from_file_scope() {
    let raw = extract("setup()\nfunction run() { go() }\n");

    assert_eq!(callers(&raw), [("setup", FILE_SCOPE), ("go", SymbolId(0))]);
}

#[test]
fn a_missing_assignment_target_binds_no_name() {
    let raw = extract("function outer() {\n  = function() { a() }\n}\n");

    assert_eq!(kinds(&raw), [("outer", SymbolKind::Function)]);
    assert_eq!(callers(&raw), [("a", SymbolId(0))]);
}

#[test]
fn members_of_an_anonymous_class_nested_in_a_named_class_are_ownerless() {
    let raw = extract(
        "class Foo {\n  function m() {\n    register(class {\n      function build() {}\n      \
         constructor() {}\n    })\n  }\n}\n",
    );

    assert_eq!(
        kinds(&raw),
        [
            ("Foo", SymbolKind::Class),
            ("Foo.m", SymbolKind::Method),
            ("build", SymbolKind::Function),
            ("constructor", SymbolKind::Function),
        ]
    );
}

#[test]
fn the_paren_form_of_a_class_records_its_parent() {
    let raw =
        extract("class Foo(Bar) {}\nclass Qux(a.b.Base) {}\nclass Plain {}\nclass Empty() {}\n");

    let parents: Vec<(&str, Vec<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.name.as_str(),
                symbol.parents.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        parents,
        [
            ("Foo", vec!["Bar"]),
            ("Qux", vec!["a.b.Base"]),
            ("Plain", vec![]),
            ("Empty", vec![]),
        ]
    );
}

#[test]
fn an_unbound_function_value_gives_its_calls_and_uses_to_the_function_around_it() {
    for source in [
        "function outer() {\n  register(function() { helper(handler) })\n}\n",
        "function outer() {\n  obj[k] <- function() { helper(handler) }\n}\n",
        "function outer() {\n  subscribe(\"k\", @(e) helper(e, handler))\n}\n",
        "function outer() {\n  return @() helper(handler)\n}\n",
        "function outer() {\n  return @() run(function() { helper(handler) })\n}\n",
    ] {
        let raw = extract(source);

        assert_eq!(
            caller_of(&raw, "helper"),
            id_of(&raw, "outer"),
            "{source:?}"
        );
        let handler = uses(&raw).into_iter().find(|&(name, ..)| name == "handler");
        assert_eq!(
            handler,
            Some(("handler", id_of(&raw, "outer"), 2)),
            "{source:?}"
        );
    }
}

#[test]
fn a_bound_or_named_function_value_keeps_its_calls_and_one_at_file_scope_stays_there() {
    let bound = extract("function outer() {\n  local cb = @(x) helper(x)\n}\n");
    assert_eq!(caller_of(&bound, "helper"), id_of(&bound, "cb"));

    let named = extract("function outer() {\n  run(function inner() { helper() })\n}\n");
    assert_eq!(caller_of(&named, "helper"), id_of(&named, "inner"));

    let top = extract("function outer() {}\nsubscribe(\"k\", @() helper(handler))\n");
    assert_eq!(caller_of(&top, "helper"), FILE_SCOPE);
    assert_eq!(uses(&top), [("handler", FILE_SCOPE, 2)]);
}

#[test]
fn function_valued_bindings_emit_named_symbols_with_scopes() {
    let raw = extract(
        "local t = {\n  setup = function() { init() }\n  f = function() { a() }\n  g = function() { b() }\n}\n\
         obj.teardown <- function() { cleanup() }\nobj?.n <- function() { x() }\nobj.l <- @() go()\n",
    );

    let symbols: Vec<(&str, SymbolKind, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.kind,
                symbol.owner.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        symbols,
        [
            ("setup", SymbolKind::Function, None),
            ("f", SymbolKind::Function, None),
            ("g", SymbolKind::Function, None),
            ("obj.teardown", SymbolKind::Method, Some("obj")),
            ("obj.n", SymbolKind::Method, Some("obj")),
            ("obj.l", SymbolKind::Method, Some("obj")),
        ]
    );
    assert_eq!(
        callers(&raw),
        [
            ("init", SymbolId(0)),
            ("a", SymbolId(1)),
            ("b", SymbolId(2)),
            ("cleanup", SymbolId(3)),
            ("x", SymbolId(4)),
            ("go", SymbolId(5)),
        ]
    );
}

#[test]
fn a_lambda_binds_in_every_function_value_context() {
    let slot =
        extract("let ui = {\n  onFn = function() { fnBody() }\n  onLam = @() lamBody()\n}\n");
    assert_eq!(caller_of(&slot, "lamBody"), id_of(&slot, "onLam"));

    let class = extract("class C {\n  m = @() act()\n}\n");
    assert_eq!(
        kinds(&class),
        [("C", SymbolKind::Class), ("C.m", SymbolKind::Method)]
    );
    assert_eq!(caller_of(&class, "act"), id_of(&class, "C.m"));
}

const DOCUMENTED: &str = "let x = 1
// Parses.
# Fails on nothing.
function parse() {}

/* Renders. */
function render() {}
";

fn doc_line(name: &str) -> u32 {
    let lines: Vec<&[u8]> = DOCUMENTED.lines().map(str::as_bytes).collect();
    let line = named(&extract(DOCUMENTED), name).line;
    Quirrel::new().doc_comments().first_line(&lines, line)
}

#[test]
fn every_comment_form_above_a_function_documents_it() {
    assert_eq!(doc_line("parse"), 2);
    assert_eq!(doc_line("render"), 6);
}
