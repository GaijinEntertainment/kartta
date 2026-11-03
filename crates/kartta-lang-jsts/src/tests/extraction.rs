use std::time::{Duration, Instant};

use kartta_lang::tree::{child_nodes, named_child_nodes, parse, preorder};
use kartta_lang::{Extraction, Language, OwnMembers, Receiver, SymbolKind};
use tree_sitter::Node;

use super::{
    LANGUAGES, SAMPLE_TS, attributed, caller_of, extracted, imports, language_of, names, symbol,
};
use crate::{JavaScript, Tsx, TypeScript};

const SAMPLE_VALUES: &str = r#"
export const fetchUser = async (id) => {
    return request(id);
};

const Card = memo((props) => {
    track(props);
    return render(props);
});

export const handlers = {
    onClick: () => {
        fire();
    },
    label: "static",
};

const first = () => { fa(); }, second = () => { fb(); };

let counter = 0;

export default () => {
    boot();
};
"#;

fn values() -> Extraction {
    extracted("src/values.ts", SAMPLE_VALUES)
}

fn calls(extraction: &Extraction) -> Vec<(&str, Option<&str>)> {
    (extraction.calls.iter())
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect()
}

fn calls_that_may_bind(extraction: &Extraction) -> Vec<&str> {
    (extraction.calls.iter())
        .filter(|call| !call.binds_apart)
        .map(|call| call.callee.as_str())
        .collect()
}

#[test]
fn each_language_names_itself_and_shares_the_jsts_family() {
    let identities = LANGUAGES.map(|language| (language.name(), language.family()));

    assert_eq!(
        identities,
        [
            ("typescript", "jsts"),
            ("tsx", "jsts"),
            ("javascript", "jsts")
        ]
    );
}

#[test]
fn each_language_claims_its_extensions_in_any_case() {
    let claimed: Vec<(&str, &str)> = [
        "a.ts", "a.mts", "a.cts", "a.d.ts", "A.TS", "a.tsx", "a.TSX", "a.js", "a.jsx", "a.mjs",
        "a.cjs", "a.JS",
    ]
    .into_iter()
    .map(|path| (path, language_of(path).name()))
    .collect();
    let unclaimed = ["a.json", "a.ts.map", "ts", "dir.ts/readme"];

    assert_eq!(
        claimed,
        [
            ("a.ts", "typescript"),
            ("a.mts", "typescript"),
            ("a.cts", "typescript"),
            ("a.d.ts", "typescript"),
            ("A.TS", "typescript"),
            ("a.tsx", "tsx"),
            ("a.TSX", "tsx"),
            ("a.js", "javascript"),
            ("a.jsx", "javascript"),
            ("a.mjs", "javascript"),
            ("a.cjs", "javascript"),
            ("a.JS", "javascript"),
        ]
    );
    for path in unclaimed {
        assert!(
            !TypeScript::new().claims(path)
                && !Tsx::new().claims(path)
                && !JavaScript::new().claims(path),
            "{path}"
        );
    }
}

#[test]
fn a_bare_call_reaches_enclosing_namespaces_but_not_its_own_type() {
    let conventions = TypeScript::new().conventions();

    assert_eq!(conventions.own_members, OwnMembers::Unreached);
    assert_eq!(conventions.owner_chain_separator, Some("."));
    assert!(conventions.bare_call_reaches_enclosing_owners);
    assert!(conventions.values_are_call_targets);
    assert_eq!(conventions.scope_separator, None);
    assert!(!conventions.directory_packages);
    assert_eq!(Tsx::new().conventions(), conventions);
    assert_eq!(JavaScript::new().conventions(), conventions);
}

#[test]
fn the_runtime_methods_ascend_without_a_repeat_and_name_a_string_s_and_the_dom_s() {
    let methods = TypeScript::new().conventions().runtime_methods;

    assert!(methods.is_sorted_by(|earlier, later| earlier < later));
    for method in ["trim", "split", "defineProperty", "addEventListener"] {
        assert!(methods.binary_search(&method).is_ok(), "{method}");
    }
}

#[test]
fn each_language_has_a_fingerprint_of_its_own_that_stays_put() {
    let [typescript, tsx, javascript] = LANGUAGES.map(Language::fingerprint);

    assert_ne!(typescript, tsx);
    assert_ne!(typescript, javascript);
    assert_ne!(tsx, javascript);
    assert_eq!(LANGUAGES[0].fingerprint(), typescript);
}

#[test]
fn deep_jsx_member_tags_keep_an_empty_receiver() {
    let extraction = extracted(
        "src/deep.tsx",
        "export const App = () => <UI.Item.Sub prop={1} />;\n\nexport const Flat = () => <UI.Item />;\n",
    );

    assert_eq!(
        calls(&extraction),
        [("Sub", Some("")), ("Item", Some("UI"))]
    );
}

#[test]
fn object_literal_shorthand_methods_are_local_functions() {
    let extraction = extracted(
        "src/obj.ts",
        "const obj = {\n    doThing() { inner(); }\n};\n",
    );

    let do_thing = symbol(&extraction, "doThing");
    assert_eq!(
        (do_thing.kind, do_thing.owner.as_deref(), do_thing.exported),
        (SymbolKind::Function, None, false)
    );
    assert_eq!(caller_of(&extraction, "inner").name, "doThing");
}

#[test]
fn type_only_imports_ride_the_include_channels() {
    let extraction = extracted(
        "src/t.ts",
        r#"import type { T } from "./types";
export type { U } from "./more";

export function anchor(): void {}
"#,
    );

    let re_exports: Vec<(&str, &str)> = (extraction.re_exports.iter())
        .map(|re_export| {
            (
                re_export.local_name.as_str(),
                re_export.source_path.as_str(),
            )
        })
        .collect();
    assert_eq!(imports(&extraction), ["./types", "./more"]);
    assert_eq!(re_exports, [("U", "./more")]);
}

#[test]
fn value_bound_functions_fill_signatures() {
    let extraction = extracted(
        "src/sig.ts",
        r"
export const f = (a: number): number => a;

export const Card = memo(() => null);

const handlers = { onClick: (e: Event) => e };

class C {
    handler = (x: number) => x;
    data = 1;
}

export default (n: number) => n;
",
    );
    let signature = |name: &str| symbol(&extraction, name).signature.as_deref();

    assert_eq!(signature("f"), Some("f = (a: number): number =>"));
    assert_eq!(signature("Card"), Some("Card = memo(() =>"));
    assert_eq!(signature("onClick"), Some("onClick: (e: Event) =>"));
    assert_eq!(signature("handler"), Some("handler = (x: number) =>"));
    assert_eq!(signature("default"), Some("export default (n: number) =>"));
    assert_eq!(signature("data"), None);
}

#[test]
fn a_signature_leaves_out_its_comments() {
    let extraction = extracted(
        "src/sig.ts",
        "export const f = /* the handler */ (a: number) => a;\n",
    );

    assert_eq!(
        symbol(&extraction, "f").signature.as_deref(),
        Some("f = (a: number) =>")
    );
}

#[test]
fn ts_namespaces_emit_and_qualify_members() {
    let extraction = extracted(
        "src/ns.ts",
        r#"
export namespace NS {
    export function inside() { work(); }
}

namespace A {
    namespace B {
        export function deep() {}
    }
}

declare module "ext" {
    export function ambient() {}
}
"#,
    );

    let qualified: Vec<(&str, SymbolKind)> = (extraction.symbols.iter())
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        qualified,
        [
            ("NS", SymbolKind::Namespace),
            ("NS.inside", SymbolKind::Function),
            ("A", SymbolKind::Namespace),
            ("A.B", SymbolKind::Namespace),
            ("A.B.deep", SymbolKind::Function),
            ("ext", SymbolKind::Namespace),
            ("ext.ambient", SymbolKind::Function),
        ]
    );
    let inside = symbol(&extraction, "inside");
    assert_eq!(
        (inside.owner.as_deref(), inside.exported),
        (Some("NS"), true)
    );
}

#[test]
fn js_class_fields_extract_via_the_property_field() {
    let extraction = extracted(
        "src/store.js",
        "class Store {\n    items = [];\n    #secret = 1;\n    refresh = () => { reload(); };\n}\n",
    );

    let qualified: Vec<(&str, SymbolKind)> = (extraction.symbols.iter())
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        qualified,
        [
            ("Store", SymbolKind::Class),
            ("Store.items", SymbolKind::Field),
            ("Store.#secret", SymbolKind::Field),
            ("Store.refresh", SymbolKind::Method),
        ]
    );
    assert!(!symbol(&extraction, "#secret").exported);
    assert_eq!(caller_of(&extraction, "reload").name, "refresh");
}

#[test]
fn a_specifier_keeps_a_quote_of_the_other_kind() {
    let extraction = extracted("src/app.ts", "import x from \"./foo'\";\n");

    assert_eq!(imports(&extraction), ["./foo'"]);
}

#[test]
fn an_empty_sourced_export_clause_re_exports_nothing() {
    let extraction = extracted(
        "src/index.ts",
        "export {} from \"./side\";\nexport * from \"./all\";\n",
    );

    let re_exports: Vec<(&str, &str)> = (extraction.re_exports.iter())
        .map(|re_export| {
            (
                re_export.local_name.as_str(),
                re_export.source_path.as_str(),
            )
        })
        .collect();
    assert_eq!(re_exports, [("*", "./all")]);
}

#[test]
fn an_export_clause_without_a_source_exports_the_declarations_it_names() {
    let source = "const helper = () => 1;\nfunction hidden() {}\nexport { helper };\n";

    let extraction = extracted("src/util.ts", source);

    assert!(symbol(&extraction, "helper").exported);
    assert!(!symbol(&extraction, "hidden").exported);
}

#[test]
fn a_private_method_is_not_visible_outside_its_module() {
    let source =
        "export class Store {\n    private load() {}\n    #reset() {}\n    refresh() {}\n}\n";

    let extraction = extracted("src/store.ts", source);

    assert!(!symbol(&extraction, "load").exported);
    assert!(!symbol(&extraction, "#reset").exported);
    assert!(symbol(&extraction, "refresh").exported);
}

#[test]
fn extracts_symbols_with_kinds_in_preorder() {
    let extraction = extracted("src/app.ts", SAMPLE_TS);

    let kinds: Vec<(&str, SymbolKind)> = (extraction.symbols.iter())
        .map(|symbol| (symbol.name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Shape", SymbolKind::Interface),
            ("area", SymbolKind::Method),
            ("Alias", SymbolKind::TypeAlias),
            ("Color", SymbolKind::Enum),
            ("LIMIT", SymbolKind::Constant),
            ("Circle", SymbolKind::Class),
            ("area", SymbolKind::Method),
            ("render", SymbolKind::Function),
        ]
    );
    assert!(extraction.symbols.iter().all(|symbol| symbol.exported));
}

#[test]
fn interface_members_are_methods_owned_by_the_interface() {
    let extraction = extracted("src/app.ts", SAMPLE_TS);

    let member = (extraction.symbols.iter())
        .find(|symbol| symbol.qualified_name == "Shape.area")
        .expect("Shape.area");
    assert_eq!(
        (
            member.kind,
            member.owner.as_deref(),
            member.signature.as_deref()
        ),
        (SymbolKind::Method, Some("Shape"), Some("area(): number"))
    );
}

#[test]
fn the_members_of_an_object_type_alias_are_owned_by_the_alias() {
    let extraction = extracted(
        "src/gsea.ts",
        "export type GSEA = {\n  open(page?: string): void;\n  config: Config;\n  onClose: () => void;\n};\n\
         namespace Auth { export type Session = { logout(): Promise<void> }; }\n",
    );

    let owned: Vec<(&str, Option<&str>, SymbolKind)> = (extraction.symbols.iter())
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.owner.as_deref(),
                symbol.kind,
            )
        })
        .collect();
    assert_eq!(
        owned,
        [
            ("GSEA", None, SymbolKind::TypeAlias),
            ("GSEA.open", Some("GSEA"), SymbolKind::Method),
            ("GSEA.config", Some("GSEA"), SymbolKind::Field),
            ("GSEA.onClose", Some("GSEA"), SymbolKind::Method),
            ("Auth", None, SymbolKind::Namespace),
            ("Auth.Session", Some("Auth"), SymbolKind::TypeAlias),
            (
                "Auth.Session.logout",
                Some("Auth.Session"),
                SymbolKind::Method
            ),
        ]
    );
}

#[test]
fn arrow_consts_are_functions_that_own_their_calls() {
    let extraction = values();

    let fetch_user = symbol(&extraction, "fetchUser");
    assert_eq!(
        (fetch_user.kind, fetch_user.exported),
        (SymbolKind::Function, true)
    );
    assert_eq!(caller_of(&extraction, "request").name, "fetchUser");
}

#[test]
fn hoc_wrapped_arrows_are_functions_named_by_the_binding() {
    let extraction = values();

    assert_eq!(symbol(&extraction, "Card").kind, SymbolKind::Function);
    assert_eq!(caller_of(&extraction, "track").name, "Card");
    assert_eq!(caller_of(&extraction, "memo").name, "Card");
}

#[test]
fn object_property_arrows_are_named_functions() {
    let extraction = values();

    assert_eq!(symbol(&extraction, "onClick").kind, SymbolKind::Function);
    assert_eq!(caller_of(&extraction, "fire").name, "onClick");
    assert!(!names(&extraction).contains(&"label"));
    assert_eq!(symbol(&extraction, "handlers").kind, SymbolKind::Constant);
}

#[test]
fn sibling_declarator_scopes_do_not_bleed() {
    let extraction = values();

    assert_eq!(caller_of(&extraction, "fa").name, "first");
    assert_eq!(caller_of(&extraction, "fb").name, "second");
}

#[test]
fn anonymous_default_export_synthesizes_default() {
    let extraction = values();

    let default = symbol(&extraction, "default");
    assert_eq!(
        (default.kind, default.exported),
        (SymbolKind::Function, true)
    );
    assert_eq!(caller_of(&extraction, "boot").name, "default");
}

#[test]
fn plain_bindings_keep_their_declaration_kind() {
    let extraction = values();

    let counter = symbol(&extraction, "counter");
    assert_eq!(
        (counter.kind, counter.exported),
        (SymbolKind::Variable, false)
    );
}

#[test]
fn method_is_owned_and_qualified_by_its_class() {
    let extraction = extracted("src/app.ts", SAMPLE_TS);

    let area = (extraction.symbols.iter())
        .find(|symbol| symbol.qualified_name == "Circle.area")
        .expect("Circle.area");
    assert_eq!(
        (area.owner.as_deref(), area.kind),
        (Some("Circle"), SymbolKind::Method)
    );
}

#[test]
fn class_heritage_yields_parents() {
    let extraction = extracted("src/app.ts", SAMPLE_TS);

    assert_eq!(symbol(&extraction, "Circle").parents, ["Base", "Shape"]);
}

#[test]
fn calls_are_attributed_to_their_caller() {
    let extraction = extracted("src/app.ts", SAMPLE_TS);

    let compute_caller = caller_of(&extraction, "compute");
    let area_call = (extraction.calls.iter())
        .find(|call| call.callee == "area")
        .expect("area call");
    assert_eq!(compute_caller.qualified_name, "Circle.area");
    assert_eq!(area_call.receiver.as_deref(), Some("Circle"));
}

#[test]
fn a_receiver_the_caller_declares_names_no_type() {
    let extraction = extracted(
        "src/f.ts",
        "import * as ns from './ns';\nconst store = make();\nclass Widget {\n  static create() {}\n  \
         run(Widget: number) { this.step(); Widget.create(); }\n  step() {}\n}\n\
         export function f(p: Svc, { a, b: [c] }: Opts, ...rest: X[]) {\n  p.go(); a.go(); c.go(); rest.go();\n  \
         let v = 1, { w } = o;\n  v.go(); w.go(); store.go();\n  for (const it of items) it.go();\n  \
         try {} catch (err) { err.go(); }\n  [1].map(x => x.go());\n  ns.go(); Widget.create(); console.log();\n  \
         later.go();\n}\nconst later = 1;\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("make", None),
            ("step", Some("Widget")),
            ("create", Some("")),
            ("go", Some("Svc")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("make()")),
            ("go", Some("")),
            ("go", Some("")),
            ("map", Some("")),
            ("go", Some("")),
            ("go", Some("ns")),
            ("create", Some("Widget")),
            ("log", Some("console")),
            ("go", Some("later")),
        ]
    );
}

#[test]
fn a_parameter_its_context_types_names_the_type_the_context_spells() {
    let extraction = extracted(
        "src/f.ts",
        "type Replay<M> = (kartta: Kartta, params: M) => void;\n\
         interface Handlers { open(store: Store): void; close: (pool: Pool) => void; }\n\
         type Wrapped = Replay<number>;\n\
         type Generic<T> = (value: T, rest: Rest) => void;\n\
         const replays: { [M in keyof Methods]: Replay<M> } = { build: (kartta, params) => kartta.build(params) };\n\
         const one: Replay<number> = (kartta) => kartta.one();\n\
         const handlers: Handlers = { open(store) { store.open(); }, close: (pool) => pool.close() };\n\
         const wrapped: Wrapped = function (kartta) { kartta.wrapped(); };\n\
         const inline = { run: (task) => task.run() } satisfies { run: (task: Task) => void };\n\
         const keyed: { [key: string]: (job: Job) => void } = { any: (job) => job.start() };\n\
         const generic: Generic<Widget> = (value, rest) => { value.go(); rest.go(); };\n\
         const typed: Replay<number> = (kartta: Other) => kartta.typed();\n\
         const untyped = { run: (task) => task.run() };\n\
         type Pair = (first: First, second: Second) => void;\n\
         const mixed: Pair = (first: Other, second) => { first.left(); second.right(); };\n\
         type Deep1 = Deep2;\ntype Deep2 = Deep3;\ntype Deep3 = (deep: Deep) => void;\n\
         const deep: Deep1 = (deep) => deep.down();\n\
         type Loop1 = Loop2;\ntype Loop2 = Loop1;\nconst looped: Loop1 = (value) => value.around();\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("build", Some("Kartta")),
            ("one", Some("Kartta")),
            ("open", Some("Store")),
            ("close", Some("Pool")),
            ("wrapped", Some("Kartta")),
            ("run", Some("Task")),
            ("start", Some("Job")),
            ("go", Some("")),
            ("go", Some("Rest")),
            ("typed", Some("Other")),
            ("run", Some("")),
            ("left", Some("Other")),
            ("right", Some("Second")),
            ("down", Some("Deep")),
            ("around", Some("")),
        ]
    );
}

#[test]
fn a_parameter_names_the_one_type_its_annotation_spells() {
    let extraction = extracted(
        "src/f.ts",
        "export function f(a: Svc, b?: Svc, c: Svc = make(), d: ns.Svc, e: Box<Svc>) {\n  \
         a.required(); b.optional(); c.defaulted(); d.qualified(); e.generic();\n  \
         new a.Pool();\n  [1].forEach(() => a.nested());\n  { const a = 1; a.shadowed(); }\n}\n\
         export function g(s: string, u: Svc | null, l: Svc[], o: { go(): void }, { w }: Svc) {\n  \
         s.go(); u.go(); l.go(); o.go(); w.go();\n}\n\
         export function h<T extends Svc>(t: T) { t.go(); }\n\
         export class Box<V> { put(v: V) { v.go(); } }\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("make", None),
            ("required", Some("Svc")),
            ("optional", Some("Svc")),
            ("defaulted", Some("Svc")),
            ("qualified", Some("ns.Svc")),
            ("generic", Some("Box")),
            ("Pool", Some("Svc")),
            ("forEach", Some("")),
            ("nested", Some("Svc")),
            ("shadowed", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
            ("go", Some("")),
        ]
    );
}

#[test]
fn a_declared_value_names_no_type_only_inside_the_scope_that_declares_it() {
    let extraction = extracted(
        "src/f.ts",
        "class Widget { static create() {} }\nexport function f() {\n  { const Widget = 1; Widget.go(); }\n  \
         Widget.create();\n  try {} catch (Widget) { var kept = 1; Widget.go(); kept.go(); }\n  Widget.create();\n}\n\
         let Box = make();\nexport function g() {\n  class Box { static go() {} }\n  Box.go();\n}\n\
         export function h() { Box.go(); }\nexport function k() {\n  const Box = class { static go() {} };\n  \
         Box.go();\n}\nexport function m(n) {\n  for (let Widget = 0; Widget < n; Widget++) Widget.go();\n  \
         switch (n) { case 1: const Widget = 2; Widget.go(); }\n  Widget.create();\n}\n\
         export function p() {\n  { function Widget() {} Widget.go(); }\n  Widget.create();\n}\n\
         function merged() {}\nnamespace merged { export function g() {} }\nexport function q() { merged.g(); }\n\
         namespace Inner { const Widget = 1; }\nexport function r() { Widget.create(); }\n\
         const Named = 0;\nexport const Y = class Named { static run() { Named.go(); } };\n\
         export function s() { Named.go(); }\nexport const t = function Widget() { Widget.go(); };\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("go", Some("")),
            ("create", Some("Widget")),
            ("go", Some("")),
            ("go", Some("")),
            ("create", Some("Widget")),
            ("make", None),
            ("go", Some("Box")),
            ("go", Some("make()")),
            ("go", Some("Box")),
            ("go", Some("")),
            ("go", Some("")),
            ("create", Some("Widget")),
            ("go", Some("")),
            ("create", Some("Widget")),
            ("g", Some("merged")),
            ("create", Some("Widget")),
            ("go", Some("Named")),
            ("go", Some("")),
            ("go", Some("")),
        ]
    );
}

#[test]
fn a_definition_records_the_one_type_its_return_annotation_spells() {
    let extraction = extracted(
        "src/f.ts",
        "export function opened(): Kartta {}\n\
         export async function connected(): Promise<ns.Kartta> {}\n\
         export function held<T>(value: T): T {}\n\
         export async function promised<T>(value: T): Promise<T> {}\n\
         export function listed(): Kartta[] {}\n\
         export function counted(): number {}\n\
         export function either(): Promise<Kartta | null> {}\n\
         export function open(): void {}\n\
         export const reopened = (): Kartta => make();\n\
         export function untyped() {}\n\
         export class Box<V> {\n  made(): Kartta {}\n  kept(): V {}\n  field = (): Kartta => make();\n}\n",
    );

    let value_types = |name| symbol(&extraction, name).value_types.as_slice();
    assert_eq!(value_types("opened"), ["Kartta"]);
    assert_eq!(value_types("connected"), ["?ns.Kartta"]);
    assert_eq!(value_types("reopened"), ["Kartta"]);
    assert_eq!(value_types("made"), ["Kartta"]);
    assert_eq!(value_types("field"), ["Kartta"]);
    for name in [
        "held", "promised", "listed", "counted", "either", "open", "untyped", "kept",
    ] {
        assert!(value_types(name).is_empty(), "{name}");
    }
}

#[test]
fn a_local_a_call_or_a_new_initializes_records_the_path_of_calls() {
    let extraction = extracted(
        "src/f.ts",
        "import { Kartta } from './kartta';\n\
         export class Suite {\n\
         \x20 run(value: Kartta, list: Kartta[]) {\n\
         \x20   const bare = opened();\n    bare.a();\n\
         \x20   const named = Kartta.open();\n    named.b();\n\
         \x20   const chained = named.child();\n    chained.c();\n\
         \x20   const awaited = await connect();\n    awaited.d();\n\
         \x20   const own = this.made();\n    own.e();\n\
         \x20   const typed = value.child();\n    typed.f();\n\
         \x20   const built = new Kartta();\n    built.g();\n\
         \x20   const written: Kartta = made();\n    written.h();\n\
         \x20   const first = list[0].child();\n    first.i();\n\
         \x20   const element = list[0];\n    element.j();\n\
         \x20   const nested = opened().child();\n    nested.k();\n\
         \x20   const lost = list[0].child().next();\n    lost.l();\n\
         \x20 }\n\
         }\n",
    );

    let receiver_of = |callee: &str| {
        (extraction.calls.iter())
            .find(|call| call.callee == callee)
            .and_then(|call| call.receiver.as_deref())
    };
    assert_eq!(receiver_of("a"), Some("opened()"));
    assert_eq!(receiver_of("b"), Some("Kartta::open()"));
    assert_eq!(receiver_of("c"), Some("Kartta::open()::child()"));
    assert_eq!(receiver_of("d"), Some("connect()?"));
    assert_eq!(receiver_of("e"), Some("Suite::made()"));
    assert_eq!(receiver_of("f"), Some("Kartta::child()"));
    assert_eq!(receiver_of("g"), Some("Kartta()"));
    assert_eq!(receiver_of("h"), Some("Kartta"));
    assert_eq!(receiver_of("i"), Some(""));
    assert_eq!(receiver_of("j"), Some(""));
    assert_eq!(receiver_of("k"), Some("opened()::child()"));
    assert_eq!(receiver_of("l"), Some(""));
}

#[test]
fn a_javascript_receiver_the_file_declares_names_no_type() {
    let extraction = extracted(
        "src/f.js",
        "class Widget { static create() {} }\nfunction f(p, { a }) {\n  p.go(); a.go();\n  var v = make();\n  \
         v.go();\n  Widget.create();\n}\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("go", Some("")),
            ("go", Some("")),
            ("make", None),
            ("go", Some("make()")),
            ("create", Some("Widget")),
        ]
    );
}

#[test]
fn re_exports_emit_specifiers_and_wildcard() {
    let extraction = extracted("src/app.ts", SAMPLE_TS);

    let re_exports: Vec<(&str, &str, &str)> = (extraction.re_exports.iter())
        .map(|re_export| {
            (
                re_export.local_name.as_str(),
                re_export.source_name.as_str(),
                re_export.source_path.as_str(),
            )
        })
        .collect();
    assert_eq!(
        re_exports,
        [
            ("help", "helper", "./more"),
            ("other", "other", "./more"),
            ("*", "*", "./everything")
        ]
    );
}

#[test]
fn tsx_flavor_parses_components() {
    let extraction = extracted(
        "src/app.tsx",
        "export function App() {\n    return <div onClick={() => handle()}>hi</div>;\n}\nfunction handle() {}\n",
    );

    assert_eq!(names(&extraction), ["App", "handle"]);
    assert_eq!(caller_of(&extraction, "handle").name, "App");
}

#[test]
fn generic_and_qualified_heritage_entries_keep_their_base_name() {
    let extraction = extracted(
        "src/h.ts",
        r"
class Repo<T> extends Base<T> implements Comparable<Repo<T>> {}
class Widget extends React.Component<Props> {}
interface Page extends Layout<Slots> {}
",
    );

    assert_eq!(symbol(&extraction, "Repo").parents, ["Base", "Comparable"]);
    assert_eq!(symbol(&extraction, "Widget").parents, ["Component"]);
    assert_eq!(symbol(&extraction, "Page").parents, ["Layout"]);
}

#[test]
fn a_heritage_expression_records_the_class_it_names_in_every_grammar() {
    let source = r"
class Controller extends Target {}
class Widget extends React.Component {}
class Deep extends a.b.Base {}
class Mixed extends mixin(Base) {}
class Chosen extends (flag ? First : Second) {}
class Keyed extends registry['Base'] {}
class Root extends null {}
";

    for path in ["src/h.js", "src/h.ts", "src/h.tsx"] {
        let extraction = extracted(path, source);

        let parents = |name| symbol(&extraction, name).parents.as_slice();
        assert_eq!(parents("Controller"), ["Target"], "{path}");
        assert_eq!(parents("Widget"), ["Component"], "{path}");
        assert_eq!(parents("Deep"), ["Base"], "{path}");
        for name in ["Mixed", "Chosen", "Keyed", "Root"] {
            assert_eq!(parents(name), [] as [&str; 0], "{path} {name}");
        }
    }
}

#[test]
fn an_anonymous_default_class_records_the_class_it_extends() {
    for path in ["src/h.js", "src/h.ts"] {
        let extraction = extracted(path, "export default class extends Base {}\n");

        assert_eq!(symbol(&extraction, "default").parents, ["Base"], "{path}");
    }
}

#[test]
fn class_fields_split_into_fields_and_arrow_methods() {
    let extraction = extracted(
        "src/store.ts",
        r"
export class Store {
    items: string[] = [];
    #secret = 1;
    private hidden = 2;
    refresh = () => {
        reload(this.items);
    };
}
",
    );

    let items = symbol(&extraction, "items");
    assert_eq!(
        (items.qualified_name.as_str(), items.kind, items.exported),
        ("Store.items", SymbolKind::Field, true)
    );
    assert!(!symbol(&extraction, "#secret").exported);
    assert!(!symbol(&extraction, "hidden").exported);
    assert_eq!(symbol(&extraction, "refresh").kind, SymbolKind::Method);
    assert_eq!(
        caller_of(&extraction, "reload").qualified_name,
        "Store.refresh"
    );
}

#[test]
fn jsx_component_tags_become_call_edges() {
    let extraction = extracted(
        "src/app.tsx",
        r"
function List() { return null; }
export function App() {
    return (
        <main>
            <List />
            <UI.Spinner size={2} />
        </main>
    );
}
",
    );

    assert_eq!(
        attributed(&extraction),
        [("App", "List"), ("App", "Spinner")]
    );
    assert_eq!(
        calls(&extraction),
        [("List", None), ("Spinner", Some("UI"))]
    );
}

#[test]
fn new_expressions_are_constructor_calls() {
    let extraction = extracted(
        "src/boot.ts",
        "function boot() {\n    const a = new Service();\n    const b = new api.Client(1);\n}\n",
    );

    assert_eq!(caller_of(&extraction, "Service").name, "boot");
    assert_eq!(
        calls(&extraction),
        [("Service", None), ("Client", Some("api"))]
    );
}

#[test]
fn test_registrations_carry_their_bodies_calls() {
    let extraction = extracted(
        "src/header.test.ts",
        r#"
describe("Header", () => {
    beforeEach(() => {
        resetDom();
    });

    it("renders the title", () => {
        renderTitle();
    });

    it.skip("renders the subtitle", () => {
        renderSubtitle();
    });

    xit("renders the footer", () => {
        renderFooter();
    });
});
"#,
    );

    assert_eq!(
        names(&extraction),
        [
            "Header",
            "beforeEach",
            "renders the title",
            "renders the subtitle",
            "renders the footer"
        ]
    );
    assert!(
        extraction
            .symbols
            .iter()
            .all(|symbol| symbol.flags.is_test() && symbol.flags.is_label())
    );
    assert!(
        (extraction.symbols.iter())
            .all(|symbol| symbol.kind == SymbolKind::Function && !symbol.exported)
    );
    assert_eq!(
        attributed(&extraction),
        [
            ("", "describe"),
            ("Header", "beforeEach"),
            ("beforeEach", "resetDom"),
            ("Header", "it"),
            ("renders the title", "renderTitle"),
            ("Header", "skip"),
            ("renders the subtitle", "renderSubtitle"),
            ("Header", "xit"),
            ("renders the footer", "renderFooter"),
        ]
    );
}

#[test]
fn a_registration_in_a_production_file_is_not_marked_test_code() {
    let extraction = extracted(
        "src/rules.ts",
        r#"
describe("shipping rules", () => {
    beforeEach(() => {
        resetRules();
    });

    applyRule();
});
"#,
    );

    assert!(
        extraction
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_test())
    );
    assert!(
        extraction
            .symbols
            .iter()
            .all(|symbol| symbol.kind == SymbolKind::Function)
    );
    assert_eq!(
        attributed(&extraction),
        [
            ("", "describe"),
            ("shipping rules", "beforeEach"),
            ("beforeEach", "resetRules"),
            ("shipping rules", "applyRule")
        ]
    );
}

#[test]
fn an_empty_test_description_names_nothing() {
    let extraction = extracted(
        "src/edge.test.ts",
        "describe(\"Edge\", () => {\n    it(\"\", () => {\n        noop();\n    });\n});\n",
    );

    assert_eq!(names(&extraction), ["Edge"]);
    assert_eq!(
        attributed(&extraction),
        [("", "describe"), ("Edge", "it"), ("Edge", "noop")]
    );
}

#[test]
fn a_dynamic_test_description_opens_no_scope() {
    let extraction = extracted(
        "src/cases.test.ts",
        r#"
describe("Cases", () => {
    it(`case ${n}`, () => {
        runTemplate();
    });

    it(label, () => {
        runVariable();
    });

    it.each([[1], [2]])("case %i", () => {
        runEach();
    });
});
"#,
    );

    assert_eq!(names(&extraction), ["Cases"]);
    assert_eq!(
        attributed(&extraction),
        [
            ("", "describe"),
            ("Cases", "it"),
            ("Cases", "runTemplate"),
            ("Cases", "it"),
            ("Cases", "runVariable"),
            ("Cases", "each"),
            ("Cases", "runEach"),
        ]
    );
}

#[test]
fn a_dynamic_registration_at_file_scope_leaves_its_calls_at_file_scope() {
    let extraction = extracted(
        "src/loose.test.ts",
        "it(label, () => {\n    runAtFileScope();\n});\n",
    );

    assert_eq!(names(&extraction), Vec::<&str>::new());
    assert_eq!(
        attributed(&extraction),
        [("", "it"), ("", "runAtFileScope")]
    );
}

#[test]
fn a_call_on_this_names_the_enclosing_class() {
    let extraction = extracted(
        "src/circle.ts",
        r"export class Circle {
  area() { return 1; }
  report() { return this.area(); }
  onDraw = () => this.draw();
  onErase = function () { this.erase(); };
  later() {
    const next = () => this.redo();
    const own = function () { return this.undo(); };
    register(function () { this.clear(); });
    items.forEach((i) => this.fill(i));
    register(function* () { this.step(); });
  }
}
",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("area", Some("Circle")),
            ("draw", Some("Circle")),
            ("erase", Some("Circle")),
            ("redo", Some("Circle")),
            ("undo", Some("")),
            ("register", None),
            ("clear", Some("")),
            ("forEach", Some("items")),
            ("fill", Some("Circle")),
            ("register", None),
            ("step", Some("")),
        ]
    );
}

#[test]
fn a_call_in_a_class_body_is_made_by_the_class() {
    let extraction = extracted(
        "src/widget.ts",
        r"export function makeDefault(): number { return 1; }
export function initRegistry(): void {}
export class Widget {
  @Input() limit = makeDefault();
  static shared = makeDefault();
  static {
    initRegistry();
  }
  onClick = () => track();
  read(): number {
    return makeDefault();
  }
}
export function build() {
  class Local {
    made = makeDefault();
  }
  return Local;
}
export default class {
  ready = initRegistry();
}
boot();
",
    );

    assert_eq!(
        attributed(&extraction),
        [
            ("Widget", "Input"),
            ("Widget", "makeDefault"),
            ("Widget", "makeDefault"),
            ("Widget", "initRegistry"),
            ("onClick", "track"),
            ("read", "makeDefault"),
            ("Local", "makeDefault"),
            ("default", "initRegistry"),
            ("", "boot"),
        ]
    );
}

#[test]
fn this_in_a_class_body_names_the_class() {
    let extraction = extracted(
        "src/filters.ts",
        r"export class Filters {
  table = { fav: this.favFilter, run: () => this.apply() };
  static instance = this.create();
  favFilter() {}
}
",
    );
    let uses: Vec<(&str, &str, &Receiver)> = (extraction.uses.iter())
        .map(|site| {
            let site_of = extraction.symbols[site.site_of.0 as usize].name.as_str();
            (site_of, site.name.as_str(), &site.receiver)
        })
        .collect();

    assert_eq!(
        calls(&extraction),
        [("apply", Some("Filters")), ("create", Some("Filters"))]
    );
    assert_eq!(
        uses,
        [(
            "Filters",
            "favFilter",
            &Receiver::Path("Filters".to_owned())
        )]
    );
}

#[test]
fn this_in_a_class_expression_s_body_names_the_class_by_the_name_that_binds_it() {
    let extraction = extracted(
        "src/filters.ts",
        r"export namespace Ui {
  export const Filters = class {
    table = { fav: this.favFilter, run: () => this.apply() };
    favFilter() {}
  };
  export const Picked = class Chosen {
    table = { pick: this.pick };
    made = this.choose();
    pick() {}
    choose() {}
  };
}
",
    );
    let uses: Vec<(&str, &str, &Receiver)> = (extraction.uses.iter())
        .map(|site| {
            let site_of = extraction.symbols[site.site_of.0 as usize].name.as_str();
            (site_of, site.name.as_str(), &site.receiver)
        })
        .collect();

    assert_eq!(
        calls(&extraction),
        [("apply", Some("Ui.Filters")), ("choose", Some("Chosen"))]
    );
    assert_eq!(
        uses,
        [
            (
                "Filters",
                "favFilter",
                &Receiver::Path("Ui.Filters".to_owned())
            ),
            ("Picked", "pick", &Receiver::Path("Chosen".to_owned())),
        ]
    );
}

#[test]
fn an_anonymous_class_goes_by_the_name_that_binds_it() {
    let extraction = extracted(
        "src/socket.ts",
        "export default class {\n  send(data: string) { this.ping(); }\n  ping() {}\n}\n\
         const Box = class { static go() {} };\nlet [Lost] = [class { gone() {} }];\n",
    );

    let owned: Vec<(&str, Option<&str>, SymbolKind)> = (extraction.symbols.iter())
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.owner.as_deref(),
                symbol.kind,
            )
        })
        .collect();
    assert_eq!(
        owned,
        [
            ("default", None, SymbolKind::Class),
            ("default.send", Some("default"), SymbolKind::Method),
            ("default.ping", Some("default"), SymbolKind::Method),
            ("Box", None, SymbolKind::Constant),
            ("Box.go", Some("Box"), SymbolKind::Method),
            ("gone", None, SymbolKind::Function),
        ]
    );
    assert_eq!(calls(&extraction), [("ping", Some("default"))]);
}

#[test]
fn a_class_expression_a_member_write_or_a_property_binds_goes_by_the_name_there() {
    let extraction = extracted(
        "src/kinds.ts",
        r#"ns.Box = class extends Base {
  size = this.measure();
  copy = this.size;
  measure() { return this.size; }
};
const table = {
  K: class { value = 1; },
  "quoted": class { run() {} },
};
ns.Picked = class Chosen { pick() {} };
ns[key] = class { computed() {} };
target = class { assigned() {} };
const spread = { [key]: class { spreaded() {} } };
"#,
    );

    let owned: Vec<(&str, Option<&str>, SymbolKind)> = (extraction.symbols.iter())
        .map(|symbol| {
            (
                symbol.qualified_name.as_str(),
                symbol.owner.as_deref(),
                symbol.kind,
            )
        })
        .collect();
    assert_eq!(
        owned,
        [
            ("Box", None, SymbolKind::Class),
            ("Box.size", Some("Box"), SymbolKind::Field),
            ("Box.copy", Some("Box"), SymbolKind::Field),
            ("Box.measure", Some("Box"), SymbolKind::Method),
            ("table", None, SymbolKind::Constant),
            ("K", None, SymbolKind::Class),
            ("K.value", Some("K"), SymbolKind::Field),
            ("quoted", None, SymbolKind::Class),
            ("quoted.run", Some("quoted"), SymbolKind::Method),
            ("Chosen", None, SymbolKind::Class),
            ("Chosen.pick", Some("Chosen"), SymbolKind::Method),
            ("computed", None, SymbolKind::Function),
            ("assigned", None, SymbolKind::Function),
            ("spread", None, SymbolKind::Constant),
            ("spreaded", None, SymbolKind::Function),
        ]
    );
    assert_eq!(extraction.symbols[0].parents, ["Base"]);
    assert_eq!(calls(&extraction), [("measure", Some("Box"))]);
    let reads: Vec<(&str, &Receiver)> = (extraction.uses.iter())
        .map(|site| {
            let site_of = extraction.symbols[site.site_of.0 as usize].name.as_str();
            (site_of, &site.receiver)
        })
        .collect();
    assert_eq!(
        reads,
        [
            ("Box", &Receiver::Path("Box".to_owned())),
            ("measure", &Receiver::Own),
        ]
    );
}

#[test]
fn a_construction_and_a_tag_on_a_declared_value_name_no_type() {
    let extraction = extracted(
        "src/f.tsx",
        "import * as ui from './ui';\nexport function f(m) {\n  new m.Thing(); new ui.Thing();\n  \
         return [<m.Panel />, <ui.Panel />];\n}\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("Thing", Some("")),
            ("Thing", Some("ui")),
            ("Panel", Some("")),
            ("Panel", Some("ui")),
        ]
    );
}

#[test]
fn a_bare_call_of_a_parameter_is_a_site_that_binds_nothing() {
    let extraction = extracted(
        "src/f.tsx",
        "export function f(plain, typed: Run, { shorthand, key: renamed }: Opts, [item], tail = plain, ...rest) {\n  \
         plain(); typed(); shorthand(); renamed(); item(); tail(); rest();\n  \
         key();\n}\n\
         export const g = (arrow) => arrow();\n\
         export function h({ Icon }: Props, Pool) {\n  new Pool();\n  return <Icon />;\n}\n",
    );

    assert_eq!(calls_that_may_bind(&extraction), ["key"]);
    assert_eq!(calls(&extraction).len(), 11);
}

#[test]
fn a_bare_call_of_a_parameter_stays_where_another_definition_of_the_file_has_its_name() {
    for path in ["src/f.ts", "src/f.js"] {
        let extraction = extracted(
            path,
            "function run(save, Pool, late, cb) {\n  save(); new Pool(); late(); cb(); miss();\n}\n\
             function save() {}\nclass Pool {}\nconst late = () => {};\n\
             export function pass() {\n  return run(save, Pool, late);\n}\n\
             const handlers = { save(cb) { save(); } };\n\
             export function hook(onDone) {\n  return { onDone() { onDone(); } };\n}\n\
             export const first = { retry(retry) { retry(); } };\nexport const second = { retry() {} };\n",
        );

        assert_eq!(
            calls_that_may_bind(&extraction),
            ["save", "Pool", "late", "miss", "run", "save", "retry"]
        );
    }
}

#[test]
fn a_bare_call_of_a_parameter_binds_no_member_label_or_namespace_function_of_its_name() {
    let extraction = extracted(
        "src/f.test.ts",
        "namespace Tools {\n  export function nested() {}\n}\nclass Box {\n  member() {}\n}\n\
         it(\"labelled\", () => {});\n\
         export function run(nested, member, labelled) {\n  nested(); member(); labelled();\n}\n",
    );

    assert_eq!(calls_that_may_bind(&extraction), ["it"]);
}

#[test]
fn a_bare_call_of_a_parameter_of_a_minified_file_binds_nothing() {
    let padding = "x".repeat(600);
    let extraction = extracted(
        "src/f.min.js",
        &format!("function t(){{}}function f(t){{t()}}var a=\"{padding}\";\n"),
    );

    assert_eq!(calls_that_may_bind(&extraction), Vec::<&str>::new());
    assert_eq!(calls(&extraction), [("t", None)]);
}

#[test]
fn a_bare_call_of_a_name_a_parameter_does_not_hold_keeps_its_call() {
    let extraction = extracted(
        "src/f.ts",
        "export function outer(run) {\n  \
         function nested() {}\n  nested();\n  \
         { const run = () => {}; run(); }\n  \
         const self = function walk() { walk(); };\n  \
         return { run() { return (run) => {}; }, done() { other(); } };\n}\n\
         export function after() { run(); }\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("nested", None),
            ("run", None),
            ("walk", None),
            ("other", None),
            ("run", None),
        ]
    );
}

#[test]
fn a_member_call_on_an_unnamed_expression_keeps_an_empty_receiver() {
    let extraction = extracted(
        "src/f.ts",
        "export function f() { items[0].close(); this.pool.get(); make().run(); }\n",
    );

    assert_eq!(
        calls(&extraction),
        [
            ("close", Some("")),
            ("get", Some("")),
            ("run", Some("")),
            ("make", None)
        ]
    );
}

#[test]
fn test_files_mark_every_symbol_as_test() {
    let source = "export function helper() {}\n\nexport function thing() {}\n";

    for path in [
        "foo.test.ts",
        "bar.spec.tsx",
        "pkg/__tests__/util.ts",
        "svc/tests/run.js",
        "svc/Tests/run.js",
    ] {
        let extraction = extracted(path, source);
        assert!(
            symbol(&extraction, "helper").flags.is_test(),
            "{path} helper"
        );
        assert!(symbol(&extraction, "thing").flags.is_test(), "{path} thing");
    }
    assert!(
        !symbol(&extracted("src/foo.ts", source), "helper")
            .flags
            .is_test()
    );
}

#[test]
fn files_under_node_modules_or_vendor_are_vendored() {
    let source = "export function helper() {}\nexport function other() {}\n";
    let is_vendored = |path: &str| {
        symbol(&extracted(path, source), "helper")
            .flags
            .is_vendored()
    };

    for path in [
        "node_modules/left-pad/index.js",
        "web/node_modules/left-pad/index.js",
        "vendor/lib.ts",
        "web/vendor/lib.ts",
    ] {
        assert!(is_vendored(path), "{path}");
        assert!(
            symbol(&extracted(path, source), "other")
                .flags
                .is_vendored(),
            "{path}"
        );
    }
    for path in [
        "src/foo.ts",
        "src/vendors/lib.ts",
        "src/my_node_modules/lib.ts",
    ] {
        assert!(!is_vendored(path), "{path}");
    }
}

#[test]
fn generated_markers_and_names_are_generated() {
    let ordinary = "export function emitted() {}\n";
    let line_marker = "// @generated by tool\nexport function emitted() {}\n";
    let block_marker = "/*\n * DO NOT EDIT.\n */\nexport function emitted() {}\n";
    let is_generated = |path: &str, source: &str| {
        symbol(&extracted(path, source), "emitted")
            .flags
            .is_generated()
    };

    assert!(is_generated("src/foo.ts", line_marker));
    assert!(is_generated("src/foo.ts", block_marker));
    for path in [
        "src/schema.generated.ts",
        "src/schema.gen.js",
        "src/foo_pb.ts",
        "src/schema_pb.tsx",
        "src/schema_pb.d.ts",
        "src/schema.pb.mjs",
        "static/jquery.min.js",
        "static/bootstrap.bundle.min.js",
        "static/htmx.min.mjs",
    ] {
        assert!(is_generated(path, ordinary), "{path}");
    }
    for path in [
        "src/foo.ts",
        "src/types.d.ts",
        "src/min.js",
        "src/minify.ts",
        "src/col-min.ts",
        "src/pb.ts",
    ] {
        assert!(!is_generated(path, ordinary), "{path}");
    }
}

#[test]
fn a_first_line_of_code_a_minifier_would_write_is_generated() {
    let body = "var a=1;".repeat(64);
    let minified = format!("/*! sdk v1 */\nexport function emitted(){{{body}}}\n");
    let long_later = format!("export function emitted() {{}}\nconst data = \"{body}\";\n");

    let is_generated = |source: &str| {
        symbol(&extracted("src/sdk.mjs", source), "emitted")
            .flags
            .is_generated()
    };

    assert!(is_generated(&minified));
    assert!(!is_generated(&long_later));
}

#[test]
fn block_comment_close_with_trailing_code_does_not_swallow_later_markers() {
    let one_liner = "/* banner */ const NOTE = \"DO NOT EDIT\";\n\nexport function coded() {}\n";
    let trailing =
        "/*\n * banner\n */ const NOTE = 1;\n// DO NOT EDIT\nexport function real() {}\n";
    let genuine = "/* @generated by tool */ export function gen() {}\n";

    assert!(
        !symbol(&extracted("src/foo.ts", one_liner), "coded")
            .flags
            .is_generated()
    );
    assert!(
        !symbol(&extracted("src/foo.ts", trailing), "real")
            .flags
            .is_generated()
    );
    assert!(
        symbol(&extracted("src/foo.ts", genuine), "gen")
            .flags
            .is_generated()
    );
}

#[test]
fn a_quoted_marker_in_the_header_is_a_mention() {
    let source = "// a `@generated` / `DO NOT EDIT` marker\nexport function f() {}\n";

    for path in ["src/a.ts", "src/a.js"] {
        assert!(
            !symbol(&extracted(path, source), "f").flags.is_generated(),
            "{path}"
        );
    }
}

#[test]
fn singular_test_path_component_is_not_test_code() {
    let source = "export function helper() {}\n";

    assert!(
        !symbol(&extracted("src/test/helper.ts", source), "helper")
            .flags
            .is_test()
    );
    for path in ["src/tests/helper.ts", "src/__tests__/helper.ts"] {
        assert!(
            symbol(&extracted(path, source), "helper").flags.is_test(),
            "{path}"
        );
    }
}

#[test]
fn no_symbol_is_ever_an_entry_point() {
    let extraction = extracted(
        "src/index.ts",
        "export function main() {}\n\nexport default function () {\n    boot();\n}\n",
    );

    assert_eq!(names(&extraction), ["main", "default"]);
    assert!(
        extraction
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_entry())
    );
}

const GENERATORS_UNTYPED: &str = "\
export function plain() {
  return 1;
}

export function* walk(limit) {
  yield plain();
}

async function* stream(source) {
  yield* walk(await source);
}

export function consume() {
  return [...walk(2)];
}
";

const GENERATORS_TYPED: &str = "\
export function plain(): number {
  return 1;
}

export function* walk(limit: number): Generator<number> {
  yield plain();
}

async function* stream(source: Promise<number>): AsyncGenerator<number> {
  yield* walk(await source);
}

export function consume(): number[] {
  return [...walk(2)];
}
";

#[test]
fn a_generator_declaration_is_a_function_symbol_in_every_flavor() {
    let untyped = [
        "function plain()",
        "function* walk(limit)",
        "async function* stream(source)",
        "function consume()",
    ];
    let typed = [
        "function plain(): number",
        "function* walk(limit: number): Generator<number>",
        "async function* stream(source: Promise<number>): AsyncGenerator<number>",
        "function consume(): number[]",
    ];

    for (path, source, [plain, walk, stream, consume]) in [
        ("gen.js", GENERATORS_UNTYPED, untyped),
        ("gen.ts", GENERATORS_TYPED, typed),
        ("gen.tsx", GENERATORS_TYPED, typed),
    ] {
        let extraction = extracted(path, source);

        let symbols: Vec<_> = (extraction.symbols.iter())
            .map(|symbol| {
                (
                    symbol.name.as_str(),
                    symbol.kind,
                    symbol.exported,
                    symbol.line,
                    symbol.end_line,
                    symbol.signature.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            symbols,
            [
                ("plain", SymbolKind::Function, true, 1, 3, Some(plain)),
                ("walk", SymbolKind::Function, true, 5, 7, Some(walk)),
                ("stream", SymbolKind::Function, false, 9, 11, Some(stream)),
                ("consume", SymbolKind::Function, true, 13, 15, Some(consume)),
            ],
            "{path}"
        );
        assert_eq!(
            attributed(&extraction),
            [("walk", "plain"), ("stream", "walk"), ("consume", "walk")],
            "{path}"
        );
    }
}

#[test]
fn a_default_exported_generator_declaration_keeps_its_name() {
    for path in ["gen.js", "gen.ts", "gen.tsx"] {
        let extraction = extracted(path, "export default function* walk() {\n  step();\n}\n");

        let symbols: Vec<_> = (extraction.symbols.iter())
            .map(|symbol| (symbol.name.as_str(), symbol.kind, symbol.exported))
            .collect();
        assert_eq!(symbols, [("walk", SymbolKind::Function, true)], "{path}");
        assert_eq!(attributed(&extraction), [("walk", "step")], "{path}");
    }
}

#[test]
fn an_anonymous_generator_expression_emits_no_symbol() {
    for path in ["gen.js", "gen.ts", "gen.tsx"] {
        let extraction = extracted(
            path,
            "export function host() {\n  register(function* () {\n    step();\n  });\n}\n",
        );

        assert_eq!(names(&extraction), ["host"], "{path}");
        assert_eq!(
            attributed(&extraction),
            [("host", "register"), ("host", "step")],
            "{path}"
        );
    }
}

fn fastest_of_three_extractions(comment_lines: usize) -> Duration {
    let source = format!("{}f();\n", "////\n".repeat(comment_lines));
    let run = || {
        let started = Instant::now();
        let raw = TypeScript::new().extract(source.as_bytes());
        let elapsed = started.elapsed();
        assert_eq!(raw.calls.len(), 1);
        elapsed
    };
    run().min(run()).min(run())
}

#[test]
fn eight_times_the_top_level_comments_cost_about_eight_times_the_walk() {
    let short = fastest_of_three_extractions(2_500);
    let long = fastest_of_three_extractions(20_000);

    assert!(long < short * 24, "{short:?} became {long:?}");
}

#[test]
fn the_child_helpers_list_what_indexing_lists_at_every_width() {
    let grammar = tree_sitter_javascript::LANGUAGE.into();
    for width in 0..40 {
        let source = format!("f({});", vec!["a"; width].join(", /* c */"));
        let tree = parse(&grammar, source.as_bytes()).unwrap();
        preorder(tree.root_node(), |node| {
            let children: Vec<_> = (0..node.child_count())
                .filter_map(|index| node.child(index))
                .collect();
            let named: Vec<_> = children.iter().copied().filter(Node::is_named).collect();

            assert_eq!(child_nodes(node).collect::<Vec<_>>(), children);
            assert!(child_nodes(node).rev().eq(children.iter().rev().copied()));
            assert_eq!(named_child_nodes(node).collect::<Vec<_>>(), named);
            assert!(
                named_child_nodes(node)
                    .rev()
                    .eq(named.iter().rev().copied())
            );
            true
        });
    }
}
