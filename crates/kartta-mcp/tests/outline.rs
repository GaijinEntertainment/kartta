//! `file_outline` over one file of each language that declares a definition inside another without an owner, its
//! answer pinned in full.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test), so the client's helpers fail the test by a failed expectation"
)]

#[expect(
    dead_code,
    reason = "the helpers serve every test crate, and this one calls some of them"
)]
mod common;

use std::fmt::Write;

use serde_json::json;

use common::{Client, tree};

fn outline(file: &str, source: &str) -> String {
    let tree = tree(&[(file, source)]);
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());
    client.answer("file_outline", &json!({"file": file}))
}

fn lines(lines: &[&str]) -> String {
    lines.join("\n")
}

#[test]
fn a_typescript_local_and_inline_type_member_name_the_definition_that_holds_them() {
    let answer = outline(
        "kartta.ts",
        "export function open(root: string, {signal}: {signal?: AbortSignal} = {}): number {\n\
         \x20 const opened = root.length;\n\
         \x20 const parsed = JSON.parse(root) as {ok: number};\n\
         \x20 return opened + parsed.ok;\n\
         }\n\
         export class Kartta {\n\
         \x20 close(): void {\n\
         \x20   const handle = 1;\n\
         \x20 }\n\
         }\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="kartta.ts" symbols="8">"#,
            concat!(
                r#"<symbol name="open" kind="function" lines="1-5" "#,
                r#"sig="function open(root: string, {signal}: {signal?: AbortSignal} = {}): number"/>"#,
            ),
            r#"<symbol name="signal" in="open" kind="field" lines="1" sig="signal?: AbortSignal"/>"#,
            r#"<symbol name="opened" in="open" kind="constant" lines="2"/>"#,
            r#"<symbol name="parsed" in="open" kind="constant" lines="3"/>"#,
            r#"<symbol name="ok" in="open" kind="field" lines="3" sig="ok: number"/>"#,
            r#"<symbol name="Kartta" kind="class" lines="6-10"/>"#,
            concat!(
                r#"<symbol name="close" qn="Kartta.close" owner="Kartta" kind="method" lines="7-9" "#,
                r#"sig="close(): void"/>"#,
            ),
            r#"<symbol name="handle" in="Kartta.close" kind="constant" lines="8"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn an_outline_lists_100_definitions_by_default() {
    let mut source = String::new();
    for at in 0..101 {
        writeln!(source, "function f{at:03}() {{}}").expect("a string takes the text");
    }
    let answer = outline("many.js", &source);

    assert!(
        answer.starts_with(r#"<outline of="many.js" symbols="101" shown="100">"#),
        "{answer}"
    );
    assert_eq!(answer.matches("<symbol ").count(), 100, "{answer}");
    assert!(answer.contains(r#"name="f099""#), "{answer}");
    assert!(!answer.contains(r#"name="f100""#), "{answer}");
}

#[test]
fn an_outline_lists_its_first_definitions_up_to_the_limit_and_counts_every_one() {
    let tree = tree(&[(
        "kartta.ts",
        "export function open(root: string): number {\n\
         \x20 const opened = root.length;\n\
         \x20 return opened;\n\
         }\n\
         export class Kartta {}\n",
    )]);
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());
    let cut = client.answer("file_outline", &json!({"file": "kartta.ts", "limit": 2}));
    let whole = client.answer("file_outline", &json!({"file": "kartta.ts", "limit": 3}));

    assert_eq!(
        cut,
        lines(&[
            r#"<outline of="kartta.ts" symbols="3" shown="2">"#,
            r#"<symbol name="open" kind="function" lines="1-4" sig="function open(root: string): number"/>"#,
            r#"<symbol name="opened" in="open" kind="constant" lines="2"/>"#,
            "the limit cut this list - a higher limit lists the rest",
            "</outline>",
        ])
    );
    assert_eq!(
        whole,
        lines(&[
            r#"<outline of="kartta.ts" symbols="3">"#,
            r#"<symbol name="open" kind="function" lines="1-4" sig="function open(root: string): number"/>"#,
            r#"<symbol name="opened" in="open" kind="constant" lines="2"/>"#,
            r#"<symbol name="Kartta" kind="class" lines="5"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_definition_on_the_line_of_a_callable_names_it_only_as_a_member_of_its_signature() {
    let answer = outline(
        "pick.ts",
        "function pick(item: {id: string}) { return item.id; } const fallback = 'x';\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="pick.ts" symbols="3">"#,
            r#"<symbol name="pick" kind="function" lines="1" sig="function pick(item: {id: string})"/>"#,
            r#"<symbol name="id" in="pick" kind="field" lines="1" sig="id: string"/>"#,
            r#"<symbol name="fallback" kind="constant" lines="1"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_javascript_local_function_and_object_literal_method_name_the_definition_that_holds_them() {
    let answer = outline(
        "api.js",
        "function load() {\n  const parse = () => 1;\n  return parse();\n}\nconst api = {\n  get() {\n  },\n};\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="api.js" symbols="4">"#,
            r#"<symbol name="load" kind="function" lines="1-4" sig="function load()"/>"#,
            r#"<symbol name="parse" in="load" kind="function" lines="2" sig="parse = () =>"/>"#,
            r#"<symbol name="api" kind="constant" lines="5-8"/>"#,
            r#"<symbol name="get" in="api" kind="function" lines="6-7" sig="get()"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_python_nested_function_names_the_function_that_holds_it_and_an_attribute_keeps_its_owner() {
    let answer = outline(
        "deco.py",
        "def outer():\n    def inner():\n        pass\n    return inner\n\n\nclass Service:\n    def greet(self):\n\
         \x20       self.ready = True\n        def helper():\n            pass\n        return helper\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="deco.py" symbols="6">"#,
            r#"<symbol name="outer" qn="deco.outer" kind="function" lines="1-4" sig="def outer():"/>"#,
            r#"<symbol name="inner" qn="deco.inner" in="deco.outer" kind="function" lines="2-3" sig="def inner():"/>"#,
            r#"<symbol name="Service" qn="deco.Service" kind="class" lines="7-12"/>"#,
            concat!(
                r#"<symbol name="greet" qn="deco.Service.greet" owner="Service" kind="method" lines="8-12" "#,
                r#"sig="def greet(self):"/>"#,
            ),
            r#"<symbol name="ready" qn="deco.Service.ready" owner="Service" kind="field" lines="9"/>"#,
            concat!(
                r#"<symbol name="helper" qn="deco.helper" in="deco.Service.greet" kind="function" lines="10-11" "#,
                r#"sig="def helper():"/>"#,
            ),
            "</outline>",
        ])
    );
}

#[test]
fn a_go_local_names_the_function_that_holds_it() {
    let answer = outline(
        "count.go",
        "package p\n\nfunc Count() int {\n\tvar total int\n\treturn total\n}\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="count.go" symbols="2">"#,
            r#"<symbol name="Count" qn="p.Count" kind="function" lines="3-6" sig="func Count() int"/>"#,
            r#"<symbol name="total" qn="p.total" in="p.Count" kind="variable" lines="4"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_rust_item_of_a_body_and_of_an_inline_module_name_what_holds_them() {
    let answer = outline(
        "lib.rs",
        "pub fn top() {\n    fn inner() {}\n    inner();\n}\n\nmod tests {\n    fn case() {}\n}\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="lib.rs" symbols="4">"#,
            r#"<symbol name="top" kind="function" lines="1-4" sig="pub fn top()"/>"#,
            r#"<symbol name="inner" in="top" kind="function" lines="2" sig="fn inner()"/>"#,
            r#"<symbol name="tests" kind="namespace" lines="6-8"/>"#,
            r#"<symbol name="case" qn="tests::case" in="tests" kind="function" lines="7" sig="fn case()"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_lua_local_function_names_the_function_that_holds_it() {
    let answer = outline(
        "ui.lua",
        "local function top()\n  local function inner() end\n  return inner\nend\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="ui.lua" symbols="2">"#,
            r#"<symbol name="top" kind="function" lines="1-4" sig="local function top()"/>"#,
            r#"<symbol name="inner" in="top" kind="function" lines="2" sig="local function inner() end"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_quirrel_local_function_names_the_function_that_holds_it() {
    let answer = outline(
        "ui.nut",
        "function top() {\n  local function inner() {}\n  return inner\n}\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="ui.nut" symbols="2">"#,
            r#"<symbol name="top" kind="function" lines="1-4" sig="function top()"/>"#,
            r#"<symbol name="inner" in="top" kind="function" lines="2" sig="local function inner()"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_php_function_declared_in_a_function_names_the_function_that_holds_it() {
    let answer = outline(
        "boot.php",
        "<?php\n\nfunction boot() {\n    function helper() {}\n}\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="boot.php" symbols="2">"#,
            r#"<symbol name="boot" kind="function" lines="3-5" sig="function boot()"/>"#,
            r#"<symbol name="helper" in="boot" kind="function" lines="4" sig="function helper()"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_ruby_method_defined_in_a_method_names_the_method_that_holds_it() {
    let answer = outline(
        "service.rb",
        "class Service\n  def setup\n    def ready\n    end\n  end\nend\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="service.rb" symbols="3">"#,
            r#"<symbol name="Service" kind="class" lines="1-6"/>"#,
            r#"<symbol name="setup" qn="Service#setup" owner="Service" kind="method" lines="2-5" sig="def setup"/>"#,
            r#"<symbol name="ready" in="Service#setup" kind="function" lines="3-4" sig="def ready end"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_cpp_local_type_names_the_function_that_holds_it_and_a_typedef_struct_names_nothing() {
    let answer = outline(
        "local.cpp",
        "int top() {\n  struct Local {\n    int y;\n  };\n  return 0;\n}\n\ntypedef struct Pair {\n  int a;\n} Pair;\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="local.cpp" symbols="5">"#,
            r#"<symbol name="top" kind="function" lines="1-6" sig="int top()"/>"#,
            r#"<symbol name="Local" in="top" kind="struct" lines="2-4"/>"#,
            r#"<symbol name="y" qn="Local::y" owner="Local" kind="field" lines="3" sig="int y"/>"#,
            r#"<symbol name="Pair" kind="struct" lines="8-10"/>"#,
            r#"<symbol name="a" qn="Pair::a" owner="Pair" kind="field" lines="9" sig="int a"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_bash_function_defined_in_a_function_names_the_function_that_holds_it() {
    let answer = outline(
        "run.sh",
        "#!/bin/bash\nmain() {\n  helper() {\n    echo\n  }\n}\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="run.sh" symbols="2">"#,
            r#"<symbol name="main" kind="function" lines="2-6"/>"#,
            r#"<symbol name="helper" in="main" kind="function" lines="3-5"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn a_puppet_class_declared_in_a_class_names_the_class_that_holds_it() {
    let answer = outline("init.pp", "class outer {\n  class inner {\n  }\n}\n");

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="init.pp" symbols="2">"#,
            r#"<symbol name="outer" kind="class" lines="1-4"/>"#,
            r#"<symbol name="inner" in="outer" kind="class" lines="2-3"/>"#,
            "</outline>",
        ])
    );
}

#[test]
fn definitions_sharing_a_declaration_name_nothing_as_holding_each_other() {
    let answer = outline(
        "shared.go",
        "package p\n\nvar (\n\tfirst,\n\tsecond int\n)\n\nfunc Pair() {\n\tvar a, b int\n\t_, _ = a, b\n}\n",
    );

    assert_eq!(
        answer,
        lines(&[
            r#"<outline of="shared.go" symbols="5">"#,
            r#"<symbol name="first" qn="p.first" kind="variable" lines="4-5"/>"#,
            r#"<symbol name="second" qn="p.second" kind="variable" lines="4-5"/>"#,
            r#"<symbol name="Pair" qn="p.Pair" kind="function" lines="8-11" sig="func Pair()"/>"#,
            r#"<symbol name="a" qn="p.a" in="p.Pair" kind="variable" lines="9"/>"#,
            r#"<symbol name="b" qn="p.b" in="p.Pair" kind="variable" lines="9"/>"#,
            "</outline>",
        ])
    );
}
