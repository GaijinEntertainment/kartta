//! What a PHP call on a property of `$this` binds: a member of the class the property holds, past a namesake, and
//! nothing for a class the graph does not hold.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const COMPOSER: (&str, &str) = (
    "composer.json",
    r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
);

/// A class of the name a property's type names from outside the tree, whose method the name tiers would bind.
const NAMESAKE: (&str, &str) = (
    "src/Support/Client.php",
    r"<?php
namespace App\Support;

class Client {
    public function send() {}
}
",
);

/// The qualified name of each callee of `caller`, the first file's.
fn callees(files: &[(&str, &str)], caller: &str) -> Vec<String> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
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
    let mut reached: Vec<String> = graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| reached.symbol.qualified_name.to_owned())
        .collect();
    reached.sort();
    reached
}

fn kernel(members: &str) -> String {
    format!(
        r"<?php
namespace App;

class Container {{
    public function instance($k) {{}}
}}
class Other {{
    public function instance($k) {{}}
}}
class Kernel {{
{members}
    public function run() {{
        $this->container->instance(1);
    }}
}}
"
    )
}

fn callees_of_run(members: &str) -> Vec<String> {
    let source = kernel(members);
    let files = [("src/Kernel.php", source.as_str()), COMPOSER];
    callees(&files, "App\\Kernel::run")
}

#[test]
fn a_call_on_a_property_a_constructor_writes_binds_the_class() {
    assert_eq!(
        callees_of_run(
            "private $container;
             public function __construct() { $this->container = new Container; }",
        )
        .last()
        .map(String::as_str),
        Some("App\\Container::instance"),
    );
}

#[test]
fn a_call_on_a_typed_or_a_promoted_property_binds_the_class() {
    for members in [
        "private Container $container;",
        "private ?Other $unrelated; private ?Container $container;",
        "public function __construct(private Container $container) {}",
    ] {
        assert_eq!(
            callees_of_run(members),
            ["App\\Container::instance"],
            "{members}"
        );
    }
}

#[test]
fn two_classes_written_to_a_property_bind_neither() {
    let reached = callees_of_run(
        "private $container;
         public function __construct() { $this->container = new Container; }
         public function swap() { $this->container = new Other; }",
    );
    assert!(
        !reached.iter().any(|name| name.ends_with("::instance")),
        "{reached:?}",
    );
}

#[test]
fn a_property_of_a_class_outside_the_graph_binds_no_namesake() {
    let source = r"<?php
namespace App;

use Vendor\Client;

class Sender {
    private Client $client;
    public function run() { $this->client->send(); }
}
";
    let files = [("src/Sender.php", source), NAMESAKE, COMPOSER];
    assert_eq!(callees(&files, "App\\Sender::run"), [""; 0]);
}

#[test]
fn a_property_of_a_class_of_the_tree_binds_past_its_namesake_in_the_same_file() {
    let source = r"<?php
namespace App;

use App\Support\Client;

class Near {
    public function send() {}
}
class Sender {
    private Client $client;
    public function run() { $this->client->send(); }
}
";
    let files = [("src/Sender.php", source), NAMESAKE, COMPOSER];
    assert_eq!(
        callees(&files, "App\\Sender::run"),
        ["App\\Support\\Client::send"],
    );
}

#[test]
fn a_call_on_a_static_property_binds_the_class_it_holds() {
    let source = r"<?php
namespace App;
class Container {
    public function instance($k) {}
}
class Other {
    public function instance($k) {}
}
class Child {
    private static Container $shared;
    public function viaStatic() { self::$shared->instance(1); }
    public function viaLate() { static::$shared->instance(1); }
    public function viaName() { Child::$shared->instance(1); }
}
";
    let files = [("src/A.php", source), COMPOSER];
    for caller in ["viaStatic", "viaLate", "viaName"] {
        assert_eq!(
            callees(&files, &format!("App\\Child::{caller}")),
            ["App\\Container::instance"],
            "{caller}",
        );
    }
}

#[test]
fn a_write_from_a_parent_or_a_trait_is_outside_what_the_caller_reads() {
    let base = r"<?php
namespace App;

class Base {
    protected $container;
    public function __construct() { $this->container = new Container; }
}
trait Holds {
    private Container $container;
}
class Child extends Base {
    public function run() { $this->container->instance(1); }
}
class Using {
    use Holds;
    public function run() { $this->container->instance(1); }
}
class Container {
    public function instance($k) {}
}
class Other {
    public function instance($k) {}
}
";
    let files = [("src/Base.php", base), COMPOSER];
    for caller in ["App\\Child::run", "App\\Using::run"] {
        let reached = callees(&files, caller);
        assert!(
            !reached.contains(&"App\\Container::instance".to_owned()),
            "{caller}: {reached:?}",
        );
    }
}
