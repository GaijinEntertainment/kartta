//! What a PHP file extracts to, how its includes and uses resolve against the repository, and where the doc comment
//! of a definition starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Access, CallRef, Extraction, FILE_SCOPE, InheritedMembers, Language, MemoryRepository,
    OwnMembers, RawExtraction, Receiver, Symbol, SymbolId, SymbolKind, TypeUses,
};
use kartta_lang_php::Php;

const SAMPLE: &[u8] = br#"<?php
namespace App\Service;

interface Greeter extends Stringable {
    public function greet(): string;
}

trait Walkable {
    public function walk(): void {}
}

class Base {
}

class Service extends Base implements Greeter {
    use Walkable;

    public function greet(): string {
        return helper($this->name);
    }

    private function secret(): void {
        $this->greet();
        $svc?->maybe();
        Logger::log("x");
        $w = new Widget();
        $u = new \App\Models\User();
        $d = new $factory();
    }
}

enum Suit {
    case Hearts;
}

function helper(string $v): string {
    return $v;
}
"#;

const WIDGET: &[u8] =
    b"<?php\nclass Widget {\n    public function run(): int {\n        return 1;\n    }\n}\n";

const APP_SRC: &str = r#"{ "autoload": { "psr-4": { "App\\": "src/" } } }"#;

fn extract(source: &[u8]) -> RawExtraction {
    Php::new().extract(source)
}

fn resolve(source: &[u8], path: &str, files: &[&str]) -> Extraction {
    resolve_with(source, path, files, &[])
}

fn resolve_with(source: &[u8], path: &str, files: &[&str], configs: &[(&str, &str)]) -> Extraction {
    let php = Php::new();
    let repository = MemoryRepository::new(configs.iter().copied()).with_paths(files);
    let resolver = php.resolver(&repository);
    resolver.resolve(php.extract(source), path, &repository)
}

fn symbol<'r>(symbols: &'r [Symbol], name: &str) -> &'r Symbol {
    symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

fn id_of(symbols: &[Symbol], qualified: &str) -> SymbolId {
    let at = symbols
        .iter()
        .position(|symbol| symbol.qualified_name == qualified)
        .unwrap_or_else(|| panic!("no symbol {qualified}"));
    SymbolId(u32::try_from(at).unwrap_or(u32::MAX))
}

fn call<'r>(calls: &'r [CallRef], callee: &str) -> &'r CallRef {
    calls
        .iter()
        .find(|call| call.callee == callee)
        .unwrap_or_else(|| panic!("no call of {callee}"))
}

fn callees(calls: &[CallRef]) -> Vec<&str> {
    calls.iter().map(|call| call.callee.as_str()).collect()
}

fn names(symbols: &[Symbol]) -> Vec<&str> {
    symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect()
}

fn names_by_line(source: &[u8]) -> Vec<(String, u32)> {
    extract(source)
        .symbols
        .into_iter()
        .map(|symbol| (symbol.qualified_name, symbol.line))
        .collect()
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

fn imports(extraction: &Extraction) -> Vec<&str> {
    extraction
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect()
}

#[test]
fn php_claims_its_extensions_in_any_case() {
    let php = Php::new();

    assert_eq!(php.name(), "php");
    assert!(php.claims("src/Service.php"));
    assert!(php.claims("views/page.phtml"));
    assert!(php.claims("LEGACY.PHP"));
    assert!(!php.claims("src/Service.php.bak"));
    assert!(!php.claims("php"));
}

#[test]
fn a_bare_call_reaches_no_member_of_its_own_class() {
    let conventions = Php::new().conventions();

    assert_eq!(conventions.own_members, OwnMembers::Unreached);
    assert_eq!(conventions.inherited_members, InheritedMembers::Unreached);
    assert_eq!(conventions.owner_chain_separator, None);
    assert_eq!(conventions.base_receivers, ["parent"]);
}

#[test]
fn the_fingerprint_names_the_crate() {
    let (php, other) = (Php::new(), Php::new());

    assert!(php.fingerprint().starts_with(b"kartta-lang-php "));
    assert_eq!(php.fingerprint(), other.fingerprint());
}

#[test]
fn every_declaration_is_qualified_by_its_namespace() {
    let raw = extract(SAMPLE);

    assert_eq!(
        names(&raw.symbols),
        [
            "App\\Service\\Greeter",
            "App\\Service\\Greeter::greet",
            "App\\Service\\Walkable",
            "App\\Service\\Walkable::walk",
            "App\\Service\\Base",
            "App\\Service\\Service",
            "App\\Service\\Service::greet",
            "App\\Service\\Service::secret",
            "App\\Service\\Suit",
            "App\\Service\\Suit::Hearts",
            "App\\Service\\helper",
        ]
    );
    assert_eq!(raw.symbols[0].kind, SymbolKind::Interface);
    assert_eq!(symbol(&raw.symbols, "Walkable").kind, SymbolKind::Trait);
    assert_eq!(
        symbol(&raw.symbols, "walk").owner.as_deref(),
        Some("Walkable")
    );
    assert_eq!(symbol(&raw.symbols, "Suit").kind, SymbolKind::Enum);
    let hearts = symbol(&raw.symbols, "Hearts");
    assert_eq!(hearts.kind, SymbolKind::EnumVariant);
    assert_eq!(hearts.owner.as_deref(), Some("Suit"));
    let helper = symbol(&raw.symbols, "helper");
    assert_eq!(helper.kind, SymbolKind::Function);
    assert_eq!(helper.owner, None);
}

#[test]
fn a_private_method_is_not_exported() {
    let raw = extract(SAMPLE);

    let secret = symbol(&raw.symbols, "secret");
    assert_eq!(secret.kind, SymbolKind::Method);
    assert_eq!(secret.owner.as_deref(), Some("Service"));
    assert!(!secret.exported);
    let greet = &raw.symbols[id_of(&raw.symbols, "App\\Service\\Service::greet").0 as usize];
    assert!(greet.exported);
}

#[test]
fn extends_implements_and_a_trait_use_are_parents() {
    let raw = extract(SAMPLE);

    assert_eq!(
        symbol(&raw.symbols, "Service").parents,
        [
            "App\\Service\\Base",
            "App\\Service\\Greeter",
            "App\\Service\\Walkable"
        ]
    );
    assert_eq!(
        symbol(&raw.symbols, "Greeter").parents,
        ["App\\Service\\Stringable"]
    );
}

#[test]
fn an_enum_takes_the_interfaces_it_implements_as_parents() {
    let raw = extract(
        b"<?php\nnamespace App;\n\nenum Suit: string implements Colorful, HasIcon {\n    case Hearts = 'H';\n}\n",
    );

    let suit = symbol(&raw.symbols, "Suit");
    assert_eq!(suit.qualified_name, "App\\Suit");
    assert_eq!(suit.parents, ["App\\Colorful", "App\\HasIcon"]);
}

#[test]
fn a_trait_use_lands_on_the_class_not_a_member_of_its_name() {
    let raw = extract(b"<?php\nclass Foo {\n    const Foo = 1;\n    use Walkable;\n}\n");

    let class = raw
        .symbols
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::Class)
        .unwrap_or_else(|| panic!("no class"));
    assert_eq!(class.parents, ["Walkable"]);
    let constant = raw
        .symbols
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::Constant)
        .unwrap_or_else(|| panic!("no constant"));
    assert_eq!(constant.parents, Vec::<String>::new());
}

#[test]
fn a_trait_use_in_an_anonymous_class_is_a_parent_of_that_class_alone() {
    let raw = extract(
        b"<?php\nclass Outer {\n    public function make() {\n        return new class { use Inner; };\n    }\n}\n",
    );

    assert_eq!(symbol(&raw.symbols, "Outer").parents, Vec::<String>::new());
    assert_eq!(symbol(&raw.symbols, "{make}").parents, ["Inner"]);
}

#[test]
fn a_call_is_made_by_the_enclosing_method() {
    let raw = extract(SAMPLE);

    let helper = call(&raw.calls, "helper");
    assert_eq!(
        helper.site_of,
        id_of(&raw.symbols, "App\\Service\\Service::greet")
    );
    assert_eq!(helper.receiver, None);
}

#[test]
fn member_and_static_calls_record_their_receivers() {
    let raw = extract(SAMPLE);
    let secret = id_of(&raw.symbols, "App\\Service\\Service::secret");

    let member = call(&raw.calls, "greet");
    assert_eq!(member.receiver.as_deref(), Some("$this"));
    assert_eq!(member.site_of, secret);
    let scoped = call(&raw.calls, "log");
    assert_eq!(scoped.receiver.as_deref(), Some("Logger"));
    assert_eq!(scoped.site_of, secret);
    assert_eq!(call(&raw.calls, "maybe").receiver.as_deref(), Some("$svc"));
}

#[test]
fn a_construction_calls_the_class_by_its_bare_name() {
    let raw = extract(SAMPLE);

    assert_eq!(
        call(&raw.calls, "Widget").site_of,
        id_of(&raw.symbols, "App\\Service\\Service::secret")
    );
    assert_eq!(call(&raw.calls, "User").receiver, None);
    assert!(raw.calls.iter().all(|call| call.callee != "$factory"));
}

#[test]
fn a_relative_class_construction_calls_nothing() {
    let raw = extract(
        br"<?php
class Foo extends Bar {
    public static function create(): static {
        return new static();
    }
    public function a() { return new self(); }
    public function b() { return new parent(); }
    public function c() { return new Widget(); }
}
",
    );

    assert_eq!(callees(&raw.calls), ["Widget"]);
}

#[test]
fn a_chained_or_computed_receiver_is_empty() {
    let raw = extract(
        br#"<?php
class Runner {
    public function go(): void {
        Factory::for(1)::build();
        $this->pool->get();
        Logger::log("x");
        self::x();
        parent::y();
        $cls::vcall();
    }
}
"#,
    );
    let receiver = |callee: &str| call(&raw.calls, callee).receiver.as_deref();

    assert_eq!(receiver("build"), Some(""));
    assert_eq!(receiver("get"), Some(""));
    assert_eq!(receiver("log"), Some("Logger"));
    assert_eq!(receiver("x"), Some("self"));
    assert_eq!(receiver("y"), Some("parent"));
    assert_eq!(receiver("vcall"), Some("$cls"));
}

#[test]
fn a_qualified_scope_is_the_receiver_of_its_last_segment() {
    let raw = extract(
        br#"<?php
class Runner {
    public function go(): void {
        App\Logger::qlog("x");
        \App\Logger::flog("x");
        namespace\Helper::nhelp();
    }
}
"#,
    );
    let receiver = |callee: &str| call(&raw.calls, callee).receiver.as_deref();

    assert_eq!(receiver("qlog"), Some("Logger"));
    assert_eq!(receiver("flog"), Some("Logger"));
    assert_eq!(receiver("nhelp"), Some("Helper"));
}

#[test]
fn a_qualified_function_call_calls_its_last_segment() {
    let raw = extract(b"<?php\nfunction caller() {\n    \\strlen(\"x\");\n    App\\Helper\\run();\n    plain();\n}\n");

    assert_eq!(callees(&raw.calls), ["strlen", "run", "plain"]);
}

#[test]
fn a_relative_name_reduces_like_a_qualified_one() {
    let extraction = resolve(
        br#"<?php
class C extends namespace\Base {
    public function m(): void {
        $x = new namespace\X();
        namespace\helper();
    }
}
include "$base/x.php";
"#,
        "here.php",
        &["x.php", "here.php"],
    );

    assert_eq!(symbol(&extraction.symbols, "C").parents, ["Base"]);
    assert_eq!(callees(&extraction.calls), ["X", "helper"]);
    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_call_before_the_first_symbol_is_a_call_from_file_scope() {
    let raw = extract(b"<?php\nboot();\nfunction main(): void {}\nmain();\n");

    assert_eq!(callees(&raw.calls), ["boot", "main"]);
    assert!(raw.calls.iter().all(|call| call.site_of == FILE_SCOPE));
}

#[test]
fn a_callable_carries_its_header_as_the_signature() {
    let raw = extract(SAMPLE);

    let signature = symbol(&raw.symbols, "helper").signature.as_deref();
    assert_eq!(signature, Some("function helper(string $v): string"));
    assert_eq!(
        symbol(&raw.symbols, "secret").signature.as_deref(),
        Some("private function secret(): void")
    );
}

#[test]
fn a_closure_bound_to_a_variable_makes_the_calls_in_its_body() {
    let raw = extract(
        br"<?php
class Foo {
    public function m(): void {
        $f = function () { bar(); };
        $f();
        after_close();
        array_map(function ($x) { inner($x); }, [1]);
        $obj->cb = function () { mem(); };
        $g = 1;
        [$a, $b] = [function () {}, function () {}];
    }
}

$top = fn($x) => topcall($x);
",
    );
    let method = id_of(&raw.symbols, "Foo::m");

    let bound = id_of(&raw.symbols, "f");
    let f = &raw.symbols[bound.0 as usize];
    assert_eq!(f.kind, SymbolKind::Function);
    assert!(!f.exported);
    assert_eq!(f.signature.as_deref(), Some("$f = function ()"));
    assert_eq!(call(&raw.calls, "bar").site_of, bound);
    assert_eq!(call(&raw.calls, "after_close").site_of, method);
    assert_eq!(call(&raw.calls, "inner").site_of, method);
    assert_eq!(call(&raw.calls, "mem").site_of, method);
    for unbound in ["cb", "g", "a", "b"] {
        assert!(
            raw.symbols.iter().all(|symbol| symbol.name != unbound),
            "{unbound} is no symbol"
        );
    }
    let top = id_of(&raw.symbols, "top");
    assert_eq!(call(&raw.calls, "topcall").site_of, top);
    assert_eq!(
        raw.symbols[top.0 as usize].signature.as_deref(),
        Some("$top = fn($x) =>")
    );
}

#[test]
fn properties_constants_and_promoted_parameters_are_members() {
    let raw = extract(
        br"<?php
namespace App;

class Account {
    const STATUS_ACTIVE = 'active';
    private const SECRET = 'x';
    public int $id;
    private string $name = 'anon';
    public static array $registry = [];
    private int $width, $height;

    public function __construct(
        public readonly string $owner,
        private int $balance,
        protected array &$refs,
    ) {}
}
",
    );

    let members: Vec<(&str, SymbolKind)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        members,
        [
            ("App\\Account", SymbolKind::Class),
            ("App\\Account::STATUS_ACTIVE", SymbolKind::Constant),
            ("App\\Account::SECRET", SymbolKind::Constant),
            ("App\\Account::id", SymbolKind::Field),
            ("App\\Account::name", SymbolKind::Field),
            ("App\\Account::registry", SymbolKind::Field),
            ("App\\Account::width", SymbolKind::Field),
            ("App\\Account::height", SymbolKind::Field),
            ("App\\Account::__construct", SymbolKind::Method),
            ("App\\Account::owner", SymbolKind::Field),
            ("App\\Account::balance", SymbolKind::Field),
            ("App\\Account::refs", SymbolKind::Field),
        ]
    );
    for name in [
        "id", "name", "registry", "width", "height", "owner", "balance", "refs",
    ] {
        assert_eq!(
            symbol(&raw.symbols, name).owner.as_deref(),
            Some("Account"),
            "{name}"
        );
    }
    let exported: Vec<(&str, bool)> = raw.symbols[1..]
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::Method)
        .map(|symbol| (symbol.name.as_str(), symbol.exported))
        .collect();
    assert_eq!(
        exported,
        [
            ("STATUS_ACTIVE", true),
            ("SECRET", false),
            ("id", true),
            ("name", false),
            ("registry", true),
            ("width", false),
            ("height", false),
            ("owner", true),
            ("balance", false),
            ("refs", true),
        ]
    );
}

#[test]
fn a_constant_at_file_scope_has_no_owner() {
    let raw = extract(b"<?php\nnamespace App;\n\nconst VERSION = '1.2.3';\n");

    let version = symbol(&raw.symbols, "VERSION");
    assert_eq!(version.kind, SymbolKind::Constant);
    assert_eq!(version.owner, None);
    assert_eq!(version.qualified_name, "App\\VERSION");
}

#[test]
fn a_member_of_an_anonymous_class_is_owned_by_the_class_the_walk_names() {
    let raw = extract(
        br"<?php
namespace App;

class Factory {
    public function make(): object {
        return new class(5) extends Base implements Tagged {
            public $label;
            public function __construct(public int $tag) {}
            public function run() { $this->label(); }
        };
    }
}
",
    );

    let class = symbol(&raw.symbols, "{make}");
    assert_eq!(class.kind, SymbolKind::Class);
    assert_eq!(class.qualified_name, "App\\{make}");
    assert_eq!(class.parents, ["App\\Base", "App\\Tagged"]);
    assert!(!class.exported);
    for (member, kind) in [
        ("label", SymbolKind::Field),
        ("tag", SymbolKind::Field),
        ("run", SymbolKind::Method),
    ] {
        let member = symbol(&raw.symbols, member);
        assert_eq!(member.kind, kind);
        assert_eq!(member.owner.as_deref(), Some("{make}"));
        assert_eq!(
            member.qualified_name,
            format!("App\\{{make}}::{}", member.name)
        );
    }
    let own = call(&raw.calls, "label");
    assert_eq!(own.receiver.as_deref(), Some("$this"));
    assert_eq!(own.site_of, id_of(&raw.symbols, "App\\{make}::run"));
}

#[test]
fn an_anonymous_class_is_named_by_its_creator_and_the_name_it_is_assigned_to() {
    let raw = extract(
        br"<?php
$top = new class {};
return new class {};

class Suite {
    public function testRun() {
        $job = new class {};
        $this->handler = new class {};
        $job = new class {
            public function make() { return new class {}; }
        };
        register(new class {}, new class {});
    }
}

function boot() { return new class {}; }
",
    );

    let classes: Vec<&str> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with('{'))
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert_eq!(
        classes,
        [
            "{top}",
            "{class}",
            "{testRun\u{b7}job}",
            "{testRun\u{b7}handler}",
            "{testRun\u{b7}job\u{b7}2}",
            "{make}",
            "{testRun}",
            "{testRun\u{b7}2}",
            "{boot}",
        ]
    );
}

#[test]
fn an_anonymous_class_is_constructed_by_the_callable_that_creates_it() {
    let raw = extract(
        b"<?php\nclass Factory {\n    public function make() { return new class {}; }\n}\n",
    );

    let construction = call(&raw.calls, "{make}");
    assert_eq!(construction.receiver, None);
    assert_eq!(construction.site_of, id_of(&raw.symbols, "Factory::make"));
}

#[test]
fn a_call_on_a_variable_holding_an_anonymous_class_names_the_class_as_its_receiver() {
    let raw = extract(
        br"<?php
class Suite {
    public function testRun() {
        $job = new class { public function handle() {} public function make() {} };
        $job->handle();
        $job::make();
        $job = new Job();
        $job->dispatch();
    }

    public function testOther() {
        $job->retry();
    }
}
",
    );

    let job = Some("{testRun\u{b7}job}");
    assert_eq!(call(&raw.calls, "handle").receiver.as_deref(), job);
    assert_eq!(call(&raw.calls, "make").receiver.as_deref(), job);
    assert_eq!(
        call(&raw.calls, "dispatch").receiver.as_deref(),
        Some("\\Job")
    );
    assert_eq!(call(&raw.calls, "retry").receiver.as_deref(), Some("$job"));
}

#[test]
fn the_global_namespace_block_clears_the_namespace() {
    assert_eq!(
        names_by_line(
            b"<?php\nnamespace A { class Worker { public static function run() {} } }\n\
              namespace { class Worker { public static function run() {} } }\n"
        ),
        [
            ("A\\Worker".to_owned(), 2),
            ("A\\Worker::run".to_owned(), 2),
            ("Worker".to_owned(), 3),
            ("Worker::run".to_owned(), 3),
        ]
    );
}

#[test]
fn each_namespace_block_qualifies_its_own_declarations() {
    assert_eq!(
        names_by_line(
            br"<?php
namespace A { function first() {} }
namespace B { }
namespace { function second() {} }
namespace C { function third() {} class Job { const LIMIT = 1; } }
namespace { function fourth() {} }
"
        ),
        [
            ("A\\first".to_owned(), 2),
            ("second".to_owned(), 4),
            ("C\\third".to_owned(), 5),
            ("C\\Job".to_owned(), 5),
            ("C\\Job::LIMIT".to_owned(), 5),
            ("fourth".to_owned(), 6),
        ]
    );
}

#[test]
fn an_unbraced_namespace_holds_until_the_next_one() {
    assert_eq!(
        names_by_line(
            b"<?php\nnamespace A;\nfunction first() {}\nclass Job {}\nnamespace B;\nfunction second() {}\n"
        ),
        [
            ("A\\first".to_owned(), 3),
            ("A\\Job".to_owned(), 4),
            ("B\\second".to_owned(), 6),
        ]
    );
}

#[test]
fn an_include_of_dir_resolves_from_the_files_directory() {
    let extraction = resolve(
        b"<?php\ninclude __DIR__ . '/partials/header.php';\nfunction render(): void {}\n",
        "src/views/index.php",
        &["src/views/index.php", "src/views/partials/header.php"],
    );

    assert_eq!(includes(&extraction), ["src/views/partials/header.php"]);
    assert_eq!(imports(&extraction), Vec::<&str>::new());
}

#[test]
fn a_parenthesized_include_folds_like_a_bare_one() {
    let extraction = resolve(
        b"<?php\nrequire_once('config/app.php');\nrequire_once(__DIR__ . '/partials/header.php');\ninclude ($path);\n",
        "src/boot.php",
        &["config/app.php", "src/partials/header.php", "src/boot.php"],
    );

    assert_eq!(
        includes(&extraction),
        ["config/app.php", "src/partials/header.php"]
    );
}

#[test]
fn dirname_of_a_path_marker_climbs_from_the_files_directory() {
    let extraction = resolve(
        b"<?php\ninclude dirname(__DIR__) . '/shared.php';\ninclude dirname(__FILE__) . '/local.php';\n",
        "src/feature/page.php",
        &["src/feature/page.php", "src/shared.php", "src/feature/local.php"],
    );

    assert_eq!(
        includes(&extraction),
        ["src/shared.php", "src/feature/local.php"]
    );
}

#[test]
fn dirname_strips_a_written_dot_dot_and_nested_dirname_climbs_twice() {
    let extraction = resolve(
        br"<?php
include dirname(__DIR__ . '/..') . '/lexical.php';
include dirname(__DIR__ . '/../') . '/slashed.php';
include dirname(__DIR__ . '/') . '/trailing.php';
include dirname(dirname(__DIR__)) . '/nested.php';
",
        "src/feature/page.php",
        &[
            "src/feature/page.php",
            "src/feature/lexical.php",
            "src/feature/slashed.php",
            "src/trailing.php",
            "nested.php",
            "lexical.php",
            "slashed.php",
            "src/feature/trailing.php",
        ],
    );

    assert_eq!(
        includes(&extraction),
        [
            "src/feature/lexical.php",
            "src/feature/slashed.php",
            "src/trailing.php",
            "nested.php"
        ]
    );
}

#[test]
fn dirname_cuts_at_a_written_dot_dot_inside_the_path() {
    let extraction = resolve(
        br"<?php
include dirname(dirname(__DIR__ . '/../a')) . '/y.php';
include dirname(__DIR__ . '/../a') . '/y.php';
",
        "src/feature/page.php",
        &[
            "src/feature/page.php",
            "src/feature/y.php",
            "src/y.php",
            "y.php",
        ],
    );

    assert_eq!(includes(&extraction), ["src/feature/y.php", "src/y.php"]);
}

#[test]
fn dirname_with_levels_climbs_that_many_times() {
    let extraction = resolve(
        br"<?php
include dirname(__DIR__, 2) . '/x.php';
include dirname(__FILE__, 2) . '/y.php';
include dirname(levels: 2, path: __DIR__) . '/named.php';
include dirname('config/app/x.php', 2) . '/db.php';
include dirname(__DIR__ . '/..', 2) . '/lexical.php';
",
        "lib/a/b/use.php",
        &[
            "lib/a/b/use.php",
            "lib/x.php",
            "lib/a/x.php",
            "lib/a/y.php",
            "lib/a/b/y.php",
            "lib/named.php",
            "config/db.php",
            "lib/a/lexical.php",
        ],
    );

    assert_eq!(
        includes(&extraction),
        [
            "lib/x.php",
            "lib/a/y.php",
            "lib/named.php",
            "config/db.php",
            "lib/a/lexical.php"
        ]
    );
}

#[test]
fn dirname_with_levels_other_than_a_positive_integer_literal_includes_nothing() {
    let extraction = resolve(
        br"<?php
include dirname(__DIR__, $levels) . '/x.php';
include dirname(__DIR__, LEVELS) . '/x.php';
include dirname(__DIR__, 0) . '/x.php';
include dirname(__DIR__, 02) . '/x.php';
include dirname(__DIR__, 256) . '/x.php';
include dirname(__DIR__, 1 + 1) . '/x.php';
include dirname(__DIR__, 2, 3) . '/x.php';
include dirname(__DIR__, depth: 2) . '/x.php';
",
        "lib/a/b/use.php",
        &["lib/a/b/use.php", "lib/x.php", "lib/a/x.php"],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn fully_qualified_dirname_folds_as_the_plain_call_does() {
    let extraction = resolve(
        br"<?php
namespace App\Pages;

include \dirname(__DIR__, 2) . '/x.php';
include dirname(__DIR__, 2) . '/y.php';
include \dirname(__FILE__) . '/local.php';
include \dirname(\dirname(__DIR__)) . '/nested.php';
",
        "lib/a/b/use.php",
        &[
            "lib/a/b/use.php",
            "lib/x.php",
            "lib/y.php",
            "lib/a/b/local.php",
            "lib/nested.php",
        ],
    );

    assert_eq!(
        includes(&extraction),
        [
            "lib/x.php",
            "lib/y.php",
            "lib/a/b/local.php",
            "lib/nested.php"
        ]
    );
}

#[test]
fn a_folded_name_matches_in_any_letter_case() {
    let extraction = resolve(
        br"<?php
include DIRNAME(__DIR__) . '/x.php';
include \DirName(__file__) . '/y.php';
include Dirname(__Dir__, 2) . '/z.php';
include __dir__ . '/here.php';
INCLUDE dirname(__DIR__) . '/keyword.php';
",
        "lib/a/b/use.php",
        &[
            "lib/a/b/use.php",
            "lib/a/x.php",
            "lib/a/b/y.php",
            "lib/z.php",
            "lib/a/b/here.php",
            "lib/a/keyword.php",
        ],
    );

    assert_eq!(
        includes(&extraction),
        [
            "lib/a/x.php",
            "lib/a/b/y.php",
            "lib/z.php",
            "lib/a/b/here.php",
            "lib/a/keyword.php"
        ]
    );
}

#[test]
fn a_named_dirname_argument_matches_in_its_exact_case_alone() {
    let extraction = resolve(
        b"<?php\ninclude dirname(__DIR__, LEVELS: 2) . '/x.php';\ninclude dirname(PATH: __DIR__) . '/y.php';\n",
        "lib/a/b/use.php",
        &["lib/a/b/use.php", "lib/x.php", "lib/a/y.php"],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_qualified_name_of_another_function_includes_nothing() {
    let extraction = resolve(
        br"<?php
include \Foo\dirname(__DIR__, 1) . '/x.php';
include Foo\dirname(__DIR__, 1) . '/x.php';
include \Foo\DIRNAME(__DIR__, 1) . '/x.php';
include namespace\dirname(__DIR__, 1) . '/x.php';
include \Foo\dirname\other(__DIR__, 1) . '/x.php';
",
        "lib/a/use.php",
        &["lib/a/use.php", "lib/x.php"],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn dirname_of_a_literal_path_cuts_it_in_place() {
    let extraction = resolve(
        b"<?php\ninclude dirname('config/app/x.php') . '/db.php';\n",
        "src/boot.php",
        &["src/boot.php", "config/app/db.php"],
    );

    assert_eq!(includes(&extraction), ["config/app/db.php"]);
}

#[test]
fn dirname_of_dir_at_a_root_file_names_no_file_of_the_repository() {
    let extraction = resolve(
        b"<?php\ninclude dirname(__DIR__) . '/shared.php';\n",
        "page.php",
        &["page.php", "shared.php"],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_path_marker_after_a_literal_includes_nothing() {
    let extraction = resolve(
        b"<?php\ninclude 'inc/' . __DIR__ . '/y.php';\ninclude dirname('inc/' . __DIR__) . '/y.php';\n",
        "src/feature/page.php",
        &["src/feature/page.php", "inc/src/feature/y.php", "inc/src/y.php"],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_literal_holding_the_marker_byte_includes_nothing() {
    let extraction = resolve(
        b"<?php\ninclude '\x01D/x.php';\nfunction render(): void {}\n",
        "src/page.php",
        &["src/page.php", "src/x.php"],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_plain_path_resolves_from_the_root() {
    let extraction = resolve(
        b"<?php\nrequire_once 'config/app.php';\n",
        "src/boot.php",
        &["src/boot.php", "config/app.php"],
    );

    assert_eq!(includes(&extraction), ["config/app.php"]);
}

#[test]
fn a_path_found_nowhere_else_is_the_one_file_ending_in_it() {
    let found = resolve(
        b"<?php\nrequire 'db.php';\n",
        "src/boot.php",
        &["src/boot.php", "app/config/db.php"],
    );
    let ambiguous = resolve(
        b"<?php\nrequire 'config/db.php';\n",
        "src/boot.php",
        &["src/boot.php", "app/config/db.php", "lib/config/db.php"],
    );

    assert_eq!(includes(&found), ["app/config/db.php"]);
    assert_eq!(includes(&ambiguous), Vec::<&str>::new());
}

#[test]
fn a_dot_path_resolves_from_the_files_directory() {
    let extraction = resolve(
        b"<?php\ninclude './partials/a.php';\ninclude '../shared/b.php';\ninclude '../../../c.php';\n",
        "src/views/page.php",
        &["src/views/partials/a.php", "src/shared/b.php", "c.php"],
    );

    assert_eq!(
        includes(&extraction),
        ["src/views/partials/a.php", "src/shared/b.php"]
    );
}

#[test]
fn an_include_of_a_variable_includes_nothing() {
    let extraction = resolve(
        b"<?php\n$path = 'plugin.php';\ninclude $path;\nfunction load(): void {}\n",
        "src/loader.php",
        &["src/loader.php", "src/plugin.php"],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_use_without_an_autoload_is_an_import_only() {
    let extraction = resolve(
        b"<?php\nuse App\\Foo;\nrequire \"bar.php\";\nfunction f(): void {}\n",
        "uses.php",
        &[],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(imports(&extraction), ["App\\Foo"]);
    assert_eq!(extraction.imports[0].line, 2);
}

#[test]
fn a_use_that_psr4_maps_to_a_class_file_is_an_include_only() {
    let extraction = resolve_with(
        b"<?php\nnamespace App;\n\nuse App\\Foo;\nuse App\\Foo as Bar;\n\nclass Service {}\n",
        "src/Service.php",
        &["src/Service.php", "src/Foo.php"],
        &[("composer.json", APP_SRC)],
    );

    assert_eq!(includes(&extraction), ["src/Foo.php", "src/Foo.php"]);
    assert_eq!(imports(&extraction), Vec::<&str>::new());
}

#[test]
fn a_grouped_use_shares_its_prefix_across_the_clauses() {
    let extraction = resolve_with(
        b"<?php\nnamespace App;\n\nuse App\\{Foo, Sub\\Bar};\n\nclass Service {}\n",
        "src/Service.php",
        &["src/Service.php", "src/Foo.php", "src/Sub/Bar.php"],
        &[("composer.json", APP_SRC)],
    );

    assert_eq!(includes(&extraction), ["src/Foo.php", "src/Sub/Bar.php"]);
}

#[test]
fn a_function_use_maps_to_its_namespaces_file() {
    let extraction = resolve_with(
        b"<?php\nnamespace App;\n\nuse function App\\Text\\slugify;\n\nclass Service {}\n",
        "src/Service.php",
        &["src/Service.php", "src/Text.php"],
        &[("composer.json", APP_SRC)],
    );

    assert_eq!(includes(&extraction), ["src/Text.php"]);
}

#[test]
fn a_class_use_maps_only_to_its_own_file() {
    let extraction = resolve_with(
        b"<?php\nnamespace App;\n\nuse App\\Text\\Missing;\nuse const App\\Text\\LIMIT;\n\nclass Service {}\n",
        "src/Service.php",
        &["src/Service.php", "src/Text.php"],
        &[("composer.json", APP_SRC)],
    );

    assert_eq!(includes(&extraction), ["src/Text.php"]);
    assert_eq!(imports(&extraction), ["App\\Text\\Missing"]);
}

#[test]
fn a_mixed_group_use_maps_each_clause_by_its_own_kind() {
    let extraction = resolve_with(
        b"<?php\nnamespace App;\n\nuse App\\Text\\{Missing, function slugify};\nuse function App\\Text\\{trim};\n",
        "src/Service.php",
        &["src/Service.php", "src/Text.php"],
        &[("composer.json", APP_SRC)],
    );

    assert_eq!(includes(&extraction), ["src/Text.php", "src/Text.php"]);
    assert_eq!(imports(&extraction), ["App\\Text\\Missing"]);
}

#[test]
fn a_use_of_a_namespace_no_prefix_maps_is_an_import_only() {
    let extraction = resolve_with(
        b"<?php\nnamespace App;\n\nuse Psr\\Log\\LoggerInterface;\nuse App\\Missing;\n\nclass Service {}\n",
        "src/Service.php",
        &["src/Service.php"],
        &[("composer.json", APP_SRC)],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(
        imports(&extraction),
        ["Psr\\Log\\LoggerInterface", "App\\Missing"]
    );
}

#[test]
fn a_longer_prefix_maps_first_and_directories_rebase_on_their_composer_json() {
    let composer = r#"{
        "autoload": { "psr-4": { "App\\": "src/" } },
        "autoload-dev": { "psr-4": { "App\\Tests\\": ["tests/", "spec/"] } }
    }"#;
    let extraction = resolve_with(
        b"<?php\nuse App\\Tests\\FooTest;\nuse App\\Tests\\BarSpec;\nuse \\App\\Models\\User;\n",
        "backend/src/boot.php",
        &[
            "backend/src/Tests/FooTest.php",
            "backend/tests/FooTest.php",
            "backend/spec/BarSpec.php",
            "backend/src/Models/User.php",
        ],
        &[("backend/composer.json", composer)],
    );

    assert_eq!(
        includes(&extraction),
        [
            "backend/tests/FooTest.php",
            "backend/spec/BarSpec.php",
            "backend/src/Models/User.php"
        ]
    );
}

#[test]
fn the_empty_prefix_maps_any_name_after_every_other_prefix() {
    let composer = r#"{ "autoload": { "psr-4": { "": "lib/", "App\\": "src/" } } }"#;
    let extraction = resolve_with(
        b"<?php\nuse Widget;\nuse App\\Foo;\nuse Acme\\Gadget;\nuse Missing;\n",
        "boot.php",
        &[
            "lib/Widget.php",
            "src/Foo.php",
            "lib/App/Foo.php",
            "lib/Acme/Gadget.php",
        ],
        &[("composer.json", composer)],
    );

    assert_eq!(
        includes(&extraction),
        ["lib/Widget.php", "src/Foo.php", "lib/Acme/Gadget.php"]
    );
    assert_eq!(imports(&extraction), ["Missing"]);
}

#[test]
fn a_vendored_or_unreadable_composer_json_maps_nothing() {
    let extraction = resolve_with(
        b"<?php\nuse Vendor\\Lib\\Thing;\nuse App\\Foo;\n",
        "src/boot.php",
        &["vendor/acme/lib/src/Lib/Thing.php", "src/Foo.php"],
        &[
            (
                "vendor/acme/lib/composer.json",
                r#"{ "autoload": { "psr-4": { "Vendor\\": "src/" } } }"#,
            ),
            ("composer.json", "not json"),
        ],
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_test_file_marks_every_symbol_as_test_code() {
    for path in [
        "FooTest.php",
        "app/tests/Util.php",
        "app/test/Helper.php",
        "app/Tests/Util.php",
    ] {
        let extraction = resolve(WIDGET, path, &[]);
        assert!(!extraction.symbols.is_empty(), "{path}");
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_test()),
            "{path}"
        );
    }
    for path in [
        "app/src/Foo.php",
        "latest.php",
        "app/src/fooTest.phtml",
        "app/src/FooTest.PHP",
    ] {
        let extraction = resolve(WIDGET, path, &[]);
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| !symbol.flags.is_test()),
            "{path}"
        );
    }
}

#[test]
fn a_generated_name_marks_every_symbol_as_generated() {
    for path in ["proto/Widget.generated.php", "proto/widget_pb.php"] {
        let extraction = resolve(WIDGET, path, &[]);
        assert!(!extraction.symbols.is_empty(), "{path}");
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_generated()),
            "{path}"
        );
    }
}

#[test]
fn a_marker_in_the_leading_comments_marks_the_file_generated() {
    let generated = [
        &b"<?php\n// @generated by tooling\ndeclare(strict_types=1);\n\nclass Widget {}\n"[..],
        b"<?php /* DO NOT EDIT */\nclass Widget {}\n",
        b"<?php\n/*\n * @generated by tooling\n */\nclass Widget {}\n",
        b"<?php\n/* license */ // @generated by tooling\nclass Widget {}\n",
        b"\xEF\xBB\xBF<?php\n// @generated by tooling\nclass Widget {}\n",
    ];
    let ordinary = [
        &b"<?php\n// A plain widget.\nclass Widget {}\n"[..],
        b"<?php\nclass Widget {\n    // @generated\n    public function run(): int { return 1; }\n}\n",
        b"<?php\n/* license header */ $boot = 1;\n// @generated by tooling\nclass Widget {}\n",
        b"<?php\n/*\n * ordinary banner\n */ $mode = '@generated';\nclass Widget {}\n",
    ];

    for source in generated {
        let extraction = resolve(source, "src/Widget.php", &[]);
        assert_ne!(extraction.symbols.len(), 0);
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_generated()),
            "{}",
            String::from_utf8_lossy(source)
        );
    }
    for source in ordinary {
        let extraction = resolve(source, "src/Widget.php", &[]);
        assert_ne!(extraction.symbols.len(), 0);
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| !symbol.flags.is_generated()),
            "{}",
            String::from_utf8_lossy(source)
        );
    }
}

#[test]
fn no_symbol_is_an_entry_point() {
    let sample = resolve(SAMPLE, "sample.php", &[]);
    let front = resolve(
        b"<?php\nfunction main(): void {\n    run();\n}\n\nmain();\n",
        "public/index.php",
        &[],
    );

    for extraction in [sample, front] {
        assert_ne!(extraction.symbols.len(), 0);
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| !symbol.flags.is_entry())
        );
    }
}

fn doc_line(source: &str, name: &str) -> u32 {
    let line = symbol(&extract(source.as_bytes()).symbols, name).line;
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    Php::new().doc_comments().first_line(&lines, line)
}

#[test]
fn a_doc_block_and_the_attributes_below_it_document_a_function() {
    let source = "<?php
/**
 * Parses.
 */
#[Pure]
function parse() {}

# Renders.
// Twice.
function render() {}
";

    assert_eq!(doc_line(source, "parse"), 2);
    assert_eq!(doc_line(source, "render"), 8);
}

fn uses(raw: &RawExtraction) -> Vec<(&str, &Receiver, SymbolId, u32)> {
    raw.uses
        .iter()
        .map(|used| {
            assert_eq!(used.access, Access::Read, "{used:?}");
            (used.name.as_str(), &used.receiver, used.site_of, used.line)
        })
        .collect()
}

#[test]
fn a_callable_spelled_as_a_string_or_an_array_is_a_use_of_what_it_names() {
    let raw = extract(
        br#"<?php
function slugify($v) { return $v; }
add_action('init', 'slugify');
class Feed {
    private $hooks = [[self::class, 'make']];
    public function boot() {
        $names = array_map('slugify', $this->names);
        usort($names, "Sorter::compare");
        $hooks = ['view' => [$this, 'render'], [self::class, 'make'], [Cache::class, 'flush']];
        call_user_func(__NAMESPACE__ . '\slugify');
    }
}
"#,
    );

    let (feed, boot) = (
        id_of(&raw.symbols, "Feed"),
        id_of(&raw.symbols, "Feed::boot"),
    );
    let feed_path = Receiver::Path("Feed".to_owned());
    assert_eq!(
        uses(&raw),
        [
            ("slugify", &Receiver::Bare, FILE_SCOPE, 3),
            ("make", &feed_path, feed, 5),
            ("slugify", &Receiver::Bare, boot, 7),
            ("compare", &Receiver::Path("Sorter".to_owned()), boot, 8),
            ("render", &Receiver::Own, boot, 9),
            ("make", &Receiver::Own, boot, 9),
            ("flush", &Receiver::Path("Cache".to_owned()), boot, 9),
        ]
    );
}

#[test]
fn a_callable_string_names_a_function_of_the_namespace_it_spells() {
    let raw = extract(
        br"<?php
namespace App;
function helper() {}
function run() {
    array_map('App\helper', []);
    array_map('\App\helper', []);
    array_map('helper', []);
    array_map('Other\helper', []);
    usort($items, 'App\Sorter::compare');
    usort($items, 'Sorter::compare');
}
",
    );

    let run = id_of(&raw.symbols, "App\\run");
    assert_eq!(
        uses(&raw),
        [
            ("helper", &Receiver::Bare, run, 5),
            ("helper", &Receiver::Bare, run, 6),
            ("compare", &Receiver::Path("Sorter".to_owned()), run, 9),
        ]
    );
}

#[test]
fn a_string_no_callable_parameter_takes_names_nothing() {
    let raw = extract(
        br#"<?php
function init() {}
do_action('init');
if (!function_exists('init')) { function init() {} }
$pair = ['Cache', 'warm'];
$keyed = ['handler' => 'run'];
array_map("prefix_$name", $items);
App\Util\array_map('init', $items);
"#,
    );

    assert_eq!(uses(&raw), []);
}

#[test]
fn a_type_named_as_one_is_no_use_of_it() {
    let raw = extract(
        br"<?php
namespace App;
use App\Shapes\Shape;
interface Drawable {}
class Circle extends Shape implements Drawable {
    private Size $size;
    public function grow(Size $by): Circle {
        if ($by instanceof Size) { return new Circle(); }
        return Circle::make(Size::class);
    }
}
",
    );

    let declared = names(&raw.symbols);
    for name in ["App\\Drawable", "App\\Circle", "App\\Circle::grow"] {
        assert!(declared.contains(&name), "{name} in {declared:?}");
    }
    assert_eq!(uses(&raw), []);
    assert_eq!(Php::new().conventions().type_uses, TypeUses::Nowhere);
}

#[test]
fn a_read_or_a_write_of_a_property_or_a_constant_is_no_use_of_it() {
    let raw = extract(
        b"<?php
const TOP = 1;
class Feed {
    const MAX = 10;
    private $count = 0;
    public function bump() {
        $this->count += self::MAX + TOP;
        return Feed::MAX;
    }
}
",
    );

    for (name, kind) in [
        ("TOP", SymbolKind::Constant),
        ("Feed::MAX", SymbolKind::Constant),
        ("Feed::count", SymbolKind::Field),
    ] {
        assert!(
            raw.symbols
                .iter()
                .any(|symbol| symbol.qualified_name == name && symbol.kind == kind),
            "{name} in {:?}",
            names(&raw.symbols)
        );
    }
    assert_eq!(uses(&raw), []);
    assert_eq!(
        Php::new().conventions().unrecorded_data,
        [SymbolKind::Field, SymbolKind::Constant]
    );
}
