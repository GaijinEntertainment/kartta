//! What one daslang file extracts to from its bytes alone, and where the doc comment above a definition starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Access, BareReceivers, CallsApart, Conventions, FILE_SCOPE, InheritedMembers, Language,
    RawExtraction, Receiver, ReceiverMembers, Symbol, SymbolId, SymbolKind, TypeUses, ValueUses,
};
use kartta_lang_daslang::Daslang;

const SAMPLE: &str = r"
options gen2

require daslib/strings
require math/vec as vmath

struct Base {
  x : int;
  y : float;
}

class Service : Base {
  count : int;
  def area() : int {
    return helper(count)
  }
}

enum Color {
  red,
  green = 2,
  blue
}

typedef MyInt = int

let origin = 0
var counter : int = 1

def helper(n : int) : int {
  return n
}

def run() {
  var s = Service()
  s.area()
  s->reset()
}
";

const COVERAGE_SAMPLE: &str = r"
options gen2

include helperlib

struct Holder {
  typedef Alias = int
}

tuple Pair {
  first : int
  second : float
}

variant Shape {
  circle : int
  square : float
}

bitfield Flags : uint {
  alpha
  beta
}
";

fn extract(source: &str) -> RawExtraction {
    Daslang::new().extract(source.as_bytes())
}

fn named<'a>(raw: &'a RawExtraction, name: &str) -> &'a Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name} in {:?}", raw.symbols))
}

fn id_of(raw: &RawExtraction, name: &str) -> SymbolId {
    let position = raw
        .symbols
        .iter()
        .position(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name} in {:?}", raw.symbols));
    SymbolId(u32::try_from(position).unwrap_or_else(|_| panic!("symbol {name} is past u32::MAX")))
}

fn caller_of(raw: &RawExtraction, callee: &str) -> SymbolId {
    raw.calls
        .iter()
        .find(|call| call.callee == callee)
        .unwrap_or_else(|| panic!("no call of {callee} in {:?}", raw.calls))
        .site_of
}

fn calls(raw: &RawExtraction) -> Vec<(&str, Option<&str>)> {
    raw.calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect()
}

fn used_names(raw: &RawExtraction) -> Vec<&str> {
    raw.uses.iter().map(|used| used.name.as_str()).collect()
}

fn specs(raw: &RawExtraction) -> Vec<&str> {
    raw.directives
        .iter()
        .map(|directive| directive.spec.as_str())
        .collect()
}

fn parses_with_errors(source: &str) -> bool {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_daslang::LANGUAGE.into())
        .unwrap_or_else(|error| panic!("the grammar loads: {error}"));
    parser
        .parse(source, None)
        .unwrap_or_else(|| panic!("{source:?} parses"))
        .root_node()
        .has_error()
}

#[test]
fn daslang_claims_files_with_the_das_extension_in_any_case() {
    let daslang = Daslang::new();

    assert!(daslang.claims("daslib/strings.das"));
    assert!(daslang.claims("GAME/MAIN.DAS"));
    assert!(!daslang.claims("daslib/strings.dasx"));
    assert!(!daslang.claims("das"));
    assert!(!daslang.claims("das/README"));
    assert_eq!(daslang.name(), "daslang");
    assert_eq!(daslang.family(), "daslang");
}

#[test]
fn daslang_conventions_are_the_default_but_macros_value_uses_type_uses_and_receivers() {
    let mut expected = Conventions::default();
    expected.bare_receivers = BareReceivers::Modules;
    expected.inherited_members = InheritedMembers::Found;
    expected.calls_apart = CallsApart::Marked;
    expected.module_seen_everywhere = Some("builtin");
    expected.value_uses = ValueUses::Expanded;
    expected.native_names = &[
        "length", "float2", "float3", "float4", "int2", "int3", "int4", "uint2", "uint3", "uint4",
    ];
    expected.own_type_receivers = Some(&["self"]);
    expected.base_receivers = &["super"];
    expected.receiver_members = ReceiverMembers::Inherited;
    expected.type_uses = TypeUses::Nowhere;
    expected.unrecorded_data = &[
        SymbolKind::Field,
        SymbolKind::Variable,
        SymbolKind::Constant,
    ];
    expected.includers = Some("the files whose require or include names this file");

    assert_eq!(Daslang::new().conventions(), expected);
}

#[test]
fn a_type_named_as_one_is_no_use_of_it() {
    let raw = extract(
        "options gen2
require shapes
struct Point {
  x : int
}
typedef Alias = Point
def draw(p : Point; a : Alias?; all : array<Point>) : Point {
  var copy : Point = p
  return copy
}
",
    );

    let declared: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    for name in ["Point", "Alias", "draw"] {
        assert!(declared.contains(&name), "{name} in {declared:?}");
    }
    assert!(raw.uses.is_empty(), "{:?}", raw.uses);
}

#[test]
fn a_read_or_a_write_of_a_field_a_variable_or_a_constant_is_no_use_of_it() {
    let raw = extract(
        "options gen2
let {
  LIMIT = 10
}
var {
  counter : int
}
struct Point {
  x : int
}
def bump(var p : Point) : int {
  counter += LIMIT
  p.x = counter
  return p.x
}
",
    );

    let data: Vec<(&str, SymbolKind)> = raw
        .symbols
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::Field | SymbolKind::Variable | SymbolKind::Constant
            )
        })
        .map(|symbol| (symbol.name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        data,
        [
            ("LIMIT", SymbolKind::Constant),
            ("counter", SymbolKind::Variable),
            ("x", SymbolKind::Field)
        ]
    );
    assert!(raw.uses.is_empty(), "{:?}", raw.uses);
    assert_eq!(
        Daslang::new().conventions().unrecorded_data,
        [
            SymbolKind::Field,
            SymbolKind::Variable,
            SymbolKind::Constant
        ]
    );
}

#[test]
fn a_function_taken_by_its_address_is_a_use_of_it() {
    let raw = extract(
        "options gen2
def handler(x : int) {
}
def setup() {
  register(@@handler)
  let typed = @@<(x : int) : void> handler
  sort(items, @@cmp::less)
  bind(@@$c(\"ctor`{hash}\"))
}
let {
  callback = @@handler
}
",
    );

    let uses: Vec<(&str, &Receiver, SymbolId, u32)> = raw
        .uses
        .iter()
        .map(|used| {
            assert_eq!(used.access, Access::Read, "{used:?}");
            (used.name.as_str(), &used.receiver, used.site_of, used.line)
        })
        .collect();
    let setup = id_of(&raw, "setup");
    assert_eq!(
        uses,
        [
            ("handler", &Receiver::Bare, setup, 5),
            ("handler", &Receiver::Bare, setup, 6),
            ("less", &Receiver::Qualifier("cmp".to_owned()), setup, 7),
            ("handler", &Receiver::Bare, FILE_SCOPE, 11),
        ]
    );
}

#[test]
fn the_fingerprint_stays_put_and_names_the_crate() {
    let daslang = Daslang::new();
    let fingerprint = daslang.fingerprint();

    assert!(fingerprint.starts_with(b"kartta-lang-daslang "));
    assert_eq!(Daslang::new().fingerprint(), fingerprint);
}

#[test]
fn extracts_functions_types_members_and_enumerators() {
    let raw = extract(SAMPLE);

    let symbols: Vec<(&str, SymbolKind, Option<&str>, bool)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            let owner = symbol.owner.as_deref();
            (
                symbol.qualified_name.as_str(),
                symbol.kind,
                owner,
                symbol.exported,
            )
        })
        .collect();
    assert_eq!(
        symbols,
        [
            ("Base", SymbolKind::Struct, None, true),
            ("Base.x", SymbolKind::Field, Some("Base"), true),
            ("Base.y", SymbolKind::Field, Some("Base"), true),
            ("Service", SymbolKind::Class, None, true),
            ("Service.count", SymbolKind::Field, Some("Service"), true),
            ("Service.area", SymbolKind::Method, Some("Service"), true),
            ("Color", SymbolKind::Enum, None, true),
            ("Color.red", SymbolKind::EnumVariant, Some("Color"), true),
            ("Color.green", SymbolKind::EnumVariant, Some("Color"), true),
            ("Color.blue", SymbolKind::EnumVariant, Some("Color"), true),
            ("MyInt", SymbolKind::TypeAlias, None, true),
            ("origin", SymbolKind::Constant, None, true),
            ("counter", SymbolKind::Variable, None, true),
            ("helper", SymbolKind::Function, None, true),
            ("run", SymbolKind::Function, None, true),
        ]
    );
    assert_eq!(named(&raw, "Service").parents, ["Base"]);
    assert_eq!(
        named(&raw, "helper").signature.as_deref(),
        Some("def helper(n : int) : int")
    );
}

#[test]
fn covers_include_member_typedef_and_alias_declarations() {
    let raw = extract(COVERAGE_SAMPLE);

    assert_eq!(specs(&raw), ["helperlib"]);

    let alias = named(&raw, "Alias");
    assert_eq!(alias.kind, SymbolKind::TypeAlias);
    assert_eq!(alias.owner.as_deref(), Some("Holder"));

    assert_eq!(named(&raw, "Pair").kind, SymbolKind::Struct);
    let first = named(&raw, "first");
    assert_eq!(first.kind, SymbolKind::Field);
    assert_eq!(first.owner.as_deref(), Some("Pair"));

    assert_eq!(named(&raw, "Shape").kind, SymbolKind::Enum);
    let circle = named(&raw, "circle");
    assert_eq!(circle.kind, SymbolKind::EnumVariant);
    assert_eq!(circle.owner.as_deref(), Some("Shape"));

    assert_eq!(named(&raw, "Flags").kind, SymbolKind::Enum);
    let alpha = named(&raw, "alpha");
    assert_eq!(alpha.kind, SymbolKind::EnumVariant);
    assert_eq!(alpha.owner.as_deref(), Some("Flags"));

    for name in [
        "Alias", "Pair", "first", "Shape", "circle", "Flags", "alpha",
    ] {
        assert!(named(&raw, name).exported, "{name} is exported");
    }
}

#[test]
fn a_distinct_typedef_is_a_type_alias() {
    let raw = extract("options gen2\ntypedef distinct Meters = float\n");

    assert_eq!(named(&raw, "Meters").kind, SymbolKind::TypeAlias);
}

#[test]
fn a_field_list_declares_one_field_per_name() {
    let raw = extract("options gen2\nstruct P {\n  a, b : int\n}\n");

    let fields: Vec<(&str, Option<&str>)> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Field)
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.owner.as_deref()))
        .collect();
    assert_eq!(fields, [("P.a", Some("P")), ("P.b", Some("P"))]);
}

#[test]
fn a_macro_spliced_field_name_is_kept_as_written() {
    let raw = extract("options gen2\nstruct S {\n  $i(generated) : int\n  plain : int\n}\n");

    let quoted = named(&raw, "$i(generated)");
    assert_eq!(quoted.kind, SymbolKind::Field);
    assert_eq!(quoted.owner.as_deref(), Some("S"));
    assert_eq!(named(&raw, "plain").kind, SymbolKind::Field);
}

#[test]
fn the_private_modifier_clears_exported() {
    let raw = extract(
        "options gen2\n\
         def private secret() {}\n\
         def open() {}\n\
         struct private Hidden {}\n\
         struct Shown {}\n\
         let private p = 0\n\
         var q = 1\n",
    );

    assert!(!named(&raw, "secret").exported);
    assert!(named(&raw, "open").exported);
    assert!(!named(&raw, "Hidden").exported);
    assert!(named(&raw, "Shown").exported);
    assert!(!named(&raw, "p").exported);
    assert!(named(&raw, "q").exported);
}

#[test]
fn the_module_clause_is_a_namespace_that_qualifies_every_symbol() {
    let raw =
        extract("options gen2\nmodule ui\nstruct Button {\n  def press() {}\n}\ndef draw() {}\n");

    let module = named(&raw, "ui");
    assert_eq!(module.kind, SymbolKind::Namespace);
    assert!(module.exported);
    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(names, ["ui", "ui.Button", "ui.Button.press", "ui.draw"]);
    assert_eq!(raw.package.as_deref(), Some("ui"));
}

#[test]
fn the_builtin_module_clause_names_no_namespace() {
    let source = "options gen2\nmodule $\ndef dot() {}\n";
    assert!(!parses_with_errors(source));

    let raw = extract(source);

    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(names, ["dot"]);
    assert_eq!(raw.package, None);
}

#[test]
fn a_global_symbol_spans_its_own_binding() {
    let raw = extract("options gen2\nlet {\n  a = 1\n  b = 2\n}\n");

    let lines: Vec<(&str, u32)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.line))
        .collect();
    assert_eq!(lines, [("a", 3), ("b", 4)]);
}

#[test]
fn records_calls_attributed_to_their_callable() {
    let raw = extract(SAMPLE);

    let helper = raw
        .calls
        .iter()
        .find(|call| call.callee == "helper")
        .unwrap_or_else(|| panic!("no helper call in {:?}", raw.calls));
    assert_eq!(helper.site_of, id_of(&raw, "area"));
    assert_eq!(helper.receiver, None);
}

#[test]
fn a_module_scoped_call_keeps_its_scope_as_the_receiver() {
    let raw = extract("options gen2\ndef g() {\n  geom::dot()\n  ::top()\n}\n");

    assert_eq!(calls(&raw), [("dot", Some("geom")), ("top", None)]);
}

#[test]
fn a_call_scoped_to_the_compiling_module_has_no_receiver() {
    let raw = extract("options gen2\ndef g() {\n  _::dot()\n  __::top()\n}\n");

    assert_eq!(calls(&raw), [("dot", None), ("top", None)]);
}

#[test]
fn only_a_member_call_is_marked_as_one() {
    let raw =
        extract("options gen2\ndef g() {\n  geom::dot()\n  obj.go()\n  obj.run() <| $ { }\n}\n");

    let marks: Vec<_> = (raw.calls.iter())
        .map(|call| (call.callee.as_str(), call.calls_a_method))
        .collect();
    assert_eq!(marks, [("dot", false), ("go", true), ("run", true)]);
}

#[test]
fn member_calls_capture_their_receiver() {
    let raw = extract(SAMPLE);

    let run_calls: Vec<(&str, Option<&str>)> = raw
        .calls
        .iter()
        .filter(|call| call.site_of == id_of(&raw, "run"))
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect();
    assert_eq!(
        run_calls,
        [("Service", None), ("area", Some("s")), ("reset", Some("s"))]
    );
}

#[test]
fn a_member_call_on_an_unnamed_expression_keeps_an_empty_receiver() {
    let raw = extract(
        "options gen2\n\
         def f() {\n\
           items[0].close()\n\
           self.pool->get()\n\
           items[1].handler { use() }\n\
         }\n",
    );

    assert_eq!(
        calls(&raw),
        [
            ("close", Some("")),
            ("get", Some("")),
            ("handler", Some("")),
            ("use", None)
        ]
    );
}

#[test]
fn a_call_with_named_arguments_records_its_callee_and_receiver() {
    let raw = extract(
        "options gen2\n\
         def f() {\n\
           g([k = 1])\n\
           obj.m([k = 1])\n\
           w->render([title = \"brk\", depth = 3])\n\
         }\n",
    );

    assert_eq!(
        calls(&raw),
        [("g", None), ("m", Some("obj")), ("render", Some("w"))]
    );
}

#[test]
fn a_new_expression_calls_the_type_it_constructs() {
    let raw = extract(
        "options gen2\n\
         def make() {\n\
           var p = new Prototype()\n\
           var q = new mod::Widget()\n\
           var r = new Bare\n\
         }\n",
    );

    assert_eq!(
        calls(&raw),
        [("Prototype", None), ("Widget", Some("mod")), ("Bare", None)]
    );
    assert!(
        raw.calls
            .iter()
            .all(|call| call.site_of == id_of(&raw, "make"))
    );
}

#[test]
fn a_new_over_a_container_type_calls_nothing() {
    let raw = extract(
        "options gen2\n\
         def make() {\n\
           var xs = new array<int>()\n\
           var m = new table<string; int>()\n\
         }\n",
    );

    assert!(raw.calls.is_empty(), "calls: {:?}", raw.calls);
}

#[test]
fn a_new_over_a_make_expression_parses_without_error() {
    for form in [
        "let shared protos <- new { 1 => Proto(name = \"a\") }",
        "let shared ids <- new [ 1, 2 ]",
        "let shared sq <- new [for (x in xs); x]",
        "let shared tb <- new {for (x in xs); x => x}",
        "let shared { protos <- new { 1 => 2 } }",
        "let p <- new Widget()",
        "let q <- new array<int>()",
    ] {
        assert!(
            !parses_with_errors(&format!("options gen2\n{form}\n")),
            "{form:?}"
        );
    }
}

#[test]
fn a_new_over_a_make_expression_names_no_constructed_type() {
    let make_only = extract("options gen2\ndef f() {\n  var b = new { 1 => 2 }\n}\n");
    assert!(make_only.calls.is_empty(), "calls: {:?}", make_only.calls);

    let raw =
        extract("options gen2\ndef f() {\n  var a = new Widget()\n  var b = new { 1 => 2 }\n}\n");
    assert_eq!(calls(&raw), [("Widget", None)]);
}

#[test]
fn a_struct_constructor_records_one_call_to_its_type() {
    let raw = extract(
        "options gen2\n\
         def make() {\n\
           var a = Widget(size = 1)\n\
           var b = mod::Gadget(size = 2)\n\
           var c = default<Widget>\n\
         }\n",
    );

    assert_eq!(calls(&raw), [("Widget", None), ("Gadget", Some("mod"))]);
}

#[test]
fn a_macro_splice_in_call_position_records_no_call() {
    let raw = extract("options gen2\ndef emit() {\n  var e = qmacro($c(fn_name)(1))\n}\n");

    assert_eq!(calls(&raw), [("qmacro", None)]);
}

#[test]
fn quoted_code_records_a_call_as_a_use_of_its_name_and_no_call() {
    let raw = extract(
        "options gen2\n\
         def shapes() {\n\
           var shape = quote(foo(1 + 2, \"bar\"))\n\
           var piped = quote() { baz(@@_::handler) }\n\
           var scoped = shell::quote(path)\n\
           var here = qmacro(_::helper(1) + mod::other())\n\
           var made = qmacro(Widget(size = 1))\n\
           var fresh = qmacro(new Gadget())\n\
         }\n",
    );

    assert_eq!(
        calls(&raw),
        [
            ("quote", None),
            ("quote", None),
            ("quote", Some("shell")),
            ("qmacro", None),
            ("qmacro", None),
            ("qmacro", None)
        ]
    );
    let in_mod = Receiver::Qualifier("mod".to_owned());
    let uses: Vec<(&str, &Receiver, u32)> = raw
        .uses
        .iter()
        .map(|used| (used.name.as_str(), &used.receiver, used.line))
        .collect();
    assert_eq!(
        uses,
        [
            ("foo", &Receiver::Bare, 3),
            ("baz", &Receiver::Bare, 4),
            ("handler", &Receiver::Bare, 4),
            ("helper", &Receiver::Bare, 6),
            ("other", &in_mod, 6),
            ("Widget", &Receiver::Bare, 7),
            ("Gadget", &Receiver::Bare, 8)
        ]
    );
    let shapes = id_of(&raw, "shapes");
    assert!(raw.uses.iter().all(|used| used.site_of == shapes));
}

#[test]
fn a_qmatch_pattern_records_no_call_but_the_matched_expression_runs() {
    let raw = extract(
        "options gen2\n\
         def check(e : Expression?) {\n\
           let r = qmatch(first_of(e), foo($e(cap), bar()))\n\
           let b = qmatch_block(e) $ {\n\
             if (a < limit()) {\n\
               return b\n\
             }\n\
           }\n\
           let f = qmatch_function(find(name)) $(x : int) : int {\n\
             return x + helper()\n\
           }\n\
         }\n",
    );

    assert_eq!(
        calls(&raw),
        [
            ("qmatch", None),
            ("first_of", None),
            ("qmatch_block", None),
            ("qmatch_function", None),
            ("find", None)
        ]
    );
    assert_eq!(used_names(&raw), ["foo", "bar", "limit", "helper"]);
}

#[test]
fn a_qmacro_records_the_calls_of_its_splices_and_none_of_the_rest() {
    let raw = extract(
        "options gen2\n\
         def emit(arg : Expression?) {\n\
           var plain = qmacro(helper($e(arg), $v(count())))\n\
           var stmts = qmacro_block() {\n\
             var x = make($i(name_of(arg)))\n\
             run($e(clone_expression(arg)))\n\
           }\n\
           var fn = qmacro_function(fn_name()) $(a : int) : int {\n\
             return compute(a, $v(limit()))\n\
           }\n\
           var callee = qmacro($c(callee_name())(arg_of()))\n\
           var nested = qmacro($e(qmacro(inner())) + outer())\n\
           var method = qmacro_method(method_name(), cls()) $(self : Foo) {\n\
             self.do_it($v(amount()))\n\
           }\n\
           var typed <- qmacro_variable(var_name(), type<$t(var_type())>)\n\
           var arr = qmacro_block_to_array() {\n\
             $b(body_of())\n\
             run_it()\n\
           }\n\
           var expr = qmacro_expr(${ run($a(args_of()), obj.$f(field_name())); })\n\
           var kind = qmacro_type(type<$t(elem_of())>)\n\
         }\n",
    );

    let expected = [
        "qmacro",
        "count",
        "qmacro_block",
        "name_of",
        "clone_expression",
        "qmacro_function",
        "fn_name",
        "limit",
        "qmacro",
        "callee_name",
        "qmacro",
        "qmacro",
        "qmacro_method",
        "method_name",
        "cls",
        "amount",
        "qmacro_variable",
        "var_name",
        "var_type",
        "qmacro_block_to_array",
        "body_of",
        "qmacro_expr",
        "args_of",
        "field_name",
        "qmacro_type",
        "elem_of",
    ];
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();
    assert_eq!(callees, expected);
    let emit = id_of(&raw, "emit");
    assert!(raw.calls.iter().all(|call| call.site_of == emit));
    assert_eq!(
        used_names(&raw),
        [
            "helper", "make", "run", "compute", "arg_of", "inner", "outer", "run_it", "run"
        ]
    );
}

#[test]
fn require_and_include_record_their_specifiers_in_source_order() {
    let raw = extract(SAMPLE);

    assert_eq!(specs(&raw), ["daslib/strings", "math/vec"]);
    let lines: Vec<u32> = raw
        .directives
        .iter()
        .map(|directive| directive.line)
        .collect();
    assert_eq!(lines, [4, 5]);
}

#[test]
fn an_optional_require_records_its_target_and_not_its_guard() {
    let raw = extract(
        "options gen2\nrequire ?llvm_tune llvm\nrequire ?llvm/daslib/llvm_tune daslib/llvm\n",
    );

    assert_eq!(specs(&raw), ["llvm", "daslib/llvm"]);
}

#[test]
fn a_require_of_a_module_group_records_nothing() {
    let source = "options gen2\n\
                  require [sql_provider]\n\
                  require [tune_framework] public\n\
                  require strings\n";
    assert!(!parses_with_errors(source));

    assert_eq!(specs(&extract(source)), ["strings"]);
}

#[test]
fn directive_forms_the_compiler_rejects_keep_their_valid_prefix() {
    for (form, expected) in [
        ("require strings if defined", &["strings"][..]),
        ("expect x == 1", &[]),
    ] {
        let source = format!("options gen2\n{form}\n");
        assert!(parses_with_errors(&source), "{form:?} parses cleanly");

        assert_eq!(specs(&extract(&source)), expected, "{form:?}");
    }
}

#[test]
fn an_include_records_its_file_name_as_written() {
    let raw = extract("options gen2\ninclude \"foo.das\"\ninclude lib/foo.das\n");

    assert_eq!(specs(&raw), ["\"foo.das\"", "lib/foo.das"]);
}

#[test]
fn a_file_without_the_gen2_option_extracts_to_nothing() {
    for source in [
        "def foo() {}\nstruct Bar {}\n",
        "options ni\ndef foo() {}\n",
        "options gen2 = false\nrequire ecs\ndef foo() {\n  bar()\n}\n",
        "options gen2\noptions gen2 = false\ndef foo() {}\n",
        "/*\noptions gen2\n*/\ndef foo() {}\nstruct Bar {}\n",
    ] {
        assert_eq!(extract(source), RawExtraction::default(), "{source:?}");
    }
}

#[test]
fn each_spelling_of_the_gen2_option_opts_in() {
    for source in [
        "options gen2\ndef foo() {}\n",
        "options gen2 = true\ndef foo() {}\n",
        "options indenting = 4, gen2\ndef foo() {}\n",
        "\u{feff}options gen2\ndef foo() {}\n",
    ] {
        assert_eq!(
            named(&extract(source), "foo").kind,
            SymbolKind::Function,
            "{source:?}"
        );
    }

    let late = format!("{}options gen2\ndef deep() {{}}\n", "\n".repeat(80));
    assert_eq!(named(&extract(&late), "deep").kind, SymbolKind::Function);
}

#[test]
fn only_a_marker_in_the_leading_comments_marks_generated_code() {
    let generated = |source: &str| {
        extract(source)
            .symbols
            .iter()
            .any(|symbol| symbol.flags.is_generated())
    };

    assert!(generated("// @generated\noptions gen2\ndef f() {}\n"));
    assert!(generated("/* DO NOT EDIT */\noptions gen2\ndef f() {}\n"));
    assert!(generated(
        "// Code generated by tool.\noptions gen2\ndef f() {}\n"
    ));
    assert!(!generated("options gen2\ndef f() {}\n// @generated\n"));
    assert!(!generated("options gen2\ndef f() {}\n"));
}

#[test]
fn inline_expression_forms_keep_the_calls_around_them() {
    let raw = extract(
        "options gen2\n\
         def run(p : int?) {\n\
           with (module daslib/math) {\n\
             helper(p)\n\
           }\n\
           var a = addr<int?>(p)\n\
         }\n",
    );

    assert_eq!(caller_of(&raw, "helper"), id_of(&raw, "run"));
}

#[test]
fn a_stored_block_keeps_its_calls_out_of_the_enclosing_def() {
    let lambda = extract("options gen2\ndef outer() {\n  var f = @(e) { cb(e) }\n  reg()\n}\n");
    assert_eq!(caller_of(&lambda, "cb"), FILE_SCOPE);
    assert_eq!(caller_of(&lambda, "reg"), id_of(&lambda, "outer"));

    let local = extract(
        "options gen2\n\
         def outer() {\n\
           var f = @@(x : int) { cb(x) }\n\
           reg(@@(x : int) { on(x) })\n\
         }\n",
    );
    let outer = id_of(&local, "outer");
    assert_eq!(caller_of(&local, "cb"), FILE_SCOPE);
    assert_eq!(caller_of(&local, "on"), outer);
    assert_eq!(caller_of(&local, "reg"), outer);
}

#[test]
fn an_applied_block_keeps_its_calls_in_the_enclosing_def() {
    let piped = extract("options gen2\ndef sys() {\n  query() <| ${ process(eid) }\n}\n");
    assert_eq!(caller_of(&piped, "query"), id_of(&piped, "sys"));
    assert_eq!(caller_of(&piped, "process"), id_of(&piped, "sys"));

    let annotated = extract(
        "options gen2\ndef sys2() {\n  query() <| $ [es] (eid : EntityId) {\n    tick()\n  }\n}\n",
    );
    assert_eq!(caller_of(&annotated, "tick"), id_of(&annotated, "sys2"));

    let argument = extract("options gen2\ndef reg() {\n  subscribe(\"k\", @(e) { on(e) })\n}\n");
    assert_eq!(caller_of(&argument, "on"), id_of(&argument, "reg"));

    let constructed =
        extract("options gen2\ndef reg() {\n  var w = new Widget(@(e) { on(e) })\n}\n");
    assert_eq!(caller_of(&constructed, "on"), id_of(&constructed, "reg"));
}

#[test]
fn a_block_applied_to_a_field_calls_the_field() {
    let braced = extract("options gen2\ndef outer() {\n  obj.handler { use() }\n}\n");
    let handler = braced
        .calls
        .iter()
        .find(|call| call.callee == "handler")
        .unwrap_or_else(|| panic!("no handler call in {:?}", braced.calls));
    assert_eq!(handler.receiver.as_deref(), Some("obj"));
    assert_eq!(handler.site_of, id_of(&braced, "outer"));
    assert_eq!(caller_of(&braced, "use"), id_of(&braced, "outer"));

    let piped = extract("options gen2\ndef outer() {\n  obj.handler <| ${ use() }\n}\n");
    assert_eq!(calls(&piped), [("handler", Some("obj")), ("use", None)]);
}

#[test]
fn a_data_pipe_calls_nothing_through_its_field() {
    let raw = extract("options gen2\ndef outer() {\n  obj.field |> compute()\n}\n");

    assert_eq!(calls(&raw), [("compute", None)]);
}

#[test]
fn an_expression_body_signature_drops_its_arrow() {
    let raw = extract("options gen2\ndef f() => 1\ndef g() : int => h()\ndef k() { m() }\n");

    assert_eq!(named(&raw, "f").signature.as_deref(), Some("def f()"));
    assert_eq!(named(&raw, "g").signature.as_deref(), Some("def g() : int"));
    assert_eq!(named(&raw, "k").signature.as_deref(), Some("def k()"));
}

#[test]
fn a_brace_less_if_body_on_the_next_line_keeps_the_rest_of_the_file() {
    let raw = extract(
        "options gen2\n\
         def classify(x : int) : int {\n\
           if (x < 0)\n\
             return negative()\n\
           elif (x == 0)\n\
             return zero()\n\
           else\n\
             return positive()\n\
         }\n\
         def after() {\n\
           tail()\n\
         }\n",
    );

    let classify = id_of(&raw, "classify");
    for callee in ["negative", "zero", "positive"] {
        assert_eq!(caller_of(&raw, callee), classify, "{callee}");
    }
    assert_eq!(caller_of(&raw, "tail"), id_of(&raw, "after"));
}

#[test]
fn braced_and_same_line_if_bodies_keep_their_shape() {
    let raw = extract(
        "options gen2\n\
         def f(x : int) {\n\
           if (x < 0) { a() }\n\
           if (x > 0) b()\n\
           if (x == 0)\n\
           {\n\
             c()\n\
           }\n\
         }\n",
    );

    let f = id_of(&raw, "f");
    for callee in ["a", "b", "c"] {
        assert_eq!(caller_of(&raw, callee), f, "{callee}");
    }
}

#[test]
fn an_annotation_after_an_arrow_bodied_or_abstract_def_opens_the_next_def() {
    let raw = extract(
        "options gen2\n\
         def lane(v : int) : int => v\n\
         \n\
         [export]\n\
         def main() {}\n\
         class C {\n\
           def abstract area() : float\n\
           [unused_argument(dt)]\n\
           def update(dt : float) {}\n\
         }\n",
    );

    let lane = named(&raw, "lane");
    assert_eq!((lane.line, lane.end_line), (2, 2));
    let main = named(&raw, "main");
    assert_eq!(main.line, 4);
    assert_eq!(main.signature.as_deref(), Some("[export]\ndef main()"));

    let area = named(&raw, "area");
    assert_eq!((area.line, area.end_line), (7, 7));
    assert_eq!(
        area.signature.as_deref(),
        Some("def abstract area() : float")
    );
    let update = named(&raw, "update");
    assert_eq!(update.line, 8);
    assert_eq!(
        update.signature.as_deref(),
        Some("[unused_argument(dt)]\ndef update(dt : float)")
    );
}

#[test]
fn a_def_an_annotation_registers_is_an_entry_point_and_a_compile_hint_registers_nothing() {
    let raw = extract(
        "options gen2\n[es(tag=gameClient), unused_argument(evt)]\ndef on_update(evt : Event) {\n  pass\n}\n\
         [unused_argument(a), no_aot]\ndef tuned(a : int) {\n  pass\n}\ndef plain() {\n  pass\n}\n",
    );
    let entries = ["on_update", "tuned", "plain"].map(|name| named(&raw, name).flags.is_entry());

    assert_eq!(entries, [true, false, false]);
}

const DOCUMENTED: &str = "options gen2

// Parses.
// Fails on nothing.
[export]
def parse() {
  return
}

/* Renders. */
def render() {
  return
}
";

fn doc_line(name: &str) -> u32 {
    let lines: Vec<&[u8]> = DOCUMENTED.lines().map(str::as_bytes).collect();
    let line = named(&extract(DOCUMENTED), name).line;
    Daslang::new().doc_comments().first_line(&lines, line)
}

#[test]
fn the_comments_above_a_function_and_its_annotation_document_it() {
    assert_eq!(doc_line("parse"), 3);
    assert_eq!(doc_line("render"), 10);
}
