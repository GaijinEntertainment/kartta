//! What one Go file extracts to from its bytes alone, and where the doc comment of a definition starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use std::time::{Duration, Instant};

use kartta_lang::{
    Access, DataUses, Language, MemberCalls, OwnMembers, RawExtraction, Receiver, Symbol, SymbolId,
    SymbolKind, TypeUses, UseRef,
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

fn assertions(raw: &RawExtraction) -> Vec<(&str, &str)> {
    raw.assertions
        .iter()
        .map(|assertion| (assertion.type_name.as_str(), assertion.interface.as_str()))
        .collect()
}

fn symbol(raw: &RawExtraction, id: SymbolId) -> &Symbol {
    &raw.symbols[id.0 as usize]
}

fn qualifier(site: &UseRef) -> Option<&str> {
    match &site.receiver {
        Receiver::Qualifier(qualifier) => Some(qualifier),
        _ => None,
    }
}

const fn names_no_member(site: &UseRef) -> bool {
    matches!(site.receiver, Receiver::Bare | Receiver::Qualifier(_))
}

fn uses(raw: &RawExtraction) -> Vec<(&str, Option<&str>, &str, u32)> {
    raw.uses
        .iter()
        .filter(|site| names_no_member(site))
        .map(|site| {
            let owner = symbol(raw, site.site_of).name.as_str();
            (site.name.as_str(), qualifier(site), owner, site.line)
        })
        .collect()
}

fn calls(raw: &RawExtraction) -> Vec<(&str, &str, Option<&str>, u32)> {
    raw.calls
        .iter()
        .map(|call| {
            let caller = symbol(raw, call.site_of).name.as_str();
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
            let caller = symbol(raw, call.site_of).name.as_str();
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
fn go_claims_no_file_whose_name_the_go_command_skips() {
    let go = Go::new();

    assert!(!go.claims("lua/_state.go"));
    assert!(!go.claims("lua/.state.go"));
    assert!(!go.claims("_state.go"));
    assert!(go.claims("_lua/state.go"));
    assert!(go.claims("lua/state_.go"));
}

#[test]
fn go_says_why_it_leaves_out_a_go_file_it_does_not_claim() {
    let go = Go::new();
    let skipped = Some("no Go build reads a file whose name starts with _ or .");

    assert_eq!(go.declines("lua/_state.go"), skipped);
    assert_eq!(go.declines("lua/.state.GO"), skipped);
    assert_eq!(go.declines("lua/state.go"), None);
    assert_eq!(go.declines("_lua/state.go"), None);
    assert_eq!(go.declines("lua/_state.txt"), None);
    assert_eq!(go.declines(".go.mod"), None);
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
fn a_selector_a_literal_s_key_and_a_package_level_name_use_the_field_or_value_they_name() {
    let raw = extract(
        b"package p\n\ntype T struct{ N int }\n\nconst C = 1\n\nvar v T\n\nfunc f() int {\n\tv = T{N: C}\n\t\
          return v.N\n}\n",
    );
    let used: Vec<(&str, Access)> = raw
        .uses
        .iter()
        .filter(|site| site.name != "T")
        .map(|site| (site.name.as_str(), site.access))
        .collect();

    assert_eq!(
        used,
        [
            ("v", Access::Write),
            ("N", Access::Write),
            ("C", Access::Read),
            ("N", Access::Read),
            ("v", Access::Read),
        ]
    );
    assert_eq!(Go::new().conventions().data_uses, DataUses::Every);
}

#[test]
fn a_struct_tag_that_hands_its_field_to_a_library_marks_the_field_reflected() {
    let raw = extract(
        b"package p\n\ntype T struct {\n\tA int `json:\"a,omitempty\"`\n\tB int `json:\"-\"`\n\t\
          C int `json:\"-\" yaml:\"c\"`\n\tD int\n\tE int `custom`\n\tF, G int `xml:\"f\"`\n\t\
          H int `json:\"-\" yaml:\"-\"`\n\tI int \"json:\\\"-\\\"\"\n\tJ int \"json:\\\"j\\\"\"\n}\n",
    );
    let reflected: Vec<(&str, bool)> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Field)
        .map(|symbol| (symbol.name.as_str(), symbol.flags.is_reflected()))
        .collect();

    assert_eq!(
        reflected,
        [
            ("A", true),
            ("B", false),
            ("C", true),
            ("D", false),
            ("E", true),
            ("F", true),
            ("G", true),
            ("H", false),
            ("I", false),
            ("J", true),
        ]
    );
}

#[test]
fn a_struct_whose_value_a_marshal_or_reflect_call_takes_hands_its_exported_fields_to_reflection() {
    let raw = extract(
        br#"
package p

import (
    "encoding/json"
    "reflect"
)

type Info struct {
    Issued string
    Token  string
    spare  int
    Hidden int `json:"-"`
    Rules  []Rule
}

type Rule struct {
    Icon string
}

type Config struct {
    Tasks any
}

type Other struct {
    Value int
}

func parse(data []byte) string {
    var info Info
    _ = json.Unmarshal(data, &info)
    return info.Token
}

func (c *Config) events() {
    _ = reflect.ValueOf(c).Elem()
}

func show(o Other) int {
    return o.Value
}
"#,
    );
    let reflected: Vec<&str> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.flags.is_reflected())
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(
        reflected,
        [
            "p.Info.Issued",
            "p.Info.Token",
            "p.Info.Rules",
            "p.Rule.Icon",
            "p.Config.Tasks"
        ]
    );
}

#[test]
fn a_skip_tag_hides_a_field_from_a_marshal_call_but_not_from_reflect() {
    for (call, hidden_reflected) in [("json.Marshal", false), ("reflect.ValueOf", true)] {
        let source = format!(
            "package p\n\ntype T struct {{\n\tF int\n\tHidden int `json:\"-\"`\n\tInner U `json:\"-\"`\n}}\n\n\
             type U struct {{\n\tG int\n}}\n\nfunc f(t T) {{\n\t{call}(t)\n}}\n"
        );

        let raw = extract(source.as_bytes());

        assert!(qualified(&raw, "p.T.F").flags.is_reflected(), "{call}");
        for hidden in ["p.T.Hidden", "p.T.Inner", "p.U.G"] {
            assert_eq!(
                qualified(&raw, hidden).flags.is_reflected(),
                hidden_reflected,
                "{call} {hidden}"
            );
        }
    }
}

#[test]
fn each_marshal_decode_and_reflect_call_name_hands_its_argument_s_fields_to_reflection() {
    for (call, reflected, hidden_reflected) in [
        ("json.Marshal", true, false),
        ("json.MarshalIndent", true, false),
        ("json.MarshalWrite", true, false),
        ("json.Unmarshal", true, false),
        ("json.UnmarshalRead", true, false),
        ("encoder.Encode", true, false),
        ("decoder.Decode", true, false),
        ("reflect.ValueOf", true, true),
        ("reflect.TypeOf", true, true),
        ("fmt.Println", false, false),
    ] {
        let source = format!(
            "package p\n\ntype T struct {{\n\tF int\n\tHidden int `json:\"-\"`\n}}\n\n\
             func f(t T) {{\n\t{call}(t)\n}}\n"
        );

        let raw = extract(source.as_bytes());

        assert_eq!(
            qualified(&raw, "p.T.F").flags.is_reflected(),
            reflected,
            "{call}"
        );
        assert_eq!(
            qualified(&raw, "p.T.Hidden").flags.is_reflected(),
            hidden_reflected,
            "{call}"
        );
    }
}

#[test]
fn a_marshal_call_reaches_the_struct_fields_through_an_alias_or_a_container_type() {
    let raw = extract(
        br"
package p

type Info struct {
    Token string
}

type Rule struct {
    Icon string
}

type Infos []Info

type Shown = Rule

func f(infos Infos, shown Shown) {
    _, _ = json.Marshal(infos)
    _, _ = json.Marshal(shown)
}
",
    );

    assert!(qualified(&raw, "p.Info.Token").flags.is_reflected());
    assert!(qualified(&raw, "p.Rule.Icon").flags.is_reflected());
}

#[test]
fn a_marshal_call_reaches_the_struct_fields_through_a_defined_type() {
    let raw = extract(
        br#"
package b

type S struct {
    Name   string
    Hidden string `json:"-"`
    Inner  Held
}

type C struct {
    Tag string
}

type Held Kept

type Kept struct {
    Deep string
}

type T S

type Twice T

type U = C

func run() {
    _, _ = json.Marshal(Twice{})
    _, _ = json.Marshal(U{})
}
"#,
    );

    assert!(qualified(&raw, "b.S.Name").flags.is_reflected());
    assert!(!qualified(&raw, "b.S.Hidden").flags.is_reflected());
    assert!(qualified(&raw, "b.Kept.Deep").flags.is_reflected());
    assert!(qualified(&raw, "b.C.Tag").flags.is_reflected());
}

#[test]
fn a_marshal_call_reads_no_field_through_a_func_type() {
    for declared in [
        "type H func() S",
        "type H = func() S",
        "type F func() S\ntype H F",
        "type F func() S\ntype H []F",
        "type F func() S\ntype H struct{ Run F }",
    ] {
        let source = format!(
            "package b\n\ntype S struct {{\n\tName string\n}}\n\n{declared}\n\n\
             func run(h H) {{\n\t_, _ = json.Marshal(h)\n}}\n"
        );

        let raw = extract(source.as_bytes());

        assert!(
            !qualified(&raw, "b.S.Name").flags.is_reflected(),
            "{declared}"
        );
    }
}

#[test]
fn a_reflect_call_reaches_the_struct_fields_through_a_func_type() {
    let raw = extract(
        br"
package b

type S struct {
    Name string
}

type H func() S

func run(h H) int { return reflect.TypeOf(h).Out(0).NumField() }
",
    );

    assert!(qualified(&raw, "b.S.Name").flags.is_reflected());
}

#[test]
fn a_marshal_call_reads_no_field_through_a_channel() {
    for (declared, parameter, body) in [
        ("", "h chan S", ""),
        ("", "h <-chan S", ""),
        ("", "h chan<- S", ""),
        ("", "h *chan S", ""),
        ("", "h (chan S)", ""),
        ("", "h []chan S", ""),
        ("", "h map[string]chan S", ""),
        ("", "h chan []S", ""),
        ("", "g chan S", "h := g\n\t"),
        ("", "g chan S", "h := &g\n\t"),
        ("", "g chan S", "h := (g)\n\t"),
        ("", "g any", "h := g.(chan S)\n\t"),
        ("", "g []chan S", "h := g[0]\n\t"),
        ("", "g map[string]chan S", "h := g[\"a\"]\n\t"),
        ("", "", "h := new(chan S)\n\t"),
        ("", "", "var h chan S\n\t"),
        ("", "", "var h []chan S\n\t"),
        ("", "", "var h = make(chan S)\n\t"),
        ("", "", "h := make(chan S)\n\t"),
        ("", "", "h := make([]chan S, 1)\n\t"),
        ("", "", "h := []chan S{}\n\t"),
        ("type H chan S", "h H", ""),
        ("type H = chan S", "h H", ""),
        ("type H []chan S", "h H", ""),
        ("type F chan S\ntype H F", "h H", ""),
        ("type F chan S\ntype H []F", "h H", ""),
        ("type H struct{ Run chan S }", "h H", ""),
        ("type H struct{ Run []chan S }", "h H", ""),
        ("type F chan S\ntype H struct{ Run F }", "h H", ""),
    ] {
        let source = format!(
            "package b\n\ntype S struct {{\n\tName string\n}}\n\n{declared}\n\n\
             func run({parameter}) {{\n\t{body}_, _ = json.Marshal(h)\n}}\n"
        );

        let raw = extract(source.as_bytes());

        assert!(
            !qualified(&raw, "b.S.Name").flags.is_reflected(),
            "{declared} {parameter} {body}"
        );
    }
}

#[test]
fn a_marshal_call_reaches_the_struct_fields_of_a_value_a_channel_yields() {
    for (parameter, body, value) in [
        ("h chan S", "", "<-h"),
        ("h chan []S", "", "<-h"),
        ("h chan S", "v := <-h\n\t", "v"),
        ("h []S", "", "h"),
        ("h map[string]S", "", "h"),
        ("h *S", "", "*h"),
    ] {
        let source = format!(
            "package b\n\ntype S struct {{\n\tName string\n}}\n\n\
             func run({parameter}) {{\n\t{body}_, _ = json.Marshal({value})\n}}\n"
        );

        let raw = extract(source.as_bytes());

        assert!(
            qualified(&raw, "b.S.Name").flags.is_reflected(),
            "{parameter} {body} {value}"
        );
    }
}

fn ranged_one_name(declared: &str, parameter: &str, body: &str, ranged: &str) -> RawExtraction {
    let source = format!(
        "package b\n\ntype S struct {{\n\tName string\n}}\n\nfunc (s S) Run() {{}}\n\n{declared}\n\n\
         func run({parameter}) {{\n\t{body}for v := range {ranged} {{\n\t\t\
         _, _ = json.Marshal(v)\n\t\tv.Run()\n\t}}\n}}\n"
    );
    extract(source.as_bytes())
}

#[test]
fn one_name_over_a_channel_holds_the_element() {
    for (parameter, body, ranged) in [
        ("h chan S", "", "h"),
        ("h <-chan S", "", "h"),
        ("h (chan S)", "", "h"),
        ("h chan *S", "", "h"),
        ("h chan S", "", "(h)"),
        ("g chan S", "h := g\n\t", "h"),
        ("", "h := make(chan S)\n\t", "h"),
        ("", "var h chan S\n\t", "h"),
        ("", "", "make(chan S)"),
        ("g any", "", "g.(chan S)"),
        ("h []chan S", "", "h[0]"),
        ("h map[string]chan S", "", "h[\"a\"]"),
    ] {
        let raw = ranged_one_name("", parameter, body, ranged);

        assert!(
            qualified(&raw, "b.S.Name").flags.is_reflected(),
            "{parameter} {body} {ranged}"
        );
        assert!(
            calls(&raw)
                .iter()
                .any(|&(_, callee, receiver, _)| callee == "Run" && receiver == Some("S")),
            "{parameter} {body} {ranged}"
        );
    }
}

#[test]
fn one_name_stays_unread_where_the_walk_cannot_tell_a_channel_yields_it() {
    for (declared, parameter, ranged) in [
        ("", "h []S", "h"),
        ("", "h [2]S", "h"),
        ("", "h *[2]S", "h"),
        ("", "h map[string]S", "h"),
        ("", "h []chan S", "h"),
        ("", "h map[string]chan S", "h"),
        ("type H struct{ c chan S }", "h H", "h.c"),
        ("func h() chan S { return nil }", "", "h()"),
        ("type H chan S", "h H", "h"),
        ("type H = chan S", "h H", "h"),
        ("var h chan S", "", "h"),
        ("", "h chan []S", "h"),
    ] {
        let raw = ranged_one_name(declared, parameter, "", ranged);

        assert!(
            !qualified(&raw, "b.S.Name").flags.is_reflected(),
            "{declared} {parameter} {ranged}"
        );
        assert!(
            !calls(&raw)
                .iter()
                .any(|&(_, callee, receiver, _)| callee == "Run" && receiver == Some("S")),
            "{declared} {parameter} {ranged}"
        );
    }
}

#[test]
fn a_type_switch_case_that_names_a_channel_reads_no_field_behind_it() {
    let raw = extract(
        br"
package b

type S struct {
    Name string
}

func run(g any) {
    switch h := g.(type) {
    case chan S:
        _, _ = json.Marshal(h)
    }
}
",
    );

    assert!(!qualified(&raw, "b.S.Name").flags.is_reflected());
}

#[test]
fn a_shadowing_value_is_not_a_channel_the_outer_name_holds() {
    let raw = extract(
        br"
package b

type S struct {
    Name string
}

func run(h chan S) {
    {
        h := []S{}
        _, _ = json.Marshal(h)
    }
}
",
    );

    assert!(qualified(&raw, "b.S.Name").flags.is_reflected());
}

#[test]
fn a_reflect_call_reaches_the_struct_fields_through_a_channel() {
    for (declared, parameter, body) in [
        ("", "h chan S", ""),
        ("", "", "h := make(chan S)\n\t"),
        ("type H chan S", "h H", ""),
        ("type H struct{ Run chan S }", "h H", ""),
    ] {
        let source = format!(
            "package b\n\ntype S struct {{\n\tName string\n}}\n\n{declared}\n\n\
             func run({parameter}) {{\n\t{body}_ = reflect.TypeOf(h).Elem().NumField()\n}}\n"
        );

        let raw = extract(source.as_bytes());

        assert!(
            qualified(&raw, "b.S.Name").flags.is_reflected(),
            "{declared} {parameter} {body}"
        );
    }
}

#[test]
fn a_defined_type_no_reflective_call_takes_hands_its_struct_s_fields_to_nothing() {
    let raw = extract(
        br"
package b

type S struct {
    Name string
}

type T S

func run() ([]byte, error) { return json.Marshal(1) }
",
    );

    assert!(!qualified(&raw, "b.S.Name").flags.is_reflected());
}

#[test]
fn a_value_of_a_type_a_body_declares_hands_no_package_level_namesake_s_fields_to_reflection() {
    let raw = extract(
        br"
package p

type T struct {
    F int
}

func f() {
    type T struct {
        G int
    }
    var v T
    _, _ = json.Marshal(v)
}
",
    );

    assert!(!qualified(&raw, "p.T.F").flags.is_reflected());
}

#[test]
fn a_package_main_file_with_no_func_main_hands_its_exported_values_to_a_plugin_host() {
    let exports = b"var Selector = 1\n\nvar spare = 2\n\nconst Version = \"1\"\n\nfunc Add() {\n\tvar Local = 3\n\t\
                    _ = Local\n}\n\nfunc helper() {}\n\ntype T struct{}\n\nfunc (T) Run() {}\n";
    let plugin = [b"package main\n\n".as_slice(), exports].concat();
    let program = [b"package main\n\nfunc main() {}\n\n".as_slice(), exports].concat();
    let library = [b"package lib\n\n".as_slice(), exports].concat();
    let reflected = |source: &[u8]| -> Vec<String> {
        extract(source)
            .symbols
            .into_iter()
            .filter(|symbol| symbol.flags.is_reflected())
            .map(|symbol| symbol.qualified_name)
            .collect()
    };

    assert_eq!(reflected(&plugin), ["main.Selector", "main.Add"]);
    assert_eq!(reflected(&program), Vec::<String>::new());
    assert_eq!(reflected(&library), Vec::<String>::new());
}

#[test]
fn a_package_is_a_directory_and_names_join_no_scopes() {
    let conventions = Go::new().conventions();

    assert!(conventions.directory_packages);
    assert_eq!(conventions.scope_separator, None);
}

#[test]
fn a_type_named_in_a_declaration_or_a_composite_literal_is_a_use_as_a_type_and_in_a_conversion_a_call()
 {
    let raw = extract(
        b"package p\n\nimport \"example.com/q\"\n\ntype A struct{}\ntype B int\ntype C struct{}\ntype D struct{}\n\
          type E struct{}\ntype F int\ntype G map[int]int\n\nvar _ = D{}\n\n\
          func f(x int) {\n\tvar a A\n\t_ = a\n\t_ = B(x)\n\t_ = C{}\n\t_ = new(E)\n\t_ = (F)(x)\n\t_ = make(G, 1)\n\
          \t_ = new(q.H)\n}\n",
    );
    let used: Vec<(&str, Option<&str>, Access)> = raw
        .uses
        .iter()
        .map(|site| {
            let owner = raw.symbols.get(site.site_of.0 as usize);
            let owner = owner.map(|owner| owner.name.as_str());
            (site.name.as_str(), owner, site.access)
        })
        .collect();
    let called: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(
        used,
        [
            ("D", None, Access::Type),
            ("A", Some("f"), Access::Type),
            ("C", Some("f"), Access::Type),
            ("E", Some("f"), Access::Type),
            ("F", Some("f"), Access::Type),
            ("G", Some("f"), Access::Type),
            ("H", Some("f"), Access::Type)
        ],
        "an allocation's type and a parenthesized conversion's operand name a type as one, an imported one too"
    );
    assert_eq!(called, ["B", "new", "make", "new"]);
    assert_eq!(Go::new().conventions().type_uses, TypeUses::Every);
}

fn accesses_on_line(raw: &RawExtraction, line: u32) -> Vec<(&str, Access)> {
    raw.uses
        .iter()
        .filter(|site| site.line == line)
        .map(|site| (site.name.as_str(), site.access))
        .collect()
}

#[test]
fn a_type_argument_of_an_uncalled_generic_function_is_a_use_as_a_type() {
    let raw = extract(
        br#"package p

import "example.com/sdk"

type Item struct{}

func errorIs[T error](err error) bool { return false }

func F() {
	_ = errorIs[*sdk.Timeout]
	_ = errorIs[Item]
	_ = errorIs[[]sdk.Timeout]
	_ = errorIs[Box[Item]]
	_ = pair[other, sdk.Timeout]
	_ = sdk.Check[other]
	_ = sibling[*sdk.Timeout]
	_ = errorIs[sdk.Timeout](nil)
}
"#,
    );

    assert_eq!(
        accesses_on_line(&raw, 10),
        [("errorIs", Access::Read), ("Timeout", Access::Type)],
        "a pointer to an imported type; the generic function itself is read"
    );
    assert_eq!(
        accesses_on_line(&raw, 11),
        [("errorIs", Access::Read), ("Item", Access::Type)]
    );
    assert_eq!(
        accesses_on_line(&raw, 12),
        [("errorIs", Access::Read), ("Timeout", Access::Type)]
    );
    assert_eq!(
        accesses_on_line(&raw, 13),
        [
            ("errorIs", Access::Read),
            ("Box", Access::Type),
            ("Item", Access::Type)
        ],
        "an instantiated type argument is recorded once"
    );
    assert_eq!(
        accesses_on_line(&raw, 14),
        [
            ("pair", Access::Read),
            ("other", Access::Type),
            ("Timeout", Access::Type)
        ],
        "two indices are a type argument list"
    );
    assert_eq!(
        accesses_on_line(&raw, 15),
        [("Check", Access::Read), ("other", Access::Type)],
        "a bare name indexing an imported name is a type argument"
    );
    assert_eq!(
        accesses_on_line(&raw, 16),
        [("sibling", Access::Read), ("Timeout", Access::Type)],
        "a generic function a sibling file declares, known by the pointer"
    );
    assert_eq!(
        accesses_on_line(&raw, 17),
        [("Timeout", Access::Type)],
        "a call instantiates what no argument's spelling shows"
    );
}

#[test]
fn an_index_of_a_slice_or_a_map_reads_its_index() {
    let raw = extract(
        br#"package p

import "example.com/sdk"

var table = map[string]int{}

const first = "a"

func F(xs []int, i int) {
	_ = xs[i]
	_ = table[first]
	_ = xs[first]
	_ = table[sdk.Key]
	_ = sdk.Table[sdk.Key]
	_ = sibling[other]
	_ = sibling[sdk.Key]
	_ = sdk.Pick[sdk.Timeout]
}

func G(sdk Client, xs []int) {
	_ = sdk.Items[other]
	_ = xs[*head]
}

func H(xs []int) {
	_ = sdk.Table[*head]
	_ = xs[*sdk.Head]
}
"#,
    );

    assert_eq!(accesses_on_line(&raw, 10), [] as [(&str, Access); 0]);
    assert_eq!(
        accesses_on_line(&raw, 11),
        [("table", Access::Read), ("first", Access::Read)]
    );
    assert_eq!(accesses_on_line(&raw, 12), [("first", Access::Read)]);
    assert_eq!(
        accesses_on_line(&raw, 13),
        [("table", Access::Read), ("Key", Access::Read)]
    );
    assert_eq!(
        accesses_on_line(&raw, 14),
        [("Table", Access::Read), ("Key", Access::Read)]
    );
    assert_eq!(
        accesses_on_line(&raw, 15),
        [("sibling", Access::Read), ("other", Access::Read)],
        "a sibling file's map keyed by its constant"
    );
    assert_eq!(
        accesses_on_line(&raw, 16),
        [("sibling", Access::Read), ("Key", Access::Read)]
    );
    assert_eq!(
        accesses_on_line(&raw, 17),
        [("Pick", Access::Read), ("Timeout", Access::Read)],
        "an imported name indexed by an imported name reads as a map keyed by a constant, which outnumbers it"
    );
    assert_eq!(
        accesses_on_line(&raw, 21),
        [("Items", Access::Read), ("other", Access::Read)],
        "a parameter that shadows an import holds a value"
    );
    assert_eq!(
        accesses_on_line(&raw, 22),
        [("head", Access::Read)],
        "a pointer read through in the index of a local"
    );
    assert_eq!(
        accesses_on_line(&raw, 26),
        [("Table", Access::Read), ("head", Access::Read)],
        "a pointer to a name the file declares as no type is read through"
    );
    assert_eq!(
        accesses_on_line(&raw, 27),
        [("Head", Access::Read)],
        "a pointer on another package's name indexes a local value"
    );
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
    assert!(!raw.uses.iter().any(|site| site.name == "L"));
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
fn an_alias_is_a_type_alias_and_a_defined_non_struct_type_a_defined_type() {
    let raw = extract(SAMPLE);

    assert_eq!(qualified(&raw, "sample.Alias").kind, SymbolKind::TypeAlias);
    assert_eq!(
        qualified(&raw, "sample.Handler").kind,
        SymbolKind::DefinedType
    );
}

#[test]
fn an_embedded_type_is_a_parent_and_an_asserted_interface_is_an_assertion() {
    let raw = extract(SAMPLE);

    assert_eq!(qualified(&raw, "sample.Service").parents, ["Base"]);
    assert_eq!(
        assertions(&raw),
        [("Service", "Greeter")],
        "an assertion embeds nothing"
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

    assert_eq!(assertions(&raw), [("T", "I")]);
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

    assert_eq!(assertions(&raw), [("T", "I")]);
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
            ("item_store", None, "sizes", 24),
            ("Item", None, "sizes", 25),
            ("Item", Some("pkg"), "sizes", 26),
            ("List", None, "sizes", 27),
            ("Item", None, "sizes", 27),
            ("Buffer", None, "sizes", 28),
            ("Item", None, "sizes", 29),
            ("Item", Some("pkg"), "sizes", 29),
            ("convert", None, "sizes", 30),
            ("Item", None, "sizes", 30),
            ("Item", Some("pkg"), "sizes", 30),
        ],
        "the type operand of make and new, a parenthesized conversion type and its type argument, a type argument, \
         and a generic function used as a value are uses, a qualified one under any import spelling; a predeclared \
         constant and a local value are not, and a selector on a local value uses a member"
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
        .uses
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
        .uses
        .iter()
        .map(|site| (site.name.as_str(), qualifier(site), site.line))
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

    assert_eq!(
        assertions(&raw),
        [
            ("Provider", "Codec"),
            ("pkg.Remote", "Codec"),
            ("Box", "Codec")
        ],
        "a qualified type keeps its package, and an instantiated type names its base"
    );
}

#[test]
fn an_assertion_on_an_instantiated_or_parenthesized_interface_names_its_base() {
    let raw = extract(
        br#"
package sample

import "example.com/ext"

var _ I[int] = T{}

var _ ext.I[int] = T{}

var _ P[int, string] = T{}

var _ ext.P[int, string] = T{}

var _ (I) = T{}

var _ (ext.I) = T{}

var _ ((I[int])) = T{}

var _ interface{ M() } = T{}
"#,
    );

    assert_eq!(
        assertions(&raw),
        [
            ("T", "I"),
            ("T", "ext.I"),
            ("T", "P"),
            ("T", "ext.P"),
            ("T", "I"),
            ("T", "ext.I"),
            ("T", "I"),
        ],
        "an interface literal names nothing"
    );
}

#[test]
fn an_assertion_reads_each_spelling_of_a_conversion_to_its_type() {
    let shapes: [(&str, &str); 20] = [
        ("(*X)(nil)", "X"),
        ("(X)(nil)", "X"),
        ("X(nil)", "X"),
        ("X(0)", "X"),
        ("X(nil,)", "X"),
        ("X(nil /* no value */)", "X"),
        ("((X))(nil)", "X"),
        ("(*(X))(nil)", "X"),
        ("(pkg.X)(nil)", "pkg.X"),
        ("pkg.X(nil)", "pkg.X"),
        ("(*X[int])(nil)", "X"),
        ("(*X[int, string])(nil)", "X"),
        ("(*X[[]int])(nil)", "X"),
        ("(X[int])(nil)", "X"),
        ("X[int](nil)", "X"),
        ("X[int, string](nil)", "X"),
        ("X[[]int](nil)", "X"),
        ("(*pkg.X[int])(nil)", "pkg.X"),
        ("pkg.X[int](nil)", "pkg.X"),
        ("((X{}))", "X"),
    ];

    for (value, type_name) in shapes {
        let source = format!("package sample\n\nvar _ I = {value}\n");
        let raw = extract(source.as_bytes());

        assert_eq!(assertions(&raw), [(type_name, "I")], "{value}");
    }
}

#[test]
fn an_assertion_reads_past_parentheses_around_a_literal_or_an_allocation() {
    let shapes: [(&str, &str); 7] = [
        ("(&X{})", "X"),
        ("(X{})", "X"),
        ("(new(X))", "X"),
        ("new((X))", "X"),
        ("new(X[[]int])", "X"),
        ("&X[[]int]{}", "X"),
        ("new(pkg.X)", "pkg.X"),
    ];

    for (value, type_name) in shapes {
        let source = format!("package sample\n\nvar _ I = {value}\n");
        let raw = extract(source.as_bytes());

        assert_eq!(assertions(&raw), [(type_name, "I")], "{value}");
    }
}

#[test]
fn an_assertion_through_a_call_of_other_than_one_argument_names_none() {
    for value in ["NewT()", "NewT(a, b)", "pkg.NewT()", "pkg.NewT(a, b)"] {
        let source = format!("package sample\n\nvar _ I = {value}\n");
        let raw = extract(source.as_bytes());

        assert_eq!(
            assertions(&raw),
            [],
            "{value}: a conversion takes one operand"
        );
    }
}

#[test]
fn a_blank_name_asserts_the_value_at_its_own_position() {
    type Asserted = &'static [(&'static str, &'static str)];
    let shapes: [(&str, Asserted); 7] = [
        ("_, _ I = A{}, B{}", &[("A", "I"), ("B", "I")]),
        ("_, _ I = pair(), B{}", &[("B", "I")]),
        ("_, _ I = A{}, /* second */ B{}", &[("A", "I"), ("B", "I")]),
        ("_, kept I = A{}, B{}", &[("A", "I")]),
        ("kept, _ I = A{}, B{}", &[("B", "I")]),
        ("_, _ I = pair(T{})", &[]),
        ("_, kept I = pair(T{})", &[]),
    ];

    for (spec, expected) in shapes {
        let source = format!("package sample\n\nvar {spec}\n");
        let raw = extract(source.as_bytes());

        assert_eq!(assertions(&raw), expected, "{spec}");
    }
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

fn owned(raw: &RawExtraction, qualified_name: &str) -> (SymbolKind, Option<String>, Vec<String>) {
    let symbol = qualified(raw, qualified_name);
    (
        symbol.kind,
        symbol.owner.clone(),
        symbol.value_types.clone(),
    )
}

const ANONYMOUS_FIELDS: &[u8] = br"package app

type Server struct {
	Limits struct {
		MaxConns int
		Codec    Codec
	}
	Name string
}

type Table struct {
	Rows []struct {
		ID   int
		Meta *struct{ Tag string }
	}
	ByKey    map[string]struct{ base; N int }
	Done     chan struct{}
	_, Spare struct{ X int }
}

func Run(s *Server) int {
	return s.Limits.MaxConns
}
";

// The path with the index in braces replaced by the qualified name of the symbol it names.
fn hosted(raw: &RawExtraction, path: &str) -> String {
    let Some((head, rest)) = path.split_once('{') else {
        return path.to_owned();
    };
    let (index, tail) = rest
        .split_once('}')
        .unwrap_or_else(|| panic!("no brace closes the owner's index in {path}"));
    let index: usize = index
        .parse()
        .unwrap_or_else(|_| panic!("no owner's index in {path}"));
    let host = &raw.symbols[index];
    format!("{head}{{{}}}{tail}", host.qualified_name)
}

fn hosted_types(raw: &RawExtraction, qualified_name: &str) -> Vec<String> {
    let symbol = qualified(raw, qualified_name);
    (symbol.value_types.iter())
        .map(|path| hosted(raw, path))
        .collect()
}

fn hosted_members(raw: &RawExtraction) -> Vec<(&str, String, Access, u32)> {
    members(raw)
        .into_iter()
        .map(|(name, path, access, line)| (name, hosted(raw, path), access, line))
        .collect()
}

fn no_unspelled_symbol(raw: &RawExtraction) -> bool {
    !raw.symbols
        .iter()
        .any(|symbol| symbol.name.contains(['{', '\u{b7}']))
}

#[test]
fn a_field_of_an_anonymous_struct_a_field_declares_is_owned_by_that_field() {
    let raw = extract(ANONYMOUS_FIELDS);

    assert_eq!(
        owned(&raw, "app.Server.Limits"),
        (
            SymbolKind::Field,
            Some("Server".to_owned()),
            vec!["{1}".to_owned()]
        ),
        "the field keeps its name and its owner, and its value names its own index"
    );
    assert_eq!(
        owned(&raw, "app.Server.Limits.MaxConns"),
        (
            SymbolKind::Field,
            Some("Limits".to_owned()),
            vec!["int".to_owned()]
        )
    );
    assert!(no_unspelled_symbol(&raw), "the struct is no symbol");
    assert_eq!(
        owned(&raw, "app.Server.Limits.Codec").2,
        ["Codec"],
        "a field of an anonymous struct records its value's type"
    );
    assert_eq!(owned(&raw, "app.Server.Name").1.as_deref(), Some("Server"));
    let site = raw
        .uses
        .iter()
        .find(|site| site.name == "MaxConns")
        .map(|site| &site.receiver);
    assert_eq!(
        site,
        Some(&Receiver::Path("Server.Limits".to_owned())),
        "the member names the type path the graph follows through the field's value type"
    );
}

#[test]
fn an_anonymous_struct_behind_a_pointer_or_a_container_or_inside_another_is_owned_by_its_field() {
    let raw = extract(ANONYMOUS_FIELDS);

    assert_eq!(hosted_types(&raw, "app.Table.Rows"), ["[]{app.Table.Rows}"]);
    assert_eq!(owned(&raw, "app.Table.Rows.ID").1.as_deref(), Some("Rows"));
    assert_eq!(
        hosted_types(&raw, "app.Table.Rows.Meta"),
        ["{app.Table.Rows.Meta}"]
    );
    assert_eq!(
        owned(&raw, "app.Table.Rows.Meta.Tag").1.as_deref(),
        Some("Meta")
    );
    assert_eq!(
        hosted_types(&raw, "app.Table.ByKey"),
        ["[]{app.Table.ByKey}"]
    );
    assert_eq!(
        qualified(&raw, "app.Table.ByKey").parents,
        ["base"],
        "the field holds the types its struct embeds"
    );
    assert_eq!(
        owned(&raw, "app.Table.Spare.X").1.as_deref(),
        Some("Spare"),
        "the names of one declaration share its struct, owned by the first that is not blank"
    );
}

#[test]
fn an_anonymous_struct_without_a_field_takes_no_name() {
    let raw = extract(ANONYMOUS_FIELDS);

    assert_eq!(owned(&raw, "app.Table.Done").2, Vec::<String>::new());
    assert!(
        !raw.symbols
            .iter()
            .any(|symbol| symbol.name.contains("Done") && symbol.kind == SymbolKind::Struct),
        "struct{{}} holds nothing to name"
    );
}

#[test]
fn namesake_fields_of_two_bodies_types_each_own_their_anonymous_struct() {
    let raw = extract(
        b"package app

func A() {
	type row struct{ Cell struct{ a int } }
}

func B() {
	type row struct{ Cell struct{ b int } }
}
",
    );

    assert_eq!(owned(&raw, "app.row.Cell.a").1.as_deref(), Some("Cell"));
    assert_eq!(owned(&raw, "app.row.Cell.b").1.as_deref(), Some("Cell"));
    let cells: Vec<(usize, Vec<String>)> = (raw.symbols.iter().enumerate())
        .filter(|(_, symbol)| symbol.name == "Cell")
        .map(|(index, symbol)| (index, symbol.value_types.clone()))
        .collect();
    assert_eq!(
        cells,
        [(2, vec!["{2}".to_owned()]), (6, vec!["{6}".to_owned()])],
        "each field's value names its own index"
    );
}

#[test]
fn an_anonymous_struct_a_long_name_owns_is_reachable_through_the_owner_s_index() {
    let long = "F".repeat(160);
    let source = format!("package app\n\ntype T struct {{\n\t{long} struct{{ X int }}\n}}\n");
    let raw = extract(source.as_bytes());

    assert_eq!(owned(&raw, &format!("app.T.{long}")).2, ["{1}"]);
    assert_eq!(
        owned(&raw, &format!("app.T.{long}.X")).1.as_deref(),
        Some(long.as_str())
    );
}

#[test]
fn a_use_inside_an_anonymous_struct_a_field_declares_is_the_field_s_once() {
    let raw = extract(ANONYMOUS_FIELDS);

    assert_eq!(
        uses(&raw)
            .into_iter()
            .filter(|(name, ..)| *name == "Codec")
            .collect::<Vec<_>>(),
        [("Codec", None, "Limits", 6)]
    );
}

const ANONYMOUS_VARIABLES: &[u8] = br"package app

var settings struct {
	Timeout int
}

var left, right *struct{ X int }

func Run() int {
	var limits struct{ Max int }
	limits.Max = settings.Timeout
	return limits.Max
}

func (s *Server) Run() {
	var limits struct{ Max int }
	_ = limits
}
";

#[test]
fn a_variable_declared_with_an_anonymous_struct_owns_its_fields() {
    let raw = extract(ANONYMOUS_VARIABLES);

    assert_eq!(
        owned(&raw, "app.settings"),
        (SymbolKind::Variable, None, vec!["{0}".to_owned()])
    );
    assert_eq!(
        owned(&raw, "app.settings.Timeout").1.as_deref(),
        Some("settings")
    );
    assert_eq!(
        (
            hosted_types(&raw, "app.left"),
            hosted_types(&raw, "app.right")
        ),
        (vec!["{app.left}".to_owned()], vec!["{app.left}".to_owned()]),
        "the names of one spec share its struct, owned by the first"
    );
    assert_eq!(owned(&raw, "app.left.X").1.as_deref(), Some("left"));
    assert!(no_unspelled_symbol(&raw));
}

#[test]
fn a_variable_a_body_declares_qualifies_its_anonymous_struct_s_fields_by_the_callable_too() {
    let raw = extract(ANONYMOUS_VARIABLES);

    assert_eq!(
        owned(&raw, "app.Run.limits.Max").1.as_deref(),
        Some("limits")
    );
    assert_eq!(
        owned(&raw, "app.Server.Run.limits.Max").1.as_deref(),
        Some("limits"),
        "a method's value is qualified by its receiver's type"
    );
    let reads: Vec<(&str, String)> = hosted_members(&raw)
        .into_iter()
        .map(|(name, path, ..)| (name, path))
        .collect();
    assert_eq!(
        reads,
        [
            ("Max", "{app.limits}".to_owned()),
            ("Timeout", "settings".to_owned()),
            ("Max", "{app.limits}".to_owned())
        ],
        "a local's members name the local, and a package value's members the value"
    );
    let local = |qualified_name: &str| {
        raw.local_symbols
            .iter()
            .any(|&id| symbol(&raw, id).qualified_name == qualified_name)
    };
    assert!(local("app.limits") && local("app.Run.limits.Max"));
    assert!(!local("app.settings"));
}

const ANONYMOUS_LITERALS: &[u8] = br#"package app

var cases = []struct{ in, want string }{
	{in: "a", want: "A"},
}

var byName map[string][]struct{ ID int }

func TestRun(t *testing.T) {
	tests := []struct {
		name string
		run  func() int
	}{
		{name: "one"},
	}
	for _, tt := range tests {
		t.Run(tt.name, nil)
	}
	point, n := &struct{ X, Y int }{X: 1}, 2
	_ = point.Y
	print(struct{ Z int }{Z: n})
	wrapped := (struct{ W int }{})
	_ = wrapped
}
"#;

#[test]
fn a_literal_assigned_to_a_name_declares_its_anonymous_struct_s_fields() {
    let raw = extract(ANONYMOUS_LITERALS);

    assert_eq!(hosted_types(&raw, "app.cases"), ["[]{app.cases}"]);
    assert_eq!(owned(&raw, "app.cases.want").1.as_deref(), Some("cases"));
    assert_eq!(
        hosted_types(&raw, "app.byName"),
        ["[][]{app.byName}"],
        "a variable's written type names it through containers"
    );
    assert_eq!(owned(&raw, "app.byName.ID").1.as_deref(), Some("byName"));
    assert_eq!(
        owned(&raw, "app.TestRun.tests.run").1.as_deref(),
        Some("tests")
    );
    assert_eq!(
        owned(&raw, "app.TestRun.point.Y").1.as_deref(),
        Some("point"),
        "a literal behind & is the value's too"
    );
    assert_eq!(
        owned(&raw, "app.TestRun.wrapped.W").1.as_deref(),
        Some("wrapped"),
        "and so is one in parentheses"
    );
    assert!(
        !raw.symbols.iter().any(|symbol| symbol.name == "Z"),
        "a literal assigned to no name declares nothing"
    );
    let variables: Vec<(&str, String, bool)> = (raw.symbols.iter().enumerate())
        .filter(|(_, symbol)| symbol.kind == SymbolKind::Variable)
        .map(|(index, symbol)| {
            let local = raw.local_symbols.iter().any(|id| id.0 as usize == index);
            let types = symbol.value_types.iter().map(|path| hosted(&raw, path));
            (symbol.name.as_str(), types.collect(), local)
        })
        .collect();
    assert_eq!(
        variables,
        [
            ("cases", "[]{app.cases}".to_owned(), false),
            ("byName", "[][]{app.byName}".to_owned(), false),
            ("tests", "[]{app.tests}".to_owned(), true),
            ("point", "{app.point}".to_owned(), true),
            ("wrapped", "{app.wrapped}".to_owned(), true),
        ],
        "a `:=` name is a variable of its body where it owns its literal's struct, and no other is"
    );
    assert!(no_unspelled_symbol(&raw));
}

#[test]
fn a_name_a_short_declaration_assigns_again_owns_no_second_struct() {
    let raw = extract(
        b"package app\n\nfunc F() {\n\tp := struct{ X int }{}\n\tp, q := struct{ X int }{X: 1}, 2\n\t_ = q\n}\n",
    );

    let owners: Vec<(&str, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.owner.as_deref()))
        .collect();
    assert_eq!(owners, [("F", None), ("p", None), ("X", Some("p"))]);
}

#[test]
fn a_key_of_a_literal_assigned_to_a_name_writes_its_anonymous_struct_s_field() {
    let raw = extract(ANONYMOUS_LITERALS);

    let hosted = |name, path: &str, access, line| (name, path.to_owned(), access, line);
    assert_eq!(
        hosted_members(&raw),
        [
            hosted("in", "{app.cases}", Access::Write, 4),
            hosted("want", "{app.cases}", Access::Write, 4),
            hosted("name", "{app.tests}", Access::Write, 14),
            hosted("name", "{app.tests}", Access::Read, 17),
            hosted("X", "{app.point}", Access::Write, 19),
            hosted("Y", "{app.point}", Access::Read, 20),
        ],
        "an element's struct comes from its slice, a range's element reads it, and a literal assigned to no name \
         keeps no key"
    );
    assert!(
        !raw.uses.iter().any(|site| site.name == "Z"),
        "nor is its key a use of another kind"
    );
}

#[test]
fn a_value_s_struct_keeps_its_field_keys_beside_a_container_type_of_the_value_s_name() {
    let raw = extract(
        br"package app

type cases map[string]int

func f() {
	cases := struct{ in int }{in: 1}
	_ = cases
}
",
    );

    let hosted = |name, path: &str, access, line| (name, path.to_owned(), access, line);
    assert_eq!(
        hosted_members(&raw),
        [hosted("in", "{app.cases}", Access::Write, 6)],
        "`{{N}}` here names a value, so the package's type of its name says nothing of the keys"
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
        symbol(&raw, helper_call.site_of).qualified_name,
        "sample.Service.Greet"
    );
    assert_eq!(helper_call.receiver, None);
}

fn receivers(raw: &RawExtraction) -> Vec<(&str, Option<&str>)> {
    raw.calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect()
}

#[test]
fn a_member_call_on_a_chain_records_the_type_path_of_its_operand() {
    let raw = extract(b"package p\nfunc f(s S) { s.Items().First(v) }\n");

    assert_eq!(
        receivers(&raw),
        [("First", Some("S.Items()")), ("Items", Some("S"))]
    );
}

#[test]
fn a_member_call_on_a_value_the_walk_cannot_type_keeps_an_empty_receiver() {
    let raw = extract(
        b"package p\nfunc f(fn func() S, one, two T) {\n\tfn().First()\n\tsum := one + two\n\tsum.Close()\n}\n",
    );

    assert_eq!(receivers(&raw), [("First", Some("")), ("Close", Some(""))]);
}

#[test]
fn a_receiver_names_the_type_its_value_is_declared_with() {
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

    assert_eq!(
        receivers(&raw),
        [
            ("draw", Some("Widget")),
            ("Close", Some("Server")),
            ("Dial", Some("conn")),
            ("Close", Some("conn.Dial()")),
            ("Close", Some("Server")),
            ("Close", Some("conn")),
            ("helper", Some("Widget")),
            ("Close", Some("Server")),
            ("Flush", Some("Widget")),
            ("Close", Some("servers[]")),
            ("Close", Some("Server")),
        ]
    );
}

#[test]
fn a_call_of_a_local_spells_a_value_apart_from_a_conversion_to_its_type() {
    let raw = extract(
        br#"package p

import "example.com/pkg"

func (r Recv) F(g G, q pkg.G) {
	g().A()
	q().B()
	r().C()
	G(g).D()
	pkg.G(q).E()
	s := g
	s().I()
	one, two := g()
	two.J()
	_ = one
	type L func() E
	L(g).K()
}
"#,
    );

    let calls = receivers(&raw);
    assert_eq!(
        calls
            .iter()
            .filter(|(callee, _)| callee.len() == 1
                && callee.as_bytes()[0] >= b'A'
                && *callee != "G")
            .copied()
            .collect::<Vec<_>>(),
        [
            ("A", Some("G~()")),
            ("B", Some("pkg.G~()")),
            ("C", Some("Recv~()")),
            ("D", Some("G()")),
            ("E", Some("pkg.G()")),
            ("I", Some("G~()")),
            ("J", Some("G~()#1")),
            ("K", Some("{1}")),
        ]
    );
}

#[test]
fn a_type_a_body_declares_is_spelled_by_its_symbol_from_its_identifier_on() {
    let raw = extract(
        br"package p

func f(s S) {
	s.A()
	type S struct{ next *S }
	var t S
	t.B()
	t.next.C()
	new(S).D()
	S.E(t)
	[]S{t}[0].F()
	type U S
	U{}.G()
	new(t).H()
}
",
    );

    let symbol = raw
        .symbols
        .iter()
        .position(|symbol| symbol.name == "S")
        .expect("the walk declares the local type");
    let held = format!("{{{symbol}}}");
    let next = format!("{held}.next");
    let own = raw
        .symbols
        .iter()
        .position(|symbol| symbol.name == "U")
        .expect("the walk declares the local type");
    let defined = format!("{{{own}}}");
    assert_eq!(
        receivers(&raw)
            .into_iter()
            .filter(|(callee, _)| callee.len() == 1)
            .collect::<Vec<_>>(),
        [
            ("A", Some("S")),
            ("B", Some(held.as_str())),
            ("C", Some(next.as_str())),
            ("D", Some(held.as_str())),
            ("E", Some(held.as_str())),
            ("F", Some(held.as_str())),
            ("G", Some(defined.as_str())),
            ("H", Some(held.as_str())),
        ],
        "a name reads as what the scope at its site gives, so the site before the declaration names the parameter's type"
    );
}

#[test]
fn a_local_s_type_path_follows_how_its_declaration_gives_the_value() {
    let raw = extract(
        br#"package p

import "example.com/pkg"

func F(xs []*pkg.Item, m map[string]Box, ch chan Box, anything any) {
	lit := &Box{}
	lit.A()
	made := new(pkg.Item)
	made.B()
	one, two := pkg.Open()
	one.C()
	two.D()
	asserted, ok := anything.(*Box)
	asserted.E()
	xs[0].F()
	for _, each := range m {
		each.G()
	}
	got := <-ch
	got.H()
	value, found := m["k"]
	value.I()
	switch typed := anything.(type) {
	case Box, *pkg.Item:
		typed.J()
	}
	_, _ = ok, found
}
"#,
    );

    assert_eq!(
        receivers(&raw),
        [
            ("A", Some("Box")),
            ("new", None),
            ("B", Some("pkg.Item")),
            ("Open", Some("pkg")),
            ("C", Some("pkg.Open()")),
            ("D", Some("pkg.Open()#1")),
            ("E", Some("Box")),
            ("F", Some("pkg.Item")),
            ("G", Some("Box")),
            ("H", Some("Box")),
            ("I", Some("Box")),
            ("J", Some("any")),
        ],
        "a clause that names several types leaves the variable the guard's type"
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
fn a_definition_records_the_types_of_the_values_it_yields() {
    let raw = extract(
        br#"package p

import "example.com/pkg"

type Server struct {
	client, backup *pkg.Client
	hooks          []Hook
	handler        func()
}

type Alias = pkg.Client

type Servers []*Server

type Factory func() (*Server, error)

type AliasOfFunction = func() Server

type NoResults func()

type Defined Server

type OverImported pkg.Client

type OverGeneric pkg.Box[Server]

type OverPointer *Server

type OverParenthesized (Server)

type OverParenthesizedPointer (*Server)

var Default = NewServer()

var typed pkg.Client

var literal = &Server{}

func NewServer() *Server { return nil }

func Open() (conn *pkg.Conn, closer, other func(), err error) { return }

func (s *Server) Wait() {}

func Each[T any](v T) T { return v }
"#,
    );

    let expected: [(&str, &[&str]); 22] = [
        ("p.Server.client", &["pkg.Client"]),
        ("p.Server.backup", &["pkg.Client"]),
        ("p.Server.hooks", &["[]Hook"]),
        ("p.Server.handler", &[]),
        ("p.Alias", &["pkg.Client"]),
        ("p.Servers", &["[]Server"]),
        ("p.Factory", &["()", "Server", "error"]),
        ("p.AliasOfFunction", &["()", "Server"]),
        ("p.NoResults", &[]),
        ("p.Defined", &["~Server"]),
        ("p.OverImported", &["~pkg.Client"]),
        ("p.OverGeneric", &["~pkg.Box"]),
        ("p.OverPointer", &[]),
        ("p.OverParenthesized", &["~Server"]),
        ("p.OverParenthesizedPointer", &[]),
        ("p.Default", &["NewServer()"]),
        ("p.typed", &["pkg.Client"]),
        ("p.literal", &["Server"]),
        ("p.NewServer", &["Server"]),
        ("p.Open", &["pkg.Conn", "", "", "error"]),
        ("p.Server.Wait", &[]),
        ("p.Each", &[]),
    ];
    for (qualified, value_types) in expected {
        let symbol = raw
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == qualified)
            .unwrap_or_else(|| panic!("{qualified} is extracted"));
        assert_eq!(
            symbol.value_types, value_types,
            "{qualified}: a function literal type, a pointer a type is defined over and a type parameter name no path"
        );
    }
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
    assert_eq!(assertions(&raw), [("Stack", "Pusher")]);
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
fn every_free_init_of_a_package_is_an_entry_point() {
    let raw = extract(
        br"
package worker

type Pool struct{}

func (p *Pool) init() {}

func init() {}

func setup() {}

func init() { setup() }
",
    );

    let inits: Vec<&Symbol> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "worker.init")
        .collect();
    assert_eq!(inits.len(), 2);
    assert!(inits.iter().all(|init| init.flags.is_entry()));
    assert!(!qualified(&raw, "worker.Pool.init").flags.is_entry());
    assert!(!qualified(&raw, "worker.setup").flags.is_entry());
}

#[test]
fn an_init_that_takes_or_returns_a_value_is_not_an_entry_point() {
    for source in [
        &b"package worker\n\nfunc init(n int) {}\n"[..],
        b"package worker\n\nfunc init() error { return nil }\n",
    ] {
        let raw = extract(source);

        assert!(!qualified(&raw, "worker.init").flags.is_entry());
    }
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
            ("F", "Get", Some("Table"), 17),
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
        .uses
        .iter()
        .filter(|site| names_no_member(site))
        .map(|site| (site.name.as_str(), qualifier(site), site.line))
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
        .uses
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
            let caller = symbol(&raw, call.site_of).name.as_str();
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
            ("Later", 15, "helper", SymbolKind::DefinedType, 15),
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
        .uses
        .iter()
        .filter(|site| names_no_member(site))
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
            ("renamed", None, "Run", 9),
            ("settings", None, "Run", 10),
        ],
        "a package clause need not be the import's last element, so a qualifier no import spells is also read as a \
         value a sibling file may declare; a package-level value of the file is read by name"
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

const ACCESSES: &[u8] = br#"package svc

import "example.com/a"

type Config struct {
	Timeout int
	Retries int
}

var limit = 3
const ratio = 2

func Load(c *Config) {
	c.Timeout = limit
	c.Retries += ratio
	c.Retries++
	_ = c.Timeout
	x := Config{Timeout: 1}
	_ = []Config{{Retries: 2}}
	_ = map[string]Config{"k": {Timeout: 3}}
	_ = map[int]int{limit: ratio}
	_ = []struct{ name string }{{name: "t"}}
	_ = a.Options{Debug: true}
	limit = x.Timeout
	c.Reset()
	f := c.Reset
	n := len(f)
	n++
}
"#;

fn members(raw: &RawExtraction) -> Vec<(&str, &str, Access, u32)> {
    raw.uses
        .iter()
        .filter_map(|site| match &site.receiver {
            Receiver::Path(path) => {
                Some((site.name.as_str(), path.as_str(), site.access, site.line))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_member_taken_on_a_value_is_a_use_spelled_with_its_type_and_how_it_is_touched() {
    let raw = extract(ACCESSES);

    assert_eq!(
        members(&raw),
        [
            ("Timeout", "Config", Access::Write, 14),
            ("Retries", "Config", Access::Update, 15),
            ("Retries", "Config", Access::Update, 16),
            ("Timeout", "Config", Access::Read, 17),
            ("Timeout", "Config", Access::Write, 18),
            ("Retries", "Config", Access::Write, 19),
            ("Timeout", "Config", Access::Write, 20),
            ("Debug", "a.Options", Access::Write, 23),
            ("Timeout", "Config", Access::Read, 24),
            ("Reset", "Config", Access::Read, 26),
        ],
        "a key of a struct literal writes its field, an element's type comes from its slice or map; a method value \
         is a use and a called method is not; the keys of an anonymous struct assigned to no name name no field"
    );
    assert!(
        !raw.uses.iter().any(|site| site.receiver == Receiver::Bare
            && matches!(site.name.as_str(), "Timeout" | "Retries" | "Reset" | "name")),
        "a member's name is no bare use"
    );
}

#[test]
fn a_literal_key_takes_its_type_from_the_literal_around_it_at_any_depth() {
    let raw = extract(
        br#"package svc

type Config struct {
	Timeout int
}

type Holder struct {
	Run   func()
	Limit int
}

func Load() {
	_ = map[Config]Config{{Timeout: 1}: {Timeout: 2}}
	_ = [][]Config{{{Timeout: 3}}, {{Timeout: 4}}}
	_ = map[string][]Config{"k": {{Timeout: 5}}}
	_ = Holder{Run: func() { _ = Config{Timeout: 6} }, Limit: 7}
}
"#,
    );

    assert_eq!(
        members(&raw),
        [
            ("Timeout", "Config", Access::Write, 13),
            ("Timeout", "Config", Access::Write, 14),
            ("Timeout", "Config", Access::Write, 14),
            ("Timeout", "Config", Access::Write, 15),
            ("Run", "Holder", Access::Write, 16),
            ("Timeout", "Config", Access::Write, 16),
            ("Limit", "Holder", Access::Write, 16),
        ],
        "an element's type comes down through each literal that elides it, a map key's literal keeps none, and a key \
         after a closed literal belongs to the literal still open"
    );
    assert!(
        !raw.uses
            .iter()
            .any(|site| site.receiver == Receiver::Bare && site.name == "Timeout"),
        "a key the walk cannot type is no bare use"
    );
}

#[test]
fn a_key_inside_a_parse_error_reads_one_literal_deeper_than_the_error() {
    let raw = extract(
        br"package svc

type T struct {
	A int
}

func Load() {
	_ = T{B: {A: 1 ) A: 2}}
}
",
    );

    assert_eq!(
        members(&raw),
        [
            ("B", "T", Access::Write, 8),
            ("A", "T[][]", Access::Write, 8),
            ("A", "T[]", Access::Write, 8),
        ],
        "an error node stands in the chain of literals as an elided element does"
    );
}

#[test]
fn a_key_of_a_named_container_or_an_unexported_key_of_another_packages_type_is_a_value() {
    let raw = extract(
        br#"package svc

import "example.com/a"

type Kind int

const (
	KindA Kind = iota
	KindB
)

type (
	Names map[Kind]string
	Table = Names
	Order [2]string
)

func Build() {
	_ = Names{KindA: "a"}
	_ = Table{KindB: "b"}
	_ = &Order{KindA: "x"}
	_ = []Names{{KindB: "b"}}
	_ = a.Filter{fieldName: 1}
	_ = []a.Filter{{fieldName: 2}}
	_ = a.Options{Debug: true}
	_ = Remote{KindA: "r"}
	type Own map[Kind]string
	_ = Own{KindB: "o"}
	_ = []Own{{KindA: "o"}}
}

const fieldName = "f"
"#,
    );
    let reads: Vec<(&str, u32)> = raw
        .uses
        .iter()
        .filter(|site| site.receiver == Receiver::Bare && site.access == Access::Read)
        .filter(|site| matches!(site.name.as_str(), "KindA" | "KindB" | "fieldName"))
        .map(|site| (site.name.as_str(), site.line))
        .collect();

    assert_eq!(
        reads,
        [
            ("KindA", 19),
            ("KindB", 20),
            ("KindA", 21),
            ("KindB", 22),
            ("fieldName", 23),
            ("fieldName", 24),
            ("KindB", 28),
            ("KindA", 29),
        ],
        "a key of a map or an array type the file declares, or an unexported key of a type another package declares, \
         reads the value it names"
    );
    assert_eq!(
        members(&raw),
        [
            ("Debug", "a.Options", Access::Write, 25),
            ("KindA", "Remote", Access::Write, 26)
        ],
        "an exported key of another package's type, or a key of a type the file does not declare, writes a field"
    );
}

#[test]
fn a_positional_struct_literal_writes_the_fields_its_file_declares_in_order() {
    let raw = extract(
        br#"package svc

import "example.com/a"

type P struct{ X, Y int }

type (
	Row struct {
		Base
		_    int
		Name string
	}
	Rows  []Row
	Point = P
)

func Build() {
	type pair struct{ L, R string }
	_ = P{1, 2}
	_ = &Point{3, 4}
	_ = []P{{5, 6}}
	_ = map[string]P{"k": {7, 8}}
	_ = [...]*P{{9, 10}}
	_ = Rows{{Base{}, 0, "r"}}
	_ = pair{"l", "r"}
	cases := []struct{ in, want string }{{"a", "A"}}
	_ = cases
	_ = Remote{1, 2}
	_ = a.Options{true}
	_ = P{7}
}
"#,
    );
    let members = hosted_members(&raw);
    let writes: Vec<(&str, &str, u32)> = members
        .iter()
        .filter(|&&(_, _, access, _)| access == Access::Write)
        .map(|(name, path, _, line)| (*name, path.as_str(), *line))
        .collect();

    assert_eq!(
        writes,
        [
            ("X", "P", 19),
            ("Y", "P", 19),
            ("X", "Point", 20),
            ("Y", "Point", 20),
            ("X", "P", 21),
            ("Y", "P", 21),
            ("X", "P", 22),
            ("Y", "P", 22),
            ("X", "P", 23),
            ("Y", "P", 23),
            ("Name", "Rows[]", 24),
            ("L", "{svc.pair}", 25),
            ("R", "{svc.pair}", 25),
            ("in", "{svc.cases}", 26),
            ("want", "{svc.cases}", 26),
        ],
        "each element writes the field in its place, an embedded or blank field takes a place and records nothing, and \
         a type the file does not declare or a literal that does not set every field records none"
    );
}

#[test]
fn a_positional_literal_follows_eight_names_to_its_struct_and_a_cycle_records_nothing() {
    let raw = extract(
        b"package svc\n\ntype S struct{ X int }\n\ntype (\n\tA1 = S\n\tA2 = A1\n\tA3 = A2\n\tA4 = A3\n\t\
          A5 = A4\n\tA6 = A5\n\tA7 = *A6\n\tL1 B1\n\tB1 L1\n)\n\n\
          var _ = []A7{{1}}\nvar _ = L1{2}\n",
    );

    assert_eq!(
        members(&raw),
        [("X", "A7", Access::Write, 17)],
        "eight names reach the struct, a pointer among them counting none, and a cycle of names records nothing"
    );
}

fn values(raw: &RawExtraction) -> Vec<(&str, Access, u32)> {
    raw.uses
        .iter()
        .filter(|site| site.receiver == Receiver::Bare && site.name.starts_with(char::is_lowercase))
        .map(|site| (site.name.as_str(), site.access, site.line))
        .collect()
}

#[test]
fn a_range_or_a_receive_that_assigns_writes_the_names_on_its_left() {
    let raw = extract(
        b"package svc

var last, cursor int

func Drain(ch chan int, items []int) {
	for cursor, last = range items {
	}
	select {
	case last = <-ch:
	}
}
",
    );

    assert_eq!(
        values(&raw),
        [
            ("cursor", Access::Write, 6),
            ("last", Access::Write, 6),
            ("last", Access::Write, 9),
        ]
    );
}

#[test]
fn a_name_a_for_clause_declares_is_local_through_the_loop_s_body() {
    let raw = extract(
        b"package svc

func Sum(items []int) int {
	sum := 0
	for index := 0; index < 3; index++ {
		sum += items[index]
	}
	return sum
}
",
    );

    assert_eq!(values(&raw), []);
}

#[test]
fn a_type_case_gives_the_switch_s_variable_the_type_it_names() {
    let raw = extract(
        b"package svc

type T struct{}

func (T) Run() {}

func Drive(value any) {
	switch held := value.(type) {
	case T:
		held.Run()
	}
}
",
    );

    assert_eq!(receivers(&raw), [("Run", Some("T"))]);
}

#[test]
fn a_selector_a_type_declaration_holds_finds_its_access_past_the_walk() {
    let raw = extract(
        br#"package svc

import "unsafe"

type Config struct {
	Limit int
}

var cfg Config

type Table [unsafe.Sizeof(func() { cfg.Limit = 1 })]int
"#,
    );

    assert_eq!(
        members(&raw),
        [("Limit", "cfg", Access::Write, 11)],
        "the walk never descends a type declaration, so its stack of ancestors stops above this selector"
    );
}

#[test]
fn a_name_no_local_binds_is_a_read_or_a_write_of_what_the_package_declares() {
    let raw = extract(ACCESSES);

    assert_eq!(
        values(&raw),
        [
            ("limit", Access::Read, 14),
            ("ratio", Access::Read, 15),
            ("limit", Access::Read, 21),
            ("ratio", Access::Read, 21),
            ("limit", Access::Write, 24),
        ],
        "a map literal's keys are values; a declared local, a parameter and a builtin are no use"
    );
}

#[test]
fn a_selector_operand_no_import_of_the_file_names_is_also_a_read_of_a_package_value() {
    let raw = extract(
        b"package svc

import (
	\"fmt\"
	yaml \"gopkg.in/yaml.v3\"
)

func Run() int {
	client.Do()
	_, _ = fmt.Println, yaml.Marshal
	return cfg.Server.Address
}
",
    );

    assert_eq!(
        values(&raw),
        [("client", Access::Read, 9), ("cfg", Access::Read, 11)],
        "a sibling file of the package may declare the operand; the name an import binds is no value"
    );
    assert_eq!(
        uses(&raw),
        [
            ("client", None, "Run", 9),
            ("Println", Some("fmt"), "Run", 10),
            ("Marshal", Some("yaml"), "Run", 10),
            ("Server", Some("cfg"), "Run", 11),
            ("cfg", None, "Run", 11),
        ],
        "the selector keeps the qualifier, for the graph to bind through an import or the package's value"
    );
}

fn doc_line(source: &str, qualified_name: &str) -> u32 {
    let line = qualified(&extract(source.as_bytes()), qualified_name).line;
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Go::new().doc_comments().first_line(&lines, line)
}

#[test]
fn line_and_block_comments_directly_above_a_definition_document_it() {
    let source = "package a

// Parse reads.
// It fails on nothing.
func Parse() {}

/*
Render draws.
*/
func Render() {}

var x = 1
func After() {}
";

    assert_eq!(doc_line(source, "a.Parse"), 3);
    assert_eq!(doc_line(source, "a.Render"), 7);
    assert_eq!(doc_line(source, "a.After"), 13);
}

#[test]
fn the_library_interface_methods_ascend_without_a_repeat_and_name_the_library_s() {
    let methods = Go::new().conventions().library_interface_methods;

    assert!(methods.is_sorted_by(|earlier, later| earlier < later));
    for method in [
        "Error",
        "Is",
        "Less",
        "ServeHTTP",
        "Unwrap",
        "UnmarshalJSONFrom",
    ] {
        assert!(methods.binary_search(&method).is_ok(), "{method}");
    }
}

fn fastest_of_three_extractions(source: &str) -> Duration {
    let run = || {
        let started = Instant::now();
        extract(source.as_bytes());
        started.elapsed()
    };
    run().min(run()).min(run())
}

#[test]
fn eight_times_the_top_level_comments_cost_about_eight_times_the_walk() {
    let comments = |lines: usize| format!("package p\n{}", "//\n".repeat(lines));
    let short = fastest_of_three_extractions(&comments(2_500));
    let long = fastest_of_three_extractions(&comments(20_000));

    assert!(long < short * 24, "{short:?} became {long:?}");
}

#[test]
fn eight_times_the_comments_in_one_field_cost_about_eight_times_a_positional_literal() {
    let literal = |lines: usize| {
        let comments = "//\n".repeat(lines);
        format!(
            "package p\ntype T struct {{ a,\n{comments}b int }}\nfunc f() {{ _ = T{{ 1, 2 }} }}\n"
        )
    };
    let short = fastest_of_three_extractions(&literal(2_500));
    let long = fastest_of_three_extractions(&literal(20_000));

    assert!(long < short * 24, "{short:?} became {long:?}");
}
