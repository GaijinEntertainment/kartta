//! How the inline modules, impls and blocks of one Rust file name what it declares: the chain that qualifies an item,
//! the owner an impl settles on, the receiver a scoped call names, and the calls that bind an item of a block.

use kartta_lang::{Language, RawExtraction, SymbolKind};
use kartta_lang_rust::Rust;

fn extract(source: &str) -> RawExtraction {
    Rust::new().extract(source.as_bytes())
}

fn identities(source: &str) -> Vec<(String, Option<String>)> {
    extract(source)
        .symbols
        .into_iter()
        .map(|symbol| (symbol.qualified_name, symbol.owner))
        .collect()
}

fn identity(qualified_name: &str, owner: Option<&str>) -> (String, Option<String>) {
    (qualified_name.to_owned(), owner.map(str::to_owned))
}

fn receivers(source: &str) -> Vec<(String, Option<String>)> {
    extract(source)
        .calls
        .into_iter()
        .map(|call| (call.callee, call.receiver))
        .collect()
}

fn receiver(callee: &str, receiver: &str) -> (String, Option<String>) {
    (callee.to_owned(), Some(receiver.to_owned()))
}

fn local_calls(source: &str) -> Vec<(String, SymbolKind, u32)> {
    let raw = extract(source);
    raw.local_calls
        .iter()
        .map(|call| {
            let local = &raw.symbols[call.local.0 as usize];
            let caller = &raw.symbols[call.site_of.0 as usize];
            (caller.qualified_name.clone(), local.kind, local.line)
        })
        .collect()
}

fn local(caller: &str, kind: SymbolKind, line: u32) -> (String, SymbolKind, u32) {
    (caller.to_owned(), kind, line)
}

#[test]
fn every_item_of_an_inline_module_takes_its_chain() {
    let got = identities(
        "mod a {\n    mod b {\n        pub struct S { pub field: u8 }\n        impl S { pub fn m() {} }\n\
         \x20       pub fn f() {}\n        pub const C: u8 = 0;\n        pub trait T { fn t(); }\n\
         \x20       pub enum E { V }\n        macro_rules! mac { () => {} }\n    }\n}\n",
    );

    assert_eq!(
        got,
        [
            identity("a", None),
            identity("a::b", None),
            identity("a::b::S", None),
            identity("a::b::S::field", Some("a::b::S")),
            identity("a::b::S::m", Some("a::b::S")),
            identity("a::b::f", None),
            identity("a::b::C", None),
            identity("a::b::T", None),
            identity("a::b::T::t", Some("a::b::T")),
            identity("a::b::E", None),
            identity("a::b::E::V", Some("a::b::E")),
            identity("mac", None),
        ]
    );
}

#[test]
fn an_impl_owner_is_the_declared_type_its_path_names_or_the_path_as_written() {
    let source = "mod a {\n    pub struct W;\n    mod inner { impl super::W { pub fn up() {} } }\n}\n\
                  impl a::W { pub fn scoped() {} }\nmod tests { impl Config { fn extra() { Self::other(); } } }\n";

    assert_eq!(
        identities(source),
        [
            identity("a", None),
            identity("a::W", None),
            identity("a::inner", None),
            identity("a::W::up", Some("a::W")),
            identity("a::W::scoped", Some("a::W")),
            identity("tests", None),
            identity("Config::extra", Some("Config")),
        ]
    );
    assert_eq!(receivers(source), [receiver("other", "Config")]);
}

#[test]
fn a_scoped_receiver_names_the_type_the_file_declares_or_keeps_its_last_segment() {
    let got = receivers(
        "mod a {\n    pub struct W;\n    pub fn go() { W::new(); self::W::new(); }\n\
         \x20   mod inner { fn up() { super::W::new(); } }\n}\nfn root() {\n    a::W::new();\n\
         \x20   kartta_graph::Graph::build();\n    crate::a::W::new();\n    ::a::W::new();\n    Vec::<u8>::new();\n\
         \x20   crate::helpers::Foo::<u8>::new();\n    a::W::<u8>::new();\n    crate::helper();\n}\n",
    );

    assert_eq!(
        got,
        [
            receiver("new", "a::W"),
            receiver("new", "a::W"),
            receiver("new", "a::W"),
            receiver("new", "a::W"),
            receiver("build", "Graph"),
            receiver("new", "W"),
            receiver("new", "W"),
            receiver("new", "Vec"),
            receiver("new", "Foo"),
            receiver("new", "a::W"),
            ("helper".to_owned(), None),
        ]
    );
}

#[test]
fn a_super_path_past_the_files_root_keeps_its_last_segment() {
    let got = receivers(
        "mod X { pub struct W; }\nmod a { fn g() { super::X::W::new(); super::super::X::W::new(); } }\n",
    );

    assert_eq!(got, [receiver("new", "X::W"), receiver("new", "W")]);
}

#[test]
fn a_trait_impl_in_a_module_adds_the_trait_to_the_type_of_that_module() {
    let raw = extract(
        "mod a {\n    pub trait Api { fn run(&self); }\n    pub struct Service;\n\
         \x20   impl Api for Service { fn run(&self) {} }\n}\nfn consume(s: a::Service) { a::Service::run(&s); }\n",
    );
    let service = raw
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "a::Service")
        .expect("a::Service");

    assert_eq!(service.parents, ["Api"]);
}

#[test]
fn an_impl_of_an_undeclared_type_renames_only_its_own_members() {
    let source = "impl Job { pub fn label(&self) { self.touch(); } }\n\
                  mod api { impl super::Job { pub fn touch(&self) { Self::label(self); } } }\n\
                  fn make() { Job::new(); }\n";

    assert_eq!(
        identities(source),
        [
            identity("Job::label", Some("Job")),
            identity("api", None),
            identity("super::Job::touch", Some("super::Job")),
            identity("make", None),
        ]
    );
    assert_eq!(
        receivers(source),
        [
            receiver("touch", "Job"),
            receiver("label", "super::Job"),
            receiver("new", "Job")
        ]
    );
}

#[test]
fn two_impls_that_resolve_to_one_name_keep_their_own_written_paths() {
    let got = identities(
        "mod a { impl super::W { pub fn x() {} } }\nmod b { pub mod c { impl super::super::W { pub fn y() {} } } }\n",
    );

    assert_eq!(
        got,
        [
            identity("a", None),
            identity("super::W::x", Some("super::W")),
            identity("b", None),
            identity("b::c", None),
            identity("super::super::W::y", Some("super::super::W")),
        ]
    );
}

#[test]
fn an_impl_whose_path_leaves_the_module_tree_keeps_its_written_path() {
    let got = identities(
        "pub struct W;\nmod a {\n    impl crate::W { pub fn x() {} }\n    impl super::super::W { pub fn y() {} }\n}\n",
    );

    assert_eq!(
        got,
        [
            identity("W", None),
            identity("a", None),
            identity("crate::W::x", Some("crate::W")),
            identity("super::super::W::y", Some("super::super::W")),
        ]
    );
}

#[test]
fn an_item_that_a_method_body_declares_is_no_member_of_its_type() {
    let raw = extract(
        "pub struct S;\nimpl S {\n    fn m(&self) {\n        fn helper() {}\n        helper();\n    }\n}\n\
         pub trait Api {\n    fn d() {\n        fn k() {}\n    }\n}\n",
    );
    let got: Vec<(&str, Option<&str>, bool)> = raw
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.owner.as_deref(),
                symbol.exported,
            )
        })
        .collect();

    assert_eq!(
        got,
        [
            ("S", None, true),
            ("S::m", Some("S"), false),
            ("S::helper", None, false),
            ("Api", None, true),
            ("Api::d", Some("Api"), true),
            ("Api::k", None, false),
        ]
    );
    assert_eq!(
        identities("pub struct W;\nimpl W { fn f() { mod m { pub fn g() {} } } }\n")[3],
        identity("W::m::g", None)
    );
}

#[test]
fn an_item_of_a_module_inside_a_trait_method_body_keeps_its_own_visibility() {
    let raw = extract("pub trait Api { fn d() { mod m { pub fn g() {} fn h() {} } } }\n");
    let exported: Vec<(&str, bool)> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name.starts_with("Api::m::") || symbol.name == "d")
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.exported))
        .collect();

    assert_eq!(
        exported,
        [("Api::d", true), ("Api::m::g", true), ("Api::m::h", false)]
    );
}

#[test]
fn a_symbol_under_a_settled_impl_settles_with_it() {
    let got =
        identities("mod a { impl W { fn f() { struct L { x: u8 } mod m { pub fn g() {} } } } }\n");
    let inner = identities("mod a { impl W { fn f() { mod m { impl T { fn g() {} } } } } }\n");

    assert_eq!(
        got,
        [
            identity("a", None),
            identity("W::f", Some("W")),
            identity("W::L", None),
            identity("W::L::x", Some("W::L")),
            identity("W::m", None),
            identity("W::m::g", None),
        ]
    );
    assert_eq!(inner.last(), Some(&identity("T::g", Some("T"))));
}

#[test]
fn a_trait_pair_or_a_receiver_under_a_settled_impl_settles_with_it() {
    let parents_source = "pub struct T;\nmod a {\n    impl W {\n        fn f() {\n            mod m {\n\
                          \x20               pub struct T;\n                impl Trait for T { fn h(&self) {} }\n\
                          \x20           }\n        }\n    }\n}\n";
    let raw = extract(parents_source);
    let parents: Vec<(&str, &[String])> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "T")
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.parents.as_slice()))
        .collect();
    let receivers_source = "mod a { impl W { fn f() { mod m {\n\
                            pub struct T;\n\
                            impl T {\n\
                            \x20   pub fn h(&self) {}\n\
                            \x20   pub fn g(&self) { Self::h(self); self.h(); T::new(); }\n\
                            }\n\
                            } } } }\n";

    assert_eq!(
        parents,
        [
            ("T", [].as_slice()),
            ("W::m::T", ["Trait".to_owned()].as_slice())
        ]
    );
    assert_eq!(
        receivers(receivers_source),
        [
            receiver("h", "W::m::T"),
            receiver("h", "W::m::T"),
            receiver("new", "W::m::T")
        ]
    );
}

#[test]
fn a_macro_named_like_the_chain_of_its_impl_keeps_its_bare_name() {
    let got = identities("mod a { impl super::W { fn f() { macro_rules! W { () => {} } } } }\n");

    assert_eq!(
        got,
        [
            identity("a", None),
            identity("super::W::f", Some("super::W")),
            identity("W", None),
        ]
    );
}

#[test]
fn a_self_receiver_names_its_impl_and_another_path_keeps_its_own_rule() {
    let own = "mod a { pub struct W; impl W { pub fn create() {} pub fn go() { Self::create(); } } }\n\
               mod b { pub struct W; impl W { pub fn create() {} } }\n";
    let spelled = "impl Job { pub fn label(&self) {} }\n\
                   mod api {\n\
                   \x20   impl super::Job { pub fn touch(&self) { Job::label(); Self::label(self); self.label(); } }\n\
                   }\n";

    assert_eq!(receivers(own), [receiver("create", "a::W")]);
    assert_eq!(
        receivers(spelled),
        [
            receiver("label", "Job"),
            receiver("label", "super::Job"),
            receiver("label", "super::Job")
        ]
    );
}

#[test]
fn an_impl_of_a_declared_enum_or_alias_keeps_its_module_chain() {
    let got = identities(
        "mod a {\n    pub enum E { V }\n    pub struct S;\n    pub type A = S;\n    mod inner {\n\
         \x20       impl super::E { pub fn up() {} }\n        impl super::A { pub fn via() {} }\n    }\n}\n",
    );
    let members: Vec<&(String, Option<String>)> = got
        .iter()
        .filter(|(name, _)| name.ends_with("::up") || name.ends_with("::via"))
        .collect();

    assert_eq!(
        members,
        [
            &identity("a::E::up", Some("a::E")),
            &identity("a::A::via", Some("a::A"))
        ]
    );
}

#[test]
fn a_receiver_path_that_names_a_declared_module_trait_or_alias_takes_its_chain() {
    let got = receivers(
        "mod a {\n    pub struct S;\n    pub type A = S;\n    pub trait Make { fn make(); }\n\
         \x20   pub mod b { pub fn f() {} }\n    pub fn go() { b::f(); Make::make(); A::new(); }\n}\n",
    );

    assert_eq!(
        got,
        [
            receiver("f", "a::b"),
            receiver("make", "a::Make"),
            receiver("new", "a::A")
        ]
    );
}

#[test]
fn a_self_path_call_records_no_receiver() {
    let nested = "mod a {\n    pub fn helper() {}\n    pub struct W;\n\
                  \x20   impl W { pub fn helper() {} pub fn g() { self::helper(); } }\n}\n";
    let root = "fn helper() {}\nstruct W;\nimpl W { fn helper() {} fn g() { self::helper(); } }\n";

    for source in [nested, root] {
        assert_eq!(receivers(source), [("helper".to_owned(), None)]);
    }
}

#[test]
fn a_plain_call_binds_only_a_callable_item_of_a_block_in_its_own_module() {
    let got = local_calls(
        "fn helper() {}\nfn nested() {}\nfn f() {}\nfn outer() {\n    helper::<u8>();\n    fn helper<T>() {}\n\
         \x20   { fn nested() {} }\n    nested();\n    mod f {}\n    f();\n\
         \x20   mod inner {\n        fn run() { helper(); }\n\
         \x20   }\n    fn pick() {}\n    { fn pick() {} pick(); }\n    struct Pair(u8);\n    const RUN: fn() = pick;\n\
         \x20   static GO: fn() = pick;\n    Pair(1);\n    RUN();\n    GO();\n    { mod tools { pub fn trim() {} } }\n\
         \x20   mod tools { pub fn trim() {} }\n    tools::trim();\n    self::pick();\n    struct Q;\n\
         \x20   impl Q { const ONLY: fn() = pick; fn only_q() { ONLY(); } }\n    only_q();\n    mod m {\n\
         \x20       fn g() { fn h() {} }\n        pub fn h() {}\n    }\n    m::h();\n    struct Shape { x: u8 }\n\
         \x20   Shape();\n    mod own { fn g() {} fn run() { g(); } }\n}\n",
    );

    assert_eq!(
        got,
        [
            local("outer", SymbolKind::Function, 6),
            local("outer", SymbolKind::Function, 15),
            local("outer", SymbolKind::Struct, 16),
            local("outer", SymbolKind::Constant, 17),
            local("outer", SymbolKind::Variable, 18),
            local("outer", SymbolKind::Function, 23),
            local("outer", SymbolKind::Function, 31),
            local("own::run", SymbolKind::Function, 36),
        ]
    );
}

#[test]
fn a_path_call_binds_the_item_that_its_module_path_names() {
    let got = local_calls(
        "fn outer() {\n    mod p {\n        pub fn up() {}\n        pub mod q {\n            pub fn deep() {}\n\
         \x20           fn run() { super::up(); self::deep(); }\n        }\n    }\n\
         \x20   p::q::deep();\n    p::up();\n}\n",
    );

    assert_eq!(
        got,
        [
            local("p::q::run", SymbolKind::Function, 3),
            local("p::q::run", SymbolKind::Function, 5),
            local("outer", SymbolKind::Function, 5),
            local("outer", SymbolKind::Function, 3),
        ]
    );
}

#[test]
fn a_use_or_a_let_shadows_a_block_item() {
    let got = local_calls(
        "fn later() {}\nfn outer() {\n    fn later() {}\n    fn pick() {}\n    fn swap() {}\n    fn other() {}\n\
         \x20   fn take() {}\n    fn keep() {}\n    fn mem() {}\n    {\n        pick(0);\n\
         \x20       use std::convert::identity as pick;\n        pick(1);\n        use std::mem::swap;\n\
         \x20       swap(&mut 1, &mut 2);\n        use std::mem::{replace, take};\n        take(&mut 0);\n\
         \x20       use std::mem::*;\n        mem();\n    }\n    later();\n    let later = { later(); || {} };\n\
         \x20   later();\n    fn nested() { later(); }\n    let (other, _) = (|| {}, 0);\n    other();\n\
         \x20   let (mut take, ref keep) = (|| {}, || {});\n    take();\n    keep();\n}\n",
    );

    assert_eq!(
        got,
        [
            local("outer", SymbolKind::Function, 9),
            local("outer", SymbolKind::Function, 3),
            local("outer", SymbolKind::Function, 3),
            local("nested", SymbolKind::Function, 3),
        ]
    );
}

#[test]
fn self_in_a_use_list_shadows_a_block_item_of_its_module_s_name() {
    let got = local_calls(
        "fn outer() {\n    fn socket() {}\n    {\n        use nix::socket::{self};\n        socket();\n    }\n\
         \x20   socket();\n}\n",
    );

    assert_eq!(got, [local("outer", SymbolKind::Function, 2)]);
}

#[test]
fn a_struct_or_tuple_struct_pattern_shadows_the_names_it_binds() {
    let got = local_calls(
        "struct S { helper: fn(), other: W }\nstruct W(fn());\nfn outer(value: S) {\n    fn helper() {}\n\
         \x20   fn keep() {}\n    fn bound() {}\n    let S { helper, other: W(keep) } = value;\n    helper();\n\
         \x20   keep();\n    bound();\n}\n",
    );

    assert_eq!(got, [local("outer", SymbolKind::Function, 6)]);
}

#[test]
fn a_parameter_shadows_a_block_item_in_its_own_function_body() {
    let got = local_calls(
        "fn outer() {\n    fn callback() {}\n    fn invoke(callback: fn(), (mut other, _): (fn(), u8)) {\n\
         \x20       callback();\n        other();\n    }\n    fn other() {}\n    callback();\n}\n",
    );

    assert_eq!(got, [local("outer", SymbolKind::Function, 2)]);
}

#[test]
fn a_macro_call_binds_only_a_macro_of_a_block_after_its_definition() {
    let got = local_calls(
        "macro_rules! tool { () => {} }\nfn helper() {}\nfn outer() {\n    tool!();\n\
         \x20   macro_rules! tool { () => {} }\n\
         \x20   tool!();\n    fn tool() {}\n    helper();\n    macro_rules! helper { () => {} }\n\
         \x20   mod inner { fn run() { tool!(); } }\n}\n",
    );

    assert_eq!(
        got,
        [
            local("outer", SymbolKind::Macro, 5),
            local("inner::run", SymbolKind::Macro, 5)
        ]
    );
}

#[test]
fn an_item_in_an_extern_block_is_an_item_of_the_block_around_it() {
    let got = local_calls(
        "pub struct S;\nimpl S {\n    fn m(&self) {\n        helper();\n        fn helper() {}\n\
         \x20       unsafe extern \"C\" {\n            fn relief();\n        }\n\
         \x20       unsafe { relief() };\n    }\n}\n\
         fn helper() {}\nfn go() { helper(); }\n",
    );

    assert_eq!(
        got,
        [
            local("S::m", SymbolKind::Function, 5),
            local("S::m", SymbolKind::Function, 7)
        ]
    );
}

#[test]
fn a_symbol_a_body_declares_is_local() {
    let raw = extract("fn outer() {\n    fn inner() {}\n    struct Local;\n}\nfn top() {}\n");
    let locals: Vec<&str> = raw
        .local_symbols
        .iter()
        .map(|id| raw.symbols[id.0 as usize].qualified_name.as_str())
        .collect();

    assert_eq!(locals, ["inner", "Local"]);
}
