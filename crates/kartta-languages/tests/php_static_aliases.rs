//! What a static PHP call on a `use` alias and on a class outside the tree binds: an alias of a class the tree holds is
//! a call on that class, and any other reaches no method of the tree.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const FILES: [(&str, &str); 8] = [
    (
        "src/T/Calls.php",
        r"<?php
namespace T;

use App\Model as Eloquent;
use App\Link;
use Mockery;
use Testing\Assert as PHPUnit;
use Vendor\Thing as Response;
use Assert as GlobalAssert;
use App\Assert as Unmapped;

class Calls {
    public function inherited() { Eloquent::timestamps(); }
    public function outside() { Mockery::close(); }
    public function outsideAlias() { PHPUnit::assertSame(); }
    public function aliasOfATreeClass() { Response::held(); }
    public function aliasOfAGlobalClass() { GlobalAssert::assertSame(); }
    public function aliasPsr4MapsNoFileTo() { Unmapped::assertSame(); }
    public function memberTheClassLacks() { Eloquent::close(); }
}
",
    ),
    (
        "src/T/Model.php",
        r"<?php
namespace T;

class Model {
    public static function timestamps() {}
    public static function held() {}
}
",
    ),
    (
        "src/App/Model.php",
        r"<?php
namespace App;

class Model {
    use HasTimestamps;
    public static function held() {}
}
",
    ),
    (
        "src/App/HasTimestamps.php",
        r"<?php
namespace App;

trait HasTimestamps {
    public static function timestamps() {}
}
",
    ),
    (
        "src/App/Response.php",
        r"<?php
namespace App;

class Response {
    public function close() {}
    public static function held() {}
}

class Assert {
    public static function assertSame() {}
}
",
    ),
    (
        "src/App/Link.php",
        r"<?php
namespace App;

use App\Response as Reply;

class Link extends Reply {}
",
    ),
    (
        "src/T/Blocks.php",
        r"<?php
namespace T\One {
    use App\Model as Thing;

    class First {
        public function run() { Thing::held(); }
    }
}

namespace T\Two {
    use App\Response as Thing;

    class Second {
        public function run() { Thing::held(); }
        public function qualified() { Sub\Thing::held(); }
    }
}
",
    ),
    ("composer.json", r#"{"autoload": {"psr-4": {"": "src/"}}}"#),
];

/// The qualified name and the confidence of each callee of `caller`.
fn callees(caller: &str) -> Vec<(String, u8)> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in FILES {
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
        .graph_of(FILES[0].0)
        .expect("PHP claims the file");
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

#[test]
fn a_static_call_on_an_alias_binds_a_member_of_a_trait_its_class_uses() {
    assert_eq!(
        callees("T\\Calls::inherited"),
        [("App\\HasTimestamps::timestamps".to_owned(), 85)]
    );
}

#[test]
fn an_alias_of_a_class_no_autoload_rule_maps_binds_the_class_it_names() {
    assert_eq!(
        callees("T\\Calls::aliasPsr4MapsNoFileTo"),
        [("App\\Assert::assertSame".to_owned(), 85)]
    );
}

#[test]
fn a_member_the_alias_s_class_lacks_stays_with_the_tiers_that_read_no_class() {
    assert_eq!(
        callees("T\\Calls::memberTheClassLacks"),
        [("App\\Response::close".to_owned(), 60)]
    );
}

#[test]
fn an_alias_of_an_outside_class_binds_no_class_of_the_alias_s_name() {
    assert_eq!(callees("T\\Calls::aliasOfATreeClass"), []);
}

#[test]
fn an_alias_of_an_outside_global_class_binds_no_class_of_its_name() {
    assert_eq!(callees("T\\Calls::aliasOfAGlobalClass"), []);
}

#[test]
fn an_alias_names_the_class_its_own_namespace_block_imports() {
    assert_eq!(
        callees("T\\One\\First::run"),
        [("App\\Model::held".to_owned(), 85)]
    );
    assert_eq!(
        callees("T\\Two\\Second::run"),
        [("App\\Response::held".to_owned(), 85)]
    );
}

#[test]
fn the_last_segment_of_a_qualified_receiver_is_no_alias() {
    assert_eq!(callees("T\\Two\\Second::qualified"), []);
}

#[test]
fn a_static_call_on_a_class_outside_the_graph_binds_no_method_an_import_holds() {
    assert_eq!(callees("T\\Calls::outside"), []);
}

#[test]
fn a_static_call_on_an_alias_of_an_outside_class_binds_no_class_of_its_last_name() {
    assert_eq!(callees("T\\Calls::outsideAlias"), []);
}
