//! What a PHP member call binds of the caller's own class: `$this`, `self` and `static` bind its member at the caller's
//! owner tier, and a variable, whose type the graph does not follow, binds it only as a guess.

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

#[test]
fn a_php_member_call_binds_the_caller_s_class_by_its_receiver() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("Loop.php"), LOOP).expect("the file is written");
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of("Loop.php")
        .expect("PHP claims the file");
    let callees = |name| {
        let walk = graph
            .callees(name, Limits::new(1))
            .expect("the name is defined");
        walk.reached
            .iter()
            .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
            .collect::<Vec<_>>()
    };
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
