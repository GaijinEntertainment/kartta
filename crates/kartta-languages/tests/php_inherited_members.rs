//! What a PHP call on a class the graph holds binds of the types above that class: the member the class does not hold
//! itself binds where its nearest parent holds it, a level's traits first, on the caller's own receivers and on a class
//! the call names.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

/// The qualified name and the confidence of each callee of `caller`, in a tree that holds `source` as one file.
fn callees(source: &str, caller: &str) -> Vec<(String, u8)> {
    callees_in(&[("App.php", source)], caller)
}

/// [`callees`] in a tree of several files.
fn callees_in(files: &[(&str, &str)], caller: &str) -> Vec<(String, u8)> {
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
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

/// The class a call names holds no constructor but its parent does, and the caller's own stands in the file.
const NAMED_GRANDPARENT: &str = r"<?php
namespace App;

abstract class AbstractKernel {
    public function __construct($env, $debug) {}
}

class Kernel extends AbstractKernel {}

class CustomKernel extends Kernel {
    public function __construct() { parent::__construct('custom', true); }
}

class PassKernel extends CustomKernel {
    public function __construct() { Kernel::__construct('pass', true); }
}
";

#[test]
fn a_call_on_a_named_class_binds_the_constructor_its_parent_holds_and_not_the_caller_s_own() {
    assert_eq!(
        callees(NAMED_GRANDPARENT, "App\\PassKernel::__construct"),
        [("App\\AbstractKernel::__construct".to_owned(), 85)]
    );
}

const OWN_RECEIVERS: &str = r"<?php
namespace App;

class Grandparent {
    public function far() {}
    public function shadowed() {}
}

class Stranger {
    public function far() {}
    public function shadowed() {}
    public function own() {}
}

class Middle extends Grandparent {
    public function shadowed() {}
}

class Leaf extends Middle {
    public function own() {}
    public function viaThis() { $this->far(); }
    public function viaSelf() { self::far(); }
    public function viaStatic() { static::far(); }
    public function nearest() { $this->shadowed(); }
    public function held() { $this->own(); }
}
";

#[test]
fn a_call_on_an_own_receiver_binds_the_nearest_parent_that_holds_the_member() {
    let far = || [("App\\Grandparent::far".to_owned(), 85)];
    assert_eq!(callees(OWN_RECEIVERS, "App\\Leaf::viaThis"), far());
    assert_eq!(callees(OWN_RECEIVERS, "App\\Leaf::viaSelf"), far());
    assert_eq!(callees(OWN_RECEIVERS, "App\\Leaf::viaStatic"), far());
    assert_eq!(
        callees(OWN_RECEIVERS, "App\\Leaf::nearest"),
        [("App\\Middle::shadowed".to_owned(), 85)],
        "a nearer parent hides a farther one"
    );
}

#[test]
fn a_member_the_class_holds_stays_with_the_caller_s_owner_tier() {
    assert_eq!(
        callees(OWN_RECEIVERS, "App\\Leaf::held"),
        [("App\\Leaf::own".to_owned(), 80)]
    );
}

const TRAIT_OVER_PARENT: &str = r"<?php
namespace App;

class Base {
    public function shared() {}
    public function own() {}
}

trait Helper {
    public function shared() {}
    public function own() {}
}

class Child extends Base {
    use Helper;
    public function own() {}
    public function viaThis() { $this->shared(); }
    public function viaThisOwn() { $this->own(); }
}
";

#[test]
fn a_trait_s_member_overrides_the_parent_s_and_the_class_s_own_overrides_the_trait_s() {
    assert_eq!(
        callees(TRAIT_OVER_PARENT, "App\\Child::viaThis"),
        [("App\\Helper::shared".to_owned(), 85)]
    );
    assert_eq!(
        callees(TRAIT_OVER_PARENT, "App\\Child::viaThisOwn"),
        [("App\\Child::own".to_owned(), 80)]
    );
}

const INTERFACE_ABOVE: &str = r"<?php
namespace App;

interface Runs {
    public function far();
    public function only();
}

class Grandparent {
    public function far() {}
}

class Middle extends Grandparent {}

abstract class Leaf extends Middle implements Runs {
    public function body() { $this->far(); }
    public function signature() { $this->only(); }
}
";

#[test]
fn a_class_s_body_above_answers_before_an_interface_s_signature_nearer_it() {
    assert_eq!(
        callees(INTERFACE_ABOVE, "App\\Leaf::body"),
        [("App\\Grandparent::far".to_owned(), 85)]
    );
    assert_eq!(
        callees(INTERFACE_ABOVE, "App\\Leaf::signature"),
        [("App\\Runs::only".to_owned(), 85)],
        "a member no body above the class holds binds the interface's declaration"
    );
}

const NAMESAKE_INTERFACE: &str = r"<?php
namespace Contracts {
    interface Container {
        public function instance();
    }
}

namespace Core {
    class Container {
        public function instance() {}
    }

    class App extends Container {
        public function boot() { $this->instance(); }
    }
}
";

#[test]
fn an_interface_of_the_parent_s_name_hides_no_class_of_that_name() {
    assert_eq!(
        callees(NAMESAKE_INTERFACE, "Core\\App::boot"),
        [("Core\\Container::instance".to_owned(), 85)]
    );
}

const CALLER: &str = r"<?php
namespace App\Http;

use App\Http\Kernel;

class Runner {
    public function start() { Kernel::boot(); }
}
";

const NAMED_CLASS: &str = r"<?php
namespace App\Http;

class Base {
    public static function boot() {}
}

class Kernel extends Base {}
";

const NAMESAKE_CLASS: &str = r"<?php
namespace Other;

class Kernel {
    public static function boot() {}
}
";

#[test]
fn what_a_named_class_inherits_answers_before_a_namesake_s_own_member() {
    assert_eq!(
        callees_in(
            &[
                ("src/Http/Runner.php", CALLER),
                ("src/Http/Kernel.php", NAMED_CLASS),
                ("src/Other/Kernel.php", NAMESAKE_CLASS),
            ],
            "App\\Http\\Runner::start"
        ),
        [("App\\Http\\Base::boot".to_owned(), 85)]
    );
}

const VALUE_OF_A_CLASS: &str = r"<?php
namespace App;

class Base {
    public function run() {}
}

class Other {
    public function run() {}
}

class Runner {
    public function start() {
        $task = new class extends Base {};
        $task->run();
    }
}
";

#[test]
fn a_variable_an_anonymous_class_holds_binds_what_the_class_inherits() {
    assert_eq!(
        callees(VALUE_OF_A_CLASS, "App\\Runner::start"),
        [
            ("App\\Base::run".to_owned(), 85),
            ("App\\{start·task}".to_owned(), 80)
        ]
    );
}
