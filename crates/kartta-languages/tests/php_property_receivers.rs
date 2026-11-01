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
fn a_call_on_what_a_chained_assignment_wrote_binds_the_class() {
    let source = r"<?php
namespace App;

class DB {
    public function connect() {}
}
class Other {
    public function connect() {}
}
class Uses {
    private $db;
    private $plain;
    public function __construct() {
        $this->db = $db = new DB;
        $this->plain = new DB;
    }
    public function viaNested() { $this->db->connect(); }
    public function viaPlain() { $this->plain->connect(); }
    public function viaLocal() { $held = $this->fresh = new DB; $held->connect(); }
}
";
    let files = [("src/A.php", source), COMPOSER];
    for caller in ["viaNested", "viaPlain"] {
        assert_eq!(
            callees(&files, &format!("App\\Uses::{caller}")),
            ["App\\DB::connect"],
            "{caller}",
        );
    }
    assert_eq!(
        callees(&files, "App\\Uses::viaLocal"),
        ["App\\DB", "App\\DB::connect"],
    );
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

const NARROWED: &str = r"<?php
namespace App;
use Vendor\Outside;
interface Decider {
    public function decide();
}
class Traced implements Decider {
    public function decide() {}
    public function voters() {}
}
class Plain {
    public function voters() {}
}
class Namesake {
    public function outside() {}
}
class Collector {
    private Decider $manager;
    public function run() {
        if ($this->manager instanceof Traced) {
            $this->manager->voters();
        }
    }
    public function local(Decider $d) {
        if ($d instanceof Traced) {
            $d->voters();
        }
    }
    public function early(Decider $d) {
        if (!$d instanceof Traced) {
            return;
        }
        $d->voters();
    }
    public function declared(Decider $d) {
        if ($d instanceof Outside) {
            $d->decide();
            $d->outside();
        }
    }
}
";

#[test]
fn a_member_only_a_child_holds_binds_past_an_instanceof() {
    let files = [("src/A.php", NARROWED), COMPOSER];
    for caller in ["run", "local", "early"] {
        assert_eq!(
            callees(&files, &format!("App\\Collector::{caller}")),
            ["App\\Traced::voters"],
            "{caller}",
        );
    }
}

#[test]
fn a_test_for_a_class_outside_the_graph_binds_no_namesake() {
    let files = [("src/A.php", NARROWED), COMPOSER];
    assert_eq!(callees(&files, "App\\Collector::declared"), [""; 0]);
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
fn a_scoped_call_on_a_property_binds_the_class_it_holds() {
    let source = r"<?php
namespace App;
class Container {
    public static function make() {}
    public function instance($k) {}
}
class Other {
    public static function make() {}
    public function instance($k) {}
}
class Uses {
    private static Container $own;
    private Container $held;
    private $written;
    public function __construct() { $this->written = new Container; }
    public function viaColon() { self::$own::make(); }
    public function viaColonLate() { static::$own::make(); }
    public function viaColonThis() { $this->held::make(); }
    public function viaColonWrite() { $this->written::make(); }
    public function viaColonInstance() { $this->held::instance(1); }
    public function viaArrow() { $this->held->instance(1); }
}
";
    let files = [("src/A.php", source), COMPOSER];
    for (caller, callee) in [
        ("viaColon", "App\\Container::make"),
        ("viaColonLate", "App\\Container::make"),
        ("viaColonThis", "App\\Container::make"),
        ("viaColonWrite", "App\\Container::make"),
        ("viaColonInstance", "App\\Container::instance"),
        ("viaArrow", "App\\Container::instance"),
    ] {
        assert_eq!(
            callees(&files, &format!("App\\Uses::{caller}")),
            [callee],
            "{caller}",
        );
    }
}

#[test]
fn a_scoped_call_on_a_string_property_binds_nothing() {
    let source = r"<?php
namespace App;
class Container {
    public static function make() {}
}
class Other {
    public static function make() {}
}
class Uses {
    private string $class = Container::class;
    private static string $late;
    public function viaThis() { $this->class::make(); }
    public function viaLate() { static::$late::make(); }
}
";
    let files = [("src/A.php", source), COMPOSER];
    for caller in ["viaThis", "viaLate"] {
        assert_eq!(
            callees(&files, &format!("App\\Uses::{caller}")),
            [""; 0],
            "{caller}",
        );
    }
}

#[test]
fn a_write_to_a_computed_static_name_binds_no_call_on_the_text_in_its_braces() {
    let source = r#"<?php
namespace App;
class Container {
    public function m() {}
}
class Other {
    public function m() {}
}
const X = "other";
class Suite {
    public static $other;
    public static $X;
    public static function set() { self::${X} = new Container; }
    public function run() { self::$X->m(); }
}
"#;
    let spelled = source.replace("self::${X} =", "self::$X =");
    assert_eq!(
        callees(&[("src/A.php", &spelled), COMPOSER], "App\\Suite::run"),
        ["App\\Container::m"],
    );
    assert_eq!(
        callees(&[("src/A.php", source), COMPOSER], "App\\Suite::run"),
        [""; 0],
    );
}

/// What `App\Child::run` calls, where `child` is the body of `Child`, which starts with `class Child extends Base` and
/// holds its call on `$this->container`, `above` declares `Base` and more in a file of its own, and two classes hold
/// `instance`.
fn callees_of_a_child(above: &str, child: &str) -> Vec<String> {
    let child = format!(
        "<?php
namespace App;

class Child extends Base {{
{child}
}}
"
    );
    let above = format!(
        "<?php
namespace App;

class Container {{
    public function instance($k) {{}}
}}
class Other {{
    public function instance($k) {{}}
}}
{above}
"
    );
    let files = [
        ("src/Child.php", child.as_str()),
        ("src/Base.php", above.as_str()),
        COMPOSER,
    ];
    callees(&files, "App\\Child::run")
}

const RUN: &str = "public function run() { $this->container->instance(1); }";
const CONTAINER_INSTANCE: [&str; 1] = ["App\\Container::instance"];

#[test]
fn a_call_on_a_property_a_parent_gives_binds_the_class_in_another_file() {
    for above in [
        "class Base { protected Container $container; }",
        "class Base { public ?Container $container; }",
        "class Base { public function __construct(protected Container $container) {} }",
        "class Base {
             protected $container;
             public function __construct() { $this->container = new Container; }
         }",
        "class Base {
             protected $container;
             public function __construct(Container $c) { $this->container = $c; }
         }",
        "class Top { protected Container $container; }
         class Base extends Top {}",
        "trait Holds { protected Container $container; }
         class Base { use Holds; }",
    ] {
        assert_eq!(
            callees_of_a_child(above, RUN),
            CONTAINER_INSTANCE,
            "{above}"
        );
    }
}

#[test]
fn a_private_property_above_the_caller_s_class_binds_nothing() {
    for above in [
        "class Base { private Container $container; }",
        "trait Holds { private Container $container; }
         class Base { use Holds; }",
        "class Top { protected Other $container; }
         class Base extends Top { private Container $container; }",
    ] {
        assert_eq!(callees_of_a_child(above, RUN), [""; 0], "{above}");
    }
}

#[test]
fn a_private_property_of_a_trait_the_class_uses_binds_the_class() {
    let trait_of_a_trait = "trait Inner { private Container $container; }
                            trait Holds { use Inner; }
                            class Base {}";
    let trait_of_its_own = "trait Holds { private Container $container; }
                            class Base {}";
    let child = format!("use Holds;\n{RUN}");
    for above in [trait_of_its_own, trait_of_a_trait] {
        assert_eq!(
            callees_of_a_child(above, &child),
            CONTAINER_INSTANCE,
            "{above}"
        );
    }
}

#[test]
fn the_nearest_declaration_of_a_property_answers() {
    let above = "class Top { protected Other $container; }
                 class Base extends Top { protected Other $container; }";
    assert_eq!(
        callees_of_a_child(above, &format!("public Other $x;\n{RUN}")),
        ["App\\Other::instance"],
    );
    let says_nothing = "class Top { protected Container $container; }
                        class Base extends Top { protected $container; }";
    assert_eq!(callees_of_a_child(says_nothing, RUN), [""; 0]);
}

#[test]
fn a_property_of_a_parent_that_holds_no_class_binds_nothing() {
    for above in [
        "class Base { protected $container; }",
        "class Base { protected string $container; }",
        "class Base { protected $other; }",
        "class Base {
             protected $container;
             public function __construct() { $this->container = new Container; $this->container = new Other; }
         }",
    ] {
        assert_eq!(callees_of_a_child(above, RUN), [""; 0], "{above}");
    }
}

#[test]
fn a_parent_outside_the_graph_binds_nothing_on_a_property_it_may_hold() {
    let child = "<?php
namespace App;

class Child extends \\Vendor\\Base {
    public function run() { $this->container->instance(1); }
}
";
    let namesakes = "<?php
namespace App;

class Container {
    public function instance($k) {}
}
class Other {
    public function instance($k) {}
}
";
    let files = [
        ("src/Child.php", child),
        ("src/Container.php", namesakes),
        COMPOSER,
    ];
    assert_eq!(callees(&files, "App\\Child::run"), [""; 0]);
}

#[test]
fn a_keyword_of_a_parent_s_property_names_the_class_it_names_there() {
    let self_type = "class Base {
                         protected self $container;
                         public function instance($k) {}
                     }";
    let run = format!("public function instance($k) {{}}\n{RUN}");
    assert_eq!(callees_of_a_child(self_type, &run), ["App\\Base::instance"]);
    let parent_type = "class Top {
                           public function instance($k) {}
                       }
                       class Base extends Top { protected parent $container; }";
    assert_eq!(
        callees_of_a_child(parent_type, &run),
        ["App\\Top::instance"]
    );
    let created = "class Base {
                       protected $container;
                       public function instance($k) {}
                       public function set() { $this->container = new static; }
                   }";
    assert_eq!(callees_of_a_child(created, &run), ["App\\Child::instance"]);
}

#[test]
fn a_parent_s_static_property_binds_the_class_through_a_child_s_own_names() {
    let above = "class Base { protected static Container $container; }";
    for read in [
        "self::$container->instance(1);",
        "static::$container->instance(1);",
        "Child::$container->instance(1);",
    ] {
        let child = format!("public function run() {{ {read} }}");
        assert_eq!(
            callees_of_a_child(above, &child),
            CONTAINER_INSTANCE,
            "{read}"
        );
    }
}

#[test]
fn a_scoped_call_on_a_parent_s_property_binds_the_class() {
    let above = "class Base { protected Container $container; }";
    let child = "public function run() { $this->container::instance(1); }";
    assert_eq!(callees_of_a_child(above, child), CONTAINER_INSTANCE);
}

#[test]
fn a_property_named_as_a_method_of_the_parent_is_no_property() {
    let above = "class Base {
                     public function container() {}
                     public function __construct(protected $other = null) {}
                 }";
    assert_eq!(callees_of_a_child(above, RUN), [""; 0]);
}

#[test]
fn a_property_nothing_above_names_leaves_the_call_to_the_name_tiers() {
    let child = "<?php
namespace App;

class Child extends Base {
    public function run() { $this->container->only(1); }
}
";
    let above = "<?php
namespace App;

class Base {}
class Lone {
    public function only($k) {}
}
";
    let files = [("src/Child.php", child), ("src/Base.php", above), COMPOSER];
    assert_eq!(callees(&files, "App\\Child::run"), ["App\\Lone::only"]);
}

#[test]
fn a_property_nothing_above_names_is_no_receiver_a_namesake_of_the_caller_s_class_answers() {
    let caller = "<?php
class Base {}
$x = new class extends Base {
    public function run() { $this->container->only(); }
};
";
    let namesake = "<?php
$x = new class {
    public function only() {}
};
";
    let others = "<?php
class One {
    public function only() {}
}
class Two {
    public function only() {}
}
";
    let files = [
        ("src/caller.php", caller),
        ("src/namesake.php", namesake),
        ("src/others.php", others),
    ];
    assert_eq!(callees(&files, "{x}::run"), [""; 0]);
}

#[test]
fn a_property_of_an_anonymous_class_in_a_parent_is_not_the_parent_s() {
    let above = "class Base {
                     public function make() {
                         return new class { public Container $container; };
                     }
                 }";
    assert_eq!(callees_of_a_child(above, RUN), [""; 0]);
}

#[test]
fn a_property_of_a_namesake_of_the_parent_in_its_file_is_not_the_parent_s() {
    let child = "<?php
namespace App;

class Child extends Base {
    public function run() { $this->container->instance(1); }
}
";
    let above = "<?php
namespace Elsewhere {
    class Base { public \\App\\Container $container; }
}
namespace App {
    class Container {
        public function instance($k) {}
    }
    class Other {
        public function instance($k) {}
    }
    class Base {}
}
";
    let files = [("src/Child.php", child), ("src/Base.php", above), COMPOSER];
    assert_eq!(callees(&files, "App\\Child::run"), [""; 0]);
}

#[test]
fn a_cycle_of_parents_ends_the_read() {
    let above = "class Base extends Child {}";
    assert_eq!(callees_of_a_child(above, RUN), [""; 0]);
}

#[test]
fn a_write_in_the_child_reads_ahead_of_a_parent_s_declaration() {
    let above = "class Base { protected Other $container; }";
    let child =
        format!("public function __construct() {{ $this->container = new Container; }}\n{RUN}");
    assert_eq!(callees_of_a_child(above, &child), CONTAINER_INSTANCE);
}
