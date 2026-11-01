//! What class a PHP call on a property of the class around it records as its receiver: the one the property's declared
//! type names, or, for a property with no type, the one every write in the class's own body gives it.

use kartta_lang::{Language, SymbolKind};
use kartta_lang_php::Php;

const CONTAINER: Option<&str> = Some("\\App\\Container");
const UNNAMED: Option<&str> = Some("");
const INHERITED: Option<&str> = Some("->c");

/// The receiver of the call of `m` in `Suite`, whose body is `members` and which calls `m` on the property `c`, in the
/// namespace `App`.
fn receiver(members: &str) -> Option<String> {
    let source = format!(
        "<?php\nnamespace App;\nuse Other\\Client;\nclass Suite extends Base {{\n    public function run() {{ \
         $this->c->m(); }}\n{members}\n}}\n"
    );
    let raw = Php::new().extract(source.as_bytes());
    raw.calls
        .into_iter()
        .find(|call| call.callee == "m")
        .and_then(|call| call.receiver)
}

fn in_a_method(body: &str) -> String {
    format!("public function set($a) {{ {body} }}")
}

#[test]
fn a_declared_type_names_the_class_as_php_compiles_it() {
    for (declared, class) in [
        ("Container", "\\App\\Container"),
        ("\\App\\Container", "\\App\\Container"),
        ("namespace\\Container", "\\App\\Container"),
        ("Client", "\\Other\\Client"),
        ("Client\\Part", "\\Other\\Client\\Part"),
        ("self", "self"),
        ("SELF", "self"),
        ("parent", "parent"),
    ] {
        assert_eq!(
            receiver(&format!("private {declared} $c;")).as_deref(),
            Some(class),
            "{declared}",
        );
    }
}

#[test]
fn a_promoted_parameter_is_a_declared_property() {
    assert_eq!(
        receiver("public function __construct(private Container $c) {}").as_deref(),
        CONTAINER,
    );
    assert_eq!(
        receiver("public function __construct(public readonly ?Container $c = null) {}").as_deref(),
        CONTAINER,
    );
    assert_eq!(
        receiver("public function __construct(private Container &$c) {}").as_deref(),
        CONTAINER,
    );
}

#[test]
fn a_call_the_walk_drops_moves_no_receiver() {
    let raw = Php::new().extract(
        b"<?php\nclass Suite {\n    private First $c;\n    private Second $d;\n    public function run() \
          {\n        $this->c->();\n        $this->d->m();\n    }\n}\n",
    );
    let call = raw.calls.iter().find(|call| call.callee == "m");
    assert_eq!(
        call.and_then(|call| call.receiver.as_deref()),
        Some("\\Second")
    );
}

#[test]
fn an_alternative_that_holds_no_object_leaves_the_class() {
    for declared in [
        "?Container",
        "Container|null",
        "null|Container",
        "Container|false",
        "int|Container|array",
    ] {
        assert_eq!(
            receiver(&format!("private {declared} $c;")).as_deref(),
            CONTAINER,
            "{declared}",
        );
    }
}

#[test]
fn a_type_that_can_hold_two_classes_names_none() {
    for declared in [
        "Container|Client",
        "Container&Client",
        "(Container&Client)|null",
        "object",
        "?object",
        "mixed",
        "iterable",
        "iterable|null",
        "Container|iterable",
        "int",
        "array",
    ] {
        assert_eq!(
            receiver(&format!("private {declared} $c;")).as_deref(),
            UNNAMED,
            "{declared}",
        );
    }
}

#[test]
fn a_declared_type_holds_for_every_read_whatever_the_writes() {
    let writes = in_a_method("$this->c = new Client; $this->c = $a;");
    assert_eq!(
        receiver(&format!("private Container $c;\n{writes}")).as_deref(),
        CONTAINER,
    );
    assert_eq!(
        receiver(&format!("{writes}\nprivate Container $c;")).as_deref(),
        CONTAINER,
    );
    assert_eq!(
        receiver(&format!("private int $c;\n{writes}")).as_deref(),
        UNNAMED,
    );
}

#[test]
fn every_declaration_of_a_list_takes_the_type() {
    assert_eq!(receiver("private Container $a, $c;").as_deref(), CONTAINER);
}

#[test]
fn a_write_in_any_member_names_the_class_of_a_property_with_no_type() {
    for members in [
        "private $c;\npublic function __construct() { $this->c = new Container; }",
        "public function __construct() { $this->c = new Container; }\nprivate $c = null;",
        "public function __construct() { $this->c = new Container(1); }",
        "public function get() { return $this->c ??= new Container; }",
        "public function get() { if (!$this->c) { $this->c = new Container; } }",
        "public function get() { $f = function () { $this->c = new Container; }; }",
    ] {
        assert_eq!(receiver(members).as_deref(), CONTAINER, "{members}");
    }
}

#[test]
fn a_write_of_a_variable_names_the_class_the_variable_holds() {
    let members = in_a_method("$v = new Container; $this->c = $v;");
    assert_eq!(receiver(&members).as_deref(), CONTAINER);
}

#[test]
fn a_chained_assignment_writes_the_value_assigned_whatever_the_target_between() {
    for write in [
        "$this->c = $v = new Container;",
        "$this->c = $v = $w = new Container;",
        "$this->c = $this->d = new Container;",
        "$this->c = $this->typed = new Container;",
        "$this->c = self::$s = new Container;",
        "$this->c = $a->p = new Container;",
        "$this->c = $a['k'] = new Container;",
        "$this->c = [$v] = new Container;",
        "$this->c ??= $v = new Container;",
        "$v = new Container; $this->c = $w = $v;",
    ] {
        let members = format!("private string $typed;\n{}", in_a_method(write));
        assert_eq!(receiver(&members).as_deref(), CONTAINER, "{write}");
    }
}

#[test]
fn a_chained_null_says_nothing_of_the_property() {
    let members = in_a_method("$this->c = new Container; $this->c = $v = null;");
    assert_eq!(receiver(&members).as_deref(), CONTAINER);
}

#[test]
fn a_cleared_property_keeps_its_class() {
    for clear in ["$this->c = null;", "$this->c = NULL;", "unset($this->c);"] {
        let members = in_a_method(&format!("$this->c = new Container; {clear}"));
        assert_eq!(receiver(&members).as_deref(), CONTAINER, "{clear}");
    }
}

#[test]
fn only_writes_of_null_name_no_class() {
    assert_eq!(
        receiver(&in_a_method("$this->c = null;")).as_deref(),
        INHERITED
    );
}

#[test]
fn two_classes_written_to_one_property_name_none() {
    let members = in_a_method("$this->c = new Container; $this->c = new Client;");
    assert_eq!(receiver(&members).as_deref(), UNNAMED);
}

#[test]
fn a_class_beside_any_other_value_names_none() {
    for write in [
        "$this->c = $a;",
        "$this->c = $this->make();",
        "$this->c = new $a;",
        "$this->c = $a ? new Container : null;",
        "$this->c = $b = $a;",
        "$this->c = $b = $this->make();",
        "$this->c = $b ??= new Container;",
        "$this->c = $b = &$a;",
        "$this->c = $b = new class {};",
        "$this->c = &$a;",
        "$this->c .= 'x';",
        "$other = &$this->c;",
        "[$this->c, $b] = $a;",
        "['k' => $this->c] = $a;",
        "foreach ($a as $this->c) {}",
        "foreach ($a as $k => $this->c) {}",
    ] {
        let first = in_a_method(&format!("$this->c = new Container; {write}"));
        assert_eq!(receiver(&first).as_deref(), UNNAMED, "{write} after");
        let second = in_a_method(&format!("{write} $this->c = new Container;"));
        assert_eq!(receiver(&second).as_deref(), UNNAMED, "{write} before");
    }
}

#[test]
fn a_compound_assignment_stores_its_operator_s_result_whatever_its_right_side_names() {
    for operator in [
        ".=", "+=", "-=", "*=", "/=", "%=", "**=", "|=", "&=", "^=", "<<=", ">>=",
    ] {
        for write in [
            format!("$this->c {operator} new Container;"),
            format!("self::$c {operator} new Container;"),
            format!("$v = new Container; $this->c {operator} $v;"),
            format!("$this->c {operator} $v = new Container;"),
        ] {
            assert_eq!(
                receiver(&in_a_method(&write)).as_deref(),
                UNNAMED,
                "{write}"
            );
        }
        let beside_a_class = format!("$this->c = new Container; $this->c {operator} null;");
        assert_eq!(
            receiver(&in_a_method(&beside_a_class)).as_deref(),
            UNNAMED,
            "{beside_a_class}",
        );
    }
}

#[test]
fn a_null_coalescing_assignment_writes_its_right_side() {
    for write in [
        "$v = new Container; $this->c ??= $v;",
        "$this->c = new Container; $this->c ??= null;",
    ] {
        assert_eq!(
            receiver(&in_a_method(write)).as_deref(),
            CONTAINER,
            "{write}"
        );
    }
}

#[test]
fn a_default_or_an_untyped_promotion_is_another_value_beside_a_write() {
    let write = in_a_method("$this->c = new Container;");
    assert_eq!(
        receiver(&format!("private $c = [];\n{write}")).as_deref(),
        UNNAMED,
    );
    assert_eq!(
        receiver(&format!(
            "public function __construct(private $c) {{}}\n{write}"
        ))
        .as_deref(),
        UNNAMED,
    );
}

#[test]
fn an_anonymous_class_written_to_a_property_goes_by_the_name_the_walk_gave_it() {
    let members = in_a_method("$this->c = new class { public function m() {} };");
    assert_eq!(receiver(&members).as_deref(), Some("{set·c}"));
}

#[test]
fn a_nullsafe_read_reads_the_same_class() {
    for read in ["$this->c?->m();", "$this?->c->m();"] {
        let source = format!(
            "<?php\nnamespace App;\nclass Suite {{\n    private Container $c;\n    public function run() {{ \
             {read} }}\n}}\n"
        );
        let raw = Php::new().extract(source.as_bytes());
        assert_eq!(raw.calls[0].receiver.as_deref(), CONTAINER, "{read}");
    }
}

#[test]
fn a_write_elsewhere_than_the_callers_class_names_nothing() {
    for members in [
        "public function set($o) { $o->c = new Container; }",
        "public function set() { $this->a->c = new Container; }",
        "public function set() { $this->c['k'] = new Container; }",
        "public function set() { $this->c->d = new Container; }",
        "public function set() { $k = new class { public function s() { $this->c = new Container; } }; }",
    ] {
        assert_eq!(receiver(members).as_deref(), INHERITED, "{members}");
    }
}

#[test]
fn a_class_body_answers_for_its_own_property_alone() {
    let raw = Php::new().extract(
        br"<?php
class One {
    private First $c;
    public function run() { $this->c->m(); }
}
class Two {
    private Second $c;
    public function run() {
        $inner = new class {
            private Third $c;
            public function run() { $this->c->m(); }
        };
        $this->c->m();
    }
}
trait Three {
    public function run() { $this->c->m(); }
}
function free() { $this->c->m(); }
",
    );
    let receivers: Vec<Option<&str>> = raw
        .calls
        .iter()
        .filter(|call| call.callee == "m")
        .map(|call| call.receiver.as_deref())
        .collect();
    assert_eq!(
        receivers,
        [
            Some("\\First"),
            Some("\\Third"),
            Some("\\Second"),
            Some(""),
            Some("")
        ],
    );
}

#[test]
fn a_receiver_past_the_property_keeps_its_shape() {
    for read in [
        "$this->c->d->m();",
        "$this->c()->m();",
        "$other->c->m();",
        "$this->$c->m();",
        "$this->c[0]->m();",
    ] {
        let source = format!(
            "<?php\nclass Suite {{\n    private Container $c;\n    public function run() {{ {read} }}\n}}\n"
        );
        let raw = Php::new().extract(source.as_bytes());
        let call = raw
            .calls
            .iter()
            .find(|call| call.callee == "m")
            .expect("the call");
        assert_eq!(call.receiver.as_deref(), Some(""), "{read}");
    }
}

/// The receiver of the call of `m` in `read`, the body of a method of `App\Suite` next to `members`.
fn static_receiver(read: &str, members: &str) -> Option<String> {
    let source = format!(
        "<?php\nnamespace App;\nuse App\\Suite as Alias;\nclass Suite extends Base {{\n    public static function \
         run() {{ {read} }}\n{members}\n}}\n"
    );
    let raw = Php::new().extract(source.as_bytes());
    raw.calls
        .into_iter()
        .find(|call| call.callee == "m")
        .and_then(|call| call.receiver)
}

#[test]
fn a_static_property_of_the_class_around_the_call_reads_as_an_instance_one_does() {
    for read in [
        "self::$c->m();",
        "SELF::$c->m();",
        "static::$c->m();",
        "self::$c?->m();",
        "Suite::$c->m();",
        "suite::$c->m();",
        "\\App\\Suite::$c->m();",
        "namespace\\Suite::$c->m();",
        "Alias::$c->m();",
    ] {
        assert_eq!(
            static_receiver(read, "private static Container $c;").as_deref(),
            CONTAINER,
            "{read}",
        );
    }
}

#[test]
fn a_write_of_a_static_property_names_the_class_as_a_write_of_an_instance_one_does() {
    for members in [
        "private static $c;\npublic static function set() { self::$c = new Container; }",
        "public static function set() { static::$c = new Container; }\nprivate static $c = null;",
        "public static function set() { Suite::$c ??= new Container; }",
    ] {
        assert_eq!(
            static_receiver("self::$c->m();", members).as_deref(),
            CONTAINER,
            "{members}",
        );
    }
    assert_eq!(
        static_receiver(
            "self::$c->m();",
            "public static function set() { self::$c = new Container; self::$c = new Client; }",
        )
        .as_deref(),
        UNNAMED,
    );
    assert_eq!(
        static_receiver(
            "self::$c->m();",
            "public static function set() { self::$c = new class { public function m() {} }; }",
        )
        .as_deref(),
        Some("{set·c}"),
    );
}

#[test]
fn a_static_property_of_a_class_but_the_one_around_the_call_names_nothing() {
    for read in [
        "parent::$c->m();",
        "Base::$c->m();",
        "$class::$c->m();",
        "Suite::$$c->m();",
        "self::$$c->m();",
    ] {
        assert_eq!(
            static_receiver(read, "private static Container $c;").as_deref(),
            UNNAMED,
            "{read}",
        );
    }
    assert_eq!(
        static_receiver(
            "self::$c->m();",
            "public static function set() { parent::$c = new Container; Base::$c = new Container; }",
        )
        .as_deref(),
        INHERITED,
    );
}

#[test]
fn a_static_property_a_trait_declares_reads_from_the_trait() {
    let raw = Php::new().extract(
        br"<?php
trait Holds {
    private static Container $c;
    public function run() { self::$c->m(); }
}
class Using {
    use Holds;
    public function run() { self::$c->m(); }
}
",
    );
    let receivers: Vec<Option<&str>> = raw
        .calls
        .iter()
        .map(|call| call.receiver.as_deref())
        .collect();
    assert_eq!(receivers, [Some("\\Container"), Some("->c")]);
}

#[test]
fn a_computed_property_name_names_no_property() {
    let raw = Php::new().extract(
        br"<?php
class Suite {
    public function run($c) {
        $this->$c = new Container;
        $this->$c->m();
    }
}
",
    );
    let call = raw.calls.iter().find(|call| call.callee == "m");
    assert_eq!(call.and_then(|call| call.receiver.as_deref()), Some(""));
}

#[test]
fn a_computed_name_writes_no_property() {
    for write in [
        "$this->$c = new Container;",
        "$this->{$c} = new Container;",
        "$this->{c} = new Container;",
        "self::$$c = new Container;",
        "self::${$c} = new Container;",
        "self::${c} = new Container;",
    ] {
        assert_eq!(
            static_receiver(
                "self::$c->m();",
                &format!("public function set($c) {{ {write} }}"),
            )
            .as_deref(),
            INHERITED,
            "{write}",
        );
    }
}

#[test]
fn a_computed_name_reads_no_property() {
    for read in [
        "$this->$c->m();",
        "$this->{$c}->m();",
        "$this->{c}->m();",
        "$this?->{c}->m();",
        "self::$$c->m();",
        "self::${$c}->m();",
        "self::${c}->m();",
    ] {
        assert_eq!(
            static_receiver(read, "private Container $c;").as_deref(),
            UNNAMED,
            "{read}",
        );
    }
}

#[test]
fn a_scoped_call_on_a_property_reads_the_class_the_property_holds() {
    for read in [
        "$this->c::m();",
        "$this?->c::m();",
        "self::$c::m();",
        "SELF::$c::m();",
        "static::$c::m();",
        "Suite::$c::m();",
        "\\App\\Suite::$c::m();",
        "Alias::$c::m();",
    ] {
        assert_eq!(
            static_receiver(read, "private Container $c;").as_deref(),
            CONTAINER,
            "{read}",
        );
        assert_eq!(
            static_receiver(
                read,
                "private $c;\npublic function set() { $this->c = new Container; }",
            )
            .as_deref(),
            CONTAINER,
            "{read} on a write",
        );
    }
}

#[test]
fn a_scoped_call_on_a_property_that_holds_no_one_class_names_none() {
    for (members, class) in [
        ("private string $c;", UNNAMED),
        ("private ?string $c;", UNNAMED),
        ("private $c;", INHERITED),
        ("private object $c;", UNNAMED),
        ("private $c = 'Container';", UNNAMED),
        ("public function set($a) { $this->c = $a; }", UNNAMED),
        (
            "public function set() { $this->c = Container::class; }",
            UNNAMED,
        ),
        (
            "public function set() { $this->c = new Container; $this->c = 'Other'; }",
            UNNAMED,
        ),
    ] {
        for read in ["$this->c::m();", "self::$c::m();", "static::$c::m();"] {
            assert_eq!(
                static_receiver(read, members).as_deref(),
                class,
                "{read} on {members}",
            );
        }
    }
}

#[test]
fn a_property_a_class_with_no_parents_holds_nothing_of_keeps_the_empty_receiver() {
    let raw = Php::new().extract(b"<?php\nclass A { function f() { $this->c->m(); } }\n");
    assert_eq!(raw.calls[0].receiver.as_deref(), UNNAMED);
}

#[test]
fn a_trait_a_class_uses_after_the_call_counts_as_a_parent() {
    let raw = Php::new().extract(b"<?php\nclass A { function f() { $this->c->m(); } use T; }\n");
    assert_eq!(raw.calls[0].receiver.as_deref(), INHERITED);
}

/// The value type of the field `c` of each class of `source`, in the namespace `App`, in the order of the file.
fn field_types(source: &str) -> Vec<Vec<String>> {
    let source = format!("<?php\nnamespace App;\n{source}\n");
    let raw = Php::new().extract(source.as_bytes());
    let fields = raw.symbols.into_iter();
    fields
        .filter(|symbol| symbol.kind == SymbolKind::Field)
        .map(|symbol| symbol.value_types)
        .collect()
}

#[test]
fn a_field_holds_the_class_a_child_reads_from_the_root() {
    for (declaration, class) in [
        ("class K { protected Container $c; }", "\\App\\Container"),
        (
            "class K { protected $c; function f() { $this->c = new Container; } }",
            "\\App\\Container",
        ),
        (
            "class K { function __construct(protected ?Container $c) {} }",
            "\\App\\Container",
        ),
        ("class K { protected self $c; }", "\\App\\K"),
        (
            "class K { protected $c; function f() { $this->c = new static; } }",
            "static",
        ),
        (
            "trait K { protected $c; function f() { $this->c = new static; } }",
            "static",
        ),
        ("class K extends B { protected parent $c; }", "\\App\\B"),
    ] {
        assert_eq!(
            field_types(declaration),
            [[class.to_owned()]],
            "{declaration}"
        );
    }
}

#[test]
fn a_field_with_no_class_a_child_can_name_holds_none() {
    for declaration in [
        "class K { protected int $c; }",
        "class K { protected $c; }",
        "class K { protected parent $c; }",
        "trait K { protected self $c; }",
        "trait K { protected parent $c; }",
        "class K { protected $c; function f() { $this->c = new class {}; } }",
    ] {
        assert_eq!(
            field_types(declaration),
            [Vec::<String>::new()],
            "{declaration}"
        );
    }
}

#[test]
fn a_scoped_call_past_the_class_around_it_or_a_computed_name_names_none() {
    for read in [
        "parent::$c::m();",
        "Base::$c::m();",
        "$other->c::m();",
        "$this->$c::m();",
        "$this->{c}::m();",
        "self::$$c::m();",
        "$this->c->d::m();",
        "$this->c::$d::m();",
        "$this->c()::m();",
    ] {
        assert_eq!(
            static_receiver(read, "private Container $c;").as_deref(),
            UNNAMED,
            "{read}",
        );
    }
}

#[test]
fn a_computed_method_on_a_property_keeps_its_name_and_takes_the_class() {
    for (read, callee) in [("$this->c::{X}();", "$X"), ("$this->c::$m();", "$m")] {
        let source = format!(
            "<?php\nnamespace App;\nclass Suite {{\n    private Container $c;\n    public function run() {{ {read} \
             }}\n}}\n"
        );
        let raw = Php::new().extract(source.as_bytes());
        assert_eq!(raw.calls[0].callee, callee, "{read}");
        assert_eq!(raw.calls[0].receiver.as_deref(), CONTAINER, "{read}");
    }
}

#[test]
fn a_constant_on_a_property_is_no_call() {
    let raw = Php::new().extract(
        b"<?php\nclass Suite {\n    private Container $c;\n    public function run() { return $this->c::NAME; \
          }\n}\n",
    );
    assert_eq!(raw.calls, [] as [kartta_lang::CallRef; 0]);
}

#[test]
fn a_spelled_name_beside_a_comment_stays_the_property() {
    for read in ["$this->c /* note */ ->m();", "self::$c /* note */ ->m();"] {
        assert_eq!(
            static_receiver(read, "private Container $c;").as_deref(),
            CONTAINER,
            "{read}",
        );
    }
}
