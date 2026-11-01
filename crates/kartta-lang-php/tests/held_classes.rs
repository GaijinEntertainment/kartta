//! What class a PHP call on a variable records as its receiver: the one a `new` last assigned the variable, where the
//! file alone settles that the variable still holds it.

use kartta_lang::Language;
use kartta_lang_php::Php;

const LOCAL: Option<&str> = Some("\\App\\Local");
const FAR: Option<&str> = Some("\\App\\Far");
const UNTYPED: Option<&str> = Some("$v");

/// The receiver of the call of `name` in `body`, which runs in a method of `Suite` in the namespace `App`.
fn receiver(body: &str, name: &str) -> Option<String> {
    let source = format!(
        "<?php\nnamespace App;\nuse Other\\Client;\nuse Other\\Pool as Clients;\nclass Suite extends Base {{\n    \
         public function run($a) {{\n{body}\n    }}\n}}\n"
    );
    let raw = Php::new().extract(source.as_bytes());
    raw.calls
        .into_iter()
        .find(|call| call.callee == name)
        .and_then(|call| call.receiver)
}

#[test]
fn a_class_is_its_name_from_the_root_as_php_compiles_it() {
    for (creation, class) in [
        ("new Local", "\\App\\Local"),
        ("new \\App\\Local", "\\App\\Local"),
        ("new namespace\\Local", "\\App\\Local"),
        ("new Client", "\\Other\\Client"),
        ("new client()", "\\Other\\Client"),
        ("new Clients", "\\Other\\Pool"),
        ("new Client\\Part", "\\Other\\Client\\Part"),
        ("new \\Exception", "\\Exception"),
        ("new Sub\\Local", "\\App\\Sub\\Local"),
    ] {
        assert_eq!(
            receiver(&format!("$v = {creation}; $v->m();"), "m").as_deref(),
            Some(class),
            "{creation}",
        );
    }
}

#[test]
fn a_scope_keyword_names_the_class_as_a_static_call_does() {
    for (creation, keyword) in [
        ("new self", "self"),
        ("new static", "static"),
        ("new parent", "parent"),
        ("new STATIC()", "static"),
    ] {
        assert_eq!(
            receiver(&format!("$v = {creation}; $v->m();"), "m").as_deref(),
            Some(keyword),
            "{creation}",
        );
    }
}

#[test]
fn a_static_call_and_a_nullsafe_call_on_the_variable_read_the_class() {
    let body = "$v = new Local; $v::make(); $v?->m();";
    assert_eq!(receiver(body, "make").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);
}

#[test]
fn a_new_read_through_a_variable_or_kept_in_a_value_names_no_class() {
    for body in [
        "$v = new $a; $v->m();",
        "$v = new ($a); $v->m();",
        "$v = $a ? new Local : null; $v->m();",
        "$v = $a ?? new Local; $v->m();",
        "$v = (new Local)->other(); $v->m();",
        "$v->m(); $v = new Local;",
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
    }
}

#[test]
fn every_write_to_the_variable_ends_the_class() {
    for write in [
        "$v = $a;",
        "$v = null;",
        "$v = new $a;",
        "$v .= 'x';",
        "$v ??= new Other;",
        "$v = &$a;",
        "$a = &$v;",
        "[$v, $b] = $a;",
        "list($b, $v) = $a;",
        "['k' => $v] = $a;",
        "[[$v]] = $a;",
        "[&$v] = $a;",
        "unset($v);",
        "unset($a, $v);",
        "foreach ($a as $v) {}",
        "foreach ($a as $v => $b) {}",
        "foreach ($a as $b => $v) {}",
        "foreach ($a as &$v) {}",
        "foreach ($a as [$v, $b]) {}",
        "try { f(); } catch (E $v) {}",
        "$f = function () use (&$v) {};",
    ] {
        let body = format!("$v = new Local; {write} $v->m();");
        assert_eq!(receiver(&body, "m").as_deref(), UNTYPED, "{write}");
    }
}

#[test]
fn a_write_to_something_the_variable_reaches_keeps_the_class() {
    for write in [
        "$v->p = 1;",
        "$v['k'] = 1;",
        "[$v->p] = $a;",
        "$b = $v;",
        "$b .= $v;",
    ] {
        let body = format!("$v = new Local; {write} $v->m();");
        assert_eq!(receiver(&body, "m").as_deref(), LOCAL, "{write}");
    }
}

#[test]
fn the_value_an_assignment_reads_is_the_one_before_it() {
    let body = "$v = new Local; $v = $v->with(); $v->m();";
    assert_eq!(receiver(body, "with").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);

    let body = "$v = new Local; $v = new Far($v->with()); $v->m();";
    assert_eq!(receiver(body, "with").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), FAR);
}

#[test]
fn a_class_assigned_in_a_branch_ends_with_the_branch() {
    for (body, inside) in [
        ("if ($a) { $v = new Local; $v->in(); } $v->m();", true),
        ("if ($a) $v = new Local; $v->m();", false),
        ("if ($a): $v = new Local; $v->in(); endif; $v->m();", true),
        (
            "if ($a) { f(); } else { $v = new Local; $v->in(); } $v->m();",
            true,
        ),
        (
            "if ($a) { f(); } elseif ($a) { $v = new Local; $v->in(); } $v->m();",
            true,
        ),
        ("while ($a) { $v = new Local; $v->in(); } $v->m();", true),
        (
            "do { $v = new Local; $v->in(); } while ($a); $v->m();",
            true,
        ),
        ("for (;;) { $v = new Local; $v->in(); } $v->m();", true),
        (
            "foreach ($a as $b) { $v = new Local; $v->in(); } $v->m();",
            true,
        ),
        (
            "switch ($a) { case 1: $v = new Local; $v->in(); break; } $v->m();",
            true,
        ),
        (
            "switch ($a) { default: $v = new Local; $v->in(); } $v->m();",
            true,
        ),
        (
            "try { $v = new Local; $v->in(); } catch (E $e) { f(); } $v->m();",
            true,
        ),
        (
            "try { f(); } catch (E $e) { $v = new Local; $v->in(); } $v->m();",
            true,
        ),
        (
            "try { f(); } finally { $v = new Local; $v->in(); } $v->m();",
            true,
        ),
        ("$a ? ($v = new Local) : f(); $v->m();", false),
        ("$a && $v = new Local; $v->m();", false),
        ("$a && ($v = new Local) && $v->m();", false),
        (
            "$b = match ($a) { 1 => $v = new Local, default => f() }; $v->m();",
            false,
        ),
    ] {
        assert_eq!(receiver(body, "m").as_deref(), UNTYPED, "{body}");
        if inside {
            assert_eq!(receiver(body, "in").as_deref(), LOCAL, "{body}");
        }
    }
}

#[test]
fn one_branch_s_class_stops_at_the_next_branch() {
    let body =
        "if ($a) { $v = new Local; } else { $v->one(); $v = new Far; $v->two(); } $v->three();";
    assert_eq!(receiver(body, "one").as_deref(), UNTYPED);
    assert_eq!(receiver(body, "two").as_deref(), FAR);
    assert_eq!(receiver(body, "three").as_deref(), UNTYPED);

    let body = "switch ($a) { case 1: $v = new Local; case 2: $v->m(); }";
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);
}

#[test]
fn a_write_in_a_branch_ends_the_class_past_the_branch() {
    let body = "$v = new Local; if ($a) { $v = $a; } $v->m();";
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);
}

#[test]
fn a_class_assigned_before_a_branch_stands_in_it() {
    let body = "$v = new Local; if ($a) { $v->in(); } $v->m();";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);
}

#[test]
fn a_closure_s_use_copies_the_class_it_finds() {
    for closure in [
        "$f = function () use ($v) { $v->m(); };",
        "g(function () use ($v) { $v->m(); });",
    ] {
        let body = format!("$v = new Local; {closure}");
        assert_eq!(receiver(&body, "m").as_deref(), LOCAL, "{closure}");
    }

    let body = "$v = new Local; $f = function () use ($v) { $v = $a; $v->in(); }; $v->m();";
    assert_eq!(receiver(body, "in").as_deref(), UNTYPED);
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);

    let body = "$v = new Local; $v = $a; $f = function () use ($v) { $v->m(); };";
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);
}

#[test]
fn a_closure_reads_no_variable_of_the_function_around_it_past_its_use() {
    for closure in [
        "g(function () { $v->m(); });",
        "g(function ($v) { $v->m(); });",
        "$f = function () { $v->m(); };",
        "$f = function () use (&$v) { $v->m(); };",
        "g(fn ($v) => $v->m());",
    ] {
        let body = format!("$v = new Local; {closure}");
        assert_eq!(receiver(&body, "m").as_deref(), UNTYPED, "{closure}");
    }
}

#[test]
fn an_arrow_function_copies_the_classes_around_it() {
    let body = "$v = new Local; g(fn ($b) => $v->m());";
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);
}

#[test]
fn a_closure_s_writes_stay_in_the_closure() {
    let body = "$v = new Local; g(function () { $v = 1; }); g(fn () => $v = 1); $v->m();";
    assert_eq!(receiver(body, "m").as_deref(), LOCAL);
}

#[test]
fn a_class_assigned_in_a_closure_ends_with_it() {
    let body = "g(function () { $v = new Local; $v->in(); }); $v->m();";
    assert_eq!(receiver(body, "in").as_deref(), LOCAL);
    assert_eq!(receiver(body, "m").as_deref(), UNTYPED);
}

#[test]
fn a_variable_of_another_function_is_another_variable() {
    let source = b"<?php
class Suite {
    public function one() { $v = new Local; }
    public function two() { $v->m(); }
    public function three() { $v = new Local; $o = new class { public function in() { $v->n(); } }; }
}
function free() { $v->f(); }
$v = new Local;
function late() { $v->l(); }
$v->top();
";
    let raw = Php::new().extract(source);
    let receiver = |name: &str| {
        raw.calls
            .iter()
            .find(|call| call.callee == name)
            .and_then(|call| call.receiver.as_deref())
    };
    for name in ["m", "n", "f", "l"] {
        assert_eq!(receiver(name), UNTYPED, "{name}");
    }
    assert_eq!(receiver("top"), Some("\\Local"));
}
