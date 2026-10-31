//! Which class a static PHP call starts its parent walk from where a transitive include also reaches a class of the
//! receiver's name: the class the file imports, else the one of its namespace.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const COMPOSER: (&str, &str) = ("composer.json", r#"{"autoload": {"psr-4": {"": "src/"}}}"#);

/// The qualified name and the confidence of each callee of `caller`.
fn callees(files: &[(&str, &str)], caller: &str) -> Vec<(String, u8)> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files.iter().chain([&COMPOSER]) {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["php"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of(files[0].0)
        .expect("PHP claims the file");
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

/// `Far\Thing` sorts ahead of `Near\Thing`, and the caller's file reaches it through `Link`.
const OWN_NAMESPACE: [(&str, &str); 5] = [
    (
        "src/Near/Own.php",
        r"<?php
namespace Near;

use Other\Link;

class Own extends Link {
    public function run() { Thing::inherited(); }
}
",
    ),
    (
        "src/Near/Thing.php",
        r"<?php
namespace Near;

class Thing extends Parental {}
",
    ),
    (
        "src/Near/Parental.php",
        r"<?php
namespace Near;

class Parental {
    public static function inherited() {}
}
",
    ),
    (
        "src/Other/Link.php",
        r"<?php
namespace Other;

use Far\Thing;

class Link extends Thing {}
",
    ),
    (
        "src/Far/Thing.php",
        r"<?php
namespace Far;

class Base {
    public static function inherited() {}
}

class Thing extends Base {}
",
    ),
];

#[test]
fn a_receiver_of_the_caller_s_namespace_stands_ahead_of_a_namesake_an_include_reaches() {
    assert_eq!(
        callees(&OWN_NAMESPACE, "Near\\Own::run"),
        [("Near\\Parental::inherited".to_owned(), 85)]
    );
}

/// `Aside\Thing` sorts ahead of `Far\Thing`, and the caller's file reaches it through `Link`.
const IMPORTED: [(&str, &str); 4] = [
    (
        "src/Near/Own.php",
        r"<?php
namespace Near;

use Far\Thing;
use Other\Link;

class Own {
    public function run() { Thing::inherited(); }
}
",
    ),
    (
        "src/Far/Thing.php",
        r"<?php
namespace Far;

class Base {
    public static function inherited() {}
}

class Thing extends Base {}
",
    ),
    (
        "src/Other/Link.php",
        r"<?php
namespace Other;

use Aside\Thing;

class Link {}
",
    ),
    (
        "src/Aside/Thing.php",
        r"<?php
namespace Aside;

class Parental {
    public static function inherited() {}
}

class Thing extends Parental {}
",
    ),
];

#[test]
fn a_receiver_the_file_imports_stands_ahead_of_a_namesake_an_include_reaches() {
    assert_eq!(
        callees(&IMPORTED, "Near\\Own::run"),
        [("Far\\Base::inherited".to_owned(), 85)]
    );
}
