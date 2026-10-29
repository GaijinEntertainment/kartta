//! What a Rust call reaches where a `cfg` picks one of several definitions of its callee: every definition a branch
//! gives the name, whether the branches import it from different modules or define it twice in one file, and no
//! namesake that no branch picks between, as the members two impls of one type define.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Selector, confidence};
use tempfile::TempDir;

const WATCH: &str = "#[cfg(target_os = \"linux\")]
mod inotify;
#[cfg(target_os = \"macos\")]
mod notified;
#[cfg(not(any(target_os = \"linux\", target_os = \"macos\")))]
mod unsupported;

#[cfg(target_os = \"linux\")]
use inotify::Backend;
#[cfg(target_os = \"macos\")]
use notified::Backend;
#[cfg(not(any(target_os = \"linux\", target_os = \"macos\")))]
use unsupported::Backend;

pub struct Watcher;

impl Watcher {
    pub fn start() {
        event_prefixes();
        Backend::start();
    }
}

#[cfg(target_os = \"macos\")]
pub fn event_prefixes() {}

#[cfg(not(target_os = \"macos\"))]
fn event_prefixes() {}

#[cfg(target_os = \"linux\")]
pub fn linux() {
    inotify::open();
}
";

const BACKEND: &str = "pub struct Backend;

impl Backend {
    pub fn start() {}
}

pub fn open() {}
";

const APART: &str = "mod one;
mod two;

pub struct Gen<T>(T);

impl Gen<u8> {
    pub fn make() {}
}

impl Gen<u16> {
    pub fn make() {}
}

#[cfg(unix)]
fn run() {
    run();
}

#[cfg(not(unix))]
fn run() {
    run();
}

pub fn first() {
    fn local() {}
    local();
    Gen::make();
    Probe::new();
}

pub fn second() {
    fn local() {}
    local();
}

#[cfg(unix)]
pub struct Ctx;

#[cfg(unix)]
impl Ctx {
    pub fn new() {}
}

#[cfg(not(unix))]
pub struct Ctx;

#[cfg(not(unix))]
impl Ctx {
    pub fn new() {}
}

pub fn third() {
    Ctx::new();
    Probe::open();
}
";

const PROBE_ONE: &str = "pub struct Probe;

impl Probe {
    fn new() {}

    pub fn open() {}
}
";

const PROBE_TWO: &str = "pub struct Probe;

impl Probe {
    pub fn new() {}

    fn open() {}
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"fs\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub mod watch;\npub mod apart;\n"),
        ("src/watch.rs", WATCH),
        ("src/watch/inotify.rs", BACKEND),
        ("src/watch/notified.rs", BACKEND),
        ("src/watch/unsupported.rs", BACKEND),
        ("src/apart.rs", APART),
        ("src/apart/one.rs", PROBE_ONE),
        ("src/apart/two.rs", PROBE_TWO),
    ];
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["rust"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callers(engine: &Engine, selector: Selector<'_>) -> Vec<(String, u32, u8)> {
    let (_, graph) = engine
        .graph_of("src/watch.rs")
        .expect("Rust claims the file");
    let walk = graph
        .callers(selector, Limits::new(1))
        .expect("the name is defined");
    (walk.reached.iter())
        .map(|reached| {
            (
                reached.symbol.qualified_name.to_owned(),
                reached.line,
                reached.confidence,
            )
        })
        .collect()
}

const fn at_line<'s>(name: &'s str, path: &'s str, line: u32) -> Selector<'s> {
    Selector::new(name, Some(path)).with_line(Some(line))
}

#[test]
fn a_call_through_a_name_each_cfg_branch_imports_from_its_own_module_binds_every_branch_s_definition()
 {
    let (_tree, engine) = build();

    for module in ["inotify", "notified", "unsupported"] {
        let path = format!("src/watch/{module}.rs");
        assert_eq!(
            callers(&engine, Selector::new("Backend::start", Some(&path))),
            [("Watcher::start".to_owned(), 20, confidence::RECEIVER)],
            "{module}"
        );
    }
}

#[test]
fn a_call_of_a_function_each_cfg_branch_defines_binds_both_definitions() {
    let (_tree, engine) = build();

    for line in [25, 28] {
        assert_eq!(
            callers(&engine, at_line("event_prefixes", "src/watch.rs", line)),
            [("Watcher::start".to_owned(), 19, confidence::SAME_FILE)],
            "line {line}"
        );
    }
}

#[test]
fn the_callees_of_a_caller_of_cfg_twins_list_every_twin() {
    let (_tree, engine) = build();
    let (_, graph) = engine
        .graph_of("src/watch.rs")
        .expect("Rust claims the file");

    let walk = graph
        .callees("Watcher::start", Limits::new(1))
        .expect("the name is defined");

    let mut callees: Vec<(&str, &str, u32)> = (walk.reached.iter())
        .map(|reached| {
            let callee = &reached.symbol;
            (callee.qualified_name, callee.file, callee.line)
        })
        .collect();
    callees.sort_unstable();
    assert_eq!(
        callees,
        [
            ("Backend::start", "src/watch/inotify.rs", 4),
            ("Backend::start", "src/watch/notified.rs", 4),
            ("Backend::start", "src/watch/unsupported.rs", 4),
            ("event_prefixes", "src/watch.rs", 25),
            ("event_prefixes", "src/watch.rs", 28),
        ]
    );
}

#[test]
fn a_call_of_a_member_two_impls_of_one_type_define_binds_one_of_them() {
    let (_tree, engine) = build();

    assert_eq!(
        callers(&engine, at_line("Gen::make", "src/apart.rs", 7)),
        [("first".to_owned(), 27, confidence::RECEIVER)]
    );
    assert_eq!(
        callers(&engine, at_line("Gen::make", "src/apart.rs", 11)),
        []
    );
}

#[test]
fn a_call_of_a_member_of_a_type_each_cfg_branch_defines_in_one_file_binds_both_members() {
    let (_tree, engine) = build();

    for line in [41, 49] {
        assert_eq!(
            callers(&engine, at_line("Ctx::new", "src/apart.rs", line)),
            [("third".to_owned(), 53, confidence::RECEIVER)],
            "line {line}"
        );
    }
}

#[test]
fn a_call_inside_one_cfg_twin_binds_no_other_twin() {
    let (_tree, engine) = build();

    assert_eq!(callers(&engine, at_line("run", "src/apart.rs", 20)), []);
}

#[test]
fn a_call_of_a_function_local_to_one_body_binds_no_namesake_local_to_another() {
    let (_tree, engine) = build();

    assert_eq!(
        callers(&engine, at_line("local", "src/apart.rs", 32)),
        [("second".to_owned(), 33, confidence::SAME_FILE)]
    );
}

#[test]
fn a_call_of_a_private_member_binds_no_exported_namesake_in_another_module() {
    let (_tree, engine) = build();

    assert_eq!(
        callers(
            &engine,
            Selector::new("Probe::new", Some("src/apart/one.rs"))
        ),
        [("first".to_owned(), 28, confidence::RECEIVER)]
    );
    assert_eq!(
        callers(
            &engine,
            Selector::new("Probe::new", Some("src/apart/two.rs"))
        ),
        []
    );
}

#[test]
fn a_call_of_an_exported_member_binds_no_private_namesake_in_another_module() {
    let (_tree, engine) = build();

    assert_eq!(
        callers(
            &engine,
            Selector::new("Probe::open", Some("src/apart/one.rs"))
        ),
        [("third".to_owned(), 54, confidence::RECEIVER)]
    );
    assert_eq!(
        callers(
            &engine,
            Selector::new("Probe::open", Some("src/apart/two.rs"))
        ),
        []
    );
}

#[test]
fn a_call_through_the_path_of_one_branch_s_module_binds_that_branch_alone() {
    let (_tree, engine) = build();

    assert_eq!(
        callers(&engine, Selector::new("open", Some("src/watch/inotify.rs"))),
        [("linux".to_owned(), 32, confidence::IMPORT)]
    );
    for module in ["notified", "unsupported"] {
        let path = format!("src/watch/{module}.rs");
        assert_eq!(
            callers(&engine, Selector::new("open", Some(&path))),
            [],
            "{module}"
        );
    }
}

#[test]
fn no_cfg_twin_of_a_called_definition_is_unused() {
    let (_tree, engine) = build();
    let (_, graph) = engine
        .graph_of("src/watch.rs")
        .expect("Rust claims the file");

    let dead = graph.dead_code(&[], Some("src/watch"), 50);

    let names: Vec<(&str, &str, u32)> = (dead.found.iter())
        .map(|dead| {
            (
                dead.definition.qualified_name,
                dead.definition.file,
                dead.definition.line,
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("Watcher::start", "src/watch.rs", 18),
            ("linux", "src/watch.rs", 31)
        ]
    );
}
