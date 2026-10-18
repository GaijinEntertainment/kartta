//! The graph tools over a small Go module, their answers pinned in full.
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

use serde_json::{Value, json};

use common::{Client, go_module, rooted, tree};

const NOT_HELD: &str = "not in a code graph - an unrecognized extension, a language not indexed, or a path the \
                        index does not hold: pass a repo-relative path as printed by find_files";

fn answers(calls: &[(&str, Value)]) -> Vec<String> {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());
    calls
        .iter()
        .map(|(tool, arguments)| client.answer(tool, arguments))
        .collect()
}

fn lines(lines: &[&str]) -> String {
    lines.join("\n")
}

#[test]
fn find_symbols_ranks_the_named_symbol_first_and_counts_what_the_limit_cut() {
    let answers = answers(&[
        ("find_symbols", json!({"query": "Greet"})),
        ("find_symbols", json!({"query": "service", "limit": 1})),
        ("find_symbols", json!({"query": "nothing here"})),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<symbols of="Greet" count="1">"#,
                concat!(
                    r#"<hit name="Greet" qn="pkg.Service.Greet" owner="Service" kind="method" lang="go" "#,
                    r#"loc="pkg/service.go:12-14" sig="func (s *Service) Greet(name string) string"/>"#,
                ),
                "</symbols>",
            ]),
            lines(&[
                r#"<symbols of="service" count="1" total="3" truncated="true">"#,
                r#"<hit name="Service" qn="pkg.Service" kind="struct" lang="go" loc="pkg/service.go:4"/>"#,
                "</symbols>",
            ]),
            lines(&[concat!(
                r#"<symbols of="nothing here" count="0" unmatched="nothing, here">no match - if the language is "#,
                "absent from the index, index_stats lists what is indexed</symbols>",
            )]),
        ]
    );
}

#[test]
fn read_definitions_answers_each_body_with_its_doc_comment() {
    let answers = answers(&[
        (
            "read_definitions",
            json!({"names": ["Greet", "Missing"], "from": "pkg/service.go"}),
        ),
        ("read_definitions", json!({"names": ["New"], "limit": 1})),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<definitions count="2">"#,
                r#"<name of="Greet" total="1">"#,
                concat!(
                    r#"<definition name="Greet" qn="pkg.Service.Greet" owner="Service" kind="method" lang="go" "#,
                    r#"loc="pkg/service.go:12-14" scope="same-directory">"#,
                ),
                "    11\t// Greet greets name.",
                "    12\tfunc (s *Service) Greet(name string) string {",
                "    13\t\treturn \"hi \" + name",
                "    14\t}",
                "</definition>",
                "</name>",
                r#"<name of="Missing" total="0">no definition matches - find_symbols can locate the name</name>"#,
                "</definitions>",
            ]),
            lines(&[
                r#"<definitions count="1">"#,
                r#"<name of="New" total="1">"#,
                concat!(
                    r#"<definition name="New" qn="pkg.New" kind="function" lang="go" loc="pkg/service.go:7-9" "#,
                    r#"scope="other">"#,
                ),
                "     6\t// New builds a Service.",
                "     7\tfunc New() *Service {",
                "     8\t\treturn &Service{}",
                "     9\t}",
                "</definition>",
                "</name>",
                "</definitions>",
            ]),
        ]
    );
}

#[test]
fn file_outline_find_importers_and_find_implementations_answer_for_the_module() {
    let answers = answers(&[
        ("file_outline", json!({"file": "pkg/service.go"})),
        ("file_outline", json!({"file": "README.md"})),
        ("find_importers", json!({"file": "pkg/service.go"})),
        ("find_importers", json!({"file": "README.md"})),
        ("find_implementations", json!({"name": "Service"})),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<outline of="pkg/service.go" symbols="3">"#,
                r#"<symbol name="Service" qn="pkg.Service" kind="struct" lines="4-4"/>"#,
                r#"<symbol name="New" qn="pkg.New" kind="function" lines="7-9" sig="func New() *Service"/>"#,
                concat!(
                    r#"<symbol name="Greet" qn="pkg.Service.Greet" owner="Service" kind="method" lines="12-14" "#,
                    r#"sig="func (s *Service) Greet(name string) string"/>"#,
                ),
                "</outline>",
            ]),
            format!(r#"<outline of="README.md" symbols="0">{NOT_HELD}, or read or grep the file</outline>"#),
            lines(&[
                r#"<importers of="pkg/service.go" count="1">"#,
                r#"<file path="cmd/app/main.go"/>"#,
                "</importers>",
            ]),
            concat!(
                r#"<importers of="README.md" count="0">not in a code graph (an unrecognized extension, a "#,
                "language not indexed, or a path the index does not hold) - pass a repo-relative path as printed by \
                 find_files</importers>",
            )
            .to_owned(),
            concat!(
                r#"<implementations of="Service" count="0">the symbol is indexed but nothing implements or "#,
                "extends it</implementations>",
            )
            .to_owned(),
        ]
    );
}

#[test]
fn find_symbols_tops_up_with_fuzzy_hits_only_when_every_graph_underfills_the_limit() {
    let tree = go_module();
    std::fs::write(tree.path().join("x.py"), "def sxerxvice():\n    pass\n")
        .expect("the file is written");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_symbols", &json!({"query": "service", "limit": 1})),
        lines(&[
            r#"<symbols of="service" count="1" total="3" truncated="true">"#,
            r#"<hit name="Service" qn="pkg.Service" kind="struct" lang="go" loc="pkg/service.go:4"/>"#,
            "</symbols>",
        ])
    );
    let topped_up = client.answer("find_symbols", &json!({"query": "service", "limit": 5}));
    assert!(
        topped_up
            .lines()
            .any(|line| line.contains(r#"name="sxerxvice""#) && line.contains(r#"fuzzy="true""#)),
        "{topped_up}"
    );
}

#[test]
fn find_implementations_lists_at_most_500_across_every_graph() {
    let mut python = String::from("class Base:\n    pass\n");
    let mut typescript = String::from("class Base {}\n");
    for at in 0..300 {
        let _ = writeln!(python, "class P{at}(Base):\n    pass");
        let _ = writeln!(typescript, "class T{at} extends Base {{}}");
    }
    let tree = tree(&[("base.py", &python), ("base.ts", &typescript)]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_implementations", &json!({"name": "Base"}));

    assert_eq!(
        answer.lines().next(),
        Some(r#"<implementations of="Base" count="500" total="600">"#)
    );
    assert_eq!(
        answer
            .lines()
            .filter(|line| line.starts_with("<type "))
            .count(),
        500
    );
}

#[test]
fn find_importers_counts_the_importers_it_did_not_follow_past_three_hops() {
    let chain = tree(&[
        ("go.mod", "module example.com/chain\n\ngo 1.22\n"),
        ("a/a.go", "package a\n\nfunc A() {}\n"),
        (
            "b/b.go",
            "package b\n\nimport \"example.com/chain/a\"\n\nfunc B() { a.A() }\n",
        ),
        (
            "c/c.go",
            "package c\n\nimport \"example.com/chain/b\"\n\nfunc C() { b.B() }\n",
        ),
        (
            "d/d.go",
            "package d\n\nimport \"example.com/chain/c\"\n\nfunc D() { c.C() }\n",
        ),
        (
            "e/e.go",
            "package e\n\nimport \"example.com/chain/d\"\n\nfunc E() { d.D() }\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(chain.path());

    assert_eq!(
        client.answer("find_importers", &json!({"file": "a/a.go"})),
        lines(&[
            r#"<importers of="a/a.go" count="3" unexpanded="1">"#,
            r#"<file path="b/b.go"/>"#,
            r#"<file path="c/c.go" depth="2"/>"#,
            r#"<file path="d/d.go" depth="3"/>"#,
            "</importers>",
        ])
    );
}

#[test]
fn find_call_path_and_find_references_follow_the_calls() {
    let answers = answers(&[
        ("find_call_path", json!({"from": "main", "to": "Greet"})),
        (
            "find_call_path",
            json!({"from": "main", "to": "Greet", "hops": 1}),
        ),
        ("find_references", json!({"name": "Service"})),
        (
            "find_references",
            json!({"name": "Greet", "min_confidence": "certain"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<call_path from="main" to="Greet" matches="1">"#,
                r#"<match lang="go" from="main.main" to="pkg.Service.Greet" hops="2" max_hops="8">"#,
                r#"<ref name="main" qn="main.main" loc="cmd/app/main.go:5" role="entry" call="cmd/app/main.go:7"/>"#,
                concat!(
                    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10" role="consumer" "#,
                    r#"call="cmd/app/main.go:11"/>"#,
                ),
                "</match>",
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="main" to="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="main.main" to="pkg.Service.Greet" hops="0" max_hops="1" "#,
                    r#"at_depth_bound="1" found="false">no call chain within 1 calls - the graph misses dynamic "#,
                    "and unresolved calls, so this does not show that the code holds none</match>",
                ),
                "</call_path>",
            ]),
            lines(&[
                r#"<references of="Service" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Service" qn="pkg.Service" loc="pkg/service.go:4" kind="struct" "#,
                    r#"count="4" types="4">"#,
                ),
                concat!(
                    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10" role="consumer" site="type" "#,
                    r#"at="cmd/app/main.go:10"/>"#,
                ),
                concat!(
                    r#"<ref name="New" qn="pkg.New" loc="pkg/service.go:7" role="consumer" site="type" "#,
                    r#"at="pkg/service.go:7,8"/>"#,
                ),
                concat!(
                    r#"<ref name="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12" role="consumer" "#,
                    r#"site="type" at="pkg/service.go:12"/>"#,
                ),
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12" kind="method" "#,
                    r#"count="1" calls="1">"#,
                ),
                concat!(
                    "<note>a call through a value of the owner's type resolves only where the graph can name ",
                    "that type, so some of those calls can be missing</note>",
                ),
                concat!(
                    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10" role="consumer" site="call" "#,
                    r#"at="cmd/app/main.go:11"/>"#,
                ),
                "</match>",
                "</references>",
            ]),
        ]
    );
}

#[test]
fn find_references_lists_the_reads_and_writes_of_a_field_and_keeps_the_writers_on_request() {
    let module = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "pkg/config.go",
            "package pkg\n\ntype Config struct {\n\tTimeout int\n}\n\nfunc Load() Config {\n\
             \tc := Config{Timeout: 5}\n\tc.Timeout += 1\n\treturn c\n}\n",
        ),
        (
            "cmd/main.go",
            "package main\n\nimport \"example.com/app/pkg\"\n\nfunc main() {\n\tc := pkg.Load()\n\
             \tc.Timeout = 3\n\tprintln(c.Timeout)\n\tprintln(c.Timeout)\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(module.path());

    let every = client.answer("find_references", &json!({"name": "pkg.Config.Timeout"}));
    let writers = client.answer(
        "find_references",
        &json!({"name": "pkg.Config.Timeout", "access": "write"}),
    );
    let callers = client.answer("find_callers", &json!({"name": "pkg.Config.Timeout"}));

    let open = concat!(
        r#"<match lang="go" of="Timeout" qn="pkg.Config.Timeout" loc="pkg/config.go:4" kind="field" "#,
        r#"count="{count}" reads="2" writes="2" updates="1">"#,
    );
    assert_eq!(
        every,
        lines(&[
            r#"<references of="pkg.Config.Timeout" matches="1">"#,
            &open.replace("{count}", "5"),
            r#"<ref name="main" qn="main.main" loc="cmd/main.go:5" role="entry" site="write" at="cmd/main.go:7"/>"#,
            r#"<ref name="main" qn="main.main" loc="cmd/main.go:5" role="entry" site="read" at="cmd/main.go:8,9"/>"#,
            r#"<ref name="Load" qn="pkg.Load" loc="pkg/config.go:7" role="consumer" site="write" at="pkg/config.go:8"/>"#,
            concat!(
                r#"<ref name="Load" qn="pkg.Load" loc="pkg/config.go:7" role="consumer" site="update" "#,
                r#"at="pkg/config.go:9"/>"#,
            ),
            "</match>",
            "</references>",
        ]),
        "a site per enclosing symbol, kind and line, the lines of one kind joined"
    );
    assert_eq!(
        writers,
        lines(&[
            r#"<references of="pkg.Config.Timeout" matches="1">"#,
            &open.replace("{count}", "3"),
            r#"<ref name="main" qn="main.main" loc="cmd/main.go:5" role="entry" site="write" at="cmd/main.go:7"/>"#,
            r#"<ref name="Load" qn="pkg.Load" loc="pkg/config.go:7" role="consumer" site="write" at="pkg/config.go:8"/>"#,
            concat!(
                r#"<ref name="Load" qn="pkg.Load" loc="pkg/config.go:7" role="consumer" site="update" "#,
                r#"at="pkg/config.go:9"/>"#,
            ),
            "</match>",
            "</references>",
        ]),
        "an update sets the field too, and the counts still count every site"
    );
    assert!(
        callers.contains(
            "a field is read and written, not called; find_references lists its reads and writes"
        ),
        "{callers}"
    );
}

#[test]
fn find_references_lists_a_typescript_field_s_sites_and_says_what_an_untyped_access_leaves_out() {
    let module = tree(&[
        (
            "src/config.ts",
            "export class Config {\n  timeout = 0;\n  grow(by: number) {\n    this.timeout += by;\n  }\n}\n",
        ),
        (
            "src/main.ts",
            "import { Config } from './config';\nimport { LIMIT } from './index';\n\
             export function run(cfg: Config, raw: any) {\n  cfg.timeout = 5;\n  return cfg.timeout + raw.timeout;\n}\n\
             export const cap = () => LIMIT;\n",
        ),
        ("src/index.ts", "export { LIMIT } from './limits';\n"),
        ("src/limits.ts", "export const LIMIT = 5;\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(module.path());

    let answer = client.answer("find_references", &json!({"name": "Config.timeout"}));
    let limit = client.answer("find_references", &json!({"name": "LIMIT"}));

    assert_eq!(
        answer,
        lines(&[
            r#"<references of="Config.timeout" matches="1">"#,
            concat!(
                r#"<match lang="jsts" of="timeout" qn="Config.timeout" loc="src/config.ts:2" kind="field" "#,
                r#"count="3" reads="1" writes="1" updates="1" ambiguous="1">"#,
            ),
            concat!(
                "<note>a member access on a value whose declaration spells no one type binds nothing; ambiguous ",
                "counts those</note>",
            ),
            concat!(
                r#"<ref name="grow" qn="Config.grow" loc="src/config.ts:3" role="consumer" site="update" "#,
                r#"at="src/config.ts:4"/>"#,
            ),
            r#"<ref name="run" loc="src/main.ts:3" role="consumer" site="write" at="src/main.ts:4"/>"#,
            r#"<ref name="run" loc="src/main.ts:3" role="consumer" site="read" at="src/main.ts:5"/>"#,
            "</match>",
            "</references>",
        ]),
        "this and an annotated parameter bind the field; a member of an untyped value does not"
    );
    assert_eq!(
        limit,
        lines(&[
            r#"<references of="LIMIT" matches="1">"#,
            concat!(
                r#"<match lang="jsts" of="LIMIT" loc="src/limits.ts:1" kind="constant" count="1" "#,
                r#"reads="1">"#,
            ),
            concat!(
                r#"<ref name="cap" loc="src/main.ts:7" role="consumer" site="read" at="src/main.ts:7" "#,
                r#"confidence="likely"/>"#,
            ),
            "</match>",
            "</references>",
        ]),
        "an imported value binds through the barrel that re-exports it"
    );
}

#[test]
fn find_references_lists_a_python_attribute_s_sites_and_a_module_value_read_through_its_module() {
    let module = tree(&[
        (
            "pkg/config.py",
            "LIMIT = 3\n\n\nclass Config:\n    def __init__(self):\n        self.timeout = 0\n\n    \
             def grow(self, by):\n        self.timeout += by\n",
        ),
        (
            "main.py",
            "from pkg import config\n\n\ndef run(cfg, other):\n    cfg.timeout = 5\n    \
             return config.LIMIT + other.timeout\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(module.path());

    let field = client.answer(
        "find_references",
        &json!({"name": "pkg.config.Config.timeout"}),
    );
    let value = client.answer("find_references", &json!({"name": "pkg.config.LIMIT"}));

    assert_eq!(
        field,
        lines(&[
            r#"<references of="pkg.config.Config.timeout" matches="1">"#,
            concat!(
                r#"<match lang="python" of="timeout" qn="pkg.config.Config.timeout" loc="pkg/config.py:6" "#,
                r#"kind="field" count="2" writes="1" updates="1" ambiguous="2">"#,
            ),
            concat!(
                "<note>an attribute of a value whose class the extraction cannot name binds only where one field ",
                "of the name is in its own file; ambiguous counts the rest</note>",
            ),
            concat!(
                r#"<ref name="__init__" qn="pkg.config.Config.__init__" loc="pkg/config.py:5" role="entry" "#,
                r#"site="write" at="pkg/config.py:6"/>"#,
            ),
            concat!(
                r#"<ref name="grow" qn="pkg.config.Config.grow" loc="pkg/config.py:8" role="consumer" "#,
                r#"site="update" at="pkg/config.py:9"/>"#,
            ),
            "</match>",
            "</references>",
        ]),
        "the attribute a method assigns is a field its methods set; an attribute of a parameter does not bind"
    );
    assert_eq!(
        value,
        lines(&[
            r#"<references of="pkg.config.LIMIT" matches="1">"#,
            concat!(
                r#"<match lang="python" of="LIMIT" qn="pkg.config.LIMIT" loc="pkg/config.py:1" kind="variable" "#,
                r#"count="1" reads="1">"#,
            ),
            r#"<ref name="run" qn="main.run" loc="main.py:4" role="consumer" site="read" at="main.py:6"/>"#,
            "</match>",
            "</references>",
        ]),
        "a module the file imports names its own value"
    );
}

#[test]
fn find_references_lists_a_cpp_field_s_sites_and_says_what_an_untyped_access_leaves_out() {
    let module = tree(&[
        (
            "widget.h",
            "struct Widget {\n  int width;\n  void grow();\n};\nstruct Pane {\n  int width;\n};\n",
        ),
        (
            "widget.cpp",
            "#include \"widget.h\"\n\nvoid Widget::grow() {\n  width += 1;\n}\n\n\
             void paint(Widget &widget) {\n  widget.width = 3;\n  draw(widget.width);\n  draw(make().width);\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(module.path());

    let answer = client.answer("find_references", &json!({"name": "Widget::width"}));

    assert_eq!(
        answer,
        lines(&[
            r#"<references of="Widget::width" matches="1">"#,
            concat!(
                r#"<match lang="cpp" of="width" qn="Widget::width" loc="widget.h:2" kind="field" count="3" "#,
                r#"reads="1" writes="1" updates="1" ambiguous="1">"#,
            ),
            concat!(
                "<note>a member access on a value whose type the graph cannot name binds only where one field of the ",
                "name is in the file or the files it includes; ambiguous counts the rest</note>",
            ),
            concat!(
                r#"<ref name="grow" qn="Widget::grow" loc="widget.cpp:3" role="consumer" site="update" "#,
                r#"at="widget.cpp:4"/>"#,
            ),
            r#"<ref name="paint" loc="widget.cpp:7" role="consumer" site="write" at="widget.cpp:8"/>"#,
            r#"<ref name="paint" loc="widget.cpp:7" role="consumer" site="read" at="widget.cpp:9"/>"#,
            "</match>",
            "</references>",
        ]),
        "the own class's field and one on a declared value bind; one on a returned value two structs offer does not"
    );
}

#[test]
fn impact_of_edit_answers_the_symbols_a_range_overlaps() {
    let answers = answers(&[
        (
            "impact_of_edit",
            json!({"file": "pkg/service.go", "start_line": 12}),
        ),
        (
            "impact_of_edit",
            json!({"file": "pkg/service.go", "start_line": 1, "end_line": 2}),
        ),
        (
            "impact_of_edit",
            json!({"file": "README.md", "start_line": 1}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<impact of="pkg/service.go" start_line="12" end_line="end" symbols="1">"#,
                concat!(
                    r#"<symbol name="Greet" qn="pkg.Service.Greet" kind="method" lines="12-14" callers="1" "#,
                    r#"callees="0" reach="2" reach_files="1" reach_band="low" "#,
                    r#"sig="func (s *Service) Greet(name string) string">"#,
                ),
                r#"<callers shown="1" of="1">"#,
                r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10" role="consumer" at="cmd/app/main.go:11"/>"#,
                "</callers>",
                "</symbol>",
                r#"<includes count="0"/>"#,
                r#"<included-by count="1">"#,
                r#"<file path="cmd/app/main.go"/>"#,
                "</included-by>",
                "</impact>",
            ]),
            lines(&[
                r#"<impact of="pkg/service.go" start_line="1" end_line="2" symbols="0">"#,
                concat!(
                    "no symbols defined in this range - it may cover only file-scope code (imports, comments); ",
                    "zero symbols is not proof the edit is safe",
                ),
                r#"<includes count="0"/>"#,
                r#"<included-by count="1">"#,
                r#"<file path="cmd/app/main.go"/>"#,
                "</included-by>",
                "</impact>",
            ]),
            format!(
                r#"<impact of="README.md" start_line="1" end_line="end" symbols="0">{NOT_HELD}</impact>"#
            ),
        ]
    );
}

#[test]
fn find_callees_walks_forward_with_the_line_of_each_direct_call() {
    let answers = answers(&[
        ("find_callees", json!({"name": "main"})),
        ("find_callees", json!({"name": "main", "hops": 2})),
        ("find_callees", json!({"name": "Greet"})),
        ("find_callees", json!({"name": "Gret"})),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callees of="main" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="main" qn="main.main" loc="cmd/app/main.go:5" count="2" "#,
                    r#"unexpanded="2">"#,
                ),
                concat!(
                    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10" role="consumer" hops="1" "#,
                    r#"at="cmd/app/main.go:7"/>"#,
                ),
                concat!(
                    r#"<ref name="New" qn="pkg.New" loc="pkg/service.go:7" role="consumer" hops="1" "#,
                    r#"at="cmd/app/main.go:6"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="main" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="main" qn="main.main" loc="cmd/app/main.go:5" count="3" "#,
                    r#"unexpanded="1">"#,
                ),
                concat!(
                    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10" role="consumer" hops="1" "#,
                    r#"at="cmd/app/main.go:7"/>"#,
                ),
                concat!(
                    r#"<ref name="New" qn="pkg.New" loc="pkg/service.go:7" role="consumer" hops="1" "#,
                    r#"at="cmd/app/main.go:6"/>"#,
                ),
                r#"<ref name="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12" role="consumer" hops="2"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12" count="0">"#,
                    "no callees - no call in its body binds a definition the graph holds; a call into a library or a ",
                    "builtin, through a function value, or by dynamic dispatch binds none</match>",
                ),
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="Gret" matches="0">"#,
                "no symbol matches - did you mean:",
                r#"<suggestion name="Greet" qn="pkg.Service.Greet" kind="method" lang="go" loc="pkg/service.go:12"/>"#,
                "</callees>",
            ]),
        ]
    );
}

#[test]
fn find_callees_counts_the_calls_that_bound_nothing_and_the_callees_below_the_floor() {
    let tree = tree(&[
        ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
        (
            "run.go",
            "package lib\n\nimport \"fmt\"\n\nfunc Run() {\n\tfmt.Println(\"x\")\n\thelper()\n}\n",
        ),
        ("helper.go", "package lib\n\nfunc helper() {}\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        [
            client.answer("find_callees", &json!({"name": "Run"})),
            client.answer(
                "find_callees",
                &json!({"name": "Run", "min_confidence": "certain"})
            ),
        ],
        [
            lines(&[
                r#"<callees of="Run" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Run" qn="lib.Run" loc="run.go:5" count="1" unexpanded="1" "#,
                    r#"unresolved="1">"#,
                ),
                concat!(
                    r#"<ref name="helper" qn="lib.helper" loc="helper.go:3" role="consumer" confidence="likely" "#,
                    r#"hops="1" at="run.go:7"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="Run" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Run" qn="lib.Run" loc="run.go:5" count="0" below_confidence="1" "#,
                    r#"unresolved="1">no callees - no call in its body binds a definition the graph holds at this "#,
                    "confidence floor; a call into a library or a builtin, through a function value, or by dynamic ",
                    "dispatch binds none</match>",
                ),
                "</callees>",
            ]),
        ]
    );
}

const GREET_SEED: [&str; 5] = [
    concat!(
        r#"<seed name="Greet" qn="pkg.Service.Greet" kind="method" loc="pkg/service.go:12-14" callers="1" "#,
        r#"reach="2" reach_band="low">"#,
    ),
    r#"<callers shown="1" of="1">"#,
    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10" role="consumer" at="cmd/app/main.go:11"/>"#,
    "</callers>",
    "</seed>",
];

const MAIN_HOTSPOT: [&str; 3] = [
    r#"<hotspots shown="1" of="1">"#,
    concat!(
        r#"<hotspot name="main" qn="main.main" kind="function" loc="cmd/app/main.go:5" role="entry" "#,
        r#"reached_by="2" hops="1"/>"#,
    ),
    "</hotspots>",
];

#[test]
fn impact_of_change_lists_the_changed_symbols_and_where_their_reaches_meet() {
    let answers = answers(&[(
        "impact_of_change",
        json!({"regions": [{"file": "pkg/service.go", "start_line": 7, "end_line": 14}]}),
    )]);

    let new_seed = [
        concat!(
            r#"<seed name="New" qn="pkg.New" kind="function" loc="pkg/service.go:7-9" callers="1" "#,
            r#"reach="1" reach_band="low">"#,
        ),
        r#"<callers shown="1" of="1">"#,
        r#"<ref name="main" qn="main.main" loc="cmd/app/main.go:5" role="entry" at="cmd/app/main.go:6"/>"#,
        "</callers>",
        "</seed>",
    ];
    assert_eq!(
        answers,
        [lines(
            &[
                &[
                    concat!(
                        r#"<change regions="1" changed_symbols="2" changed_files="1" footprint="2" "#,
                        r#"footprint_files="1">"#,
                    ),
                    r#"<seeds shown="2" of="2">"#,
                ][..],
                &GREET_SEED,
                &new_seed,
                &["</seeds>"],
                &MAIN_HOTSPOT,
                &["</change>"],
            ]
            .concat()
        )]
    );
}

#[test]
fn impact_of_change_reads_a_diff_and_counts_the_regions_no_symbol_holds() {
    let diff = "\
diff --git a/pkg/service.go b/pkg/service.go
--- a/pkg/service.go
+++ b/pkg/service.go
@@ -8 +8 @@ func New() *Service {
-\treturn nil
+\treturn &Service{}
@@ -13 +13 @@ func (s *Service) Greet(name string) string {
-\treturn name
+\treturn \"hi \" + name
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-# old
+# app
";
    let answers = answers(&[
        ("impact_of_change", json!({"diff": diff, "limit": 1})),
        (
            "impact_of_change",
            json!({"regions": [{"file": "README.md"}]}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(
                &[
                    &[
                        concat!(
                            r#"<change regions="3" changed_symbols="2" changed_files="1" footprint="2" "#,
                            r#"footprint_files="1" unheld_regions="1">"#,
                        ),
                        r#"<seeds shown="1" of="2">"#,
                    ][..],
                    &GREET_SEED,
                    &["</seeds>"],
                    &MAIN_HOTSPOT,
                    &["</change>"],
                ]
                .concat()
            ),
            concat!(
                r#"<change regions="1" changed_symbols="0" changed_files="0" footprint="0" footprint_files="0" "#,
                r#"unheld_regions="1">no symbols in these regions - they cover only file-scope code (imports, "#,
                "comments) or files no code graph holds (counted in unheld_regions); zero symbols is not proof the ",
                "change is safe, and find_importers lists the files that include a changed file</change>",
            )
            .to_owned(),
        ]
    );
}

#[test]
fn find_unused_lists_what_nothing_reaches_and_counts_what_it_did_not_judge() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "main.go",
            "package main\n\nfunc main() {\n\tused()\n\tHelper()\n}\n\nfunc used() {}\n\nfunc unused() {}\n\n\
             func Helper() {}\n",
        ),
        ("pkg/pkg.go", "package pkg\n\nfunc Helper() {}\n"),
        (
            "pkg/pkg_test.go",
            "package pkg\n\nimport \"testing\"\n\nfunc TestHelper(t *testing.T) {}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        [
            client.answer("find_unused", &json!({})),
            client.answer("find_unused", &json!({"limit": 1})),
            client.answer("find_unused", &json!({"path_prefix": "pkg/"})),
            client.answer("find_unused", &json!({"path_prefix": "pkg/pkg_test.go"})),
            client.answer("find_unused", &json!({"path_prefix": "cmd/"})),
            client.answer("find_unused", &json!({"languages": []})),
        ],
        [
            lines(&[
                r#"<unused count="2" live="2">"#,
                r#"<excluded test="1" entry="1"/>"#,
                r#"<symbol name="unused" qn="main.unused" kind="function" lang="go" loc="main.go:10"/>"#,
                concat!(
                    r#"<symbol name="Helper" qn="pkg.Helper" kind="function" lang="go" loc="pkg/pkg.go:3" "#,
                    r#"name_used_elsewhere="true"/>"#,
                ),
                "</unused>",
            ]),
            lines(&[
                r#"<unused count="1" total="2" truncated="true" live="2">"#,
                r#"<excluded test="1" entry="1"/>"#,
                r#"<symbol name="unused" qn="main.unused" kind="function" lang="go" loc="main.go:10"/>"#,
                "</unused>",
            ]),
            lines(&[
                r#"<unused path_prefix="pkg/" count="1" live="0">"#,
                r#"<excluded test="1"/>"#,
                concat!(
                    r#"<symbol name="Helper" qn="pkg.Helper" kind="function" lang="go" loc="pkg/pkg.go:3" "#,
                    r#"name_used_elsewhere="true"/>"#,
                ),
                "</unused>",
            ]),
            lines(&[
                r#"<unused path_prefix="pkg/pkg_test.go" count="0" live="0">"#,
                r#"<excluded test="1"/>"#,
                "no unused definition among those judged - what was not judged is counted in <excluded>",
                "</unused>",
            ]),
            concat!(
                r#"<unused path_prefix="cmd/" count="0" live="0">no definition in a code graph lies under "#,
                "path_prefix - pass a repo-relative prefix as find_files prints paths</unused>",
            )
            .to_owned(),
            concat!(
                r#"<unused languages="[]" count="0" live="0">no language selected - an empty languages list "#,
                "searches no graph; omit languages to search every indexed language</unused>",
            )
            .to_owned(),
        ]
    );
}

#[test]
fn index_stats_counts_how_the_calls_bind_when_asked_for_coverage() {
    let tree = tree(&[
        ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
        (
            "run.go",
            "package lib\n\nimport \"fmt\"\n\nfunc Run() {\n\tfmt.Println(\"x\")\n\thelper()\n\tmissing()\n}\n",
        ),
        ("helper.go", "package lib\n\nfunc helper() {}\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let root = tree.path();
    let index = r#"<index root="<root>" files="3" gitignore="respected" watch="following" symbols="2" calls="3">"#;
    let language = r#"<language name="go" files="2" symbols="2" calls="3" uses="0" includes="0"/>"#;
    assert_eq!(
        [
            rooted(&client.answer("index_stats", &json!({})), root),
            rooted(
                &client.answer("index_stats", &json!({"coverage": true})),
                root
            ),
        ],
        [
            lines(&[index, language, "</index>"]),
            lines(&[
                index,
                language,
                concat!(
                    r#"<coverage lang="go" sites="3" bound="1" likely="1" edges="1" external_import="1" "#,
                    r#"undefined="1"/>"#,
                ),
                "</index>",
            ]),
        ]
    );
}

#[test]
fn a_wrong_call_answers_a_tool_error() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    for (tool, arguments) in [
        ("read_definitions", json!({"names": []})),
        ("read_definitions", json!({"names": [""]})),
        ("find_callees", json!({"name": "main", "hops": 0})),
        (
            "impact_of_edit",
            json!({"file": "pkg/service.go", "start_line": 0}),
        ),
        (
            "impact_of_edit",
            json!({"file": "pkg/service.go", "start_line": 5, "end_line": 2}),
        ),
        ("find_symbols", json!({"query": "x", "languages": ["nut"]})),
        ("impact_of_change", json!({})),
        (
            "impact_of_change",
            json!({"regions": [{"file": "pkg/service.go", "start_line": 5, "end_line": 2}]}),
        ),
        ("impact_of_change", json!({"diff": "not a diff"})),
        (
            "impact_of_change",
            json!({"diff": "+++ b/x\n@@ -1 +x @@\n"}),
        ),
    ] {
        assert!(client.call(tool, &arguments).is_error, "{tool} {arguments}");
    }
}
