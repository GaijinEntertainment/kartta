//! What an engine built from a tree on disk binds: a selection of part of the TypeScript family leaves the files of the
//! other languages out of the graph, no Rust call binds a module, a Rust call into another crate of the workspace binds
//! what that crate exports, a macro invocation the `#[macro_export]` macro of its root file, through a `use` inside a
//! function body or an inline module only from inside it, and a Rust call on a value, on `self` or through a type
//! binds no free function, not even through a value named like a module the file imports, while a call on a value
//! still binds the one runtime method whose owner the value's name spells.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, SymbolKind, confidence};
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
