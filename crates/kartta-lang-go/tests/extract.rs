//! What one Go file extracts to from its bytes alone.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Language, MemberCalls, OwnMembers, RawExtraction, Symbol, SymbolId, SymbolKind, TypeUses,
};
use kartta_lang_go::Go;

const SAMPLE: &[u8] = br#"
package sample

type Greeter interface {
    Greet() string
}

type Base struct {
    Name string
}

type Service struct {
    Base
}

type Alias = Service

type Handler func(v string) string

const Version = "1.0"

const A, B = 1, 2

var count = 0

var (
    x int
    y int
)

var _ Greeter = &Service{}

func New(name string) *Service {
    return &Service{Base: Base{Name: name}}
}

func (s *Service) Greet() string {
    count++
    return helper(s.Name)
}

func helper(v string) string { return v }
"#;

fn extract(source: &[u8]) -> RawExtraction {
    Go::new().extract(source)
}

fn qualified<'a>(raw: &'a RawExtraction, qualified_name: &str) -> &'a Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("no symbol {qualified_name}"))
}

fn named<'a>(raw: &'a RawExtraction, name: &str) -> &'a Symbol {
    raw.symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn symbol(raw: &RawExtraction, id: SymbolId) -> &Symbol {
    &raw.symbols[id.0 as usize]
}

fn uses(raw: &RawExtraction) -> Vec<(&str, Option<&str>, &str, u32)> {
    raw.references
        .iter()
        .map(|site| {
            let owner = symbol(raw, site.site_of).name.as_str();
            (
                site.name.as_str(),
                site.qualifier.as_deref(),
                owner,
                site.line,
            )
        })
        .collect()
}

fn calls(raw: &RawExtraction) -> Vec<(&str, &str, Option<&str>, u32)> {
    raw.calls
        .iter()
        .map(|call| {
            let caller = symbol(raw, call.caller_id).name.as_str();
            (
                caller,
                call.callee.as_str(),
                call.receiver.as_deref(),
                call.line,
            )
        })
        .collect()
}

fn local_calls(raw: &RawExtraction) -> Vec<(&str, &str)> {
    raw.local_calls
        .iter()
        .map(|call| {
            let caller = symbol(raw, call.caller_id).name.as_str();
            (caller, symbol(raw, call.local).name.as_str())
        })
        .collect()
}

#[test]
fn go_claims_files_with_the_go_extension_in_any_case() {
    let go = Go::new();

    assert!(go.claims("cmd/server/main.go"));
    assert!(go.claims("LEGACY.GO"));
    assert!(!go.claims("go.mod"));
    assert!(!go.claims("cmd/server/main.gox"));
    assert!(!go.claims("cmd/go/README"));
}

#[test]
fn a_bare_call_never_reaches_a_member_and_a_value_is_callable() {
    let conventions = Go::new().conventions();

    assert_eq!(conventions.own_members, OwnMembers::Unreached);
    assert!(conventions.values_are_call_targets);
    assert_eq!(conventions.owner_chain_separator, None);
    assert_eq!(conventions.member_calls, MemberCalls::Members);
}

#[test]
fn a_package_is_a_directory_and_names_join_no_scopes() {
    let conventions = Go::new().conventions();

    assert!(conventions.directory_packages);
    assert_eq!(conventions.scope_separator, None);
}

#[test]
fn a_type_named_in_a_declaration_or_a_composite_literal_is_a_use_and_in_a_conversion_a_call() {
    let raw = extract(
        b"package p\n\ntype A struct{}\ntype B int\ntype C struct{}\ntype D struct{}\n\nvar _ = D{}\n\n\
          func f(x int) {\n\tvar a A\n\t_ = a\n\t_ = B(x)\n\t_ = C{}\n}\n",
    );
    let used: Vec<(&str, Option<&str>)> = raw
        .references
        .iter()
        .map(|site| {
            let owner = raw.symbols.get(site.site_of.0 as usize);
            (site.name.as_str(), owner.map(|owner| owner.name.as_str()))
        })
        .collect();
    let called: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(used, [("D", None), ("A", Some("f")), ("C", Some("f"))]);
    assert_eq!(called, ["B"]);
    assert_eq!(Go::new().conventions().type_uses, TypeUses::Every);
}

#[test]
fn a_type_local_to_a_body_is_a_local_symbol_no_use_names() {
    let raw = extract(b"package p\n\nfunc f() {\n\ttype L struct{}\n\tvar l L\n\t_ = l\n}\n");
    let local: Vec<&str> = raw
        .local_symbols
        .iter()
        .map(|id| raw.symbols[id.0 as usize].name.as_str())
        .collect();

    assert_eq!(local, ["L", "l"]);
    assert!(!raw.references.iter().any(|site| site.name == "L"));
}

#[test]
fn symbols_are_qualified_by_the_package_in_declaration_order() {
    let raw = extract(SAMPLE);

    let names: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "sample.Greeter",
            "sample.Greeter.Greet",
            "sample.Base",
            "sample.Base.Name",
            "sample.Service",
            "sample.Alias",
            "sample.Handler",
            "sample.Version",
            "sample.A",
            "sample.B",
            "sample.count",
            "sample.x",
            "sample.y",
            "sample.New",
            "sample.Service.Greet",
            "sample.helper",
        ]
    );
    assert_eq!(raw.package.as_deref(), Some("sample"));

    let greeter = qualified(&raw, "sample.Greeter");
    assert_eq!(greeter.kind, SymbolKind::Interface);
    assert!(greeter.exported);

    let greet = qualified(&raw, "sample.Service.Greet");
    assert_eq!(greet.kind, SymbolKind::Method);
    assert_eq!(greet.owner.as_deref(), Some("Service"));
}

#[test]
fn interface_members_are_methods_owned_by_the_interface() {
    let raw = extract(SAMPLE);

    let greet = qualified(&raw, "sample.Greeter.Greet");
    assert_eq!(greet.kind, SymbolKind::Method);
    assert_eq!(greet.owner.as_deref(), Some("Greeter"));
    assert!(greet.exported);
    assert_eq!(greet.signature.as_deref(), Some("Greet() string"));
}

#[test]
fn aliases_and_defined_non_struct_types_are_type_aliases() {
    let raw = extract(SAMPLE);

    assert_eq!(qualified(&raw, "sample.Alias").kind, SymbolKind::TypeAlias);
    assert_eq!(
        qualified(&raw, "sample.Handler").kind,
        SymbolKind::TypeAlias
    );
}

#[test]
fn an_embedded_type_and_an_asserted_interface_are_parents() {
    let raw = extract(SAMPLE);

    assert_eq!(
        qualified(&raw, "sample.Service").parents,
        ["Base", "Greeter"]
    );
}

#[test]
fn an_assertion_through_new_names_its_type_and_one_through_a_constructor_names_none() {
    let raw = extract(
        br"
package sample

type I interface{ M() }

type T struct{}

type U struct{}

var _ I = new(T)

var _ I = NewU()
",
    );

    assert_eq!(qualified(&raw, "sample.T").parents, ["I"]);
    assert_eq!(qualified(&raw, "sample.U").parents, [] as [&str; 0]);
}

#[test]
fn new_of_an_expression_parses_and_the_calls_in_it_are_recorded() {
    let source: &[u8] = br"
package sample

type I interface{ M() }

type T struct{}

var _ I = new(T)

func alloc(x int64) {
    _ = new(2)
    _ = new(int64(x))
    _ = new(seed())
}
";
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_golang::LANGUAGE.into())
        .expect("the grammar loads");
    let tree = parser.parse(source, None).expect("the source parses");
    assert!(!tree.root_node().has_error());

    let raw = extract(source);

    assert_eq!(qualified(&raw, "sample.T").parents, ["I"]);
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();
    assert!(callees.contains(&"seed"), "callees: {callees:?}");
}

#[test]
fn a_type_written_as_an_expression_is_a_use() {
    let raw = extract(
        br#"package sample

import (
	"example.com/item-store"
	"example.com/pkg"
)

type Buffer []byte

type Item struct{ ID int }

type List[E any] []E

func convert[A, B any](v any) {}

func sizes(n int, local Item, req Item) {
	_ = make(Buffer, n)
	_ = make([]Item, 5)
	_ = new(Item)
	_ = new(pkg.Item)
	_ = new(true)
	_ = new(local)
	_ = new(req.ID)
	_ = new(item_store.Item)
	_ = (*Item)(nil)
	_ = (*pkg.Item)(nil)
	_ = (List[Item])(nil)
	_ = (Buffer)(nil)
	convert[Item, pkg.Item](nil)
	_ = convert[Item, pkg.Item]
}
"#,
    );

    assert_eq!(
        uses(&raw),
        [
            ("Item", None, "sizes", 16),
            ("Item", None, "sizes", 16),
            ("Buffer", None, "sizes", 17),
            ("Item", None, "sizes", 18),
            ("Item", None, "sizes", 19),
            ("Item", Some("pkg"), "sizes", 20),
            ("Item", Some("item_store"), "sizes", 24),
            ("Item", None, "sizes", 25),
            ("Item", Some("pkg"), "sizes", 26),
            ("List", None, "sizes", 27),
            ("Item", None, "sizes", 27),
            ("Buffer", None, "sizes", 28),
            ("Item", None, "sizes", 29),
            ("Item", Some("pkg"), "sizes", 29),
            ("Item", Some("pkg"), "sizes", 30),
        ],
        "the type operand of make and new, a parenthesized conversion type and its type argument, and a type \
         argument are uses, a qualified one under any import spelling; a predeclared constant, a local value, a \
         selector on a local value, and a generic function used as a value are not"
    );
}

#[test]
fn a_generic_method_type_parameter_is_not_a_use() {
    let raw = extract(
        br"package sample

type Box struct{}

func (Box) Map[T any](v T) []T { var out []T; return out }
",
    );

    let uses: Vec<(&str, u32)> = raw
        .references
        .iter()
        .map(|site| (site.name.as_str(), site.line))
        .collect();
    assert_eq!(uses, [("Box", 5)], "only the receiver names a package type");
}

#[test]
fn a_result_written_as_a_type_declares_no_local() {
    let raw = extract(
        br#"package sample

import "example.com/pkg"

type Logger struct{}

func adapt() *pkg.Logger {
	var _ Logger
	return nil
}
"#,
    );

    let uses: Vec<(&str, Option<&str>, u32)> = raw
        .references
        .iter()
        .map(|site| (site.name.as_str(), site.qualifier.as_deref(), site.line))
        .collect();
    assert_eq!(
        uses,
        [("Logger", Some("pkg"), 7), ("Logger", None, 8)],
        "the name of a qualified result type does not shadow the package type"
    );
}

#[test]
fn an_assertion_of_a_type_the_file_does_not_declare_is_recorded_as_an_assertion() {
    let raw = extract(
        br#"
package sample

import "example.com/pkg"

type Codec interface{ Encode() }

var _ Codec = (*Provider)(nil)

var _ Codec = (*pkg.Remote)(nil)

var _ Codec = (*Box[int])(nil)
"#,
    );

    let pairs: Vec<(&str, &str)> = raw
        .assertions
        .iter()
        .map(|assertion| (assertion.type_name.as_str(), assertion.iface.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("Provider", "Codec"),
            ("pkg.Remote", "Codec"),
            ("Box", "Codec")
        ],
        "a qualified type keeps its package, and an instantiated type names its base"
    );
}

#[test]
fn struct_fields_are_field_symbols_and_an_embedded_type_is_none() {
    let raw = extract(SAMPLE);

    let name = qualified(&raw, "sample.Base.Name");
    assert_eq!(name.kind, SymbolKind::Field);
    assert_eq!(name.owner.as_deref(), Some("Base"));
    assert!(name.exported);

    let service_fields = raw.symbols.iter().filter(|symbol| {
        symbol.kind == SymbolKind::Field && symbol.owner.as_deref() == Some("Service")
    });
    assert_eq!(
        service_fields.count(),
        0,
        "an embedded field is a parent, not a field symbol"
    );
}

#[test]
fn a_literal_struct_alias_emits_its_fields() {
    let raw = extract(
        br"
package sample

type Payload = struct {
    ID, Ref string
    Name    string
}
",
    );

    assert_eq!(qualified(&raw, "sample.Payload").kind, SymbolKind::Struct);
    let id = qualified(&raw, "sample.Payload.ID");
    assert_eq!(id.kind, SymbolKind::Field);
    assert_eq!(id.owner.as_deref(), Some("Payload"));
    assert!(id.exported);
    assert_eq!(
        qualified(&raw, "sample.Payload.Ref").kind,
        SymbolKind::Field
    );
    assert_eq!(
        qualified(&raw, "sample.Payload.Name").kind,
        SymbolKind::Field
    );
}

#[test]
fn a_blank_struct_field_emits_no_symbol() {
    let raw = extract(
        br"
package sample

type Padded struct {
    _    [4]byte
    Real int
}
",
    );

    let blank_fields = raw.symbols.iter().filter(|symbol| symbol.name == "_");
    assert_eq!(blank_fields.count(), 0);
    assert_eq!(
        qualified(&raw, "sample.Padded.Real").kind,
        SymbolKind::Field
    );
}

#[test]
fn a_field_whose_first_rune_is_unicode_upper_case_is_exported() {
    let raw = extract("package sample\n\ntype T struct {\n\tÄ int\n\té string\n}\n".as_bytes());

    assert!(qualified(&raw, "sample.T.Ä").exported);
    assert!(!qualified(&raw, "sample.T.é").exported);
}

#[test]
fn signatures_hold_the_declaration_without_the_body() {
    let raw = extract(SAMPLE);

    assert_eq!(
        qualified(&raw, "sample.New").signature.as_deref(),
        Some("func New(name string) *Service")
    );
    assert_eq!(
        qualified(&raw, "sample.Service.Greet").signature.as_deref(),
        Some("func (s *Service) Greet() string")
    );
    assert_eq!(qualified(&raw, "sample.Service").signature, None);
}

#[test]
fn calls_are_attributed_to_the_enclosing_function() {
    let raw = extract(SAMPLE);

    let helper_call = raw
        .calls
        .iter()
        .find(|call| call.callee == "helper")
        .expect("helper is called");
    assert_eq!(
        symbol(&raw, helper_call.caller_id).qualified_name,
        "sample.Service.Greet"
    );
    assert_eq!(helper_call.receiver, None);
}

#[test]
fn a_member_call_on_an_unnamed_expression_keeps_an_empty_receiver() {
    let raw = extract(b"package p\nfunc f(s S) { s.Findings().Verdict(v) }\n");

    let receivers: Vec<(&str, Option<&str>)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect();
    assert_eq!(receivers, [("Verdict", Some("")), ("Findings", Some(""))]);
}

#[test]
fn a_receiver_the_caller_declares_as_a_value_names_no_type_and_a_method_s_own_names_its_owner() {
    let raw = extract(
        br#"package p

import "example.com/conn"

type Widget struct{}

func (w *Widget) Run(param Server) {
	w.draw()
	param.Close()
	local := conn.Dial()
	local.Close()
	var declared Server
	declared.Close()
	conn.Close()
	Widget.helper(w)
	go func(inner Server) { inner.Close(); w.Flush() }(param)
	for _, each := range servers {
		each.Close()
	}
	switch typed := param.(type) {
	case Server:
		typed.Close()
	}
}
"#,
    );

    let receivers: Vec<(&str, Option<&str>)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect();
    assert_eq!(
        receivers,
        [
            ("draw", Some("Widget")),
            ("Close", Some("")),
            ("Dial", Some("conn")),
            ("Close", Some("")),
            ("Close", Some("")),
            ("Close", Some("conn")),
            ("helper", Some("Widget")),
            ("Close", Some("")),
            ("Flush", Some("Widget")),
            ("Close", Some("")),
            ("Close", Some("")),
        ]
    );
}

#[test]
fn the_initializer_of_a_package_level_variable_calls_as_that_variable() {
    let raw = extract(
        br#"package p

var Handler = func() { serve() }

var config, other = load(), build(func() { serve() })

var first, second = pair()

var _, last = skip(), keep()

const size = len("abc")

func f() { serve() }
"#,
    );

    assert_eq!(
        calls(&raw),
        [
            ("Handler", "serve", None, 3),
            ("config", "load", None, 5),
            ("other", "build", None, 5),
            ("other", "serve", None, 5),
            ("first", "pair", None, 7),
            ("last", "skip", None, 9),
            ("last", "keep", None, 9),
            ("size", "len", None, 11),
            ("f", "serve", None, 13),
        ]
    );
}

#[test]
fn grouped_const_declarations_extract_every_spec() {
    let raw = extract(
        br"
package pkg

const (
    GroupedA = 1
    GroupedB = 2
)

const Single = 3
",
    );

    for name in ["GroupedA", "GroupedB", "Single"] {
        assert_eq!(named(&raw, name).kind, SymbolKind::Constant, "{name}");
    }
}

#[test]
fn generic_types_reduce_to_their_base_name() {
    let raw = extract(
        br"
package pkg

type Stack[T any] struct{}

func (s *Stack[T]) Push(v T) {}

type Wrapper struct {
    Stack[int]
}

type Pusher interface {
    Stack[int]
}

var _ Pusher = Stack[int]{}
",
    );

    let push = named(&raw, "Push");
    assert_eq!(push.owner.as_deref(), Some("Stack"));
    assert_eq!(push.qualified_name, "pkg.Stack.Push");
    assert_eq!(named(&raw, "Wrapper").parents, ["Stack"]);
    assert_eq!(named(&raw, "Pusher").parents, ["Stack"]);
    assert_eq!(named(&raw, "Stack").parents, ["Pusher"]);
}

#[test]
fn every_import_spec_is_a_directive_with_its_alias_as_written() {
    let raw = extract(
        br#"
package main

import (
    "fmt"
    u "example.com/app/lib/util"
    . "example.com/app/lib/dot"
    _ "example.com/app/lib/blank"
)
"#,
    );

    let directives: Vec<(&str, &[String], u32)> = raw
        .directives
        .iter()
        .map(|directive| {
            (
                directive.spec.as_str(),
                directive.names.as_slice(),
                directive.line,
            )
        })
        .collect();
    assert_eq!(
        directives,
        [
            ("fmt", &[][..], 5),
            ("example.com/app/lib/util", &["u".to_owned()][..], 6),
            ("example.com/app/lib/dot", &[".".to_owned()][..], 7),
            ("example.com/app/lib/blank", &["_".to_owned()][..], 8),
        ]
    );
}

#[test]
fn the_generated_banner_marks_every_symbol_as_generated() {
    let generated = extract(
        b"// Code generated by protoc-gen-go. DO NOT EDIT.\n\npackage sample\n\nfunc Emitted() {}\n",
    );
    let indented = extract(
        b"  // Code generated by x. DO NOT EDIT.\npackage sample\n\nfunc Handwritten() {}\n",
    );
    let after_package =
        extract(b"package\tmain\n\n// Code generated by x. DO NOT EDIT.\nfunc G() {}\n");
    let crlf = extract(
        b"// Code generated by x. DO NOT EDIT.\r\n\r\npackage sample\r\n\r\nfunc Crlf() {}\r\n",
    );
    let trailing_space =
        extract(b"// Code generated by x. DO NOT EDIT. \n\npackage sample\n\nfunc Spaced() {}\n");
    let byte_order_mark = extract(
        b"\xEF\xBB\xBF// Code generated by x. DO NOT EDIT.\n\npackage sample\n\nfunc Marked() {}\n",
    );

    assert!(qualified(&generated, "sample.Emitted").flags.is_generated());
    assert!(qualified(&crlf, "sample.Crlf").flags.is_generated());
    assert!(
        qualified(&byte_order_mark, "sample.Marked")
            .flags
            .is_generated()
    );
    assert!(
        !qualified(&trailing_space, "sample.Spaced")
            .flags
            .is_generated()
    );
    assert!(
        !qualified(&indented, "sample.Handwritten")
            .flags
            .is_generated()
    );
    assert!(!qualified(&after_package, "main.G").flags.is_generated());
}

#[test]
fn a_free_main_in_package_main_is_the_entry_point() {
    let raw = extract(
        br"
package main

type App struct{}

func (a *App) main() {}

func helper() {}

func main() {}
",
    );

    let main = qualified(&raw, "main.main");
    assert!(main.flags.is_entry());
    assert!(!main.flags.is_test() && !main.flags.is_generated());
    assert!(!qualified(&raw, "main.App.main").flags.is_entry());
    let helper = qualified(&raw, "main.helper");
    assert!(!helper.flags.is_entry() && !helper.flags.is_test());
}

#[test]
fn main_outside_package_main_is_not_an_entry_point() {
    let raw = extract(b"\npackage worker\n\nfunc main() {}\n");

    assert!(!qualified(&raw, "worker.main").flags.is_entry());
}

#[test]
fn an_instantiation_call_names_the_generic_it_instantiates() {
    let raw = extract(
        br#"package p

import "example.com/pkg"

func Generic[T any](x T) T { return x }

var handlers []func(int)

type Table struct{ fns []func(int); ip int }

func (Table) Get[T any](v T) T { return v }

func F(i int, t Table) {
	Generic[int](42)
	Generic[float64]()
	pkg.Map[string, int](nil, nil)
	t.Get[int](3)
	handlers[i](1)
	t.fns[t.ip](2)
}
"#,
    );

    assert_eq!(
        calls(&raw),
        [
            ("F", "Generic", None, 14),
            ("F", "Generic", None, 15),
            ("F", "Map", Some("pkg"), 16),
            ("F", "Get", Some(""), 17),
        ],
        "an index by a local value calls an element, not an instantiation"
    );
}

#[test]
fn an_index_by_or_of_a_package_value_calls_an_element() {
    let raw = extract(
        br"package p

func pick[T any](x T) T { return x }

type Kind int

var handlers []func()

const first = 0

func F() {
	routes[slot]()
	routes[first]()
	handlers[elsewhere]()
	pick[Kind](3)
}

var slot = 1
",
    );

    assert_eq!(
        calls(&raw),
        [("F", "pick", None, 15)],
        "an index by a package-level var or const of the file and any index of a package-level var of the file \
         call an element; a generic function of the file is instantiated"
    );
}

#[test]
fn a_local_that_shadows_an_import_and_a_prior_specs_parameter_are_not_uses() {
    let raw = extract(
        br#"package svc

import cfg "example.com/config"

type (
	Box[T any] struct{ v T }
	Other struct{ t T }
)

func Run(api int) {
	_ = cfg.Default
	_ = api.Missing
	cfg := 1
	_ = cfg.Shadowed
	for _, cfgx := range nil {
		_ = cfgx.Value
	}
	f := func(cfg int) { _ = cfg.Param }
	_ = f
}
"#,
    );

    assert_eq!(
        uses(&raw),
        [("T", None, "t", 7), ("Default", Some("cfg"), "Run", 11)],
        "Other's T is a package type once Box's parameter list has ended; the aliased import binds cfg until a \
         local of that name shadows it"
    );
}

#[test]
fn every_binding_form_named_like_an_import_shadows_it() {
    let raw = extract(
        br#"package svc

import "example.com/cfg"

type T struct{}

func Param(a, b, cfg int) { _ = cfg.Param }

func (cfg T) Receiver() { _ = cfg.Receiver }

func Result() (cfg int) { _ = cfg.Result; return }

func Switch(v any) {
	switch cfg := v.(type) {
	case nil:
	default:
		_ = cfg.Switch
	}
}

func Select(ch chan int) {
	select {
	case cfg := <-ch:
		_ = cfg.Select
	}
	_ = cfg.After
}

var hook = func() {
	var cfg int
	_ = cfg.Literal
}

func Headers(xs []int) {
	for _, cfg := range xs {
		_ = cfg.Range
	}
	if cfg := 1; cfg > 0 {
		_ = cfg.If
	}
	_ = cfg.Tail
}

func Cases(n int) {
	switch n {
	case 1:
		cfg := 1
		_ = cfg.Case
	default:
		_ = cfg.Default
	}
	f := func() { _ = cfg.Literal }
	_ = f
}

func Guard(v any) {
	switch cfg := cfg.Guard.(type) {
	default:
		_ = cfg
	}
}

func Signature(T T) {}
"#,
    );

    let uses: Vec<(&str, Option<&str>, u32)> = raw
        .references
        .iter()
        .map(|site| (site.name.as_str(), site.qualifier.as_deref(), site.line))
        .collect();
    assert_eq!(
        uses,
        [
            ("T", None, 9),
            ("After", Some("cfg"), 26),
            ("Tail", Some("cfg"), 41),
            ("Default", Some("cfg"), 50),
            ("Literal", Some("cfg"), 52),
            ("Guard", Some("cfg"), 57),
            ("T", None, 63),
        ],
        "a parameter in any position of its list, a receiver, a named result, a type-switch alias, a select \
         receive, a var in a function literal, and a statement header's name each shadow the import in their own \
         scope; a type-switch guard is read before its alias exists; `case nil` names no type; a parameter \
         shadows from the body, so its signature's type names the package type"
    );
}

#[test]
fn a_type_declared_in_a_body_shadows_the_package_type_there() {
    let raw = extract(
        br"package svc

type cfg struct{}

func Local() {
	type cfg int
	var _ cfg
}

func Package() {
	var _ cfg
}
",
    );

    let uses: Vec<(&str, u32)> = raw
        .references
        .iter()
        .map(|site| (site.name.as_str(), site.line))
        .collect();
    assert_eq!(
        uses,
        [("cfg", 11)],
        "Local's use names its own type; only Package's use names the package type"
    );
}

#[test]
fn a_symbol_declared_in_a_body_is_local_and_a_package_symbol_is_not() {
    let raw = extract(
        br"package svc

var h = func() { var inLiteral int; _ = inLiteral }

const Limit = 3

type Config struct{ Name string }

func Run() {
	type body struct{ field int }
	const inFunc = 1
	if true {
		var inBlock int
		_ = inBlock
	}
}
",
    );

    let locals: Vec<&str> = raw
        .local_symbols
        .iter()
        .map(|&id| symbol(&raw, id).name.as_str())
        .collect();
    assert_eq!(
        locals,
        ["inLiteral", "body", "field", "inFunc", "inBlock"],
        "a function-literal, function, or nested-block declaration is local; the literal's variable, the package \
         constant, type, field, and function are not"
    );
}

#[test]
fn a_call_of_a_local_value_with_no_symbol_records_no_call() {
    let raw = extract(
        br"package p

type S func()

func Short() { f := target; f() }
func Param(g func()) { g() }
func Ranged(fns []func()) { for _, h := range fns { h() } }
func RangedOne(ch chan func()) { for h := range ch { h() } }
func Named() (r func()) { r(); return }
func Receive(ch chan func()) { select { case k := <-ch: k() } }
func Switch(v any) { switch s := v.(type) { case func(): s() } }
func Literal() { _ = func(m func()) { m() } }
func (s S) Recv() { s() }
func Declared() { var d = target; d(); type t func(); t(nil) }
func After() { if true { target := 1; _ = target }; target() }
func Shadow(n func()) { { var n = target; n() } }
",
    );

    let callers: Vec<(&str, &str)> = calls(&raw)
        .into_iter()
        .map(|(caller, callee, _, _)| (caller, callee))
        .collect();
    assert_eq!(
        callers,
        [("After", "target")],
        "a call of a :=, range, receive, or type switch variable, a parameter, a result, or a receiver records no \
         call; a name past its local's scope does"
    );
    assert_eq!(
        local_calls(&raw),
        [("Declared", "d"), ("Declared", "t"), ("Shadow", "n")],
        "a call of a declared local calls that local"
    );
}

#[test]
fn a_call_binds_the_local_of_its_own_scope() {
    let raw = extract(
        br"package p

func Literal() {
	go func() { var helper = func() {}; helper() }()
	helper()
}
func Block() {
	if true { var helper = func() {}; _ = helper }
	helper()
}
func Later() {
	helper()
	var helper = func() {}
	helper()
	{ type helper int; helper(0) }
	helper()
}
func Capture() {
	var helper = func() {}
	go func() { helper() }()
}
func Pair() {
	var first, second = func() {}, func() {}
	first()
	second()
}
",
    );

    let call_lines: Vec<(&str, u32)> = calls(&raw)
        .into_iter()
        .map(|(caller, _, _, line)| (caller, line))
        .collect();
    assert_eq!(
        call_lines,
        [("Literal", 5), ("Block", 9), ("Later", 12)],
        "a call before its local's declaration or past its local's block names no local"
    );
    let locals: Vec<(&str, u32, &str, SymbolKind, u32)> = raw
        .local_calls
        .iter()
        .map(|call| {
            let local = symbol(&raw, call.local);
            let caller = symbol(&raw, call.caller_id).name.as_str();
            (
                caller,
                call.line,
                local.name.as_str(),
                local.kind,
                local.line,
            )
        })
        .collect();
    assert_eq!(
        locals,
        [
            ("Literal", 4, "helper", SymbolKind::Variable, 4),
            ("Later", 14, "helper", SymbolKind::Variable, 13),
            ("Later", 15, "helper", SymbolKind::TypeAlias, 15),
            ("Later", 16, "helper", SymbolKind::Variable, 13),
            ("Capture", 20, "helper", SymbolKind::Variable, 19),
            ("Pair", 24, "first", SymbolKind::Variable, 23),
            ("Pair", 25, "second", SymbolKind::Variable, 23),
        ],
        "a call inside its local's scope calls the innermost local of the name"
    );
}

#[test]
fn an_assignment_to_a_declared_local_keeps_its_call() {
    let raw = extract(
        br"package p

func Assigned(fns []func(), ch chan func()) {
	var h = target
	h, err := build()
	_ = err
	h()
	for _, h = range fns { h() }
	select { case h = <-ch: h() }
	switch { case true: var g = target; g, err := build(); _ = err; g() }
}
",
    );

    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();
    assert_eq!(callees, ["build", "build"]);
    assert_eq!(
        local_calls(&raw),
        [
            ("Assigned", "h"),
            ("Assigned", "h"),
            ("Assigned", "h"),
            ("Assigned", "g")
        ],
        "a `:=` that reuses a name of its block and an `=` assignment declare nothing, so the declared local keeps \
         its calls"
    );
}

#[test]
fn a_colon_equals_in_a_nested_block_declares_a_new_local() {
    let raw = extract(
        br"package p

func Nested(v any, ch chan int, fns []func()) {
	switch {
	case true:
		var h = target
		if true { h, err := build(); _ = err; h() }
	}
	switch v.(type) {
	case int:
		var h = target
		{ h := other; h() }
	}
	select {
	case <-ch:
		var h = target
		for _, h := range fns { h() }
	}
}
",
    );

    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();
    assert_eq!(callees, ["build"]);
    assert_eq!(
        local_calls(&raw),
        [],
        "a `:=` in a block or a statement header nested in a case clause declares a new local, even where that \
         scope ends at the clause's last byte"
    );
}

#[test]
fn a_local_shadows_from_its_declaration_to_its_block_end() {
    let raw = extract(
        br#"package svc

import "example.com/cfg"

func Run() {
	cfg := cfg.Default
	_ = cfg
}

func Block() {
	if true {
		cfg := 1
		_ = cfg.Hidden
	}
	_ = cfg.Visible
	_ = cfg.Map[int](nil)
}
"#,
    );

    let uses: Vec<(&str, u32)> = raw
        .references
        .iter()
        .map(|site| (site.name.as_str(), site.line))
        .collect();
    assert_eq!(
        uses,
        [("Default", 6), ("Visible", 15)],
        "the initializer still names the import; the block's local ends at its brace; an instantiation is a call, \
         not a use"
    );
    let instantiation = raw
        .calls
        .iter()
        .find(|call| call.callee == "Map")
        .map(|call| call.receiver.as_deref());
    assert_eq!(
        instantiation,
        Some(Some("cfg")),
        "the instantiation stays a call"
    );
}

#[test]
fn a_selector_on_a_name_the_file_does_not_declare_is_a_use_of_what_an_import_may_bind() {
    let raw = extract(
        br#"package svc

import "example.com/lib/go-thing"

var settings = struct{ Limit int }{}

func Run(local Options) {
	_ = thing.Limit
	_ = renamed.Limit
	_ = settings.Limit
	_ = local.Limit
	_ = string.Limit
}
"#,
    );

    assert_eq!(
        uses(&raw),
        [
            ("Options", None, "Run", 7),
            ("Limit", Some("thing"), "Run", 8),
            ("Limit", Some("renamed"), "Run", 9),
        ],
        "a package clause need not be the import's last element, and the graph binds a qualifier only to an import"
    );
}

#[test]
fn uses_outside_calls_are_recorded_against_their_enclosing_symbol() {
    let raw = extract(
        br#"package svc

import "example.com/a"

type Server struct {
	cfg a.Config
	a.Embedded
	n int
}
type List[T any] struct{ next *List[T]; v T }
var defaultCfg = a.Config{Limit: a.Limit}
func Run(c a.Config) *Server {
	x := a.Config{}
	_, _ = "a.Config", c.Limit
	// a.Config in a comment
	a.New()
	return &Server{cfg: x}
}
func (l *List[T]) Push(v T) {}
"#,
    );

    assert_eq!(
        uses(&raw),
        [
            ("Config", Some("a"), "cfg", 6),
            ("Embedded", Some("a"), "Server", 7),
            ("List", None, "next", 10),
            ("Config", Some("a"), "defaultCfg", 11),
            ("Limit", Some("a"), "defaultCfg", 11),
            ("Config", Some("a"), "Run", 12),
            ("Server", None, "Run", 12),
            ("Config", Some("a"), "Run", 13),
            ("Server", None, "Run", 17),
            ("List", None, "Push", 19),
        ]
    );
    let call = raw
        .calls
        .iter()
        .find(|call| call.callee == "New")
        .map(|call| call.receiver.as_deref());
    assert_eq!(call, Some(Some("a")), "the call stays a call");
}
