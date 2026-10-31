//! What an engine built from a tree on disk binds: a selection of part of the TypeScript family leaves the files of the
//! other languages out of the graph, no Rust call binds a module, a Rust call into another crate of the workspace binds
//! what that crate exports, through its `use`s and globs the item they name, one through `crate::`, `self::` or
//! `super::` the item of the module it names, as does a
//! `use`, a glob or a path call whose first segment is a module the file declares, a private one
//! only from that module or below and a test one only from its crate's test code, a tuple struct's constructor the
//! struct its `use` names, a bare call through a glob what the glob's module shows it, a macro invocation the
//! `#[macro_export]` macro of its root file or the one of the files that file includes, through a `use` inside a
//! function body or an inline module only from inside it, a call no macro and a macro invocation no function of its
//! name, a method call no constant of its name, which a path call binds, a method named for its own type, which is no
//! constructor there, and the one method of its name whatever value
//! a body declares of that name, a Rust path call through a module no item of its
//! own file that the path does not name, a Rust call without a path no free item of an inline module it is outside of,
//! nor a member of a private type of one, and a Rust call on a value, on `self` or through a
//! type binds no free function, not even through a value named like a module the file imports, while a call on a
//! value still binds the one runtime method whose owner the value's name spells, one on a parameter or a field of
//! `self` the method of the type it is declared as, never the caller's own, of the caller's own package where two
//! versions of its crate declare that type, a bare call through a parameter or a local binds nothing, a call through a
//! type its wasm-bindgen `static_method_of` or `constructor` function, and a call through a type the tree does not declare, `Vec::new()`, binds no member of
//! another type; a Rust call binds by name only within the crates its crate is built with. A JavaScript bare call of a
//! name imported from a package binds no namesake of the project, unless the caller's file defines it. A TypeScript or
//! JavaScript call of a runtime method on a value binds an imported class's member of its name only where its receiver
//! spells the class, and an imported interface's member on any value. A JavaScript call through a class its file sees
//! binds no function outside every type, unless its file is minified. A Python call of a builtin binds a function of
//! its name only where its file imports or defines that name, and a Python call through or of a name that an import of
//! a module outside the tree binds binds no namesake of the project.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Miss, Selector, SymbolKind, confidence};
use tempfile::TempDir;

fn tree_of(files: &[(&str, &str)]) -> TempDir {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    tree
}

fn tree() -> TempDir {
    tree_of(&[
        (
            "app.ts",
            "import { Button } from \"./Button\";\n\nexport function render() {\n  return Button();\n}\n",
        ),
        (
            "Button.tsx",
            "export function Button() {\n  return <button />;\n}\n",
        ),
    ])
}

fn build(tree: &TempDir, names: &[&str]) -> Engine {
    let languages = kartta_languages::select(names).expect("every name is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    outcome.engine
}

type Callee<'e> = (&'e str, SymbolKind, &'e str);

fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> (Vec<Callee<'e>>, usize) {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    let reached = (callees.reached.iter())
        .map(|reached| {
            let symbol = reached.symbol;
            (symbol.name, symbol.kind, symbol.file)
        })
        .collect();
    (reached, callees.unresolved)
}

#[test]
fn typescript_without_tsx_indexes_a_tsx_file_and_binds_no_call_into_it() {
    let tree = tree();
    let engine = build(&tree, &["typescript"]);

    assert!(engine.index().size("Button.tsx").is_some());
    assert!(engine.graph_of("Button.tsx").is_none());
    assert_eq!(callees(&engine, "app.ts", "render"), (vec![], 1));
}

#[test]
fn typescript_with_tsx_binds_the_call_into_the_tsx_file() {
    let tree = tree();
    let engine = build(&tree, &["typescript", "tsx"]);

    assert_eq!(
        callees(&engine, "app.ts", "render"),
        (vec![("Button", SymbolKind::Function, "Button.tsx")], 0)
    );
}

#[test]
fn a_rust_call_named_like_a_module_binds_no_call_to_the_module() {
    let tree = tree_of(&[
        ("lib.rs", "mod walk;\n"),
        (
            "walk.rs",
            "pub fn raw_walk(tree: &Tree) {\n    tree.walk();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(callees(&engine, "walk.rs", "raw_walk"), (vec![], 1));
}

#[test]
fn a_rust_call_through_another_workspace_crate_binds_the_function_that_crate_exports() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other-crate\"\n",
        ),
        ("crates/other/src/lib.rs", "pub mod m;\n"),
        ("crates/other/src/m.rs", "pub fn f() {}\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/main.rs",
            "mod helpers;\n\nuse other_crate::m::f;\n\nfn through_use() {\n    f();\n}\n\n\
             fn through_path() {\n    other_crate::m::f();\n}\n\nfn main() {}\n",
        ),
        ("crates/app/src/helpers.rs", "pub fn f() {}\n"),
    ]);
    let engine = build(&tree, &["rust"]);
    let exported = (
        vec![("f", SymbolKind::Function, "crates/other/src/m.rs")],
        0,
    );

    assert_eq!(
        callees(&engine, "crates/app/src/main.rs", "through_use"),
        exported
    );
    assert_eq!(
        callees(&engine, "crates/app/src/main.rs", "through_path"),
        exported
    );
}

#[test]
fn a_rust_call_through_a_re_export_binds_the_function_the_re_export_names() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other-crate\"\n",
        ),
        (
            "crates/other/src/lib.rs",
            "mod cycle;\nmod outer;\nmod reading;\nmod walking;\n\n\
             pub use crate::reading::{matched, unmatched as gone};\npub use outer::deep;\npub use walking::*;\n\
             pub use cycle::*;\n",
        ),
        (
            "crates/other/src/reading.rs",
            "pub fn matched() {}\n\npub fn unmatched() {}\n",
        ),
        (
            "crates/other/src/outer.rs",
            "mod inner;\n\npub use inner::deep;\n",
        ),
        ("crates/other/src/outer/inner.rs", "pub fn deep() {}\n"),
        ("crates/other/src/walking.rs", "pub fn walk_all() {}\n"),
        ("crates/other/src/cycle.rs", "pub use crate::*;\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod child;\n\nuse other_crate::matched;\n\nfn through_use() {\n    matched();\n}\n\n\
             fn through_paths() {\n    other_crate::gone();\n    other_crate::deep();\n    \
             other_crate::walk_all();\n}\n\nfn nowhere() {\n    other_crate::missing();\n}\n",
        ),
        (
            "crates/app/src/child.rs",
            "fn through_the_parent_s_private_use() {\n    super::matched();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "through_use"),
        ["crates/other/src/reading.rs"]
    );
    assert_eq!(
        bound_through_an_import(
            &engine,
            "crates/app/src/child.rs",
            "through_the_parent_s_private_use"
        ),
        ["crates/other/src/reading.rs"]
    );
    let mut through_paths =
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "through_paths");
    through_paths.sort_unstable();
    assert_eq!(
        through_paths,
        [
            "crates/other/src/outer/inner.rs",
            "crates/other/src/reading.rs",
            "crates/other/src/walking.rs",
        ]
    );
    let (mut named, unresolved) = callees(&engine, "crates/app/src/lib.rs", "through_paths");
    named.sort_unstable();
    assert_eq!(
        (named, unresolved),
        (
            vec![
                (
                    "deep",
                    SymbolKind::Function,
                    "crates/other/src/outer/inner.rs"
                ),
                (
                    "unmatched",
                    SymbolKind::Function,
                    "crates/other/src/reading.rs"
                ),
                (
                    "walk_all",
                    SymbolKind::Function,
                    "crates/other/src/walking.rs"
                ),
            ],
            0
        )
    );
    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "nowhere"),
        (vec![], 1)
    );
}

#[test]
fn a_rust_call_through_a_use_binds_the_function_beside_a_struct_of_its_name() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        ("crates/sys/Cargo.toml", "[package]\nname = \"sys\"\n"),
        (
            "crates/sys/src/lib.rs",
            "pub struct kevent {\n    pub ident: usize,\n}\n\npub struct timespec(pub i64);\n\n\
             extern \"C\" {\n    pub fn kevent(kq: i32) -> i32;\n}\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "use sys::{kevent, timespec};\n\nfn get_event() {\n    kevent(1);\n}\n\n\
             fn now() {\n    timespec(0);\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "get_event"),
        (
            vec![("kevent", SymbolKind::Function, "crates/sys/src/lib.rs")],
            0
        )
    );
    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "now"),
        (
            vec![("timespec", SymbolKind::Struct, "crates/sys/src/lib.rs")],
            0
        )
    );
}

#[test]
fn a_rust_tuple_struct_s_constructor_binds_the_struct_its_use_names_among_namesakes() {
    let own_crate = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "src/lib.rs",
            "mod m;\nmod n;\n\nuse crate::m::Wrapper;\n\nfn run() {\n    Wrapper(1);\n}\n",
        ),
        ("src/m.rs", "pub struct Wrapper(pub u32);\n"),
        ("src/n.rs", "pub struct Wrapper(pub u32);\n"),
    ]);
    let other_crate = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other_crate\"\n",
        ),
        ("crates/other/src/lib.rs", "pub mod m;\n"),
        ("crates/other/src/m.rs", "pub struct Wrapper(pub u32);\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod n;\n\nuse other_crate::m::Wrapper;\n\nfn run() {\n    Wrapper(1);\n}\n",
        ),
        ("crates/app/src/n.rs", "pub struct Wrapper(pub u32);\n"),
    ]);

    assert_eq!(
        bound_through_an_import(&build(&own_crate, &["rust"]), "src/lib.rs", "run"),
        ["src/m.rs"]
    );
    assert_eq!(
        bound_through_an_import(
            &build(&other_crate, &["rust"]),
            "crates/app/src/lib.rs",
            "run"
        ),
        ["crates/other/src/m.rs"]
    );
}

#[test]
fn a_rust_glob_binds_the_items_its_module_s_file_shows_the_caller() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other_crate\"\n",
        ),
        ("crates/other/src/lib.rs", "pub mod m;\n"),
        (
            "crates/other/src/m.rs",
            "pub fn shared() {}\n\npub fn wide() {}\n\nfn hidden() {}\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod dup;\nmod n;\nmod util;\n\nfn helper() {}\n\nfn local() {}\n\n#[cfg(test)]\nmod tests;\n",
        ),
        ("crates/app/src/dup.rs", "pub fn wide() {}\n"),
        (
            "crates/app/src/util.rs",
            "pub fn tool() {}\n\nfn own() {}\n",
        ),
        (
            "crates/app/src/n.rs",
            "pub fn helper() {}\n\npub fn shared() {}\n\npub fn hidden() {}\n\npub fn tool() {}\n\n\
             pub fn own() {}\n",
        ),
        (
            "crates/app/src/tests.rs",
            "use super::*;\nuse crate::dup::*;\nuse crate::util::*;\nuse other_crate::m::*;\n\nfn local() {}\n\n\
             #[test]\nfn t() {\n    helper();\n    tool();\n    own();\n    shared();\n    hidden();\n    local();\n\
             \x20   wide();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("crates/app/src/tests.rs")
        .expect("a language claims the file");
    let callees = graph.callees("t", Limits::new(1)).expect("t is defined");
    let bound_at = |level: u8| -> Vec<(&str, &str)> {
        (callees.reached.iter())
            .filter(|reached| reached.confidence == level)
            .map(|reached| (reached.symbol.name, reached.symbol.file))
            .collect()
    };
    let bound = bound_at(confidence::IMPORT);
    let own_file = bound_at(confidence::SAME_FILE);

    assert_eq!(
        bound,
        [
            ("helper", "crates/app/src/lib.rs"),
            ("tool", "crates/app/src/util.rs"),
            ("shared", "crates/other/src/m.rs")
        ]
    );
    assert_eq!(own_file, [("local", "crates/app/src/tests.rs")]);
}

#[test]
fn a_rust_use_through_a_module_the_file_declares_binds_that_module_s_item() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other_crate\"\n",
        ),
        ("crates/other/src/lib.rs", "pub fn shared() {}\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod m;\nmod n;\nmod spread;\n\nuse m::helper;\nuse other_crate::shared;\n\n\
             fn run() {\n    helper();\n    shared();\n}\n",
        ),
        ("crates/app/src/m.rs", "pub fn helper() {}\n"),
        (
            "crates/app/src/n.rs",
            "pub fn helper() {}\n\npub fn deep() {}\n",
        ),
        (
            "crates/app/src/spread.rs",
            "mod inner;\n\npub use inner::*;\n\nfn reach() {\n    deep();\n}\n",
        ),
        ("crates/app/src/spread/inner.rs", "pub fn deep() {}\n"),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "run"),
        ["crates/app/src/m.rs", "crates/other/src/lib.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/spread.rs", "reach"),
        ["crates/app/src/spread/inner.rs"]
    );
}

#[test]
fn a_rust_call_through_a_parameter_or_a_local_binds_nothing() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other_crate\"\n",
        ),
        (
            "crates/other/src/lib.rs",
            "pub fn helper() {}\n\npub fn run() {}\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "use other_crate::helper;\n\nfn f(helper: fn()) { helper(); }\n\
             fn g() { let helper = || {}; helper(); }\nfn h() { helper(); }\nfn k(run: impl Fn()) { run(); }\n\
             fn m() { let run = || {}; run(); }\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let lib = "crates/app/src/lib.rs";

    for through_a_value in ["f", "g", "k", "m"] {
        assert_eq!(
            callees(&engine, lib, through_a_value),
            (vec![], 0),
            "{through_a_value}"
        );
    }
    assert_eq!(
        bound_through_an_import(&engine, lib, "h"),
        ["crates/other/src/lib.rs"]
    );
}

#[test]
fn a_rust_call_on_a_field_of_self_binds_the_method_of_the_field_s_declared_type_never_the_caller_s()
{
    let tree = tree_of(&[
        ("src/lib.rs", "mod engine;\nmod index;\n"),
        (
            "src/index.rs",
            "pub struct Index;\n\nimpl Index {\n    pub fn refresh_paths(&mut self) {}\n}\n",
        ),
        (
            "src/engine.rs",
            "use crate::index::Index;\n\npub struct Engine {\n    index: Index,\n}\n\n\
             impl Engine {\n    pub fn refresh_paths(&mut self) {\n        self.index.refresh_paths();\n    }\n\n\
             \x20   pub fn walked_apart(&mut self) {\n        self.index.refresh_paths();\n    }\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    for caller in ["Engine::refresh_paths", "Engine::walked_apart"] {
        assert_eq!(
            callees(&engine, "src/engine.rs", caller).0,
            [("refresh_paths", SymbolKind::Method, "src/index.rs")],
            "{caller}"
        );
    }
}

#[test]
fn a_rust_method_named_for_its_own_type_binds_through_a_method_call() {
    let tree = tree_of(&[(
        "src/lib.rs",
        "pub struct Probe;\n\nimpl Probe {\n    pub fn Probe(&self) -> u8 {\n        0\n    }\n}\n\n\
         fn run(probe: Probe) -> u8 {\n    probe.Probe()\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "src/lib.rs", "run"),
        (vec![("Probe", SymbolKind::Method, "src/lib.rs")], 0)
    );
}

#[test]
fn a_rust_method_call_binds_no_constant_of_its_name_while_a_path_call_binds_one() {
    let tree = tree_of(&[(
        "src/lib.rs",
        "pub struct Register(pub u16);\n\npub struct Arm;\n\n\
         impl Arm {\n    pub const D8: Register = Register(264);\n}\n\n\
         pub struct Reader;\n\nimpl Reader {\n    pub fn run(&self) -> u8 {\n        self.D8(0)\n    }\n}\n\n\
         fn zero() -> u8 {\n    0\n}\n\nconst F: fn() -> u8 = zero;\n\nfn through_f() -> u8 {\n    F()\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(callees(&engine, "src/lib.rs", "run"), (vec![], 1));
    assert_eq!(
        callees(&engine, "src/lib.rs", "through_f"),
        (vec![("F", SymbolKind::Constant, "src/lib.rs")], 0)
    );
}

#[test]
fn a_rust_method_call_binds_the_one_method_of_its_name_whatever_value_a_body_declares_of_it() {
    let tree = tree_of(&[
        (
            "src/lib.rs",
            "mod tally;\nmod user;\n\nfn counted() -> usize {\n    const tally_up: usize = 1;\n    tally_up\n}\n",
        ),
        (
            "src/tally.rs",
            "pub struct Tally;\n\nimpl Tally {\n    pub fn tally_up(&self) -> usize {\n        0\n    }\n}\n",
        ),
        (
            "src/user.rs",
            "fn total() -> usize {\n    let made = make();\n    made.tally_up()\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "src/user.rs", "total").0,
        [("tally_up", SymbolKind::Method, "src/tally.rs")]
    );
}

#[test]
fn a_rust_call_through_a_type_binds_its_wasm_bindgen_static_method_or_constructor() {
    let tree = tree_of(&[(
        "lib.rs",
        "#[wasm_bindgen]\nextern \"C\" {\n    pub type Number;\n    pub type Array;\n\n\
         \x20   #[wasm_bindgen(static_method_of = Number, js_name = isNaN)]\n\
         \x20   pub fn is_nan(value: &JsValue) -> bool;\n\n\
         \x20   #[wasm_bindgen(constructor)]\n    pub fn new() -> Array;\n}\n\n\
         fn check(value: &JsValue) -> bool {\n    Number::is_nan(value)\n}\n\n\
         fn make() -> Array {\n    Array::new()\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "lib.rs", "check"),
        (vec![("is_nan", SymbolKind::Method, "lib.rs")], 0)
    );
    assert_eq!(
        callees(&engine, "lib.rs", "make"),
        (vec![("new", SymbolKind::Method, "lib.rs")], 0)
    );
}

fn bound_through_an_import<'e>(engine: &'e Engine, path: &str, name: &str) -> Vec<&'e str> {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .filter(|reached| reached.confidence == confidence::IMPORT)
        .map(|reached| reached.symbol.file)
        .collect()
}

fn renaming_crate() -> TempDir {
    tree_of(&[
        ("Cargo.toml", "[package]\nname = \"fx\"\n"),
        (
            "src/lib.rs",
            "mod m;\nmod other;\nmod plain;\npub use m::f as g;\n\npub fn in_the_root() {\n    g();\n}\n",
        ),
        ("src/m.rs", "pub fn f() {}\n"),
        (
            "src/other.rs",
            "use crate::m as module_alias;\nuse crate::m::f as local_alias;\n\n\
             pub fn through_crate() {\n    crate::g();\n}\n\n\
             pub fn through_alias() {\n    local_alias();\n}\n\n\
             pub fn through_module_alias() {\n    module_alias::f();\n}\n",
        ),
        (
            "src/plain.rs",
            "use crate::m::f;\n\npub fn through_plain() {\n    f();\n}\n",
        ),
    ])
}

#[test]
fn a_rust_call_of_a_name_a_use_renames_binds_the_item_the_use_names() {
    let tree = renaming_crate();
    let engine = build(&tree, &["rust"]);
    let renamed = ["src/m.rs"];

    for (path, caller) in [
        ("src/other.rs", "through_alias"),
        ("src/other.rs", "through_crate"),
        ("src/other.rs", "through_module_alias"),
        ("src/plain.rs", "through_plain"),
        ("src/lib.rs", "in_the_root"),
    ] {
        assert_eq!(
            bound_through_an_import(&engine, path, caller),
            renamed,
            "{caller} in {path}"
        );
    }
}

#[test]
fn a_rust_use_that_renames_a_module_spells_no_function_of_the_module_s_name() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"fx\"\n"),
        ("src/lib.rs", "mod apart;\nmod m;\nmod user;\n"),
        ("src/apart.rs", "pub fn m() {}\n"),
        ("src/m.rs", "pub fn f() {}\n"),
        (
            "src/user.rs",
            "use crate::m as renamed;\n\npub fn through_it() {\n    renamed::f();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine.graph_of("src/m.rs").expect("rust claims the file");

    let dead = graph.dead_code(&[], None, 10);

    let apart = (dead.found.iter())
        .find(|found| found.definition.name == "m")
        .expect("no call reaches the function");
    assert!(!apart.spelled);
}

#[test]
fn a_rust_item_lists_the_calls_of_each_name_a_use_renames_it_to() {
    let tree = renaming_crate();
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine.graph_of("src/m.rs").expect("rust claims the file");

    let callers = graph
        .callers("f", Limits::new(1))
        .expect("the name is defined");

    let mut named: Vec<&str> = (callers.reached.iter())
        .map(|reached| reached.symbol.name)
        .collect();
    named.sort_unstable();
    assert_eq!(
        named,
        [
            "in_the_root",
            "through_alias",
            "through_crate",
            "through_module_alias",
            "through_plain"
        ]
    );
}

#[test]
fn a_rust_call_through_a_type_a_use_renames_binds_the_member_of_the_type_the_use_names() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"fx\"\n"),
        ("src/lib.rs", "mod error;\nmod parser;\n"),
        (
            "src/error.rs",
            "pub struct Error;\n\nimpl Error {\n    pub fn make() -> Error {\n        Error\n    }\n\n    \
             pub fn unknown(&self) {}\n\n    pub fn other(&self) -> Error {\n        Error\n    }\n}\n",
        ),
        (
            "src/parser.rs",
            "use crate::error::Error as ClapError;\nuse outside::Error as OutsideError;\n\
             use std::fmt::Error as FmtError;\n\npub struct Near;\n\nimpl Near {\n    \
             pub fn other(&self) -> Near {\n        Near\n    }\n\n    pub fn unknown(&self) {}\n}\n\n\
             pub struct Parser;\n\nimpl Parser {\n    \
             pub fn error(&self) -> ClapError {\n        ClapError::make()\n    }\n\n    \
             pub fn maybe(&self) -> Option<ClapError> {\n        None\n    }\n}\n\n\
             pub fn through_the_type() {\n    ClapError::make();\n}\n\n\
             pub fn on_a_path_of_calls(error: &ClapError) {\n    error.other().unknown();\n}\n\n\
             pub fn on_what_a_result_holds(parser: &Parser) {\n    parser.maybe().unwrap().unknown();\n}\n\n\
             pub fn on_a_parameter(error: &ClapError) {\n    error.unknown();\n}\n\n\
             pub fn on_a_local() {\n    let error = ClapError::make();\n    error.unknown();\n}\n\n\
             pub fn on_a_result(parser: &Parser) {\n    parser.error().unknown();\n}\n\n\
             pub fn through_types_outside() {\n    FmtError::make();\n    OutsideError::make();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let member = |name| (name, SymbolKind::Method, "src/error.rs");

    assert_eq!(
        callees(&engine, "src/parser.rs", "through_the_type"),
        (vec![member("make")], 0)
    );
    assert_eq!(
        callees(&engine, "src/parser.rs", "on_a_parameter"),
        (vec![member("unknown")], 0)
    );
    assert_eq!(
        callees(&engine, "src/parser.rs", "on_a_local").0,
        [member("make"), member("unknown")]
    );
    assert_eq!(
        callees(&engine, "src/parser.rs", "on_a_result").0,
        [
            member("unknown"),
            ("error", SymbolKind::Method, "src/parser.rs")
        ]
    );
    assert_eq!(
        callees(&engine, "src/parser.rs", "on_a_path_of_calls").0,
        [member("unknown"), member("other")]
    );
    assert_eq!(
        callees(&engine, "src/parser.rs", "on_what_a_result_holds").0,
        [
            member("unknown"),
            ("maybe", SymbolKind::Method, "src/parser.rs")
        ]
    );
    assert_eq!(
        callees(&engine, "src/parser.rs", "through_types_outside"),
        (vec![], 2)
    );
}

#[test]
fn a_rust_rename_stays_as_spelled_where_its_file_declares_the_type_s_name_or_impls_the_rename() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"fx\"\n"),
        ("src/lib.rs", "mod error;\nmod extended;\nmod wrapper;\n"),
        (
            "src/error.rs",
            "pub struct Error;\n\nimpl Error {\n    pub fn extra() {}\n\n    pub fn report(&self) {}\n}\n",
        ),
        (
            "src/wrapper.rs",
            "use crate::error::Error as Inner;\n\npub struct Error {\n    inner: Inner,\n}\n\nimpl Error {\n    \
             pub fn report(&self) {\n        self.inner.report();\n    }\n}\n",
        ),
        (
            "src/extended.rs",
            "use crate::error::Error as Extended;\n\nimpl Extended {\n    pub fn extra() {}\n}\n\n\
             pub fn through_the_rename() {\n    Extended::extra();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    let (_, graph) = engine
        .graph_of("src/wrapper.rs")
        .expect("rust claims the file");
    let own = Selector::new("Error::report", Some("src/wrapper.rs"));
    let wrapper = graph
        .callees(own, Limits::new(1))
        .expect("the name is defined");
    assert!(wrapper.reached.is_empty() && wrapper.self_call.is_none());
    assert_eq!(wrapper.unresolved, 1);
    assert_eq!(
        callees(&engine, "src/extended.rs", "through_the_rename"),
        (vec![("extra", SymbolKind::Method, "src/extended.rs")], 0)
    );
}

#[test]
fn a_rust_path_through_crate_self_or_super_binds_the_item_of_the_module_it_names() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ("src/lib.rs", "mod a;\nmod b;\n\npub fn root() {}\n"),
        (
            "src/a.rs",
            "mod c;\n\npub fn f() {}\n\npub fn g() {}\n\n\
             fn through_paths() {\n    crate::b::g();\n    super::root();\n}\n",
        ),
        (
            "src/a/c.rs",
            "pub fn root() {}\n\nfn up() {\n    super::f();\n}\n",
        ),
        (
            "src/b.rs",
            "use crate::a;\nuse crate::a::f;\n\npub fn g() {}\n\n\
             fn through_use() {\n    f();\n    a::g();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "src/a.rs", "through_paths"),
        ["src/b.rs", "src/lib.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "src/a/c.rs", "up"),
        ["src/a.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "src/b.rs", "through_use"),
        ["src/a.rs", "src/a.rs"]
    );
}

#[test]
fn a_rust_path_call_ending_at_a_name_the_file_declares_binds_the_item_the_path_names() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ("src/lib.rs", "mod a;\nmod b;\n\npub fn new() {}\n"),
        (
            "src/a.rs",
            "pub fn new() {\n    super::new();\n}\n\nfn own() {\n    new();\n}\n",
        ),
        ("src/b.rs", "pub fn new() {\n    crate::new();\n}\n"),
    ]);
    let engine = build(&tree, &["rust"]);
    let bound = |path: &str, name: &str| {
        let (_, graph) = engine.graph_of(path).expect("a language claims the file");
        let callees = graph
            .callees(Selector::new(name, Some(path)), Limits::new(1))
            .expect("the name is defined");
        (callees.reached.iter())
            .map(|reached| (reached.symbol.file, reached.symbol.line, reached.confidence))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        bound("src/a.rs", "new"),
        [("src/lib.rs", 4, confidence::IMPORT)]
    );
    assert_eq!(
        bound("src/b.rs", "new"),
        [("src/lib.rs", 4, confidence::IMPORT)]
    );
    assert_eq!(
        bound("src/a.rs", "own"),
        [("src/a.rs", 1, confidence::SAME_FILE)]
    );
}

fn reached<'e>(engine: &'e Engine, path: &str, name: &str) -> Vec<(&'e str, u32, u8)> {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    let callees = graph
        .callees(Selector::new(name, Some(path)), Limits::new(1))
        .expect("the name is defined");
    (callees.reached.iter())
        .map(|reached| (reached.symbol.file, reached.symbol.line, reached.confidence))
        .collect()
}

#[test]
fn a_rust_path_call_through_a_module_the_file_declares_binds_that_module_s_item() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "src/lib.rs",
            "mod alpha;\nmod beta;\nmod inline {\n    pub fn run() {}\n}\nmod other {\n    pub fn run() {}\n}\n\n\
             pub fn go() {\n    alpha::run();\n}\n\npub fn stay() {\n    inline::run();\n}\n",
        ),
        ("src/alpha.rs", "pub fn run() {}\n"),
        ("src/beta.rs", "pub fn run() {}\n"),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        reached(&engine, "src/lib.rs", "go"),
        [("src/alpha.rs", 1, confidence::IMPORT)]
    );
    assert_eq!(
        reached(&engine, "src/lib.rs", "stay"),
        [("src/lib.rs", 4, confidence::SAME_FILE)]
    );
}

#[test]
fn a_rust_path_call_through_a_module_binds_no_item_of_the_caller_s_file_its_path_does_not_name() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "src/lib.rs",
            "mod m;\nmod other {\n    pub fn shared() {}\n}\nmod inline {\n    pub fn shared() {}\n}\n\n\
             pub struct Php;\n\nimpl Php {\n    pub fn extract(&self) {}\n\n    pub fn parse(&self) {\n\
             \x20       tree::extract();\n    }\n}\n\npub fn shared() {}\n\n\
             pub fn run() {\n    outside::shared();\n    m::shared();\n    tree::extract();\n    mem::take();\n}\n\n\
             pub fn stay() {\n    inline::shared();\n}\n",
        ),
        (
            "src/m.rs",
            "pub fn other() {}\n\npub struct Held;\n\nimpl Held {\n    pub fn take(&self) {}\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(reached(&engine, "src/lib.rs", "Php::parse"), []);
    assert_eq!(reached(&engine, "src/lib.rs", "run"), []);
    assert_eq!(
        reached(&engine, "src/lib.rs", "stay"),
        [("src/lib.rs", 6, confidence::SAME_FILE)]
    );
}

#[test]
fn a_rust_path_within_the_crate_binds_a_private_item_only_from_its_module_or_below() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ("src/lib.rs", "mod a;\nmod b;\n\nfn hidden() {}\n"),
        (
            "src/a.rs",
            "mod c;\nmod d;\n#[cfg(test)]\nmod tests;\n\nfn helper() {}\n\n#[cfg(test)]\nfn fixture() {}\n\n\
             fn up() {\n    super::hidden();\n}\n\nmacro_rules! tool {\n    () => {};\n}\n\nfn tool() {}\n\n\
             macro_rules! quiet {\n    () => {};\n}\n",
        ),
        (
            "src/a/d.rs",
            "fn tools() {\n    super::tool();\n    super::quiet!();\n}\n",
        ),
        (
            "src/a/c.rs",
            "fn below() {\n    super::helper();\n    super::fixture();\n    crate::hidden();\n}\n",
        ),
        (
            "src/a/tests.rs",
            "use super::helper;\n\nfn check() {\n    helper();\n    super::fixture();\n}\n",
        ),
        ("src/b.rs", "fn beside() {\n    crate::a::helper();\n}\n"),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "src/a.rs", "up"),
        ["src/lib.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "src/a/c.rs", "below"),
        ["src/a.rs", "src/lib.rs"]
    );
    assert_eq!(
        callees(&engine, "src/a/d.rs", "tools"),
        (
            vec![
                ("tool", SymbolKind::Function, "src/a.rs"),
                ("quiet", SymbolKind::Macro, "src/a.rs")
            ],
            0
        )
    );
    assert_eq!(
        bound_through_an_import(&engine, "src/a/d.rs", "tools"),
        ["src/a.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "src/a/tests.rs", "check"),
        ["src/a.rs", "src/a.rs"]
    );
    assert_eq!(
        callees(&engine, "src/a/tests.rs", "check"),
        (
            vec![
                ("helper", SymbolKind::Function, "src/a.rs"),
                ("fixture", SymbolKind::Function, "src/a.rs")
            ],
            0
        )
    );
    assert_eq!(
        bound_through_an_import(&engine, "src/b.rs", "beside"),
        Vec::<&str>::new()
    );
}

#[test]
fn a_rust_test_item_binds_a_call_from_its_crate_s_tests_outside_its_module_and_none_from_other_code()
 {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ("src/lib.rs", "mod a;\n#[cfg(test)]\nmod tests;\n"),
        ("src/tests.rs", "mod case;\nmod helpers;\n"),
        (
            "src/tests/helpers.rs",
            "pub fn fixture() {}\n\nfn hidden() {}\n",
        ),
        (
            "src/tests/case.rs",
            "use crate::tests::helpers::fixture;\n\nfn check() {\n    fixture();\n    \
             super::helpers::hidden();\n}\n",
        ),
        (
            "src/a.rs",
            "fn product() {\n    crate::tests::helpers::fixture();\n}\n",
        ),
        ("tests/it/main.rs", "mod case;\nmod helpers;\n"),
        ("tests/it/helpers.rs", "pub(crate) fn graph_of() {}\n"),
        (
            "tests/it/case.rs",
            "use crate::helpers::graph_of;\n\nfn sibling() {\n    graph_of();\n}\n",
        ),
        (
            "tests/outside.rs",
            "fn outside() {\n    app::tests::helpers::fixture();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "src/tests/case.rs", "check"),
        ["src/tests/helpers.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "tests/it/case.rs", "sibling"),
        ["tests/it/helpers.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "src/a.rs", "product"),
        Vec::<&str>::new()
    );
    assert_eq!(
        bound_through_an_import(&engine, "tests/outside.rs", "outside"),
        Vec::<&str>::new()
    );
}

#[test]
fn a_rust_library_root_its_manifest_names_imports_its_modules_and_its_tests_bind_their_helpers() {
    let tree = tree_of(&[
        (
            "Cargo.toml",
            "[package]\nname = \"cli\"\n\n[lib]\npath = \"src/cli.rs\"\n",
        ),
        ("src/cli.rs", "pub mod fuzz;\n\n#[cfg(test)]\nmod tests;\n"),
        ("src/fuzz.rs", "pub fn run() {}\n"),
        ("src/tests.rs", "mod case;\nmod helpers;\n"),
        ("src/tests/helpers.rs", "pub fn get_language() {}\n"),
        (
            "src/tests/case.rs",
            "use crate::tests::helpers::get_language;\n\nfn check() {\n    get_language();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("src/fuzz.rs")
        .expect("rust claims the file");
    let importers: Vec<&str> = (graph.dependents("src/fuzz.rs", 1, 0))
        .expect("the graph holds the file")
        .found
        .iter()
        .map(|dependent| dependent.file)
        .collect();

    assert_eq!(importers, ["src/cli.rs"]);
    assert_eq!(
        bound_through_an_import(&engine, "src/tests/case.rs", "check"),
        ["src/tests/helpers.rs"]
    );
}

#[test]
fn a_rust_use_inside_a_function_binds_no_call_outside_its_block() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other-crate\"\n",
        ),
        ("crates/other/src/lib.rs", "pub mod m;\n"),
        ("crates/other/src/m.rs", "pub fn f() {}\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod beside;\n\nfn scoped() {\n    use other_crate::m;\n    m::f();\n}\n",
        ),
        (
            "crates/app/src/beside.rs",
            "fn importing() {\n    use other_crate::m;\n}\n\nfn unscoped() {\n    m::f();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "scoped"),
        ["crates/other/src/m.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/beside.rs", "unscoped"),
        Vec::<&str>::new()
    );
}

#[test]
fn a_rust_use_inside_an_inline_module_binds_no_call_outside_the_module() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other-crate\"\n",
        ),
        ("crates/other/src/lib.rs", "pub mod m;\n"),
        ("crates/other/src/m.rs", "pub fn f() {}\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod beside;\n\nmod tests {\n    use other_crate::m;\n\n    fn inside() {\n        m::f();\n    }\n}\n",
        ),
        (
            "crates/app/src/beside.rs",
            "mod tests {\n    use other_crate::m;\n}\n\nfn outside() {\n    m::f();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "inside"),
        ["crates/other/src/m.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/beside.rs", "outside"),
        Vec::<&str>::new()
    );
}

#[test]
fn a_rust_macro_invocation_through_another_crate_binds_the_macro_it_exports_at_its_root() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other-crate\"\n",
        ),
        (
            "crates/other/src/lib.rs",
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n\nmacro_rules! hidden { () => {}; }\n",
        ),
        ("crates/third/Cargo.toml", "[package]\nname = \"third\"\n"),
        (
            "crates/third/src/lib.rs",
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n\n#[macro_export]\nmacro_rules! hidden { () => {}; }\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod imported;\n\nfn by_path() {\n    other_crate::mac!();\n}\n\n\
             fn hidden_by_path() {\n    other_crate::hidden!();\n}\n",
        ),
        (
            "crates/app/src/imported.rs",
            "use other_crate::mac;\n\nfn by_use() {\n    mac!();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "by_path"),
        ["crates/other/src/lib.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/imported.rs", "by_use"),
        ["crates/other/src/lib.rs"]
    );
    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "hidden_by_path"),
        Vec::<&str>::new()
    );
}

/// A workspace whose crate `other-crate` is made of `modules`, and whose crate `app` invokes `other_crate::mac!()` in
/// `by_path` and `mac!()` through a `use` in `by_use`.
fn invoking_a_macro_of(modules: &[(&str, &str)]) -> Engine {
    let mut files = vec![
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/other/Cargo.toml",
            "[package]\nname = \"other-crate\"\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "mod imported;\n\nfn by_path() {\n    other_crate::mac!();\n}\n",
        ),
        (
            "crates/app/src/imported.rs",
            "use other_crate::mac;\n\nfn by_use() {\n    mac!();\n}\n",
        ),
    ];
    files.extend_from_slice(modules);
    build(&tree_of(&files), &["rust"])
}

fn macro_bound_through_an_import(engine: &Engine) -> [Vec<&str>; 2] {
    [
        bound_through_an_import(engine, "crates/app/src/lib.rs", "by_path"),
        bound_through_an_import(engine, "crates/app/src/imported.rs", "by_use"),
    ]
}

#[test]
fn a_rust_macro_invocation_through_another_crate_binds_the_macro_a_submodule_exports() {
    let engine = invoking_a_macro_of(&[
        ("crates/other/src/lib.rs", "mod tree;\nmod hidden;\n"),
        (
            "crates/other/src/tree.rs",
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n",
        ),
        (
            "crates/other/src/hidden.rs",
            "macro_rules! mac { () => {}; }\n",
        ),
    ]);

    assert_eq!(
        macro_bound_through_an_import(&engine),
        [["crates/other/src/tree.rs"], ["crates/other/src/tree.rs"]]
    );
}

#[test]
fn a_rust_macro_invocation_through_another_crate_binds_the_macro_a_nested_submodule_exports() {
    let engine = invoking_a_macro_of(&[
        ("crates/other/src/lib.rs", "pub mod a;\n"),
        ("crates/other/src/a.rs", "mod b;\n"),
        (
            "crates/other/src/a/b.rs",
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n",
        ),
    ]);

    assert_eq!(
        macro_bound_through_an_import(&engine),
        [["crates/other/src/a/b.rs"], ["crates/other/src/a/b.rs"]]
    );
}

#[test]
fn a_rust_macro_two_submodules_export_binds_neither_through_the_import() {
    let engine = invoking_a_macro_of(&[
        (
            "crates/other/src/lib.rs",
            "#[cfg(unix)]\nmod unix;\n#[cfg(windows)]\nmod windows;\n",
        ),
        (
            "crates/other/src/unix.rs",
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n\n#[macro_export]\nmacro_rules! only { () => {}; }\n",
        ),
        (
            "crates/other/src/windows.rs",
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n",
        ),
        (
            "crates/app/src/unique.rs",
            "fn unique() {\n    other_crate::only!();\n}\n",
        ),
    ]);

    assert_eq!(
        macro_bound_through_an_import(&engine),
        [Vec::<&str>::new(), Vec::new()]
    );
    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/unique.rs", "unique"),
        ["crates/other/src/unix.rs"]
    );
}

/// What binds through an import from `by_path`, `by_use` and `by_call`, which calls `other_crate::mac()` as a function,
/// where `other-crate` is made of `modules`.
fn called_and_invoked_through_an_import(modules: &[(&str, &str)]) -> [Vec<String>; 3] {
    let mut files = modules.to_vec();
    files.push((
        "crates/app/src/called.rs",
        "fn by_call() {\n    other_crate::mac();\n}\n",
    ));
    let engine = invoking_a_macro_of(&files);
    let [by_path, by_use] = macro_bound_through_an_import(&engine);
    let by_call = bound_through_an_import(&engine, "crates/app/src/called.rs", "by_call");
    [by_path, by_use, by_call].map(|files| files.into_iter().map(str::to_owned).collect())
}

/// `bound` as the files of what a macro invocation binds twice, then what a call binds.
fn invoked_and_called(invoked: &[&str], called: &[&str]) -> [Vec<String>; 3] {
    let owned = |files: &[&str]| files.iter().map(|&file| file.to_owned()).collect();
    [owned(invoked), owned(invoked), owned(called)]
}

#[test]
fn a_rust_name_the_root_file_exports_as_a_function_and_a_macro_binds_each_by_the_call_s_form() {
    let root = "crates/other/src/lib.rs";
    let engine = invoking_a_macro_of(&[
        (
            root,
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n\npub fn mac() {}\n",
        ),
        (
            "crates/app/src/called.rs",
            "fn by_call() {\n    other_crate::mac();\n}\n",
        ),
    ]);
    let bound = |path, name| {
        let (reached, _) = callees(&engine, path, name);
        reached
    };

    assert_eq!(
        bound("crates/app/src/lib.rs", "by_path"),
        [("mac", SymbolKind::Macro, root)]
    );
    assert_eq!(
        bound("crates/app/src/imported.rs", "by_use"),
        [("mac", SymbolKind::Macro, root)]
    );
    assert_eq!(
        bound("crates/app/src/called.rs", "by_call"),
        [("mac", SymbolKind::Function, root)]
    );
}

#[test]
fn a_rust_name_a_submodule_exports_as_a_function_binds_the_macro_only_for_an_invocation() {
    let macros = "crates/other/src/macros.rs";
    for (re_export, called) in [
        ("pub use inner::mac;\n", &["crates/other/src/inner.rs"][..]),
        (
            "pub use crate::inner::mac;\n",
            &["crates/other/src/inner.rs"],
        ),
        ("", &[]),
    ] {
        let bound = called_and_invoked_through_an_import(&[
            (
                "crates/other/src/lib.rs",
                &format!("mod inner;\nmod macros;\n{re_export}"),
            ),
            ("crates/other/src/inner.rs", "pub fn mac() {}\n"),
            (macros, "#[macro_export]\nmacro_rules! mac { () => {}; }\n"),
        ]);

        assert_eq!(
            bound,
            invoked_and_called(&[macros], called),
            "re-exported by {re_export:?}"
        );
    }
}

#[test]
fn a_rust_name_the_root_file_re_exports_from_another_crate_binds_the_macro_only_for_an_invocation()
{
    let macros = "crates/other/src/macros.rs";
    let macro_file = (macros, "#[macro_export]\nmacro_rules! mac { () => {}; }\n");
    let bound = called_and_invoked_through_an_import(&[
        (
            "crates/other/src/lib.rs",
            "mod macros;\npub use dep::mac;\n",
        ),
        macro_file,
        ("crates/dep/Cargo.toml", "[package]\nname = \"dep\"\n"),
        ("crates/dep/src/lib.rs", "pub fn mac() {}\n"),
    ]);
    let outside = called_and_invoked_through_an_import(&[
        (
            "crates/other/src/lib.rs",
            "mod macros;\npub use outside_dep::mac;\n",
        ),
        macro_file,
    ]);

    assert_eq!(
        bound,
        invoked_and_called(&[macros], &["crates/dep/src/lib.rs"])
    );
    assert_eq!(outside, invoked_and_called(&[macros], &[]));
}

#[test]
fn a_rust_macro_a_submodule_exports_binds_an_invocation_beside_a_private_function_of_the_root() {
    let macros = "crates/other/src/macros.rs";
    let bound = called_and_invoked_through_an_import(&[
        ("crates/other/src/lib.rs", "mod macros;\n\nfn mac() {}\n"),
        (macros, "#[macro_export]\nmacro_rules! mac { () => {}; }\n"),
    ]);

    assert_eq!(bound, invoked_and_called(&[macros], &[]));
}

#[test]
fn a_rust_call_binds_no_macro_and_a_macro_invocation_no_function_of_its_name() {
    let tree = tree_of(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "src/lib.rs",
            "mod macros;\n\nfn ready(x: u8) -> u8 {\n    x\n}\n\n\
             fn run(x: u8, context: Context) {\n    derive(x);\n    ready!(x);\n    context.options();\n}\n",
        ),
        (
            "src/macros.rs",
            "macro_rules! derive { ($x:expr) => { $x }; }\n\nmacro_rules! options { () => {}; }\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(callees(&engine, "src/lib.rs", "run"), (vec![], 3));
}

#[test]
fn a_rust_name_crowded_past_the_candidate_index_binds_each_call_form_apart() {
    let modules: Vec<(String, &str)> = (0..64)
        .map(|n| {
            (
                format!("src/m{n}.rs"),
                "pub fn ready(x: u8) -> u8 {\n    x\n}\n",
            )
        })
        .collect();
    let declarations = (0..64)
        .map(|n| format!("mod m{n};"))
        .collect::<Vec<_>>()
        .join("\n");
    let lib = format!(
        "{declarations}\nmod macros;\n\nfn macro_first(x: u8) {{\n    ready!(x);\n    ready(x);\n}}\n\n\
         fn call_first(x: u8) {{\n    ready(x);\n    ready!(x);\n}}\n"
    );
    let mut files = vec![
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ("src/lib.rs", lib.as_str()),
        (
            "src/macros.rs",
            "macro_rules! ready { ($x:expr) => { $x }; }\n",
        ),
    ];
    files.extend(
        modules
            .iter()
            .map(|(path, source)| (path.as_str(), *source)),
    );
    let tree = tree_of(&files);
    let engine = build(&tree, &["rust"]);

    let ready = (vec![("ready", SymbolKind::Macro, "src/macros.rs")], 1);
    assert_eq!(callees(&engine, "src/lib.rs", "macro_first"), ready);
    assert_eq!(callees(&engine, "src/lib.rs", "call_first"), ready);
}

#[test]
fn a_rust_macro_invocation_binds_by_its_name_alone_only_a_macro_of_the_caller_s_package() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        ("crates/log/Cargo.toml", "[package]\nname = \"log\"\n"),
        (
            "crates/log/src/lib.rs",
            "#[macro_use]\nmod macros;\nmod run;\ncfg_unix! {\n    mod unix;\n}\n",
        ),
        (
            "crates/log/src/macros.rs",
            "macro_rules! trace { () => {}; }\n",
        ),
        ("crates/log/src/run.rs", "fn inside() {\n    trace!();\n}\n"),
        (
            "crates/log/src/unix.rs",
            "fn hidden() {\n    trace!();\n}\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "fn outside() {\n    trace!();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    let trace = (
        vec![("trace", SymbolKind::Macro, "crates/log/src/macros.rs")],
        0,
    );
    assert_eq!(callees(&engine, "crates/log/src/run.rs", "inside"), trace);
    assert_eq!(callees(&engine, "crates/log/src/unix.rs", "hidden"), trace);
    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "outside"),
        (vec![], 1)
    );
}

#[test]
fn a_rust_macro_invocation_through_a_use_binds_the_proc_macro_function_it_names() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        ("crates/indoc/Cargo.toml", "[package]\nname = \"indoc\"\n"),
        (
            "crates/indoc/src/lib.rs",
            "#[proc_macro]\npub fn indoc(input: TokenStream) -> TokenStream {\n    input\n}\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "use indoc::indoc;\n\nfn text() {\n    indoc!(\"a\");\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        bound_through_an_import(&engine, "crates/app/src/lib.rs", "text"),
        ["crates/indoc/src/lib.rs"]
    );
}

#[test]
fn a_rust_macro_a_submodule_exports_binds_beside_re_exports_of_other_names_and_a_private_namesake()
{
    let bound = called_and_invoked_through_an_import(&[
        (
            "crates/other/src/lib.rs",
            "pub mod tree;\nmod banner;\n\npub use crate::banner::Banner;\npub use dep::Other;\n",
        ),
        (
            "crates/other/src/tree.rs",
            "#[macro_export]\nmacro_rules! mac { () => {}; }\n\npub fn line() {}\n",
        ),
        (
            "crates/other/src/banner.rs",
            "pub struct Banner;\n\nfn mac() {}\n",
        ),
    ]);

    assert_eq!(
        bound,
        invoked_and_called(&["crates/other/src/tree.rs"], &[])
    );
}

#[test]
fn a_rust_method_call_on_a_value_named_like_an_imported_module_binds_no_function_of_the_module() {
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        ("crates/nix/Cargo.toml", "[package]\nname = \"nix\"\n"),
        ("crates/nix/src/lib.rs", "pub mod socket;\n"),
        ("crates/nix/src/socket.rs", "pub fn connect() {}\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "fn socketpair() {\n    use nix::socket::{self};\n    socket::connect();\n}\n\n\
             fn timestamping(socket: UdpSocket) {\n    socket.connect();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "socketpair"),
        (
            vec![("connect", SymbolKind::Function, "crates/nix/src/socket.rs")],
            0
        )
    );
    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "timestamping"),
        (vec![], 1)
    );
}

#[test]
fn a_rust_call_on_a_value_self_or_a_type_binds_no_free_function_of_its_name() {
    let tree = tree_of(&[(
        "lib.rs",
        "struct Index;\n\nimpl Index {\n    fn scan(&self) {\n        self.refresh_paths();\n        \
         Index::refresh_paths(self);\n    }\n}\n\nfn refresh(index: &Index) {\n    index.refresh_paths();\n}\n\n\
         fn refresh_paths(_index: &Index) {}\n\nstruct Ready(u32);\n\nenum Poll {\n    Ready(u32),\n}\n\n\
         fn poll() -> Poll {\n    Poll::Ready(1)\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(callees(&engine, "lib.rs", "poll"), (vec![], 1));
    assert_eq!(callees(&engine, "lib.rs", "scan"), (vec![], 2));
    assert_eq!(callees(&engine, "lib.rs", "refresh"), (vec![], 1));
}

#[test]
fn a_rust_call_outside_an_inline_module_binds_no_item_of_the_module_and_one_inside_binds_the_module_s_own()
 {
    let tree = tree_of(&[
        (
            "lib.rs",
            "mod rngs;\n\npub use crate::rngs::rng;\n\npub fn random() -> u64 {\n    rng()\n}\n\n\
             mod test {\n    pub fn rng() -> u64 {\n        0\n    }\n\n    fn inside() -> u64 {\n        rng()\n    \
             }\n}\n",
        ),
        ("rngs.rs", "pub fn rng() -> u64 {\n    4\n}\n"),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "lib.rs", "random"),
        (vec![("rng", SymbolKind::Function, "rngs.rs")], 0)
    );
    assert_eq!(
        callees(&engine, "lib.rs", "inside"),
        (vec![("rng", SymbolKind::Function, "lib.rs")], 0)
    );
}

#[test]
fn a_rust_call_at_file_scope_binds_the_file_s_top_level_item_over_an_inline_module_s_namesake() {
    let tree = tree_of(&[(
        "lib.rs",
        "mod other {\n    pub fn shared() {}\n\n    pub fn within() {\n        shared();\n    }\n}\n\n\
         pub fn shared() {}\n\npub fn caller() {\n    self::shared();\n    shared();\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("lib.rs")
        .expect("a language claims the file");
    let lines = |caller: &str| -> Vec<u32> {
        (graph.callees(caller, Limits::new(1)))
            .expect("the name is defined")
            .reached
            .iter()
            .map(|reached| reached.symbol.line)
            .collect()
    };

    assert_eq!(lines("caller"), [9]);
    assert_eq!(lines("within"), [2]);
}

#[test]
fn a_rust_method_call_outside_an_inline_module_binds_no_member_of_a_private_type_of_the_module() {
    let tree = tree_of(&[(
        "lib.rs",
        "use std::thread;\n\npub fn follow() {\n    thread::Builder::new().name(\"watch\".to_owned());\n}\n\n\
         mod tests {\n    struct WritesMidWalk;\n\n    impl WritesMidWalk {\n        \
         fn name(&self) -> &str {\n            \"writes\"\n        }\n    }\n}\n\npub struct Label;\n\n\
         impl Label {\n    pub fn name(&self) -> &str {\n        \"label\"\n    }\n}\n\n\
         pub fn label(labels: &[Label]) {\n    labels[0].name();\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("lib.rs")
        .expect("a language claims the file");

    let same_file = (graph.callees("follow", Limits::new(1)))
        .expect("the name is defined")
        .reached
        .iter()
        .any(|reached| reached.confidence == confidence::SAME_FILE);
    assert!(!same_file);
    assert_eq!(callees(&engine, "lib.rs", "label"), (vec![], 1));
}

#[test]
fn a_rust_call_binds_the_function_its_file_declares_beside_a_module_of_its_name() {
    let tree = tree_of(&[(
        "lib.rs",
        "mod text;\n\nfn text() {}\n\nfn draw() {\n    text();\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "lib.rs", "draw"),
        (vec![("text", SymbolKind::Function, "lib.rs")], 0)
    );
}

#[test]
fn a_rust_call_on_a_value_binds_the_runtime_method_whose_owner_its_name_spells() {
    let tree = tree_of(&[
        (
            "fs/src/watch.rs",
            "pub struct Watcher;\n\nimpl Watcher {\n    pub fn drain(&mut self) {}\n}\n",
        ),
        (
            "app/src/follow.rs",
            "fn caught_up(watcher: &mut Watcher) {\n    watcher.drain();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "app/src/follow.rs", "caught_up"),
        (vec![("drain", SymbolKind::Method, "fs/src/watch.rs")], 0)
    );
}

#[test]
fn a_rust_call_of_a_standard_library_method_on_an_untyped_value_binds_a_declared_module_s_method_as_a_guess()
 {
    let tree = tree_of(&[
        ("src/lib.rs", "mod pointer;\n"),
        (
            "src/pointer/mod.rs",
            "mod ptr;\n\npub use ptr::Ptr;\n\npub struct Inner;\n\nimpl Inner {\n    \
             pub fn as_ptr(&self) -> *mut u8 {\n        core::ptr::null_mut()\n    }\n}\n\n\
             fn project(src: Inner, raw: *const u8) {\n        src.as_ptr().cast::<u16>();\n        \
             raw.cast::<u16>();\n}\n\nfn through(ptr: Ptr) {\n    ptr.cast::<u16>();\n}\n",
        ),
        (
            "src/pointer/ptr.rs",
            "pub struct Ptr;\n\nimpl Ptr {\n    pub fn cast<U>(self) -> Ptr {\n        self\n    }\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("src/pointer/ptr.rs")
        .expect("a language claims the file");
    let walk = graph
        .callers("Ptr::cast", Limits::new(1))
        .expect("the name is defined");
    let callers = (walk.reached.iter())
        .map(|reached| (reached.symbol.name, reached.confidence))
        .collect::<Vec<_>>();

    assert_eq!(
        callers,
        [
            ("project", confidence::FIRST_PARTY),
            ("through", confidence::RECEIVER)
        ]
    );
}

#[test]
fn a_rust_call_of_a_standard_library_method_on_an_untyped_value_binds_the_one_method_of_its_name_as_a_guess()
 {
    let tree = tree_of(&[
        (
            "fs/src/watch.rs",
            "pub struct Watcher;\n\nimpl Watcher {\n    pub fn start(&self) {}\n\n    pub fn stop(&self) {}\n}\n",
        ),
        (
            "fs/src/requirement.rs",
            "use regex_syntax::hir::ClassUnicode;\n\n\
             fn folded(class: &ClassUnicode) -> Vec<u32> {\n\
             \x20   class.ranges().iter().map(|range| range.start()).collect()\n}\n\n\
             fn stopped(watched: Vec<Watcher>) {\n    watched.iter().for_each(|each| each.stop());\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("fs/src/watch.rs")
        .expect("a language claims the file");
    let callers = |name: &str| {
        let walk = graph
            .callers(name, Limits::new(1))
            .expect("the name is defined");
        (walk.reached.iter())
            .map(|reached| (reached.symbol.name, reached.confidence))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        callers("Watcher::start"),
        [("folded", confidence::FIRST_PARTY)]
    );
    assert_eq!(callers("Watcher::stop"), [("stopped", confidence::UNIQUE)]);
}

#[test]
fn a_rust_call_on_a_parameter_binds_the_method_of_the_type_the_parameter_is_declared_as() {
    let tree = tree_of(&[(
        "src/fuzzy.rs",
        "pub struct Ranking;\n\nimpl Ranking {\n    pub fn offer(&mut self) {}\n}\n\n\
         pub struct Queue;\n\nimpl Queue {\n    pub fn offer(&mut self) {}\n}\n\n\
         fn offer(ranking: &mut Ranking) {\n    ranking.offer();\n}\n",
    )]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("src/fuzzy.rs")
        .expect("a language claims the file");
    let callers = graph
        .callers("Ranking::offer", Limits::new(1))
        .expect("the name is defined");

    assert_eq!(
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.symbol.line, reached.confidence))
            .collect::<Vec<_>>(),
        [("offer", 13, confidence::RECEIVER)]
    );
}

#[test]
fn a_rust_call_on_a_local_binds_the_method_of_the_type_its_initializer_constructs() {
    let tree = tree_of(&[(
        "src/lib.rs",
        "pub struct Watcher;\n\
         impl Watcher {\n\
         \x20   pub fn new() -> Result<Watcher, ()> { Ok(Watcher) }\n\
         \x20   pub fn drain(&self) {}\n\
         }\n\
         pub struct Other;\n\
         impl Other {\n\
         \x20   pub fn drain(&self) {}\n\
         }\n\
         pub fn local() -> Result<(), ()> {\n\
         \x20   let w = Watcher::new()?;\n\
         \x20   w.drain();\n\
         \x20   Ok(())\n\
         }\n",
    )]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("src/lib.rs")
        .expect("a language claims the file");
    let callers = graph
        .callers("Watcher::drain", Limits::new(1))
        .expect("the name is defined");

    assert_eq!(
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.symbol.line, reached.confidence))
            .collect::<Vec<_>>(),
        [("local", 10, confidence::RECEIVER)]
    );
}

#[test]
fn a_rust_call_on_a_chain_binds_the_method_of_the_type_the_chain_holds() {
    let tree = tree_of(&[(
        "src/lib.rs",
        "pub struct Watcher;\n\
         impl Watcher {\n\
         \x20   pub fn drain(&self) {}\n\
         }\n\
         pub struct Other;\n\
         impl Other {\n\
         \x20   pub fn drain(&self) {}\n\
         }\n\
         pub fn chain(o: Option<Watcher>) {\n\
         \x20   o.expect(\"a watcher\").drain();\n\
         }\n",
    )]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("src/lib.rs")
        .expect("a language claims the file");
    let callers = graph
        .callers("Watcher::drain", Limits::new(1))
        .expect("the name is defined");

    assert_eq!(
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.symbol.line, reached.confidence))
            .collect::<Vec<_>>(),
        [("chain", 9, confidence::RECEIVER)]
    );
    let others = graph
        .callers("Other::drain", Limits::new(1))
        .expect("the name is defined");
    assert_eq!(others.reached.len(), 0);
}

#[test]
fn a_rust_call_on_a_method_s_result_binds_the_method_of_the_type_it_returns_in_another_file() {
    let tree = tree_of(&[
        (
            "src/lib.rs",
            "mod store;\nuse store::Store;\n\
             pub struct Other;\n\
             impl Other {\n\
             \x20   pub fn write(&self) {}\n\
             }\n\
             pub fn chained(s: &Store) -> Result<(), ()> {\n\
             \x20   s.data()?.write();\n\
             \x20   Ok(())\n\
             }\n\
             pub fn local(s: &Store) -> Result<(), ()> {\n\
             \x20   let data = s.data()?;\n\
             \x20   data.write();\n\
             \x20   Ok(())\n\
             }\n",
        ),
        (
            "src/store.rs",
            "pub struct Data;\n\
             impl Data {\n\
             \x20   pub fn write(&self) {}\n\
             }\n\
             pub struct Store;\n\
             impl Store {\n\
             \x20   pub fn data(&self) -> Result<&Data, ()> { Err(()) }\n\
             }\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);
    let (_, graph) = engine
        .graph_of("src/lib.rs")
        .expect("a language claims the file");
    let callers = graph
        .callers("Data::write", Limits::new(1))
        .expect("the name is defined");

    assert_eq!(
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.confidence))
            .collect::<Vec<_>>(),
        [
            ("chained", confidence::RECEIVER),
            ("local", confidence::RECEIVER)
        ]
    );
}

#[test]
fn a_rust_call_on_a_type_two_versions_of_its_crate_declare_binds_the_method_of_the_caller_s_copy() {
    let manifest =
        |version: &str| format!("[package]\nname = \"hashbrown\"\nversion = \"{version}\"\n");
    let (old, new) = (manifest("0.14.5"), manifest("0.15.5"));
    let raw = "pub struct RawTable;\n\nimpl RawTable {\n    pub fn reserve(&mut self) {}\n}\n";
    let map =
        "use crate::raw::RawTable;\n\nfn insert(table: &mut RawTable) {\n    table.reserve();\n}\n";
    let tree = tree_of(&[
        ("hashbrown-0.14.5/Cargo.toml", &old),
        ("hashbrown-0.14.5/src/lib.rs", "mod map;\nmod raw;\n"),
        ("hashbrown-0.14.5/src/map.rs", map),
        ("hashbrown-0.14.5/src/raw/mod.rs", raw),
        ("hashbrown-0.15.5/Cargo.toml", &new),
        ("hashbrown-0.15.5/src/lib.rs", "mod map;\nmod raw;\n"),
        ("hashbrown-0.15.5/src/map.rs", map),
        ("hashbrown-0.15.5/src/raw/mod.rs", raw),
    ]);
    let engine = build(&tree, &["rust"]);

    for copy in ["hashbrown-0.14.5", "hashbrown-0.15.5"] {
        let (_, graph) = engine
            .graph_of(&format!("{copy}/src/map.rs"))
            .expect("a language claims the file");
        let callees = graph
            .callees(
                Selector::new("insert", Some(&format!("{copy}/")[..])),
                Limits::new(1),
            )
            .expect("the name is defined");
        assert_eq!(
            (callees.reached.iter())
                .map(|reached| (reached.symbol.file, reached.confidence))
                .collect::<Vec<_>>(),
            [(&format!("{copy}/src/raw/mod.rs")[..], confidence::RECEIVER)]
        );
    }
}

#[test]
fn a_rust_call_through_a_type_the_tree_does_not_declare_binds_no_member_of_another_type() {
    let tree = tree_of(&[
        ("src/lib.rs", "mod follow;\nmod widget;\n"),
        (
            "src/widget.rs",
            "pub struct Widget;\n\npub static DEFAULT: Widget = Widget;\n\nimpl Widget {\n    \
             pub fn new() -> Self {\n        Widget\n    }\n\n    pub fn draw(&self) {}\n}\n",
        ),
        (
            "src/follow.rs",
            "use crate::widget::{DEFAULT, Widget};\n\nstruct Following;\n\nimpl Following {\n    \
             fn new() -> Self {\n        Following\n    }\n\n    fn plan(&self) {\n        Vec::new();\n        \
             tempfile::NamedTempFile::new();\n    }\n\n    fn widgets(&self) {\n        Widget::new();\n        \
             Following::new();\n        DEFAULT.draw();\n    }\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(callees(&engine, "src/follow.rs", "plan"), (vec![], 2));
    assert_eq!(
        callees(&engine, "src/follow.rs", "widgets"),
        (
            vec![
                ("new", SymbolKind::Method, "src/follow.rs"),
                ("new", SymbolKind::Method, "src/widget.rs"),
                ("draw", SymbolKind::Method, "src/widget.rs"),
            ],
            0
        )
    );
}

#[test]
fn a_rust_call_binds_a_definition_a_macro_writes_only_where_no_written_one_binds() {
    let tree = tree_of(&[
        ("src/lib.rs", "mod ids;\nmod limits;\nmod small;\n"),
        (
            "src/ids.rs",
            "macro_rules! index {\n    ($name:ident) => {\n        pub struct $name;\n\n        \
             impl $name {\n            pub fn as_u32(&self) -> u32 {\n                0\n            }\n\n            \
             pub fn one_more(&self) -> u32 {\n                1\n            }\n        }\n    };\n}\n\n\
             index!(StateId);\n",
        ),
        (
            "src/small.rs",
            "pub struct SmallIndex;\n\nimpl SmallIndex {\n    pub const MAX: SmallIndex = SmallIndex;\n\n    \
             pub fn as_u32(&self) -> u32 {\n        0\n    }\n}\n",
        ),
        (
            "src/limits.rs",
            "use crate::small::SmallIndex;\n\nfn check() {\n    SmallIndex::MAX.as_u32();\n    \
             SmallIndex::MAX.one_more();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "src/limits.rs", "check"),
        (
            vec![
                ("one_more", SymbolKind::Method, "src/ids.rs"),
                ("as_u32", SymbolKind::Method, "src/small.rs"),
            ],
            0
        )
    );
    let (_, graph) = engine
        .graph_of("src/limits.rs")
        .expect("a language claims the file");
    let made = graph
        .callees("check", Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .find(|reached| reached.symbol.name == "one_more")
        .map(|reached| reached.confidence);
    assert_eq!(made, Some(confidence::MADE));
}

#[test]
fn a_rust_call_binds_no_value_a_macro_writes_and_reads_none_as_the_receiver_of_a_path() {
    let tree = tree_of(&[
        ("src/lib.rs", "mod symbols;\nmod uses;\n"),
        (
            "src/symbols.rs",
            "macro_rules! symbols {\n    ($($name:ident),*) => {\n        $(pub const $name: u32 = 0;)*\n    };\n}\n\n\
             symbols!(Some, Box);\n",
        ),
        (
            "src/uses.rs",
            "struct Interner;\n\nimpl Interner {\n    fn new() -> Self {\n        Interner\n    }\n}\n\n\
             fn check() {\n    Some(1);\n    Box::new(1);\n}\n",
        ),
    ]);
    let engine = build(&tree, &["rust"]);

    assert_eq!(callees(&engine, "src/uses.rs", "check"), (vec![], 2));
}

#[test]
fn a_rust_call_binds_by_name_only_within_the_crates_its_crate_is_built_with() {
    let config = |caller: &str| {
        format!(
            "pub struct Config;\n\nimpl Config {{\n    pub fn default() -> Self {{\n        Config\n    }}\n}}\n\n\
             fn {caller}() {{\n    Config::default();\n}}\n"
        )
    };
    let core = config("in_core")
        + "\npub fn parse_config() {}\n\nfn not_built_with() {\n    Settings::load();\n}\n";
    let owning = |owner: &str, method: &str| {
        format!("pub struct {owner};\n\nimpl {owner} {{\n    pub fn {method}() {{}}\n}}\n")
    };
    let (util, bench) = (owning("Settings", "load"), owning("Harness", "run"));
    let net = config("in_net")
        + "\nstruct Watcher;\n\nimpl Watcher {\n    fn new() -> Self {\n        Watcher\n    }\n}\n\n\
           fn depended_on() {\n    Options::new();\n    parse_config();\n    Config::default().reload();\n}\n";
    let tree = tree_of(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\n\n[dependencies]\nnet = { path = \"../net\" }\n",
        ),
        (
            "crates/app/src/lib.rs",
            "pub struct Options;\n\nimpl Options {\n    pub fn new() -> Self {\n        Options\n    }\n\n    \
             pub fn reload(&self) {}\n}\n\nfn depending() {\n    Config::default();\n    parse_config();\n    \
             Settings::load();\n    Harness::run();\n}\n",
        ),
        ("crates/bench/Cargo.toml", "[package]\nname = \"bench\"\n"),
        ("crates/bench/src/lib.rs", &bench),
        ("crates/core/Cargo.toml", "[package]\nname = \"core\"\n"),
        ("crates/core/src/lib.rs", &core),
        (
            "crates/net/Cargo.toml",
            "[package]\nname = \"net\"\n\n[dependencies]\nutil = \"1\"\n\n[dev-dependencies]\nbench = \"1\"\n",
        ),
        ("crates/net/src/lib.rs", &net),
        ("crates/util/Cargo.toml", "[package]\nname = \"util\"\n"),
        ("crates/util/src/lib.rs", &util),
    ]);
    let engine = build(&tree, &["rust"]);
    let default_of = |path| vec![("default", SymbolKind::Method, path)];

    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "depending"),
        (
            vec![
                ("default", SymbolKind::Method, "crates/net/src/lib.rs"),
                ("load", SymbolKind::Method, "crates/util/src/lib.rs"),
            ],
            2
        )
    );
    assert_eq!(
        callees(&engine, "crates/core/src/lib.rs", "not_built_with"),
        (vec![], 1)
    );
    assert_eq!(
        callees(&engine, "crates/net/src/lib.rs", "in_net"),
        (default_of("crates/net/src/lib.rs"), 0)
    );
    assert_eq!(
        callees(&engine, "crates/core/src/lib.rs", "in_core"),
        (default_of("crates/core/src/lib.rs"), 0)
    );
    assert_eq!(
        callees(&engine, "crates/net/src/lib.rs", "depended_on"),
        (default_of("crates/net/src/lib.rs"), 3)
    );
}

#[test]
fn a_rust_name_crowded_past_the_candidate_index_binds_only_outside_tests_of_a_crate_built_with() {
    let settings = "pub struct Settings;\n\nimpl Settings {\n    pub fn load() {}\n}\n";
    let crowd: Vec<(String, &str)> = (0..64)
        .flat_map(|n| {
            [
                (
                    format!("crates/c{n}/Cargo.toml"),
                    "[package]\nname = \"crowd\"\n",
                ),
                (format!("crates/c{n}/src/lib.rs"), settings),
            ]
        })
        .collect();
    let mut files = vec![
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        (
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\n\n[dependencies]\nutil = { path = \"../util\" }\n",
        ),
        (
            "crates/app/src/lib.rs",
            "fn depending() {\n    Settings::load();\n}\n",
        ),
        ("crates/lone/Cargo.toml", "[package]\nname = \"lone\"\n"),
        (
            "crates/lone/src/lib.rs",
            "fn built_with_none() {\n    Settings::load();\n}\n",
        ),
        ("crates/util/Cargo.toml", "[package]\nname = \"util\"\n"),
        ("crates/util/src/lib.rs", settings),
        ("crates/util/tests/stub.rs", settings),
    ];
    files.extend(crowd.iter().map(|(path, source)| (path.as_str(), *source)));
    let tree = tree_of(&files);
    let engine = build(&tree, &["rust"]);

    assert_eq!(
        callees(&engine, "crates/app/src/lib.rs", "depending"),
        (
            vec![("load", SymbolKind::Method, "crates/util/src/lib.rs")],
            0
        )
    );
    assert_eq!(
        callees(&engine, "crates/lone/src/lib.rs", "built_with_none"),
        (vec![], 1)
    );
}

#[test]
fn a_typescript_call_on_a_parameter_typed_by_its_context_binds_the_method_of_that_type() {
    let tree = tree_of(&[
        (
            "src/kartta.ts",
            "export class Kartta {\n  dependents(p: number) { return p; }\n  grep(p: number) { return p; }\n}\n",
        ),
        (
            "test/golden.test.ts",
            "import { Kartta } from \"../src/kartta\";\n\n\
             type Replay<M> = (kartta: Kartta, params: M) => Promise<unknown>;\n\n\
             const replays: { [M in keyof Methods]: Replay<M> } = {\n\
             \x20 dependents: async (kartta, params) => kartta.dependents(params),\n\
             };\n\n\
             const grep: Replay<number> = async (kartta, params) => kartta.grep(params);\n",
        ),
    ]);
    let engine = build(&tree, &["typescript"]);
    let (_, graph) = engine
        .graph_of("src/kartta.ts")
        .expect("a language claims the file");
    let callers_of = |name: &str| {
        let callers = graph
            .callers(name, Limits::new(1))
            .expect("the name is defined");
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.symbol.line, reached.confidence))
            .collect::<Vec<_>>()
    };
    let self_call = |name: &str| {
        graph
            .callers(
                Selector::new(name, Some("test/golden.test.ts")),
                Limits::new(1),
            )
            .expect("the name is defined")
            .self_call
    };

    assert_eq!(
        callers_of("Kartta.dependents"),
        [("dependents", 6, confidence::RECEIVER)]
    );
    assert_eq!(
        callers_of("Kartta.grep"),
        [("grep", 9, confidence::RECEIVER)]
    );
    assert_eq!(self_call("dependents"), None);
    assert_eq!(self_call("grep"), None);
}

#[test]
fn a_typescript_call_on_a_typed_parameter_binds_a_member_of_that_type_and_no_namesake() {
    let tree = tree_of(&[
        (
            "src/kartta.ts",
            "export class Kartta {\n  build(p: number) { return p; }\n}\n\n\
             export interface Handler {\n  handle(p: number): void;\n}\n\n\
             export type Callbacks = Pick<Options, 'onSuccess'>;\n",
        ),
        (
            "src/hooks.ts",
            "import type { Callbacks, Handler } from \"./kartta\";\n\
             import type { Outside } from \"outside\";\n\n\
             export function useInit(options: Callbacks) {\n\
             \x20 return make({\n\
             \x20   onSuccess(data) {\n\
             \x20     options?.onSuccess?.(data);\n\
             \x20   },\n\
             \x20 });\n\
             }\n\n\
             export function useOutside(options: Outside) {\n\
             \x20 return make({\n\
             \x20   onError(data) {\n\
             \x20     options.onError(data);\n\
             \x20   },\n\
             \x20 });\n\
             }\n\n\
             export function route(handler: Handler) {\n  handler.handle(1);\n}\n",
        ),
        (
            "test/golden.test.ts",
            "import { Kartta } from \"../src/kartta\";\n\n\
             type Replay = (kartta: Kartta) => unknown;\n\n\
             export const replays: Record<string, Replay> = {\n\
             \x20 build: async () => 1,\n\
             };\n",
        ),
        (
            "test/kartta.test.ts",
            "import { Kartta } from \"../src/kartta\";\n\n\
             function walking(kartta: Kartta) {\n  kartta.build(1);\n}\n",
        ),
    ]);
    let engine = build(&tree, &["typescript"]);
    let (_, graph) = engine
        .graph_of("src/hooks.ts")
        .expect("a language claims the file");
    let callers_of = |selector: Selector<'_>| {
        let callers = graph
            .callers(selector, Limits::new(1))
            .expect("the name is defined");
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.confidence))
            .collect::<Vec<_>>()
    };
    let self_call = |name: &str| {
        graph
            .callers(name, Limits::new(1))
            .expect("the name is defined")
            .self_call
    };

    assert_eq!(self_call("onSuccess"), None);
    assert_eq!(self_call("onError"), None);
    assert_eq!(
        callers_of(Selector::new("Handler.handle", None)),
        [("route", confidence::RECEIVER)]
    );
    assert_eq!(
        callers_of(Selector::new("Kartta.build", None)),
        [("walking", confidence::RECEIVER)]
    );
    assert_eq!(
        callers_of(Selector::new("build", Some("test/golden.test.ts"))),
        []
    );
}

#[test]
fn a_typescript_call_on_a_local_a_static_call_initializes_binds_no_literal_of_its_file() {
    let tree = tree_of(&[
        (
            "src/kartta.ts",
            "export class Kartta {\n\
             \x20 static open(root: string): Kartta { return new Kartta(); }\n\
             \x20 version() { return 1; }\n\
             }\n",
        ),
        (
            "test/golden.test.ts",
            "import { Kartta } from \"../src/kartta\";\n\n\
             const replays = {\n  version: async () => 1,\n};\n\n\
             export function covers() {\n\
             \x20 const kartta = Kartta.open(\"fixture\");\n\
             \x20 kartta.version();\n\
             }\n",
        ),
    ]);
    let engine = build(&tree, &["typescript"]);
    let (_, graph) = engine
        .graph_of("src/kartta.ts")
        .expect("a language claims the file");
    let callers_of = |selector: Selector<'_>| {
        let callers = graph
            .callers(selector, Limits::new(1))
            .expect("the name is defined");
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.confidence))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        callers_of(Selector::new("version", Some("test/golden.test.ts"))),
        []
    );
    assert_eq!(
        callers_of(Selector::new("Kartta.version", None)),
        [("covers", confidence::RECEIVER)]
    );
}

#[test]
fn a_typescript_call_on_a_local_a_free_function_initializes_binds_no_literal_beside_it() {
    let tree = tree_of(&[
        (
            "src/kartta.ts",
            "export class Kartta {\n  build() { return 1; }\n}\n",
        ),
        (
            "test/golden.test.ts",
            "export const replays = {\n  build: async () => 1,\n};\n",
        ),
        (
            "test/kartta.test.ts",
            "import { Kartta } from \"../src/kartta\";\n\n\
             function opened(root: string): Kartta {\n  return new Kartta();\n}\n\n\
             export function cancels() {\n\
             \x20 const kartta = opened(\"large\");\n\
             \x20 kartta.build();\n\
             }\n",
        ),
    ]);
    let engine = build(&tree, &["typescript"]);
    let (_, graph) = engine
        .graph_of("src/kartta.ts")
        .expect("a language claims the file");
    let callers_of = |selector: Selector<'_>| {
        let callers = graph
            .callers(selector, Limits::new(1))
            .expect("the name is defined");
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.confidence))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        callers_of(Selector::new("build", Some("test/golden.test.ts"))),
        []
    );
    assert_eq!(
        callers_of(Selector::new("Kartta.build", None)),
        [("cancels", confidence::RECEIVER)]
    );
}

const RETURNS_A_TYPE: &str = "export class Kartta {\n\
     \x20 static async connect(): Promise<Kartta> { return new Kartta(); }\n\
     \x20 child(): Kartta { return this; }\n\
     \x20 build() { return 1; }\n\
     \x20 cancel() { return 2; }\n\
     \x20 refresh() { return 3; }\n\
     \x20 version() { return 4; }\n\
     \x20 stats() { return 5; }\n\
     }\n\n\
     export function opened(root: string): Kartta {\n  return new Kartta();\n}\n\n\
     export const reopened = (root: string): Kartta => new Kartta();\n\n\
     export function held<T>(value: T): T {\n  return value;\n}\n";

const CALLS_WHAT_IT_RETURNS: &str = "import { Kartta, held, opened, reopened } from \"../src/kartta\";\n\n\
     export function imported() {\n\
     \x20 const kartta = opened(\"large\");\n\
     \x20 kartta.build();\n\
     }\n\n\
     export async function awaited() {\n\
     \x20 const kartta = await Kartta.connect();\n\
     \x20 kartta.cancel();\n\
     }\n\n\
     export function chained() {\n\
     \x20 const kartta = opened(\"large\");\n\
     \x20 const child = kartta.child();\n\
     \x20 child.refresh();\n\
     }\n\n\
     export class Suite {\n\
     \x20 made(): Kartta { return new Kartta(); }\n\
     \x20 run() {\n\
     \x20   const kartta = this.made();\n\
     \x20   kartta.version();\n\
     \x20 }\n\
     }\n\n\
     export function arrow() {\n\
     \x20 const kartta = reopened(\"large\");\n\
     \x20 kartta.stats();\n\
     }\n\n\
     export function generic(value: unknown) {\n\
     \x20 const kept = held(value);\n\
     \x20 kept.stop();\n\
     }\n";

#[test]
fn a_typescript_call_on_a_local_follows_the_return_types_of_the_calls_that_initialize_it() {
    let tree = tree_of(&[
        ("src/kartta.ts", RETURNS_A_TYPE),
        (
            "test/golden.test.ts",
            "export const replays = {\n\
             \x20 build: async () => 1,\n\
             \x20 cancel: async () => 2,\n\
             \x20 refresh: async () => 3,\n\
             \x20 version: async () => 4,\n\
             \x20 stats: async () => 5,\n\
             \x20 stop: async () => 6,\n\
             };\n",
        ),
        ("test/kartta.test.ts", CALLS_WHAT_IT_RETURNS),
    ]);
    let engine = build(&tree, &["typescript"]);
    let (_, graph) = engine
        .graph_of("src/kartta.ts")
        .expect("a language claims the file");
    let callers_of = |selector: Selector<'_>| {
        let callers = graph
            .callers(selector, Limits::new(1))
            .expect("the name is defined");
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.confidence))
            .collect::<Vec<_>>()
    };

    for (method, caller) in [
        ("build", "imported"),
        ("cancel", "awaited"),
        ("refresh", "chained"),
        ("version", "run"),
        ("stats", "arrow"),
    ] {
        assert_eq!(
            callers_of(Selector::new(method, Some("test/golden.test.ts"))),
            [],
            "{method}"
        );
        assert_eq!(
            callers_of(Selector::new(&format!("Kartta.{method}"), None)),
            [(caller, confidence::RECEIVER)],
            "{method}"
        );
    }
    let stopped = callers_of(Selector::new("stop", Some("test/golden.test.ts")));
    assert_eq!(stopped.first().map(|caller| caller.0), Some("generic"));
}

#[test]
fn a_javascript_call_on_a_local_a_package_class_constructs_keeps_the_binding_of_a_value() {
    let tree = tree_of(&[
        (
            "vendor/three.min.js",
            "export class La {\n  setFromPoint() {}\n}\n",
        ),
        (
            "vendor/controls.js",
            "import { Plane } from \"three\";\n\n\
             const plane = new Plane();\n\n\
             export function aim() {\n  plane.setFromPoint();\n}\n",
        ),
    ]);
    let engine = build(&tree, &["javascript"]);
    let (_, graph) = engine
        .graph_of("vendor/controls.js")
        .expect("a language claims the file");

    let callers = graph
        .callers("La.setFromPoint", Limits::new(1))
        .expect("the name is defined");
    assert_eq!(
        (callers.reached.iter())
            .map(|reached| reached.symbol.name)
            .collect::<Vec<_>>(),
        ["aim"]
    );
}

#[test]
fn a_typescript_call_on_a_typed_or_constructed_local_binds_a_member_of_that_type() {
    let tree = tree_of(&[
        (
            "src/kartta.ts",
            "export class Kartta {\n\
             \x20 build() { return 1; }\n\
             \x20 cancel() { return 2; }\n\
             }\n",
        ),
        (
            "test/golden.test.ts",
            "export const replays = {\n\
             \x20 build: async () => 1,\n\
             \x20 cancel: async () => 2,\n\
             };\n",
        ),
        (
            "test/kartta.test.ts",
            "import { Kartta } from \"../src/kartta\";\n\n\
             export function typed(made: unknown) {\n\
             \x20 const kartta: Kartta = made as Kartta;\n\
             \x20 kartta.build();\n\
             }\n\n\
             export function constructed() {\n\
             \x20 const kartta = new Kartta();\n\
             \x20 kartta.cancel();\n\
             }\n",
        ),
    ]);
    let engine = build(&tree, &["typescript"]);
    let (_, graph) = engine
        .graph_of("src/kartta.ts")
        .expect("a language claims the file");
    let callers_of = |selector: Selector<'_>| {
        let callers = graph
            .callers(selector, Limits::new(1))
            .expect("the name is defined");
        (callers.reached.iter())
            .map(|reached| (reached.symbol.name, reached.confidence))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        callers_of(Selector::new("build", Some("test/golden.test.ts"))),
        []
    );
    assert_eq!(
        callers_of(Selector::new("cancel", Some("test/golden.test.ts"))),
        []
    );
    assert_eq!(
        callers_of(Selector::new("Kartta.build", None)),
        [("typed", confidence::RECEIVER)]
    );
    assert_eq!(
        callers_of(Selector::new("Kartta.cancel", None)),
        [("constructed", confidence::RECEIVER)]
    );
}

#[test]
fn a_javascript_call_through_a_class_binds_no_function_outside_every_type() {
    let tree = tree_of(&[
        (
            "src/popup.js",
            "export class Popup {\n  show() {\n    this.onOpen();\n  }\n}\n",
        ),
        ("src/picker.js", "export const picker = { onOpen() {} };\n"),
    ]);
    let engine = build(&tree, &["javascript"]);
    let (_, graph) = engine
        .graph_of("src/popup.js")
        .expect("a language claims the file");

    let callers = graph
        .callers("onOpen", Limits::new(1))
        .expect("the name is defined");
    assert_eq!(callers.reached.len(), 0);
}

#[test]
fn a_javascript_call_through_a_type_s_namesake_its_file_does_not_see_binds_a_function() {
    let tree = tree_of(&[
        ("lib/jquery.js", "function ajax() {}\n"),
        ("lib/bundle.js", "class $ {}\n"),
        ("lib/admin.js", "export function load() {\n  $.ajax();\n}\n"),
    ]);
    let engine = build(&tree, &["javascript"]);
    let (_, graph) = engine
        .graph_of("lib/admin.js")
        .expect("a language claims the file");

    let callers = graph
        .callers("ajax", Limits::new(1))
        .expect("the name is defined");
    assert_eq!(callers.reached.len(), 1);
}

#[test]
fn a_javascript_call_through_a_class_of_a_minified_bundle_binds_a_function() {
    let padding = "/".repeat(600);
    let bundle = format!(
        "var pad = '{padding}';class Fs {{}}\nconst fs = {{ isFile() {{}} }};\n\
         export function load() {{ Fs.isFile(); }}\n"
    );
    let tree = tree_of(&[("lib/bundle.js", &bundle)]);
    let engine = build(&tree, &["javascript"]);
    let (_, graph) = engine
        .graph_of("lib/bundle.js")
        .expect("a language claims the file");

    let callers = graph
        .callers("isFile", Limits::new(1))
        .expect("the name is defined");
    assert_eq!(callers.reached.len(), 1);
}

#[test]
fn a_javascript_bare_call_of_a_name_imported_from_a_package_binds_no_namesake_of_the_project() {
    let tree = tree_of(&[
        ("package.json", "{\"name\":\"app\"}\n"),
        (
            "src/app.js",
            "import { useState } from \"react\";\nimport { useHook } from \"./hooks\";\n\n\
             export function run() { useState(0); }\n\nexport function hook() { useHook(); }\n",
        ),
        ("src/hooks.js", "export function useHook() {}\n"),
        (
            "src/app.test.js",
            "import { beforeEach, describe } from \"vitest\";\n\n\
             describe(\"app\", () => {\n  beforeEach(() => {});\n});\n",
        ),
        (
            "other/lib.js",
            "export function useState(v) {}\n\nexport function useHook() {}\n",
        ),
    ]);
    let engine = build(&tree, &["javascript"]);

    assert_eq!(callees(&engine, "src/app.js", "run"), (vec![], 1));
    assert_eq!(
        callees(&engine, "src/app.js", "hook"),
        (vec![("useHook", SymbolKind::Function, "src/hooks.js")], 0)
    );
    assert_eq!(
        callees(&engine, "src/app.test.js", "app"),
        (vec![], 1),
        "a hook the call registers is no namesake of the registrar"
    );
    let [(_, resolution)] = engine
        .resolution(usize::MAX)
        .try_into()
        .expect("one family");
    let misses: Vec<_> = (resolution.unbound.iter())
        .map(|unbound| (unbound.callee, unbound.miss))
        .collect();
    assert_eq!(
        misses,
        [
            ("useState", Miss::ExternalImport),
            ("beforeEach", Miss::ExternalImport)
        ]
    );
}

#[test]
fn a_python_call_of_a_builtin_binds_a_namesake_only_where_its_file_imports_it() {
    let tree = tree_of(&[
        ("app/views.py", "def list():\n    return []\n"),
        (
            "tools/report.py",
            "def rows(items):\n    return list(items)\n",
        ),
        (
            "tools/index.py",
            "from app.views import list\n\ndef pages():\n    return list()\n",
        ),
    ]);
    let engine = build(&tree, &["python"]);

    assert_eq!(callees(&engine, "tools/report.py", "rows"), (vec![], 0));
    assert_eq!(
        callees(&engine, "tools/index.py", "pages"),
        (vec![("list", SymbolKind::Function, "app/views.py")], 0)
    );
}

#[test]
fn a_javascript_root_relative_import_binds_through_the_file_a_jsconfig_roots_it_at() {
    let tree = tree_of(&[
        ("package.json", "{\"name\":\"app\"}\n"),
        (
            "public/jsconfig.json",
            "{ \"compilerOptions\": { \"paths\": { \"/*\": [\"./*\"] } } }\n",
        ),
        (
            "public/a.js",
            "import { MacrosParser, register } from '/scripts/m.js';\n\n\
             export function run() { MacrosParser.registerMacro(); }\n\n\
             export function use() { register(); }\n",
        ),
        (
            "public/scripts/m.js",
            "export class MacrosParser { static registerMacro() {} }\n\nexport function register() {}\n",
        ),
        (
            "src/other.js",
            "export class Other { static registerMacro() {} }\n\nexport function register() {}\n",
        ),
    ]);
    let engine = build(&tree, &["javascript"]);

    assert_eq!(
        callees(&engine, "public/a.js", "run"),
        (
            vec![("registerMacro", SymbolKind::Method, "public/scripts/m.js")],
            0
        )
    );
    assert_eq!(
        callees(&engine, "public/a.js", "use"),
        (
            vec![("register", SymbolKind::Function, "public/scripts/m.js")],
            0
        )
    );
}

#[test]
fn a_javascript_runtime_method_s_call_binds_only_the_imported_member_its_receiver_spells() {
    let tree = tree_of(&[
        ("package.json", "{\"name\":\"app\"}\n"),
        (
            "src/macros.js",
            "export class Macros {\n  get(key) {}\n}\n\nexport const macros = new Macros();\n",
        ),
        (
            "src/sd.js",
            "import { macros } from './macros.js';\n\n\
             export function sdMessageButton($icon) { $icon.get(0); }\n\n\
             export function onClose() { macros.get('x'); }\n",
        ),
    ]);
    let engine = build(&tree, &["javascript"]);

    assert_eq!(
        callees(&engine, "src/sd.js", "sdMessageButton"),
        (vec![], 1)
    );
    assert_eq!(
        callees(&engine, "src/sd.js", "onClose"),
        (vec![("get", SymbolKind::Method, "src/macros.js")], 0),
        "a receiver that spells the class still reaches its member"
    );
}

#[test]
fn a_typescript_runtime_method_s_call_binds_an_imported_interface_s_member_on_any_value() {
    let tree = tree_of(&[
        (
            "src/drawer.ts",
            "export interface Drawer {\n  open(id: number): void;\n}\n\n\
             export type Store = {\n  set(value: string): void;\n};\n\n\
             export class Cache {\n  has(key: string) { return true; }\n}\n\n\
             export function useDrawer(): Drawer { return { open() {} }; }\n",
        ),
        (
            "src/row.ts",
            "import { Cache, Store, useDrawer } from './drawer';\n\n\
             export function Row(read: () => Store) {\n  const drawer = useDrawer();\n  drawer.open(1);\n  \
             const store = read();\n  store.set('x');\n  const seen = new Set();\n  seen.has('x');\n}\n",
        ),
    ]);
    let engine = build(&tree, &["typescript"]);

    let (reached, _) = callees(&engine, "src/row.ts", "Row");
    assert_eq!(
        reached,
        [
            ("open", SymbolKind::Method, "src/drawer.ts"),
            ("set", SymbolKind::Method, "src/drawer.ts"),
            ("useDrawer", SymbolKind::Function, "src/drawer.ts")
        ]
    );
}

#[test]
fn a_python_call_through_a_module_outside_the_tree_binds_no_namesake_of_the_project() {
    let tree = tree_of(&[
        ("rdc/envelope.py", "def dumps(value):\n    return value\n"),
        ("rdc/fields.py", "class Field:\n    pass\n"),
        (
            "tools/report.py",
            "import json\nimport json as encoded\nfrom sphinx.util.docfields import Field\n\
             from rdc import envelope\n\n\
             def write(value):\n    return json.dumps(value)\n\n\
             def aliased(value):\n    return encoded.dumps(value)\n\n\
             def field():\n    return Field()\n\n\
             def wrap(value):\n    return envelope.dumps(value)\n",
        ),
        (
            "tools/form.py",
            "from rdc.fields import Field\n\ndef form_field():\n    return Field()\n",
        ),
    ]);
    let engine = build(&tree, &["python"]);

    assert_eq!(callees(&engine, "tools/report.py", "write"), (vec![], 1));
    assert_eq!(callees(&engine, "tools/report.py", "aliased"), (vec![], 1));
    assert_eq!(callees(&engine, "tools/report.py", "field"), (vec![], 1));
    assert_eq!(
        callees(&engine, "tools/report.py", "wrap"),
        (vec![("dumps", SymbolKind::Function, "rdc/envelope.py")], 0)
    );
    assert_eq!(
        callees(&engine, "tools/form.py", "form_field"),
        (vec![("Field", SymbolKind::Class, "rdc/fields.py")], 0)
    );
}

#[test]
fn a_javascript_require_of_a_package_binds_its_names_outside_the_repository() {
    let tree = tree_of(&[
        ("package.json", "{\"name\":\"app\"}\n"),
        (
            "src/app.js",
            "const fs = require(\"fs\");\nconst { readFileSync } = require(\"fs\");\n\
             const lib = require(\"./lib\");\n\n\
             export function run() { fs.readFileSync(\"x\"); }\n\n\
             export function read() { readFileSync(\"x\"); }\n\n\
             export function use() { lib.f(); }\n",
        ),
        ("src/lib.js", "export function f() {}\n"),
        (
            "other/lib.js",
            "export function readFileSync(p) {}\n\nexport function f() {}\n",
        ),
    ]);
    let engine = build(&tree, &["javascript"]);

    assert_eq!(callees(&engine, "src/app.js", "run"), (vec![], 1));
    assert_eq!(callees(&engine, "src/app.js", "read"), (vec![], 1));
    assert_eq!(
        callees(&engine, "src/app.js", "use"),
        (vec![("f", SymbolKind::Function, "src/lib.js")], 0)
    );
}
