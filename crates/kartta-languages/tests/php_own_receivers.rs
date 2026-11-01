//! What a PHP member call binds of the caller's own class: `$this`, `self` and `static` bind its member at the caller's
//! owner tier, and a variable the walk reads no class of binds it only as a guess. A constant is no call's target
//! through any of them.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, confidence};

const LOOP: &str = r"<?php
namespace App;

class Server {
    function close() {}
}

class Loop {
    function close() {}
    function accept($conn) { $conn->close(); }
    function stop() { $this->close(); }
    function viaSelf() { self::close(); }
    function viaStatic() { static::close(); }
    function viaUpperCase() { SELF::close(); }
}
";

const LISTENER: &str = r"<?php
class Listener { public function preFoo() {} }
class DispatchTest {
  private const preFoo = 'preFoo';
  public function run($test) { $test->preFoo(); }
}
";

const ONLY_CONSTANTS: &str = r"<?php
const TOP = 1;

class Other {
    const SHARED = 2;
}

class Holder {
    const OWN = 3;
    function onVariable($value) { $value->OWN(); }
    function onThis() { $this->OWN(); }
    function onSelf() { self::OWN(); }
    function onClass() { Other::SHARED(); }
    function onNothing() { TOP(); }
}
";

const BRACES: &str = r#"<?php
namespace App;

const X = "real";

$X = new class {};

class Suite {
    public function real() {}
    public function X() {}
    public function run() { $this->{X}(); }
    public function nullsafe() { $this?->{X}(); }
    public function onVariable($o) { $o::{X}(); }
    public function onSelf() { self::{X}(); }
    public function onClass() { Suite::{X}(); }
    public function spelled() { $this->real(); }
}

function outside($o) { $o->{X}(); }
"#;

fn build(source: &str) -> Engine {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("Loop.php"), source).expect("the file is written");
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    outcome.engine
}

fn callees(engine: &Engine, name: &str) -> Vec<(String, u8)> {
    let (_, graph) = engine.graph_of("Loop.php").expect("PHP claims the file");
    let walk = graph
        .callees(name, Limits::new(1))
        .expect("the name is defined");
    walk.reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

#[test]
fn a_php_call_on_a_variable_binds_no_class_constant_of_its_name() {
    let engine = build(LISTENER);

    assert_eq!(
        callees(&engine, "DispatchTest::run"),
        [("Listener::preFoo".to_owned(), confidence::SAME_FILE)],
    );
}

#[test]
fn a_php_constant_is_no_call_s_target_through_any_receiver() {
    let engine = build(ONLY_CONSTANTS);

    for caller in ["onVariable", "onThis", "onSelf", "onClass", "onNothing"] {
        assert_eq!(
            callees(&engine, &format!("Holder::{caller}")),
            [],
            "{caller}"
        );
    }
}

#[test]
fn a_php_constant_in_braces_binds_nothing_through_its_spelling() {
    let engine = build(BRACES);

    for caller in [
        "App\\Suite::run",
        "App\\Suite::nullsafe",
        "App\\Suite::onVariable",
        "App\\Suite::onSelf",
        "App\\Suite::onClass",
        "App\\outside",
    ] {
        assert_eq!(callees(&engine, caller), [], "{caller}");
    }
    assert_eq!(
        callees(&engine, "App\\Suite::spelled"),
        [("App\\Suite::real".to_owned(), confidence::SAME_FILE)],
    );
}

#[test]
fn a_php_member_call_binds_the_caller_s_class_by_its_receiver() {
    let engine = build(LOOP);
    let callees = |name| callees(&engine, name);
    let close = |tier| vec![("App\\Loop::close".to_owned(), tier)];

    assert_eq!(callees("App\\Loop::stop"), close(confidence::SAME_FILE));
    assert_eq!(callees("App\\Loop::viaSelf"), close(confidence::SAME_FILE));
    assert_eq!(
        callees("App\\Loop::viaStatic"),
        close(confidence::SAME_FILE)
    );
    assert_eq!(
        callees("App\\Loop::viaUpperCase"),
        close(confidence::SAME_FILE),
        "PHP reads a scope keyword in any case"
    );
    assert_eq!(
        callees("App\\Loop::accept"),
        close(confidence::FIRST_PARTY),
        "a variable's type is unknown, so its call is a guess at the caller's class"
    );
}
