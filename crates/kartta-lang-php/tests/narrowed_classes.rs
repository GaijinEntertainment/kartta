//! What class a PHP call records as its receiver where an `instanceof` settles what the variable or the property of
//! `$this` holds: on the paths the test's outcome alone reaches, up to the next write.

use kartta_lang::Language;
use kartta_lang_php::Php;

const LOCAL: Option<&str> = Some("\\App\\Local");
const FAR: Option<&str> = Some("\\App\\Far");
const UNTYPED: Option<&str> = Some("$v");
const UNNAMED: Option<&str> = Some("");

/// The receiver of the call of `name` in `body`, which runs in a method of `Suite` that declares `$v, $a`.
fn receiver(body: &str, name: &str) -> Option<String> {
    receiver_given("$v, $a", body, name)
}

fn receiver_given(parameters: &str, body: &str, name: &str) -> Option<String> {
    let source = format!(
        "<?php\nnamespace App;\nuse Other\\Client;\nuse Other\\Pool as Clients;\nclass Suite extends Base {{\n    \
         public function run({parameters}) {{\n{body}\n    }}\n}}\n"
    );
    let raw = Php::new().extract(source.as_bytes());
    raw.calls
        .into_iter()
        .find(|call| call.callee == name)
        .and_then(|call| call.receiver)
}

#[test]
fn a_test_names_its_class_as_php_compiles_it() {
    for (class, narrowed) in [
        ("Local", "\\App\\Local"),
        ("\\App\\Local", "\\App\\Local"),
        ("namespace\\Local", "\\App\\Local"),
        ("Client", "\\Other\\Client"),
        ("client", "\\Other\\Client"),
        ("Clients", "\\Other\\Pool"),
        ("Client\\Part", "\\Other\\Client\\Part"),
        ("\\Exception", "\\Exception"),
        ("self", "self"),
        ("SELF", "self"),
        ("static", "static"),
        ("parent", "parent"),
    ] {
        let body = format!("if ($v instanceof {class}) {{ $v->m(); }}");
        assert_eq!(receiver(&body, "m").as_deref(), Some(narrowed), "{class}");
    }
}

#[test]
fn a_test_that_names_no_class_leaves_the_variable() {
    for test in [
        "$v instanceof $a",
        "$v instanceof $a->c",
        "$v instanceof ($a)",
        "$a instanceof Local",
    ] {
        let body = format!("if ({test}) {{ $v->m(); }}");
        assert_eq!(receiver(&body, "m").as_deref(), UNTYPED, "{test}");
    }
}

#[test]
fn the_if_body_holds_the_class_and_ends_with_it() {
    let body = "if ($v instanceof Local) { $v->in(); $v::make(); $v?->safe(); } $v->m();";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "make").as_deref(), LOCAL);
    assert_eq!(receiver(body, "safe").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);

    for body in [
        "if ($v instanceof Local) $v->in();",
        "if ($v instanceof Local): $v->in(); endif;",
        "if ($v INSTANCEOF Local) { $v->in(); }",
    ] {
        assert_eq!(receiver(body, "in").as_deref(), LOCAL, "{body}");
    }
}

#[test]
fn a_test_replaces_a_declared_class_past_which_the_type_stands_again() {
    let body = "if ($v instanceof Local) { $v->in(); } $v->m();";
    assert_eq!(receiver_given("Far $v", body, "in").as_deref(), LOCAL);
    assert_eq!(receiver_given("Far $v", body, "m").as_deref(), FAR);
}

#[test]
fn a_negated_test_with_a_body_that_leaves_holds_past_the_if() {
    for leaves in [
        "return;",
        "return 1;",
        "throw new E;",
        "exit;",
        "exit(1);",
        "die;",
        "die('x');",
        "DIE();",
        "continue;",
        "break;",
        "continue 2;",
    ] {
        for body in [
            format!("while ($a) {{ if (!$v instanceof Local) {{ {leaves} }} $v->m(); }}"),
            format!("while ($a) {{ if (!$v instanceof Local) {leaves} $v->m(); }}"),
            format!("while ($a) {{ if (!$v instanceof Local): {leaves} endif; $v->m(); }}"),
            format!(
                "while ($a) {{ if (!($v instanceof Local)) {{ {leaves} /* why */ }} $v->m(); }}"
            ),
        ] {
            assert_eq!(receiver(&body, "m").as_deref(), LOCAL, "{body}");
        }
    }
}

#[test]
fn the_last_statement_of_a_body_decides_that_it_leaves() {
    let body = "if (!$v instanceof Local) { f(); return; } $v->m();";
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);
}

#[test]
fn a_body_that_may_fall_through_leaves_the_class_at_the_if() {
    for body in [
        "if (!$v instanceof Local) { f(); } $v->m();",
        "if (!$v instanceof Local) { if ($a) { return; } } $v->m();",
        "if (!$v instanceof Local) { if ($a) { return; } else { return; } } $v->m();",
        "if ($a) { if (!$v instanceof Local) { return; } } $v->m();",
        "if ($v instanceof Local) { return; } $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }
}

#[test]
fn a_class_past_the_if_ends_with_the_part_around_the_if() {
    let body = "if ($a) { if (!$v instanceof Local) { return; } $v->in(); } else { $v->other(); } $v->m();";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "other").as_deref(), UNTYPED);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);

    let body =
        "switch ($a) { case 1: if (!$v instanceof Local) { break; } $v->in(); case 2: $v->m(); }";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);
}

#[test]
fn an_else_holds_what_the_test_s_failure_settles() {
    for body in [
        "if (!$v instanceof Local) { f(); } else { $v->m(); }",
        "if (!$v instanceof Local) { f(); } elseif ($a) { $v->m(); }",
        "if (!$v instanceof Local) { f(); } elseif ($a) { g(); } else { $v->m(); }",
        "if ($a) { f(); } elseif (!$v instanceof Local) { g(); } else { $v->m(); }",
        "if (!$v instanceof Local): f(); else: $v->m(); endif;",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), LOCAL, "{body}");
    }

    for body in [
        "if ($v instanceof Local) { f(); } else { $v->m(); }",
        "if ($v instanceof Local) { f(); } elseif ($a) { $v->m(); }",
        "if ($a) { $v->m(); } elseif ($v instanceof Local) { f(); }",
        "if ($a) { f(); } elseif ($v instanceof Local) { g(); } else { $v->m(); }",
        "if ($a) { f(); } elseif (!$v instanceof Local) { return; } $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }

    let body = "if ($a) { f(); } elseif ($v instanceof Local) { $v->m(); }";
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);
}

#[test]
fn an_else_that_leaves_holds_the_test_past_the_if() {
    let body = "if ($v instanceof Local) { f(); } else { $v->other(); return; } $v->m();";
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);
    assert_eq!(receiver(body, "other").as_deref(), UNTYPED);

    for body in [
        "if ($v instanceof Local) { f(); } else { g(); } $v->m();",
        "if ($v instanceof Local) { f(); } elseif ($a) { return; } $v->m();",
        "if ($v instanceof Local) { f(); } elseif ($a) { return; } else { return; } $v->m();",
        "if ($a) { f(); } elseif ($v instanceof Local) { g(); } else { return; } $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }
}

#[test]
fn a_conjunction_holds_each_test_past_its_operand() {
    for body in [
        "$v instanceof Local && $v->m();",
        "$v instanceof Local and $v->m();",
        "$v instanceof Local AND $v->m();",
        "$a && $v instanceof Local && $v->m();",
        "if ($a && $v instanceof Local && $v->m()) { f(); }",
        "if ($v instanceof Local && $a) { $v->m(); }",
        "if ($a && $v instanceof Local) { $v->m(); }",
        "if (($a && $v instanceof Local) && $a) { $v->m(); }",
        "if (!(!$v instanceof Local || $a)) { $v->m(); }",
        "if (!($v instanceof Local && $a)) { f(); } else { $v->m(); }",
        "return $v instanceof Local && $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), LOCAL, "{body}");
    }
}

#[test]
fn a_disjunction_holds_what_a_failure_settles() {
    for body in [
        "!$v instanceof Local || $v->m();",
        "!$v instanceof Local or $v->m();",
        "if (!$v instanceof Local || !$v->m()) { f(); }",
        "if (!$v instanceof Local || $a) { return; } $v->m();",
        "if ($a || !$v instanceof Local) { return; } $v->m();",
        "if ($a || !$v instanceof Local) { f(); } else { $v->m(); }",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), LOCAL, "{body}");
    }

    for body in [
        "if ($v instanceof Local || $a) { $v->m(); }",
        "if ($a || $v instanceof Local) { $v->m(); }",
        "if ($v instanceof Local || $v instanceof Far) { $v->m(); }",
        "$v instanceof Local || $v->m();",
        "!$v instanceof Local && $v->m();",
        "if (!$v instanceof Local && $a) { return; } $v->m();",
        "if ($v instanceof Local xor $a) { $v->m(); }",
        "$v instanceof Local xor $v->m();",
        "if (-$v instanceof Local) { f(); } else { $v->m(); }",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }
}

#[test]
fn a_conditional_holds_the_class_in_the_part_the_test_chooses() {
    for body in [
        "$b = $v instanceof Local ? $v->m() : null;",
        "$b = !$v instanceof Local ? null : $v->m();",
        "$b = ($v instanceof Local && $a) ? $v->m() : null;",
        "$b = !$v instanceof Local ?: $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), LOCAL, "{body}");
    }

    for body in [
        "$b = $v instanceof Local ? null : $v->m();",
        "$b = $v instanceof Local ?: $v->m();",
        "$b = $a ? $v instanceof Local : $v->m();",
        "$b = ($v instanceof Local ? 1 : 2); $v->m();",
        "$b = $a ? (!$v instanceof Local) : $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }
}

#[test]
fn a_while_condition_holds_in_the_body_alone() {
    let body = "while ($v instanceof Local) { $v->in(); } $v->m();";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);

    let body = "while (!$v instanceof Local) { $v->in(); break; } $v->m();";
    assert_eq!(receiver(body, "in").as_deref(), UNTYPED);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);

    let body = "while ($v instanceof Local) { $v->in(); $v = $a; $v->m(); }";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);

    let body = "do { $v->m(); } while ($v instanceof Local);";
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);
}

#[test]
fn a_match_on_true_holds_an_arm_s_lone_condition_in_its_result() {
    for body in [
        "$b = match (true) { $v instanceof Local => $v->m(), default => null };",
        "$b = match (TRUE) { $a => 1, $v instanceof Local && $a => $v->m(), default => null };",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), LOCAL, "{body}");
    }

    for body in [
        "$b = match (false) { $v instanceof Local => $v->m(), default => null };",
        "$b = match ($a) { $v instanceof Local => $v->m(), default => null };",
        "$b = match (true) { !$v instanceof Local => $v->m(), default => null };",
        "$b = match (true) { $v instanceof Local, $a => $v->m(), default => null };",
        "$b = match (true) { $v instanceof Local => 1, default => $v->m() };",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }
}

#[test]
fn a_write_ends_the_class() {
    for body in [
        "if ($v instanceof Local) { $v = $a; $v->m(); }",
        "if (!$v instanceof Local) { return; } $v = $a; $v->m();",
        "if (!$v instanceof Local) { $v = $a; return; } $v->m();",
        "if (!$v instanceof Local) { return; } if ($a) { $v = $a; } $v->m();",
        "if ($v instanceof Local) { unset($v); $v->m(); }",
        "if ($v instanceof Local && ($v = f())) { $v->m(); }",
        "if ($v instanceof Local) { f(); } else { $v = $a; return; } $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }

    let body = "if ($v instanceof Local) { $v->in(); $v = new Far; $v->m(); }";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), FAR);
}

#[test]
fn a_closure_and_an_arrow_function_take_the_class_past_the_test() {
    for body in [
        "if ($v instanceof Local) { $f = function () use ($v) { $v->m(); }; }",
        "if ($v instanceof Local) { $f = fn () => $v->m(); }",
        "$f = function ($v) { if ($v instanceof Local) { $v->m(); } };",
        "$f = fn ($v) => $v instanceof Local ? $v->m() : null;",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), LOCAL, "{body}");
    }

    let body = "if ($v instanceof Local) { $f = function () { $v->m(); }; }";
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);
}

#[test]
fn a_test_stands_ahead_of_a_new() {
    for body in [
        "$v = new Far; if ($v instanceof Local) { $v->m(); }",
        "$v = new Far; if (!$v instanceof Local) { return; } $v->m();",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), LOCAL, "{body}");
    }
}

#[test]
fn this_stays_the_class_around_the_call() {
    let body = "if ($this instanceof Local) { $this->m(); }";
    assert_eq!(receiver(body, "m").as_deref(), Some("$this"));
}

/// The receiver of the call of `m` in `body`, in a class that declares its property `$c` as `Far`.
fn on_property(body: &str) -> Option<String> {
    let source = format!(
        "<?php\nnamespace App;\nclass Suite {{\n    private Far $c;\n    public function run($a) {{\n{body}\n    \
         }}\n}}\n"
    );
    let raw = Php::new().extract(source.as_bytes());
    raw.calls
        .into_iter()
        .find(|call| call.callee == "m")
        .and_then(|call| call.receiver)
}

#[test]
fn a_property_of_this_takes_the_class_past_a_test() {
    for body in [
        "if ($this->c instanceof Local) { $this->c->m(); }",
        "if ($this?->c instanceof Local) { $this->c->m(); }",
        "if ($this->c instanceof Local) { $this?->c->m(); }",
        "if (!$this->c instanceof Local) { return; } $this->c->m();",
        "$this->c instanceof Local && $this->c->m();",
        "$b = $this->c instanceof Local ? $this->c->m() : null;",
        "if ($this->c instanceof Local) { $this->other(); $this->c->m(); }",
        "if ($this->c instanceof Local) { $this->c::m(); }",
        "if (self::$c instanceof Local) { self::$c->m(); }",
    ] {
        assert_eq!(on_property(body).as_deref(), LOCAL, "{body}");
    }
}

#[test]
fn a_property_keeps_its_declared_class_where_no_test_settles_it() {
    for body in [
        "if ($this->c instanceof Local) { f(); } $this->c->m();",
        "if ($this->d instanceof Local) { $this->c->m(); }",
        "if ($a->c instanceof Local) { $this->c->m(); }",
        "if ($c instanceof Local) { $this->c->m(); }",
    ] {
        assert_eq!(on_property(body).as_deref(), FAR, "{body}");
    }
}

#[test]
fn a_write_to_the_property_ends_the_test() {
    for body in [
        "if ($this->c instanceof Local) { $this->c = $a; $this->c->m(); }",
        "if ($this->c instanceof Local) { unset($this->c); $this->c->m(); }",
        "if (!$this->c instanceof Local) { return; } [$this->c] = $a; $this->c->m();",
    ] {
        assert_eq!(on_property(body).as_deref(), FAR, "{body}");
    }

    let body = "if ($this->c instanceof Local) { $this->c->p = 1; $this->c->m(); }";
    assert_eq!(on_property(body).as_deref(), LOCAL);
}

#[test]
fn a_closure_and_an_arrow_function_read_the_property_when_they_run() {
    for body in [
        "if ($this->c instanceof Local) { $f = function () { $this->c->m(); }; }",
        "if ($this->c instanceof Local) { $f = fn () => $this->c->m(); }",
    ] {
        assert_eq!(on_property(body).as_deref(), FAR, "{body}");
    }
}

#[test]
fn a_property_with_no_class_takes_the_test_s_alone() {
    let source = b"<?php
namespace App;
class Suite {
    public function run() {
        if ($this->c instanceof Local) { $this->c->m(); }
        $this->c->n();
    }
}
";
    let raw = Php::new().extract(source);
    let receiver = |name: &str| {
        raw.calls
            .iter()
            .find(|call| call.callee == name)
            .and_then(|call| call.receiver.as_deref())
    };
    assert_eq!(receiver("m"), LOCAL);
    assert_eq!(receiver("n"), UNNAMED);
}
