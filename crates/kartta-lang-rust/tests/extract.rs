//! What a Rust file extracts to from its bytes alone: its items and members, the calls its functions make, and the
//! code its attributes and banner mark; and where the doc comment of an item starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Access, FILE_SCOPE, Language, RawExtraction, Receiver, Symbol, SymbolId, SymbolKind, TypeUses,
};
use kartta_lang_rust::Rust;

const SAMPLE: &[u8] = br#"
pub struct Service {
    name: String,
}

enum State {
    Idle,
    Busy,
}

pub trait Greeter {
    fn greet(&self) -> String;
}

pub type Alias = Service;

macro_rules! greet_all {
    () => {};
}

mod internal {
    pub const MAX: u32 = 10;
    static COUNTER: u32 = 0;
}

impl Greeter for Service {
    fn greet(&self) -> String {
        helper(&self.name)
    }
}

impl Iterator for Service {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        None
    }
}

impl Service {
    pub fn new(name: String) -> Service {
        Service { name }
    }

    fn touch(&self) {
        self.greet();
        log::info!("touched");
        std::mem::drop(0);
        std::mem::size_of::<u8>();
        let buf: Vec<u8> = Vec::<u8>::new();
        Self::new(String::new());
    }
}

pub fn helper(v: &str) -> String {
    v.to_owned()
}

fn private_fn() {
    println!("hi");
}
"#;

fn extract(source: &[u8]) -> RawExtraction {
    Rust::new().extract(source)
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
        .position(|symbol| symbol.qualified_name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"));
    SymbolId(u32::try_from(at).unwrap_or(u32::MAX))
}

fn qualified_names(raw: &RawExtraction) -> Vec<&str> {
    raw.symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect()
}

#[test]
fn the_runtime_methods_ascend_without_a_repeat_and_name_the_standard_library_s() {
    let methods = Rust::new().conventions().runtime_methods;

    assert!(methods.is_sorted_by(|earlier, later| earlier < later));
    for method in ["clone", "unwrap", "iter", "write"] {
        assert!(methods.binary_search(&method).is_ok(), "{method}");
    }
}

fn is_test(source: &[u8], name: &str) -> bool {
    named(&extract(source), name).flags.is_test()
}

fn is_generated(source: &[u8]) -> bool {
    let raw = extract(source);
    assert_ne!(raw.symbols.len(), 0);
    raw.symbols.iter().all(|symbol| symbol.flags.is_generated())
}

#[test]
fn items_and_members_are_symbols_in_source_order() {
    let raw = extract(SAMPLE);

    assert_eq!(
        qualified_names(&raw),
        [
            "Service",
            "Service::name",
            "State",
            "State::Idle",
            "State::Busy",
            "Greeter",
            "Greeter::greet",
            "Alias",
            "greet_all",
            "internal",
            "internal::MAX",
            "internal::COUNTER",
            "Service::greet",
            "Service::Item",
            "Service::next",
            "Service::new",
            "Service::touch",
            "helper",
            "private_fn",
        ]
    );
    let kinds = [
        ("Service", SymbolKind::Struct),
        ("State", SymbolKind::Enum),
        ("Greeter", SymbolKind::Interface),
        ("Alias", SymbolKind::TypeAlias),
        ("greet_all", SymbolKind::Macro),
        ("internal", SymbolKind::Namespace),
        ("MAX", SymbolKind::Constant),
        ("COUNTER", SymbolKind::Variable),
        ("helper", SymbolKind::Function),
        ("name", SymbolKind::Field),
        ("Idle", SymbolKind::EnumVariant),
    ];
    for (name, kind) in kinds {
        assert_eq!(named(&raw, name).kind, kind, "{name}");
    }
}

#[test]
fn fields_and_variants_are_owned_by_their_type() {
    let raw = extract(SAMPLE);
    let name = qualified(&raw, "Service::name");

    assert_eq!(
        (name.kind, name.owner.as_deref(), name.exported),
        (SymbolKind::Field, Some("Service"), false)
    );
    for variant in ["State::Idle", "State::Busy"] {
        let variant = qualified(&raw, variant);
        assert_eq!(variant.kind, SymbolKind::EnumVariant);
        assert_eq!(variant.owner.as_deref(), Some("State"));
    }
}

#[test]
fn an_impl_member_is_a_method_of_its_type_and_a_free_function_is_owned_by_nothing() {
    let raw = extract(SAMPLE);
    let greet = qualified(&raw, "Service::greet");
    let helper = named(&raw, "helper");

    assert_eq!(
        (greet.kind, greet.owner.as_deref()),
        (SymbolKind::Method, Some("Service"))
    );
    assert_eq!(
        (helper.kind, helper.owner.as_deref()),
        (SymbolKind::Function, None)
    );
}

#[test]
fn a_wasm_bindgen_method_is_a_member_of_the_type_its_this_parameter_names() {
    let raw = extract(
        b"#[wasm_bindgen]\nextern \"C\" {\n    pub type Dict;\n\
          \x20   #[doc = \"Change the `b` field.\"]\n    #[wasm_bindgen(method, setter = \"b\")]\n\
          \x20   pub fn set_b(this: &Dict, val: f64);\n\
          \x20   #[wasm_bindgen::prelude::wasm_bindgen(catch, method)]\n    pub fn at(this: &Array<u8>, index: u32);\n\
          \x20   #[wasm_bindgen(js_name = isNaN)]\n    pub fn is_nan(value: f64) -> bool;\n}\n\n\
          mod web {\n    #[wasm_bindgen]\n    extern \"C\" {\n        pub type Node;\n\
          \x20       #[wasm_bindgen(method)]\n        pub fn remove(this: &Node);\n    }\n}\n",
    );
    let member = |qualified_name: &str| {
        let symbol = qualified(&raw, qualified_name);
        (symbol.kind, symbol.owner.as_deref())
    };

    assert_eq!(member("Dict::set_b"), (SymbolKind::Method, Some("Dict")));
    assert_eq!(member("Array::at"), (SymbolKind::Method, Some("Array")));
    assert_eq!(
        member("web::Node::remove"),
        (SymbolKind::Method, Some("web::Node"))
    );
    assert_eq!(member("is_nan"), (SymbolKind::Function, None));
}

#[test]
fn a_trait_impl_adds_the_trait_to_the_parents_of_the_type() {
    let raw = extract(SAMPLE);

    assert_eq!(named(&raw, "Service").parents, ["Greeter", "Iterator"]);
    assert_eq!(named(&raw, "State").parents.len(), 0);
}

#[test]
fn a_trait_member_is_a_method_of_the_trait_with_its_visibility() {
    let raw = extract(SAMPLE);
    let signature = qualified(&raw, "Greeter::greet");

    assert_eq!(
        (
            signature.kind,
            signature.owner.as_deref(),
            signature.exported
        ),
        (SymbolKind::Method, Some("Greeter"), true)
    );
}

#[test]
fn a_signature_is_the_declaration_without_its_body() {
    let raw = extract(SAMPLE);

    assert_eq!(
        qualified(&raw, "Service::new").signature.as_deref(),
        Some("pub fn new(name: String) -> Service")
    );
    assert_eq!(
        qualified(&raw, "Greeter::greet").signature.as_deref(),
        Some("fn greet(&self) -> String")
    );
    assert_eq!(named(&raw, "Service").signature, None);
}

#[test]
fn an_associated_type_is_an_alias_owned_by_its_type() {
    let raw = extract(SAMPLE);
    let item = qualified(&raw, "Service::Item");

    assert_eq!(
        (item.kind, item.owner.as_deref()),
        (SymbolKind::TypeAlias, Some("Service"))
    );
}

#[test]
fn a_trait_declared_associated_type_is_an_alias_of_the_trait() {
    let raw = extract(
        b"pub trait Stream {\n    type Item;\n    type Error: Clone;\n\
          \x20   fn next(&mut self) -> Option<Self::Item>;\n}\n",
    );
    let item = qualified(&raw, "Stream::Item");

    assert_eq!(
        qualified_names(&raw),
        ["Stream", "Stream::Item", "Stream::Error", "Stream::next"]
    );
    assert_eq!(
        (item.kind, item.owner.as_deref(), item.exported),
        (SymbolKind::TypeAlias, Some("Stream"), true)
    );
    assert_eq!(qualified(&raw, "Stream::Error").kind, SymbolKind::TypeAlias);
}

#[test]
fn visibility_follows_the_pub_modifier() {
    let raw = extract(SAMPLE);

    for name in ["Service", "new", "MAX", "Alias"] {
        assert!(named(&raw, name).exported, "{name}");
    }
    for name in ["State", "touch", "private_fn", "greet_all"] {
        assert!(!named(&raw, name).exported, "{name}");
    }
}

#[test]
fn a_visibility_restricted_to_its_own_module_is_not_exported() {
    let raw = extract(
        b"pub(self) fn own() {}\npub(in self) fn also_own() {}\npub(crate) fn shared() {}\n",
    );

    assert!(!named(&raw, "own").exported);
    assert!(!named(&raw, "also_own").exported);
    assert!(named(&raw, "shared").exported);
}

#[test]
fn a_macro_marked_macro_export_is_exported_and_keeps_its_bare_name() {
    let raw = extract(
        b"#[macro_export]\nmacro_rules! shared { () => {}; }\n\n\
          #[macro_export(local_inner_macros)]\n// a comment\nmacro_rules! inner { () => {}; }\n\n\
          macro_rules! own { () => {}; }\n\n\
          mod nested {\n    #[macro_export]\n    macro_rules! deep { () => {}; }\n}\n",
    );

    assert!(named(&raw, "shared").exported);
    assert!(named(&raw, "inner").exported);
    assert!(!named(&raw, "own").exported);
    assert!(qualified(&raw, "deep").exported);
}

#[test]
fn a_union_is_a_struct_with_fields() {
    let raw = extract(b"pub union U {\n    pub a: u32,\n    b: f32,\n}\n");

    assert_eq!(qualified_names(&raw), ["U", "U::a", "U::b"]);
    assert_eq!(named(&raw, "U").kind, SymbolKind::Struct);
    assert_eq!(qualified(&raw, "U::a").kind, SymbolKind::Field);
    assert!(qualified(&raw, "U::a").exported);
    assert!(!qualified(&raw, "U::b").exported);
}

#[test]
fn extern_crate_and_pub_use_extract_nothing() {
    let raw = extract(b"extern crate serde;\n\npub use crate::foo::Bar;\n\npub fn anchor() {}\n");

    assert_eq!(qualified_names(&raw), ["anchor"]);
    assert_eq!(raw.directives.len(), 0);
    assert_eq!(raw.re_exports.len(), 0);
}

#[test]
fn an_impl_of_a_wrapped_target_is_owned_by_its_nominal_type() {
    let raw = extract(
        b"impl Display for &str {\n    fn fmt(&self) -> u32 { Self::helper() }\n}\n\n\
          impl Pair for (u8, u8) {\n    fn swap(&self) {}\n}\n\n\
          impl Show for dyn Bar {\n    fn show(&self) {}\n}\n\n\
          impl Reader for *const u8 {\n    fn read(&self) -> u32 { 0 }\n}\n\n\
          pub struct Elem;\n\nimpl Marker for &[Elem] {}\n",
    );
    let helper = raw
        .calls
        .iter()
        .find(|call| call.callee == "helper")
        .expect("helper call");

    assert_eq!(named(&raw, "fmt").qualified_name, "str::fmt");
    assert_eq!(helper.receiver.as_deref(), Some("str"));
    assert_eq!(named(&raw, "swap").owner, None);
    assert_eq!(named(&raw, "show").owner, None);
    assert_eq!(named(&raw, "read").qualified_name, "u8::read");
    assert_eq!(named(&raw, "Elem").parents, ["Marker"]);
}

#[test]
fn a_trait_impl_whose_scoped_path_names_no_declaration_adds_no_parent() {
    let raw = extract(
        b"pub struct Foo;\nimpl Display for crate::Foo {}\nimpl Debug for other::Foo {}\nimpl Eq for Foo {}\n",
    );

    assert_eq!(named(&raw, "Foo").parents, ["Eq"]);
    assert_eq!(raw.symbols.len(), 1);
}

#[test]
fn a_trait_impl_goes_to_the_type_of_its_qualified_name_before_one_of_its_bare_name() {
    let raw = extract(
        b"mod a {\n    pub struct Foo;\n    pub struct Bar;\n}\npub struct Foo;\nimpl Eq for Foo {}\n\
          mod b {\n    use super::a::Bar;\n    impl Ord for Bar {}\n}\n",
    );

    assert_eq!(qualified(&raw, "Foo").parents, ["Eq"]);
    assert_eq!(qualified(&raw, "a::Foo").parents.len(), 0);
    assert_eq!(qualified(&raw, "a::Bar").parents, ["Ord"]);
}

#[test]
fn an_associated_const_is_owned_by_its_type_and_a_scoped_impl_target_finds_its_type() {
    let raw = extract(
        b"pub struct Limits;\nimpl Limits {\n    pub const MAX: u32 = 10;\n}\n\
          pub trait Tagged {\n    const TAG: u32;\n}\n\
          mod inner {\n    pub struct Widget;\n}\n\
          impl Tagged for inner::Widget {\n    const TAG: u32 = 1;\n}\n",
    );
    let max = qualified(&raw, "Limits::MAX");

    assert_eq!(
        (max.kind, max.owner.as_deref()),
        (SymbolKind::Constant, Some("Limits"))
    );
    assert!(qualified(&raw, "Tagged::TAG").exported);
    assert_eq!(named(&raw, "Widget").parents, ["Tagged"]);
}

#[test]
fn a_call_belongs_to_the_function_around_it_and_self_names_the_impl_type() {
    let raw = extract(SAMPLE);
    let helper = raw
        .calls
        .iter()
        .find(|call| call.callee == "helper")
        .expect("helper call");
    let greet = raw
        .calls
        .iter()
        .find(|call| call.callee == "greet")
        .expect("self.greet() call");

    assert_eq!(
        (helper.site_of, helper.receiver.as_deref()),
        (id_of(&raw, "Service::greet"), None)
    );
    assert_eq!(
        (greet.site_of, greet.receiver.as_deref()),
        (id_of(&raw, "Service::touch"), Some("Service"))
    );
}

#[test]
fn a_scoped_or_macro_call_names_its_last_segment() {
    let raw = extract(SAMPLE);
    let receiver_of = |callee: &str| {
        raw.calls
            .iter()
            .find(|call| call.callee == callee)
            .map(|call| call.receiver.clone())
    };

    assert_eq!(receiver_of("drop"), Some(Some("mem".to_owned())));
    assert_eq!(receiver_of("size_of"), Some(Some("mem".to_owned())));
    assert_eq!(receiver_of("info"), Some(None));
    assert_eq!(receiver_of("println"), Some(None));
}

#[test]
fn a_generic_path_or_self_path_names_its_type() {
    let raw = extract(SAMPLE);
    let receivers: Vec<Option<&str>> = raw
        .calls
        .iter()
        .filter(|call| call.callee == "new")
        .map(|call| call.receiver.as_deref())
        .collect();

    assert_eq!(receivers, [Some("Vec"), Some("Service"), Some("String")]);
}

#[test]
fn a_member_call_on_an_unnamed_expression_keeps_an_empty_receiver() {
    let raw = extract(b"fn f() { items[0].close(); self.pool.get(); make().run(); }\n");
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
fn a_member_call_on_a_lowercase_value_records_its_name_after_a_dot_and_on_an_item_or_other_its_name()
 {
    let raw = extract(
        b"fn f(socket: Socket, other: &Self) { socket.connect(); other.drain(); Fish.write(); SETTINGS.get(); }\n",
    );
    let calls: Vec<(&str, Option<&str>)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect();

    assert_eq!(
        calls,
        [
            ("connect", Some(".socket")),
            ("drain", Some("other")),
            ("write", Some("Fish")),
            ("get", Some("SETTINGS"))
        ]
    );
}

#[test]
fn a_method_call_on_a_parameter_records_the_type_the_parameter_is_declared_as() {
    let raw = extract(
        b"use kartta_lang::Repository;\nstruct Ranking;\nstruct Entry<'a>(&'a str);\n\
          impl Ranking {\n    fn same(&self, other: &Self) { other.offer(); }\n}\n\
          fn offer(ranking: &mut Ranking) { ranking.offer(); }\n\
          fn shapes(mut by: a::Order, plain: Ranking, entry: &Entry<'_>, boxed: Box<Ranking>, shared: Arc<Ranking>, \
          counted: Rc<Ranking>) {\n\
          \x20   by.sort();\n    plain.offer();\n    entry.kind();\n    boxed.offer();\n    shared.offer();\n\
          \x20   counted.offer();\n    assert!(plain.offer());\n}\n\
          fn traits(repository: &dyn Repository, read: impl Repository) { repository.read(); read.read(); }\n",
    );

    assert_eq!(
        calls_with_receivers(&raw),
        [
            ("offer", Some("Ranking")),
            ("offer", Some("Ranking")),
            ("sort", Some("Order")),
            ("offer", Some("Ranking")),
            ("kind", Some("Entry")),
            ("offer", Some("Ranking")),
            ("offer", Some("Ranking")),
            ("offer", Some("Ranking")),
            ("assert", None),
            ("offer", Some("Ranking")),
            ("read", Some("Repository")),
            ("read", Some("Repository")),
        ]
    );
}

#[test]
fn a_method_call_on_a_parameter_keeps_the_value_s_receiver_where_the_file_cannot_tell_its_type_is_the_tree_s()
 {
    let raw = extract(
        b"use std::fs::File;\nuse std::io;\nuse other::*;\nstruct Ranking;\n\
          fn f<T: Clone>(file: File, error: io::Error, names: Vec<u8>, generic: T, assoc: T::Item, glob: Globbed) {\n\
          \x20   file.sync_data();\n    error.kind();\n    names.len();\n    generic.clone();\n    assoc.next();\n\
          \x20   glob.run();\n}\n\
          fn g(ranking: Ranking, (left, right): (Ranking, Ranking), many: impl Ranked + Send) {\n\
          \x20   let ranking = Other::new();\n    ranking.offer();\n    left.offer();\n    many.rank();\n\
          \x20   [1].iter().for_each(|ranking| ranking.offer());\n\
          \x20   assert!(Some(1).map(|ranking| ranking.offer()));\n}\n",
    );

    assert_eq!(
        calls_with_receivers(&raw),
        [
            ("sync_data", Some(".file")),
            ("kind", Some(".error")),
            ("len", Some(".names")),
            ("clone", Some(".generic")),
            ("next", Some(".assoc")),
            ("run", Some("Globbed")),
            ("new", Some("Other")),
            ("offer", Some(".ranking")),
            ("offer", Some(".left")),
            ("rank", Some(".many")),
            ("for_each", Some("")),
            ("iter", Some("")),
            ("offer", Some(".ranking")),
            ("assert", None),
            ("Some", None),
            ("map", Some("")),
            ("offer", Some(".ranking")),
        ]
    );

    let standard_glob =
        extract(b"use std::collections::*;\nfn f(map: HashMap<u8, u8>) {\n    map.len();\n}\n");

    assert_eq!(
        calls_with_receivers(&standard_glob),
        [("len", Some(".map"))]
    );
}

#[test]
fn the_use_of_a_parameter_s_module_decides_its_type_before_a_namesake_or_a_function_body_s_use() {
    let raw = extract(
        b"use std::fs::File;\nuse parent::*;\nstruct Ranking;\nmod inner {\n    pub struct File;\n\
          \x20   use std::fs::File as Ranking;\n    fn g(ranking: Ranking) {\n        ranking.offer();\n    }\n}\n\
          fn f(file: File, rooted: ::std::fs::File, local: Local, ranking: Ranking) {\n\
          \x20   use std::fs::File as Local;\n    file.sync_data();\n    rooted.sync_data();\n    local.run();\n\
          \x20   ranking.offer();\n}\n",
    );

    assert_eq!(
        calls_with_receivers(&raw),
        [
            ("offer", Some(".ranking")),
            ("sync_data", Some(".file")),
            ("sync_data", Some(".rooted")),
            ("run", Some("Local")),
            ("offer", Some("Ranking")),
        ]
    );
}

#[test]
fn a_closure_opens_no_scope() {
    let raw = extract(
        b"fn outer() {\n    let f = |x: u32| helper(x);\n    f(1);\n}\n\nfn helper(_x: u32) {}\n",
    );
    let call = raw
        .calls
        .iter()
        .find(|call| call.callee == "helper")
        .expect("helper call");

    assert_eq!(call.site_of, id_of(&raw, "outer"));
}

#[test]
fn a_call_through_a_parameter_or_a_local_is_no_call() {
    let raw = extract(
        b"use other_crate::helper;
fn f(helper: fn()) { helper(); }
fn g() { let helper = || {}; helper(); }
fn h() { helper(); }
fn k(run: impl Fn()) { run(); }
fn m() { let run = || {}; run(); }
fn n(items: Vec<fn()>, hook: Option<fn()>) {
    let early = helper();
    let helper = early;
    items.iter().for_each(|visit| visit());
    for each in items { each(); }
    match hook { Some(call) => call(), None => run() }
    if let Some(go) = hook { go() } else { go() }
    fn nested() { helper(); }
}
",
    );
    let calls: Vec<(&str, SymbolId)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.site_of))
        .collect();

    assert_eq!(
        calls,
        [
            ("helper", id_of(&raw, "h")),
            ("helper", id_of(&raw, "n")),
            ("for_each", id_of(&raw, "n")),
            ("iter", id_of(&raw, "n")),
            ("run", id_of(&raw, "n")),
            ("go", id_of(&raw, "n")),
            ("helper", id_of(&raw, "nested")),
        ]
    );
}

fn calls_with_receivers(raw: &RawExtraction) -> Vec<(&str, Option<&str>)> {
    raw.calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect()
}

#[test]
fn a_call_in_a_macro_s_arguments_is_a_call_of_the_function_around_the_macro() {
    let raw = extract(
        b"struct C;
impl C {
    fn check(&self) {
        assert!(C.opens(helper()), \"{}\", self.name());
        assert_eq!(Self::new(), crate::m::f(), \"{}\", x.collect::<Vec<_>>());
    }
}
fn other() { debug_assert!(Vec::<u8>::new().is_empty() && !(Options::default().on())); }
",
    );

    assert_eq!(
        calls_with_receivers(&raw),
        [
            ("assert", None),
            ("opens", Some("C")),
            ("helper", None),
            ("name", Some("C")),
            ("assert_eq", None),
            ("new", Some("C")),
            ("f", Some("m")),
            ("collect", Some(".x")),
            ("debug_assert", None),
            ("new", Some("Vec")),
            ("is_empty", Some("")),
            ("default", Some("Options")),
            ("on", Some("")),
        ]
    );
    let check = id_of(&raw, "C::check");
    assert!(raw.calls[..8].iter().all(|call| call.site_of == check));
    assert!(
        raw.calls[8..]
            .iter()
            .all(|call| call.site_of == id_of(&raw, "other"))
    );
}

#[test]
fn a_macro_in_a_macro_s_arguments_is_a_call_of_the_macro_and_its_arguments_are_read() {
    let raw = extract(
        b"fn run() {\n    assert_eq!(vec![first()], format!(\"{}\", kartta_lang::named!(second())));\n}\n",
    );

    assert_eq!(
        calls_with_receivers(&raw),
        [
            ("assert_eq", None),
            ("vec", None),
            ("first", None),
            ("format", None),
            ("named", None),
            ("second", None),
        ]
    );
    let paths: Vec<(&str, &[String])> = raw
        .directives
        .iter()
        .map(|directive| (directive.spec.as_str(), directive.names.as_slice()))
        .collect();
    assert_eq!(paths, [("kartta_lang::named", &["named".to_owned()][..])]);
}

#[test]
fn a_call_in_a_macro_s_arguments_through_a_parameter_or_a_local_is_no_call() {
    let raw = extract(
        b"fn f(helper: fn() -> bool, items: Vec<fn() -> bool>) {
    let run = || true;
    assert!(helper() && run());
    assert!(items.iter().any(|visit| visit()));
    assert!(items.iter().all(move |(go)| go()), \"{}\", missing());
    assert!(items.iter().any(|seen: build, again| build() && again()));
    assert!(items.iter().any(|Some(found)| Some(found).is_some() && found()));
}
fn g(hook: Option<fn()>, pairs: Vec<(fn(), Point)>, code: fn()) {
    assert!(if let Some(f) = hook { f() } else { true });
    assert!({ for (step, Point { method: value }) in pairs { step(); method(); value(); } true });
    assert!(match code { n => n(), _ => other() });
    assert!(pairs.iter().any(|Point { label: kept }| label() && kept()));
    assert!({ let built = built(); built() });
    assert!(match code { n => { n() } m => m(), });
    assert!(if let Some(f) = hook { f() } else { f() });
    assert!(if let Some(check) = hook && check() { check() } else { false });
    assert!(match code { check if check() => true, _ => false });
}
",
    );
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(
        callees,
        [
            "assert", "assert", "iter", "any", "assert", "iter", "all", "missing", "assert",
            "iter", "any", "build", "assert", "iter", "any", "Some", "is_some", "assert", "assert",
            "method", "assert", "other", "assert", "iter", "any", "label", "assert", "built",
            "assert", "assert", "f", "assert", "assert",
        ]
    );
}

#[test]
fn a_rooted_path_or_a_keyword_in_a_macro_s_arguments_reads_as_one_outside_it() {
    let outside = extract(
        b"fn run(items: Vec<u8>) {
    ::root::f();
    ::g();
    for item in (items) { r#in(item); }
}
",
    );
    let inside = extract(
        b"fn run(items: Vec<u8>) {
    m!(::root::f(),
    ::g(),
    for item in (items) { r#in(item); });
}
",
    );
    let calls = |raw: &RawExtraction| -> Vec<(String, Option<String>, u32)> {
        raw.calls
            .iter()
            .filter(|call| call.callee != "m")
            .map(|call| (call.callee.clone(), call.receiver.clone(), call.line))
            .collect()
    };

    assert_eq!(calls(&inside), calls(&outside));
    assert_eq!(
        calls(&inside)
            .iter()
            .map(|(callee, ..)| callee.as_str())
            .collect::<Vec<_>>(),
        ["f", "g", "r#in"]
    );
    assert_eq!(inside.directives, outside.directives);
}

#[test]
fn a_call_in_an_object_or_a_static_a_macro_writes_is_read_as_one_outside_it() {
    let raw = extract(
        b"thread_local! {
    static SEEN: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}
fn answer(dir: &Path) {
    json!({ \"root\": dir.path(), \"files\": [listed(dir)] });
}
",
    );
    let calls: Vec<(&str, Option<&str>, bool)> = raw
        .calls
        .iter()
        .map(|call| {
            let from_file = call.site_of == FILE_SCOPE;
            (call.callee.as_str(), call.receiver.as_deref(), from_file)
        })
        .collect();

    assert_eq!(
        calls,
        [
            ("thread_local", None, true),
            ("new", Some("RefCell"), true),
            ("new", Some("Vec"), true),
            ("json", None, false),
            ("path", Some(".dir"), false),
            ("listed", None, false),
        ]
    );
}

#[test]
fn a_function_or_an_attribute_in_a_macro_s_arguments_holds_no_call() {
    let raw = extract(
        b"fn emit() {
    declare! {
        #[derive(Debug)]
        #![cfg(feature(\"x\"))]
        fn generated(x: u8) -> Option<u8> { helper(x) }
        fn sized() -> Foo<{ N }, { M }> { inner() }
        made();
    }
}
",
    );
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(callees, ["declare", "made"]);
}

#[test]
fn a_pattern_in_a_macro_s_arguments_names_constructors_and_holds_no_call() {
    let raw = extract(
        b"fn check(value: Option<Kind>) {
    assert!(matches!(value, Some(Kind::A(_)) if ok(value)));
    matches!(kind(), Kind::B(_) | Kind::C(_));
    println!(\"{}\", if let Some(Kind::A(x)) = value { shown(x) } else { 0 });
    format!(\"{}\", match value { Some(Kind::A(_)) => one(), _ => Some(1) });
    matches!(if ready() { value } else { fallback() }, Some(Kind::A(_)) if late());
    matches!(|x, y| make(x, y) || x(), Some(Kind::A(_)));
    assert!(match value { Some(Kind::A(x)) if guard(x) => taken(), _ => false });
}
",
    );
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(
        callees,
        [
            "assert", "matches", "ok", "matches", "kind", "println", "shown", "format", "one",
            "Some", "matches", "ready", "fallback", "late", "matches", "make", "assert", "guard",
            "taken",
        ]
    );
}

#[test]
fn a_matches_macro_the_file_defines_reads_its_arguments_as_any_macro_s() {
    let raw = extract(
        b"fn before() { matches!(value, Some(process())); }
macro_rules! matches { ($a:expr, $b:expr) => { $a == $b }; }
fn after() { matches!(value, Some(process())); }
",
    );
    let callees: Vec<(&str, SymbolId)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.site_of))
        .collect();
    let after = id_of(&raw, "after");

    assert_eq!(
        callees,
        [
            ("matches", id_of(&raw, "before")),
            ("matches", after),
            ("Some", after),
            ("process", after),
        ]
    );

    let raw = extract(
        b"fn local() {
    macro_rules! matches { ($a:expr, $b:expr) => { $a == $b }; }
    matches!(value, Some(process()));
}
fn past() { matches!(value, Some(process())); }
",
    );
    let callees: Vec<(&str, SymbolId)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.site_of))
        .collect();
    let local = id_of(&raw, "local");

    assert_eq!(
        callees,
        [
            ("Some", local),
            ("process", local),
            ("matches", id_of(&raw, "past")),
        ]
    );
}

#[test]
fn a_macro_whose_arguments_interpolate_is_a_template_of_no_call() {
    let raw = extract(
        b"fn emit() {
    quote! { let result = std::panic::catch_unwind(|| #name()); };
    quote! { #(#attrs),* checked(); };
    kept!(made());
    assert!(quote! { #name() } && helper());
}
",
    );
    let callees: Vec<&str> = raw.calls.iter().map(|call| call.callee.as_str()).collect();

    assert_eq!(
        callees,
        [
            "quote", "quote", "kept", "made", "assert", "quote", "helper"
        ]
    );
}

#[test]
fn a_call_in_a_macro_s_arguments_binds_an_item_its_block_declares() {
    let raw = extract(b"fn outer() {\n    fn inner() -> bool { true }\n    assert!(inner());\n}\n");
    let local: Vec<(&str, u32)> = raw
        .local_calls
        .iter()
        .map(|call| (raw.symbols[call.local.0 as usize].name.as_str(), call.line))
        .collect();

    assert_eq!(local, [("inner", 3)]);
    assert!(raw.calls.iter().all(|call| call.callee != "inner"));
}

#[test]
fn a_cfg_test_module_marks_everything_in_it() {
    let source = b"pub fn prod() {}\n\n#[cfg(test)]\nmod tests {\n    fn helper() -> u32 {\n        0\n    }\n\n\
                   #[test]\n    fn it_works() {\n        helper();\n    }\n}\n";

    assert!(!is_test(source, "prod"));
    for name in ["tests", "helper", "it_works"] {
        assert!(is_test(source, name), "{name}");
    }
}

#[test]
fn a_cfg_test_extern_block_marks_its_functions() {
    let source =
        b"extern \"C\" {\n    fn prod();\n}\n\n#[cfg(test)]\nextern \"C\" {\n    fn probe();\n}\n";

    assert!(!is_test(source, "prod"));
    assert!(is_test(source, "probe"));
}

#[test]
fn a_file_top_inner_cfg_test_marks_the_whole_file() {
    let source = b"#![cfg(test)]\n\nfn helper() {}\n\nfn another() {}\n";

    assert!(is_test(source, "helper"));
    assert!(is_test(source, "another"));
}

#[test]
fn an_inner_cfg_test_in_an_inline_module_marks_the_module_and_nothing_outside() {
    let source = b"pub fn prod() {}\n\nmod helpers {\n    #![cfg(test)]\n\n    fn within() {}\n}\n";

    assert!(is_test(source, "within"));
    assert!(is_test(source, "helpers"));
    assert!(!is_test(source, "prod"));
}

#[test]
fn a_cfg_gates_test_code_only_when_it_holds_only_under_test() {
    let source = br#"
#[cfg(all(test, feature = "extra"))]
mod gated { fn within() {} }
#[cfg(all(test, not(miri)))]
mod skips_under_miri { fn helper() {} }
#[cfg(not(test))]
fn prod_only() {}
#[cfg(feature = "fastest")]
fn fast_path() {}
#[cfg(any())]
fn never_compiled() {}
#[cfg(all())]
fn always_compiled() {}
#[cfg(any(test, feature = "extra"))]
mod maybe_prod { fn maybe() {} }
#[cfg(any(test))]
mod only_test { fn solely() {} }
#[cfg(not(not(test)))]
fn double_neg() {}
#[cfg(test)]
fn under_test() {}
"#;

    for name in [
        "gated",
        "within",
        "skips_under_miri",
        "helper",
        "solely",
        "double_neg",
        "under_test",
    ] {
        assert!(is_test(source, name), "{name}");
    }
    for name in [
        "prod_only",
        "fast_path",
        "never_compiled",
        "always_compiled",
        "maybe_prod",
        "maybe",
    ] {
        assert!(!is_test(source, name), "{name}");
    }
}

#[test]
fn a_repeated_cfg_atom_takes_one_truth_in_each_configuration() {
    let source = br#"
#[cfg(all(any(test, feature = "x"), not(feature = "x")))]
fn needs_test() {}
#[cfg(all(feature = "x", not(feature="x")))]
fn never_compiled() {}
#[cfg(any(feature = "x", not(feature = "x")))]
fn always_compiled() {}
"#;

    assert!(is_test(source, "needs_test"));
    assert!(!is_test(source, "never_compiled"));
    assert!(!is_test(source, "always_compiled"));
}

#[test]
fn a_cfg_of_many_atoms_written_once_still_gates_test_code() {
    let features: Vec<String> = ('a'..='p')
        .map(|name| format!("feature = \"{name}\""))
        .collect();
    let source = format!(
        "#[cfg(all(test, {all}))]\nfn needs_test() {{}}\n#[cfg(any(test, {all}))]\nfn maybe_prod() {{}}\n",
        all = features.join(", ")
    );

    assert!(is_test(source.as_bytes(), "needs_test"));
    assert!(!is_test(source.as_bytes(), "maybe_prod"));
}

#[test]
fn a_test_gated_field_or_variant_is_test_code() {
    let source =
        b"pub struct Config {\n    pub name: String,\n    #[cfg(test)]\n    pub probe: u32,\n}\n\
                   pub enum Mode {\n    Live,\n    #[cfg(test)]\n    Fake,\n}\n";

    assert!(is_test(source, "probe"));
    assert!(is_test(source, "Fake"));
    assert!(!is_test(source, "name"));
    assert!(!is_test(source, "Live"));
    assert!(!is_test(source, "Config"));
}

#[test]
fn a_test_or_bench_attribute_marks_its_function() {
    let source = b"#[test]\nfn standalone() {}\n\n#[tokio::test]\n// a comment between\nasync fn spawned() {}\n\n\
                   #[bench]\nfn measures(b: &mut Bencher) {}\n\npub fn ordinary() {}\n";

    for name in ["standalone", "spawned", "measures"] {
        assert!(is_test(source, name), "{name}");
    }
    let ordinary = named(&extract(source), "ordinary").flags;
    assert!(!ordinary.is_test() && !ordinary.is_entry());
}

#[test]
fn every_top_level_main_and_nothing_else_is_an_entry_candidate() {
    let raw = extract(
        b"#[cfg(unix)]\nfn main() {}\n\n#[cfg(windows)]\nfn main() {}\n\n\
          struct App;\nimpl App {\n    fn main(&self) {}\n}\nmod cli {\n    fn main() {}\n}\n",
    );
    let candidates: Vec<&str> = raw
        .entry_candidates
        .iter()
        .map(|id| raw.symbols[id.0 as usize].qualified_name.as_str())
        .collect();

    assert_eq!(candidates, ["main", "main"]);
    assert!(raw.symbols.iter().all(|symbol| !symbol.flags.is_entry()));
}

#[test]
fn a_bodiless_mod_leaves_a_directive_with_its_path_override_and_its_test_gate() {
    let raw = extract(
        b"#[path = \"platform/linux.rs\"]\n// platform-specific\nmod platform;\n\
          mod plain;\n#[cfg(test)]\nmod tests;\nmod inline {}\n",
    );
    let directives: Vec<(&str, &[String])> = raw
        .directives
        .iter()
        .map(|directive| (directive.spec.as_str(), directive.names.as_slice()))
        .collect();

    assert_eq!(
        directives,
        [
            ("platform", ["platform/linux.rs".to_owned()].as_slice()),
            ("plain", [].as_slice()),
            ("tests", [].as_slice())
        ]
    );
    assert_ne!(raw.directives[2].form, raw.directives[1].form);
    assert_eq!(raw.directives[0].form, raw.directives[1].form);
}

#[test]
fn a_bodiless_mod_in_an_inline_module_names_its_path_from_the_file() {
    let raw = extract(b"mod a {\n    mod b {\n        mod c;\n    }\n    mod d;\n}\nmod e;\n");
    let specs: Vec<&str> = raw
        .directives
        .iter()
        .map(|directive| directive.spec.as_str())
        .collect();

    assert_eq!(specs, ["a/b/c", "a/d", "e"]);
}

#[test]
fn a_path_override_of_a_prior_item_does_not_reach_the_next() {
    let raw = extract(b"#[path = \"custom/first.rs\"]\nmod first;\nmod second;\n");

    assert_eq!(raw.directives[0].names, ["custom/first.rs"]);
    assert_eq!(raw.directives[1].names.len(), 0);
}

#[test]
fn a_marker_in_the_leading_comments_marks_the_file_generated() {
    for source in [
        b"// @generated by a tool. DO NOT EDIT.\n\npub fn emitted() {}\n".as_slice(),
        b"// Code generated by protoc. DO NOT EDIT.\n\npub fn emitted() {}\n",
        b"//! Bindings for the wire protocol.\n//!\n//! @generated by a tool. DO NOT EDIT.\n\npub fn emitted() {}\n",
        b"/*\n * @generated by tool\n */\npub fn from_block() {}\n",
        b"/* @generated */\npub fn one_liner() {}\n",
        b"/* header */ // @generated\npub fn after_a_block() {}\n",
        b"/* a */ /* @generated */\npub fn second_block() {}\n",
        b"\xEF\xBB\xBF// @generated\npub fn after_a_byte_order_mark() {}\n",
    ] {
        assert!(is_generated(source), "{}", String::from_utf8_lossy(source));
    }
}

#[test]
fn a_mention_a_marker_after_code_or_a_plain_header_marks_nothing() {
    for source in [
        b"//! The lua extractor.\n//!\n//! Role flags: a `*.gen.lua` name or a `@generated` / `DO NOT EDIT` marker\n\
          //! in the file's leading comment run marks every symbol generated.\n\npub fn extract() {}\n"
            .as_slice(),
        b"//! a `@generated` / `DO NOT EDIT` marker\n\npub fn f() {}\n",
        b"/// Reads a banner.\n///\n/// ```\n/// let header = \"// Code generated by protoc. DO NOT EDIT.\";\n/// ```\n\
          pub struct Banner;\n",
        b"//! The scan.\n//!\n//! ```\n//! assert!(opens(b\"// @generated\\n\"));\n//! ```\n\npub fn f() {}\n",
        b"/* header */ pub fn real() {}\n// DO NOT EDIT\n",
        b"/* header\n*/ pub fn spans() {}\n// DO NOT EDIT\n",
        b"/* header */ pub const S: &str = \"DO NOT EDIT\";\npub fn coded() {}\n",
        b"pub fn handwritten() {}\n// @generated\n",
    ] {
        let raw = extract(source);

        assert_ne!(raw.symbols.len(), 0);
        assert!(
            raw.symbols.iter().all(|symbol| !symbol.flags.is_generated()),
            "{}",
            String::from_utf8_lossy(source)
        );
    }
}

#[test]
fn rust_claims_its_extension_in_any_case() {
    let rust = Rust::new();

    assert!(rust.claims("src/lib.rs"));
    assert!(rust.claims("src/Main.RS"));
    assert!(!rust.claims("Cargo.toml"));
    assert!(!rust.claims("rs"));
}

#[test]
fn the_fingerprint_names_the_crate_and_stays_put() {
    let first = Rust::new().fingerprint().to_vec();

    assert!(first.starts_with(b"kartta-lang-rust "));
    assert_eq!(Rust::new().fingerprint(), first.as_slice());
}

fn doc_line(source: &str, name: &str) -> u32 {
    let line = named(&extract(source.as_bytes()), name).line;
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Rust::new().doc_comments().first_line(&lines, line)
}

#[test]
fn the_attributes_between_a_doc_comment_and_its_item_belong_to_the_item() {
    let source = "use std::fmt;

/// Parses.
#[must_use]
#[inline]
pub fn parse() -> u32 {
    1
}

/* Renders. */
fn render() {}
";

    assert_eq!(doc_line(source, "parse"), 3);
    assert_eq!(doc_line(source, "render"), 10);
}

#[test]
fn a_macro_before_the_first_item_is_a_call_from_file_scope() {
    let raw = extract(b"lazy_static! {}\nfn run() { go(); }\n");
    let calls: Vec<(&str, bool)> = raw
        .calls
        .iter()
        .map(|call| (call.callee.as_str(), call.site_of == FILE_SCOPE))
        .collect();

    assert_eq!(calls, [("lazy_static", true), ("go", false)]);
}

fn uses(raw: &RawExtraction) -> Vec<(&str, &Receiver, SymbolId, u32)> {
    raw.uses
        .iter()
        .map(|used| {
            assert_eq!(used.access, Access::Read, "{used:?}");
            (used.name.as_str(), &used.receiver, used.site_of, used.line)
        })
        .collect()
}

#[test]
fn an_item_taken_as_a_value_is_a_use_of_it() {
    let raw = extract(
        b"fn helper(x: u8) -> u8 { x }
const LIMIT: u8 = 3;
struct Unit;
struct Service;
impl Service {
    fn make() -> Self { Service }
    fn run(&self) {
        items.map(helper);
        let table = [helper, other];
        let f = Self::make;
        let g = Service::make;
        let value = Unit;
        Handler { on_event: helper, limit: LIMIT }
    }
}
fn other() {}
const TOP: u8 = LIMIT;
fn more() -> u8 {
    let on = SETTINGS.enabled && STATE.lock().is_ok();
    for step in 0..LIMIT {}
    for entry in TABLE {}
    let ready = (PENDING.await, CHECKED?);
    loop { break LIMIT }
}
static SETTINGS: Settings = Settings { enabled: true };
static STATE: Mutex<u8> = Mutex::new(0);
static TABLE: [u8; 1] = [0];
static PENDING: Ready = Ready;
static CHECKED: Option<u8> = None;
",
    );

    let (make, run) = (id_of(&raw, "Service::make"), id_of(&raw, "Service::run"));
    let path = Receiver::Path("Service".to_owned());
    let more = id_of(&raw, "more");
    assert_eq!(
        uses(&raw),
        [
            ("Service", &Receiver::Bare, make, 6),
            ("helper", &Receiver::Bare, run, 8),
            ("helper", &Receiver::Bare, run, 9),
            ("other", &Receiver::Bare, run, 9),
            ("make", &Receiver::Own, run, 10),
            ("make", &path, run, 11),
            ("Unit", &Receiver::Bare, run, 12),
            ("helper", &Receiver::Bare, run, 13),
            ("LIMIT", &Receiver::Bare, run, 13),
            ("LIMIT", &Receiver::Bare, FILE_SCOPE, 17),
            ("SETTINGS", &Receiver::Bare, more, 19),
            ("STATE", &Receiver::Bare, more, 19),
            ("LIMIT", &Receiver::Bare, more, 20),
            ("TABLE", &Receiver::Bare, more, 21),
            ("PENDING", &Receiver::Bare, more, 22),
            ("CHECKED", &Receiver::Bare, more, 22),
            ("LIMIT", &Receiver::Bare, more, 23),
        ]
    );
}

#[test]
fn a_type_named_as_one_is_no_use_of_it() {
    let raw = extract(
        b"use crate::shapes::Shape;
struct Point { x: u32 }
struct Line { from: Point, to: Point }
trait Draw {}
impl Draw for Point {}
fn draw<T: Draw>(shape: &dyn Draw, line: Line, all: Vec<Point>) -> Point {
    let start: Point = line.from;
    Point { x: start.x }
}
",
    );

    let declared: Vec<&str> = raw
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    for name in ["Point", "Line", "Draw", "draw"] {
        assert!(declared.contains(&name), "{name} in {declared:?}");
    }
    assert!(raw.uses.is_empty(), "{:?}", raw.uses);
    assert_eq!(Rust::new().conventions().type_uses, TypeUses::Nowhere);
}

#[test]
fn a_local_a_parameter_or_a_name_the_file_declares_nothing_of_is_no_use() {
    let raw = extract(
        b"fn helper() {}
fn item() {}
fn found() {}
fn x() {}
fn run(helper: u8, data: Vec<u8>) {
    keep(helper);
}
fn other() {
    keep(helper);
    let helper = 1;
    keep(helper);
    let (a, b) = pair;
    for item in data { keep(item) }
    match value { Some(found) => keep(found), None => keep(None) }
    if let Some(x) = maybe { keep(x) }
    let closure = |helper| helper;
    keep(String::from);
    keep(missing);
}
",
    );

    assert_eq!(
        uses(&raw),
        [("helper", &Receiver::Bare, id_of(&raw, "other"), 9)]
    );
}

#[test]
fn a_binding_hides_its_name_from_its_end_to_its_block_s_and_a_read_binds_in_its_module() {
    let raw = extract(
        b"fn helper() {}
static ENABLED: bool = true;
mod other {
    pub fn tool() {}
}
struct Service;
struct Other;
impl Other {
    fn make() {}
}
fn run() {
    let helper = helper;
    {
        let tool = 1;
    }
    if ENABLED {}
    keep(tool);
    keep(Service::make);
}
fn after() {
    { let helper = 1; }
    keep(helper);
}
impl Service {
    fn build() { keep(Self::make); }
}
fn branch(maybe: Option<u8>) {
    if let Some(helper) = maybe { keep(helper) } else { keep(helper) }
}
static TABLE: [u8; 1] = [0];
fn item() {}
fn more(maybe: Option<u8>) {
    if let Some(helper) = maybe && helper > 0 {}
    while let Some(helper) = maybe { keep(helper) }
    keep(TABLE[0]);
    match ENABLED { _ => {} }
    for item in [item] { keep(item) }
    match maybe { item => keep(item) }
}
mod tools {
    pub fn tool() {}
    pub fn run() { keep(tool); let f = || tool; }
}
static mut COUNT: u8 = 0;
fn bump() { unsafe { COUNT += 1; } }
",
    );

    let (run, after, more) = (
        id_of(&raw, "run"),
        id_of(&raw, "after"),
        id_of(&raw, "more"),
    );
    let (tools_run, bump) = (id_of(&raw, "tools::run"), id_of(&raw, "bump"));
    assert_eq!(
        uses(&raw),
        [
            ("helper", &Receiver::Bare, run, 12),
            ("ENABLED", &Receiver::Bare, run, 16),
            ("helper", &Receiver::Bare, after, 22),
            ("helper", &Receiver::Bare, id_of(&raw, "branch"), 28),
            ("TABLE", &Receiver::Bare, more, 35),
            ("ENABLED", &Receiver::Bare, more, 36),
            ("item", &Receiver::Bare, more, 37),
            ("tool", &Receiver::Bare, tools_run, 42),
            ("tool", &Receiver::Bare, tools_run, 42),
            ("COUNT", &Receiver::Bare, bump, 45),
        ]
    );
}
