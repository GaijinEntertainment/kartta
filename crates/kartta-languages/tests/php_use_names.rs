//! What a parent a PHP file writes reaches through the file's `use` map and its namespace: a parent written through an
//! alias is the class the alias imports, and a parent is never a namesake of the class PHP compiles the name to.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const COMPOSER: (&str, &str) = ("composer.json", r#"{"autoload": {"psr-4": {"": "src/"}}}"#);

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
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
    (tree, outcome.engine)
}

/// The qualified name and the confidence of each callee of `caller`.
fn callees(files: &[(&str, &str)], caller: &str) -> Vec<(String, u8)> {
    let (_tree, engine) = build(files);
    let (_, graph) = engine.graph_of(files[0].0).expect("PHP claims the file");
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

const ALIASED: [(&str, &str); 2] = [
    (
        "src/N/D.php",
        r"<?php
namespace N;

use A\B as C;

class D extends C {
    public function run() { $this->held(); }
    public function held2() { parent::held(); }
}
",
    ),
    (
        "src/A/B.php",
        r"<?php
namespace A;

class B {
    public function held() {}
}
",
    ),
];

#[test]
fn find_implementations_lists_a_subclass_written_through_an_alias() {
    let (_tree, engine) = build(&ALIASED);
    let (_, graph) = engine.graph_of("src/A/B.php").expect("PHP claims the file");

    let implementations = graph
        .implementations("A\\B", 1, 10)
        .expect("the name is defined");

    let names: Vec<&str> = implementations
        .found
        .iter()
        .map(|implementor| implementor.symbol.qualified_name)
        .collect();
    assert_eq!(names, ["N\\D"]);
}

#[test]
fn a_call_reaches_the_member_of_a_parent_written_through_an_alias() {
    let held = || [("A\\B::held".to_owned(), 85)];

    assert_eq!(callees(&ALIASED, "N\\D::run"), held());
    assert_eq!(callees(&ALIASED, "N\\D::held2"), held());
}

/// `Base` in `Schema` and `Query` extends the alias of the outer `Base`, and `My` reaches that class through what it
/// includes, nearer than the `Base` of its own directory.
const NAMESAKE_PARENTS: [(&str, &str); 5] = [
    (
        "src/Db/Schema/My.php",
        r"<?php
namespace Db\Schema;

use Db\Link;

class My extends Base {
    public function compile() { $this->wrap(); }
}
",
    ),
    (
        "src/Db/Link.php",
        r"<?php
namespace Db;

use Db\Base as Outer;

class Link extends Outer {}
",
    ),
    (
        "src/Db/Schema/Base.php",
        r"<?php
namespace Db\Schema;

use Db\Base as Outer;

class Base extends Outer {
    public function wrap() {}
}
",
    ),
    (
        "src/Db/Base.php",
        r"<?php
namespace Db;

class Base {
    public function wrap() {}
}
",
    ),
    (
        "src/Db/Query/Base.php",
        r"<?php
namespace Db\Query;

use Db\Base as Outer;

class Base extends Outer {
    public function compile() { $this->wrap(); }
}
",
    ),
];

#[test]
fn a_parent_of_the_file_s_namespace_stands_ahead_of_a_namesake_an_include_reaches() {
    assert_eq!(
        callees(&NAMESAKE_PARENTS, "Db\\Schema\\My::compile"),
        [("Db\\Schema\\Base::wrap".to_owned(), 85)]
    );
}

#[test]
fn a_parent_written_through_an_alias_is_reached_past_a_class_of_its_name() {
    assert_eq!(
        callees(&NAMESAKE_PARENTS, "Db\\Query\\Base::compile"),
        [("Db\\Base::wrap".to_owned(), 85)]
    );
}

/// `Base` is a class outside the tree, and the file reaches a class of that name in another namespace.
const OUTSIDE_PARENT: [(&str, &str); 3] = [
    (
        "src/N/D.php",
        r"<?php
namespace N;

use Vendor\Base;
use M\Helper;

class D extends Base {
    public function viaParent() { parent::held(); }
    public function viaThis() { $this->held(); }
}
",
    ),
    (
        "src/M/Helper.php",
        r"<?php
namespace M;

use M\Base as Other;

class Helper extends Other {}
",
    ),
    (
        "src/M/Base.php",
        r"<?php
namespace M;

class Base {
    public function held() {}
}
",
    ),
];

#[test]
fn a_parent_outside_the_tree_is_no_namesake_the_file_reaches() {
    assert_eq!(callees(&OUTSIDE_PARENT, "N\\D::viaParent"), []);
    assert_eq!(callees(&OUTSIDE_PARENT, "N\\D::viaThis"), []);
}
