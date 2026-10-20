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
                r#"<name of="Missing" total="0">no symbol matches - find_symbols can pin the exact name</name>"#,
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
fn read_definitions_answers_an_unknown_name_with_close_alternatives() {
    let answers = answers(&[
        ("read_definitions", json!({"names": ["Gret", "New"]})),
        (
            "read_definitions",
            json!({"names": ["Gret"], "languages": ["python"]}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<definitions count="2">"#,
                r#"<name of="Gret" total="0">"#,
                "no symbol matches - did you mean:",
                r#"<suggestion name="Greet" qn="pkg.Service.Greet" kind="method" lang="go" loc="pkg/service.go:12"/>"#,
                "</name>",
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
            lines(&[
                r#"<definitions languages="python" count="1">"#,
                concat!(
                    r#"<name of="Gret" total="0">no symbol matches - find_symbols can pin the exact "#,
                    "name</name>",
                ),
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
                    r#"count="4" files="2" types="4">"#,
                ),
                r#"<file path="pkg/service.go" sites="3">"#,
                r#"<ref name="New" qn="pkg.New" line="7" role="consumer" site="type" at="7,8"/>"#,
                r#"<ref name="Greet" qn="pkg.Service.Greet" line="12" role="consumer" site="type" at="12"/>"#,
                "</file>",
                r#"<file path="cmd/app/main.go" sites="1">"#,
                r#"<ref name="run" qn="main.run" line="10" role="consumer" site="type" at="10"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12" kind="method" "#,
                    r#"count="1" files="1" calls="1">"#,
                ),
                concat!(
                    "<note>a call through a value of the owner's type resolves only where the graph can name ",
                    "that type, so some of those calls can be missing</note>",
                ),
                r#"<file path="cmd/app/main.go" sites="1">"#,
                r#"<ref name="run" qn="main.run" line="10" role="consumer" site="call" at="11"/>"#,
                "</file>",
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
        r#"count="{count}" files="2" reads="2" writes="2" updates="1">"#,
    );
    let load = [
        r#"<ref name="Load" qn="pkg.Load" line="7" role="consumer" site="write" at="8"/>"#,
        r#"<ref name="Load" qn="pkg.Load" line="7" role="consumer" site="update" at="9"/>"#,
    ];
    assert_eq!(
        every,
        lines(&[
            r#"<references of="pkg.Config.Timeout" matches="1">"#,
            &open.replace("{count}", "5"),
            r#"<file path="cmd/main.go" sites="3">"#,
            r#"<ref name="main" qn="main.main" line="5" role="entry" site="write" at="7"/>"#,
            r#"<ref name="main" qn="main.main" line="5" role="entry" site="read" at="8,9"/>"#,
            "</file>",
            r#"<file path="pkg/config.go" sites="2">"#,
            load[0],
            load[1],
            "</file>",
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
            r#"<file path="pkg/config.go" sites="2">"#,
            load[0],
            load[1],
            "</file>",
            r#"<file path="cmd/main.go" sites="1">"#,
            r#"<ref name="main" qn="main.main" line="5" role="entry" site="write" at="7"/>"#,
            "</file>",
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
fn find_references_lists_a_hot_field_by_file_most_sites_first_and_counts_what_it_leaves_out() {
    let readers: Vec<(String, String)> = (0..130)
        .map(|file| {
            let reads = if file == 30 { 9 } else { 4 };
            let body: String = (0..reads).map(|_| "\t_ = c.Timeout\n").collect();
            (
                format!("pkg/use{file:03}.go"),
                format!("package pkg\n\nfunc use{file:03}(c Config) {{\n{body}}}\n"),
            )
        })
        .collect();
    let mut files: Vec<(&str, &str)> = vec![
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "pkg/config.go",
            "package pkg\n\ntype Config struct {\n\tTimeout int\n}\n",
        ),
    ];
    files.extend(
        readers
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str())),
    );
    let module = tree(&files);
    let mut client = Client::start(&[]);
    client.index(module.path());

    let answer = client.answer("find_references", &json!({"name": "pkg.Config.Timeout"}));

    let records: Vec<&str> = answer.lines().collect();
    assert_eq!(
        records[1],
        concat!(
            r#"<match lang="go" of="Timeout" qn="pkg.Config.Timeout" loc="pkg/config.go:4" kind="field" "#,
            r#"count="525" shown="100" files="130" files_shown="74" reads="525">"#,
        ),
        "past the 500 sites the answer once listed, the counts still cover every site: {answer}"
    );
    assert_eq!(
        records[2..5],
        [
            r#"<file path="pkg/use030.go" sites="9">"#,
            concat!(
                r#"<ref name="use030" qn="pkg.use030" line="3" role="consumer" site="read" "#,
                r#"at="4,5,6,7,8,9,10,11,12"/>"#,
            ),
            "</file>",
        ],
        "the file with the most sites comes first, its sites listed: {answer}"
    );
    assert_eq!(
        records[5..8],
        [
            r#"<file path="pkg/use000.go" sites="4">"#,
            r#"<ref name="use000" qn="pkg.use000" line="3" role="consumer" site="read" at="4,5,6,7"/>"#,
            "</file>",
        ],
        "files of one count follow in path order: {answer}"
    );
    let partial = records
        .iter()
        .position(|record| record.contains(r#"shown="3">"#))
        .expect("the file where the listed sites run out says how many it lists");
    assert_eq!(
        records[partial..partial + 4],
        [
            r#"<file path="pkg/use022.go" sites="4" shown="3">"#,
            r#"<ref name="use022" qn="pkg.use022" line="3" role="consumer" site="read" at="4,5,6"/>"#,
            "</file>",
            r#"<file path="pkg/use023.go" sites="4"/>"#,
        ],
        "past the listed sites, a file carries its count alone: {answer}"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.starts_with("<file "))
            .count(),
        74,
        "the files holding the listed sites and 50 more are named: {answer}"
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
                r#"count="3" files="2" reads="1" writes="1" updates="1" ambiguous="1">"#,
            ),
            concat!(
                "<note>a member access on a value whose declaration spells no one type binds nothing; ambiguous ",
                "counts those</note>",
            ),
            r#"<file path="src/main.ts" sites="2">"#,
            r#"<ref name="run" line="3" role="consumer" site="write" at="4"/>"#,
            r#"<ref name="run" line="3" role="consumer" site="read" at="5"/>"#,
            "</file>",
            r#"<file path="src/config.ts" sites="1">"#,
            r#"<ref name="grow" qn="Config.grow" line="3" role="consumer" site="update" at="4"/>"#,
            "</file>",
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
                r#"<match lang="jsts" of="LIMIT" loc="src/limits.ts:1" kind="constant" count="1" files="1" "#,
                r#"reads="1">"#,
            ),
            r#"<file path="src/main.ts" sites="1">"#,
            r#"<ref name="cap" line="7" role="consumer" site="read" at="7" confidence="likely"/>"#,
            "</file>",
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
                r#"kind="field" count="2" files="1" writes="1" updates="1" ambiguous="2">"#,
            ),
            concat!(
                "<note>an attribute of a value whose class the extraction cannot name binds only where one field ",
                "of the name is in its own file; ambiguous counts the rest</note>",
            ),
            r#"<file path="pkg/config.py" sites="2">"#,
            r#"<ref name="__init__" qn="pkg.config.Config.__init__" line="5" role="entry" site="write" at="6"/>"#,
            r#"<ref name="grow" qn="pkg.config.Config.grow" line="8" role="consumer" site="update" at="9"/>"#,
            "</file>",
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
                r#"count="1" files="1" reads="1">"#,
            ),
            r#"<file path="main.py" sites="1">"#,
            r#"<ref name="run" qn="main.run" line="4" role="consumer" site="read" at="6"/>"#,
            "</file>",
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
                r#"files="1" reads="1" writes="1" updates="1" ambiguous="1">"#,
            ),
            concat!(
                "<note>a member access on a value whose type the graph cannot name binds only where one field of the ",
                "name is in the file or the files it includes; ambiguous counts the rest</note>",
            ),
            r#"<file path="widget.cpp" sites="3">"#,
            r#"<ref name="grow" qn="Widget::grow" line="3" role="consumer" site="update" at="4"/>"#,
            r#"<ref name="paint" line="7" role="consumer" site="write" at="8"/>"#,
            r#"<ref name="paint" line="7" role="consumer" site="read" at="9"/>"#,
            "</file>",
            "</match>",
            "</references>",
        ]),
        "the own class's field and one on a declared value bind; one on a returned value two structs offer does not"
    );
}

#[test]
fn find_references_says_the_rust_graph_records_no_use_of_a_type_as_one() {
    let answers = answers_over(
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "mod run;\nmod watch;\n"),
            (
                "src/watch.rs",
                "pub struct Watcher {\n    pub id: u32,\n}\n\npub struct Point(pub u32);\n",
            ),
            (
                "src/run.rs",
                "use crate::watch::{Point, Watcher};\n\npub fn start(watcher: &Watcher) -> Point {\n    \
                 Point(watcher.id)\n}\n",
            ),
        ],
        &[
            ("find_references", json!({"name": "Watcher"})),
            ("find_references", json!({"name": "Point"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Watcher" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="Watcher" loc="src/watch.rs:1" kind="struct" count="0">no recorded "#,
                    "site - the rust graph records no use of a type where code names it as one, such as a ",
                    "declaration, a signature, a generic argument or an import, so this does not show that nothing ",
                    "uses the type; grep finds where code names it</match>",
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="Point" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="Point" loc="src/watch.rs:5" kind="struct" count="1" files="1" "#,
                    r#"calls="1">"#,
                ),
                concat!(
                    "<note>the rust graph records no use of a type where code names it as one, such as a ",
                    "declaration, a signature, a generic argument or an import, so every such use is missing from ",
                    "this answer</note>",
                ),
                r#"<file path="src/run.rs" sites="1">"#,
                r#"<ref name="start" line="3" role="consumer" site="call" at="4" confidence="likely"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a type named only in a signature and an import has no recorded site, and a tuple struct's constructor call \
         is one"
    );
}

#[test]
fn find_references_keeps_its_answer_for_a_type_where_go_and_python_record_type_uses() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            ("pkg/kinds.go", "package pkg\n\ntype Spare struct{}\n"),
            (
                "models.py",
                "class Config:\n    pass\n\n\nclass Spare:\n    pass\n\n\ndef load(path):\n    \
                 config: Config = path\n    return config\n",
            ),
        ],
        &[
            ("find_references", json!({"name": "Spare"})),
            ("find_references", json!({"name": "Config"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Spare" matches="2">"#,
                concat!(
                    r#"<match lang="go" of="Spare" qn="pkg.Spare" loc="pkg/kinds.go:3" kind="struct" "#,
                    r#"count="0">no recorded site - resolved calls can omit dynamic or unresolved ones, and uses "#,
                    "other than calls are recorded only where the graph records them, so this does not show that ",
                    "nothing uses the symbol</match>",
                ),
                concat!(
                    r#"<match lang="python" of="Spare" qn="models.Spare" loc="models.py:5" kind="class" "#,
                    r#"count="0">no recorded site - resolved calls can omit dynamic or unresolved ones, and uses "#,
                    "other than calls are recorded only where the graph records them, so this does not show that ",
                    "nothing uses the symbol</match>",
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="Config" matches="1">"#,
                concat!(
                    r#"<match lang="python" of="Config" qn="models.Config" loc="models.py:1" kind="class" "#,
                    r#"count="1" files="1" types="1">"#,
                ),
                r#"<file path="models.py" sites="1">"#,
                r#"<ref name="load" qn="models.load" line="9" role="consumer" site="type" at="10"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a language that records uses as a type keeps the caveat that fits any language, and lists the uses"
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
fn find_callers_lists_the_calling_symbols_before_the_files_whose_top_level_calls() {
    let answers = answers_over(
        &[(
            "bin/run.sh",
            "#!/bin/sh\nhelper() {\n\techo help\n}\n\nmain() {\n\thelper\n}\n\nhelper\nmain\n",
        )],
        &[("find_callers", json!({"name": "helper"}))],
    );

    let answer = &answers[0];
    let symbol = answer.find("<ref ").expect("main calls helper");
    let file = answer
        .find("<file_scope ")
        .expect("the top level calls helper");
    assert!(
        symbol < file,
        "the calling symbols come before the file-scope records: {answer}"
    );
    assert!(
        answer.contains(r#"count="1""#) && answer.contains(r#"file_scope="1""#),
        "{answer}"
    );
}

#[test]
fn find_callers_lists_the_files_whose_top_level_calls_the_function() {
    let answers = answers_over(
        &[
            (
                "bin/run.sh",
                "#!/bin/sh\n. lib/common.sh\n\nsetup() {\n\techo ready\n}\n\nsetup\nshared\n",
            ),
            ("lib/common.sh", "shared() {\n\techo shared\n}\n"),
        ],
        &[
            ("find_callers", json!({"name": "setup"})),
            ("find_callers", json!({"name": "shared"})),
            (
                "find_callers",
                json!({"name": "shared", "min_confidence": "certain"}),
            ),
            ("find_callers", json!({"name": "setup", "role": "entry"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callers of="setup" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="setup" loc="bin/run.sh:4" count="0" reach_band="low" "#,
                    r#"file_scope="1">"#,
                ),
                r#"<file_scope path="bin/run.sh"/>"#,
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="shared" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="shared" loc="lib/common.sh:1" count="0" reach_band="low" "#,
                    r#"file_scope="1">"#,
                ),
                r#"<file_scope path="bin/run.sh" confidence="likely"/>"#,
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="shared" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="shared" loc="lib/common.sh:1" count="0" reach_band="low" "#,
                    r#"below_confidence="1">no callers at this confidence floor - 1 caller binds below it, and "#,
                    r#"min_confidence "any" lists it</match>"#,
                ),
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="setup" role="entry" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="setup" loc="bin/run.sh:4" count="0" reach_band="low" "#,
                    r#"file_scope="1" consumers="0" tests="0" entries="0" generated="0"/>"#,
                ),
                "</callers>",
            ]),
        ]
    );
}

#[test]
fn find_callers_says_how_many_callers_bind_below_the_floor_in_place_of_a_verdict() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
            ("helper.go", "package lib\n\nfunc helper() {}\n"),
            ("run.go", "package lib\n\nfunc Run() {\n\thelper()\n}\n"),
            ("stop.go", "package lib\n\nfunc Stop() {\n\thelper()\n}\n"),
        ],
        &[(
            "find_callers",
            json!({"name": "helper", "min_confidence": "certain"}),
        )],
    );

    assert_eq!(
        answers,
        [lines(&[
            r#"<callers of="helper" matches="1">"#,
            concat!(
                r#"<match lang="go" of="helper" qn="lib.helper" loc="helper.go:3" count="0" reach_band="low" "#,
                r#"below_confidence="2">no callers at this confidence floor - 2 callers bind below it, and "#,
                r#"min_confidence "any" lists them</match>"#,
            ),
            "</callers>",
        ])]
    );
}

#[test]
fn find_callers_describes_the_no_callers_note_of_a_function_a_field_and_an_enum_variant() {
    let tree = tree(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "src/lib.rs",
            "pub struct Config {\n    pub timeout: u32,\n}\n\npub enum Mode {\n    Fast,\n}\n\npub fn idle() {}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());
    let listed = client.request("tools/list", &json!({}));
    let description = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .find(|tool| tool["name"] == "find_callers")
        .and_then(|tool| tool["description"].as_str())
        .expect("find_callers is described")
        .to_owned();

    for name in ["idle", "Config::timeout", "Mode::Fast"] {
        let answer = client.answer("find_callers", &json!({"name": name}));
        let note = answer
            .split_once(">no callers - ")
            .and_then(|(_, note)| note.split_once("</match>"))
            .map(|(note, _)| note);
        assert!(
            note.is_some_and(|note| description.contains(note)),
            "the description states the no-callers note of {name}: {answer}\n{description}"
        );
    }
}

/// What a method that no call binds answers while calls of its name that several methods may answer bound none.
const NO_CALL_BINDS: &str = "no call binds it - ambiguous counts the calls of its name that bound none because \
                             several definitions of the name may be meant, and any of them may call this method; \
                             grep finds them, and if none of them calls it, it is an entry point, dead code, or \
                             reached by dynamic dispatch";

/// The answers to `calls` over a Rust crate whose methods share names: calls on a value of a type the graph does not
/// follow, and calls split between two methods of one name.
fn answers_over_shared_method_names(calls: &[(&str, Value)]) -> Vec<String> {
    answers_over(
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            (
                "src/lib.rs",
                "mod cancel;\nmod engine;\nmod handle;\nmod index;\nmod run;\nmod scan;\nmod state;\n",
            ),
            (
                "src/cancel.rs",
                "pub struct CancelToken;\n\nimpl CancelToken {\n    pub fn cancel(&self) {}\n\n    \
                 pub fn is_cancelled(&self) -> bool {\n        false\n    }\n}\n",
            ),
            (
                "src/handle.rs",
                "pub struct Handle;\n\nimpl Handle {\n    pub fn cancel(&self) {}\n\n    \
                 pub fn close(&self) {\n        self.cancel();\n    }\n}\n",
            ),
            (
                "src/run.rs",
                "use crate::cancel::CancelToken;\n\npub fn stop(token: &CancelToken) -> bool {\n    \
                 token.cancel();\n    token.is_cancelled()\n}\n",
            ),
            (
                "src/index.rs",
                "pub struct Index;\n\nimpl Index {\n    \
                 pub fn refresh_paths(&mut self, paths: &[&str]) -> usize {\n        paths.len()\n    }\n}\n",
            ),
            (
                "src/engine.rs",
                "pub struct Engine;\n\nimpl Engine {\n    \
                 pub fn refresh_paths(&mut self, paths: &[&str]) -> usize {\n        paths.len()\n    }\n\n    \
                 pub fn refresh_paths_limit(&self) -> usize {\n        8\n    }\n}\n",
            ),
            (
                "src/state.rs",
                "use crate::engine::Engine;\n\npub fn refresh(engine: &mut Engine, paths: &[&str]) -> usize {\n    \
                 if paths.len() > engine.refresh_paths_limit() {\n        return 0;\n    }\n    \
                 engine.refresh_paths(paths)\n}\n",
            ),
            (
                "src/scan.rs",
                "use crate::index::Index;\n\npub fn scan(index: &mut Index) -> usize {\n    \
                 index.refresh_paths(&[\"a.rs\"])\n}\n",
            ),
        ],
        calls,
    )
}

#[test]
fn find_callers_counts_the_calls_of_a_shared_method_name_it_could_not_bind() {
    let answers = answers_over_shared_method_names(&[
        (
            "find_callers",
            json!({"name": "CancelToken::cancel", "hops": 1}),
        ),
        (
            "find_callers",
            json!({"name": "Index::refresh_paths", "hops": 1}),
        ),
        ("find_callers", json!({"name": "Engine::refresh_paths"})),
        (
            "find_callers",
            json!({"name": "CancelToken::is_cancelled", "hops": 1}),
        ),
        (
            "find_callers",
            json!({"name": "Engine::refresh_paths_limit"}),
        ),
        ("find_callers", json!({"name": "Handle::cancel", "hops": 1})),
    ]);

    let no_call_binds = |of: &str, qn: &str, loc: &str| {
        lines(&[
            &format!(r#"<callers of="{qn}" matches="1">"#),
            &format!(
                "<match lang=\"rust\" of=\"{of}\" qn=\"{qn}\" loc=\"{loc}\" count=\"0\" reach_band=\"low\" \
                 ambiguous=\"{}\">{NO_CALL_BINDS}</match>",
                if of == "cancel" { 1 } else { 2 }
            ),
            "</callers>",
        ])
    };
    assert_eq!(
        answers,
        [
            no_call_binds("cancel", "CancelToken::cancel", "src/cancel.rs:4"),
            no_call_binds("refresh_paths", "Index::refresh_paths", "src/index.rs:4"),
            no_call_binds("refresh_paths", "Engine::refresh_paths", "src/engine.rs:4"),
            lines(&[
                r#"<callers of="CancelToken::is_cancelled" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="is_cancelled" qn="CancelToken::is_cancelled" loc="src/cancel.rs:6" "#,
                    r#"count="1" reach_band="low" unexpanded="1">"#,
                ),
                concat!(
                    r#"<ref name="stop" loc="src/run.rs:3" role="consumer" confidence="likely" hops="1" "#,
                    r#"at="src/run.rs:5"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="Engine::refresh_paths_limit" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="refresh_paths_limit" qn="Engine::refresh_paths_limit" "#,
                    r#"loc="src/engine.rs:8" count="1" reach_band="low">"#,
                ),
                concat!(
                    r#"<ref name="refresh" loc="src/state.rs:3" role="consumer" confidence="likely" hops="1" "#,
                    r#"at="src/state.rs:4"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="Handle::cancel" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="cancel" qn="Handle::cancel" loc="src/handle.rs:4" count="1" "#,
                    r#"reach_band="low" unexpanded="1" ambiguous="1">"#,
                ),
                concat!(
                    r#"<ref name="close" qn="Handle::close" loc="src/handle.rs:6" role="consumer" hops="1" "#,
                    r#"at="src/handle.rs:7"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
        ]
    );
}

#[test]
fn find_callers_kept_to_one_role_still_counts_every_call_of_the_name_it_could_not_bind() {
    let answers = answers_over_shared_method_names(&[(
        "find_callers",
        json!({"name": "Index::refresh_paths", "hops": 1, "role": "consumer"}),
    )]);

    assert_eq!(
        answers,
        [lines(&[
            r#"<callers of="Index::refresh_paths" role="consumer" matches="1">"#,
            &format!(
                "<match lang=\"rust\" of=\"refresh_paths\" qn=\"Index::refresh_paths\" loc=\"src/index.rs:4\" \
                 count=\"0\" reach_band=\"low\" ambiguous=\"2\" consumers=\"0\" tests=\"0\" entries=\"0\" \
                 generated=\"0\">{NO_CALL_BINDS}</match>"
            ),
            "</callers>",
        ])]
    );
}

#[test]
fn find_references_counts_the_calls_find_callers_could_not_bind() {
    let answers = answers_over_shared_method_names(&[(
        "find_references",
        json!({"name": "CancelToken::cancel"}),
    )]);

    assert_eq!(
        answers,
        [lines(&[
            r#"<references of="CancelToken::cancel" matches="1">"#,
            concat!(
                r#"<match lang="rust" of="cancel" qn="CancelToken::cancel" loc="src/cancel.rs:4" kind="method" "#,
                r#"count="0" ambiguous="1">no recorded site - resolved calls can omit dynamic or unresolved ones, "#,
                "and uses other than calls are recorded only where the graph records them, so this does not show ",
                "that nothing uses the symbol; a call through a value of the owner's type resolves only where the ",
                "graph can name that type, so some of those calls can be missing</match>",
            ),
            "</references>",
        ])]
    );
}

#[test]
fn the_impact_tools_count_the_calls_find_callers_could_not_bind() {
    let answers = answers_over_shared_method_names(&[
        (
            "impact_of_edit",
            json!({"file": "src/cancel.rs", "start_line": 4, "end_line": 4}),
        ),
        (
            "impact_of_change",
            json!({"regions": [{"file": "src/index.rs", "start_line": 4, "end_line": 6}]}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<impact of="src/cancel.rs" start_line="4" end_line="4" symbols="1">"#,
                concat!(
                    r#"<symbol name="cancel" qn="CancelToken::cancel" kind="method" lines="4-4" callers="0" "#,
                    r#"ambiguous="1" callees="0" reach="0" reach_files="1" reach_band="low" "#,
                    r#"sig="pub fn cancel(&self)"/>"#,
                ),
                r#"<includes count="0"/>"#,
                r#"<included-by count="1">"#,
                r#"<file path="src/lib.rs"/>"#,
                "</included-by>",
                "</impact>",
            ]),
            lines(&[
                concat!(
                    r#"<change regions="1" changed_symbols="1" changed_files="1" footprint="0" "#,
                    r#"footprint_files="0">"#,
                ),
                r#"<seeds shown="1" of="1">"#,
                concat!(
                    r#"<seed name="refresh_paths" qn="Index::refresh_paths" kind="method" loc="src/index.rs:4-6" "#,
                    r#"callers="0" ambiguous="2" reach="0" reach_band="low"/>"#,
                ),
                "</seeds>",
                "</change>",
            ]),
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
        (".git/HEAD", "ref: refs/heads/main\n"),
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
    let index = r#"<index root="<root>" files="3" gitignore="in-tree" watch="following" symbols="2" calls="3">"#;
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

#[test]
fn a_call_path_lists_each_ends_passed_over_symbols_and_what_an_unselected_end_matches() {
    let answers = answers_over_copies(&[
        ("find_call_path", json!({"from": "One", "to": "util.Parse"})),
        (
            "find_call_path",
            json!({"from": "One", "to": "util.Parse", "to_path_prefix": "c/"}),
        ),
        (
            "find_call_path",
            json!({"from": "Store.Close", "to": "util.Parse"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<call_path from="One" to="util.Parse" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="util.One" to="util.Parse" hops="1" max_hops="8" "#,
                    r#"passed_over_to="1">"#,
                ),
                r#"<ref name="One" qn="util.One" loc="a/util/util.go:5" role="consumer" call="a/util/util.go:5"/>"#,
                concat!(
                    r#"<candidate name="Parse" qn="util.Parse" kind="function" loc="b/util/util.go:3" end="to" "#,
                    r#"callers="1"/>"#,
                ),
                "</match>",
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="One" to="util.Parse" matches="0">"#,
                "<to>",
                "no definition of this name is under to_path_prefix - 2 are elsewhere:",
                r#"<candidate name="Parse" qn="util.Parse" kind="function" loc="a/util/util.go:3" lang="go"/>"#,
                r#"<candidate name="Parse" qn="util.Parse" kind="function" loc="b/util/util.go:3" lang="go"/>"#,
                "</to>",
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="Store.Close" to="util.Parse" matches="0">"#,
                "<from>",
                concat!(
                    "2 definitions end with this name and it picks none of them - a longer name, or ",
                    "from_path_prefix with one's file, picks one:",
                ),
                r#"<candidate name="Close" qn="alpha.Store.Close" kind="method" loc="a/alpha/store.go:9" lang="go"/>"#,
                r#"<candidate name="Close" qn="beta.Store.Close" kind="method" loc="b/beta/store.go:7" lang="go"/>"#,
                "</from>",
                "</call_path>",
            ]),
        ]
    );
}

/// The answers to `calls` over a tree of `files`.
fn answers_over(files: &[(&str, &str)], calls: &[(&str, Value)]) -> Vec<String> {
    let tree = tree(files);
    let mut client = Client::start(&[]);
    client.index(tree.path());
    calls
        .iter()
        .map(|(tool, arguments)| client.answer(tool, arguments))
        .collect()
}

/// The answers to `calls` over two copies of a package, and two packages whose types and methods share names.
fn answers_over_copies(calls: &[(&str, Value)]) -> Vec<String> {
    answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            (
                "a/util/util.go",
                "package util\n\nfunc Parse() {}\n\nfunc One() { Parse() }\n\nfunc Two() { Parse() }\n",
            ),
            (
                "b/util/util.go",
                "package util\n\nfunc Parse() {}\n\nfunc Three() { Parse() }\n",
            ),
            (
                "a/alpha/store.go",
                "package alpha\n\ntype Base interface{}\n\ntype Impl interface{ Base }\n\ntype Store struct{}\n\n\
             func (s *Store) Close() {}\n",
            ),
            (
                "b/beta/store.go",
                "package beta\n\ntype Base interface{}\n\ntype Store struct{}\n\nfunc (s *Store) Close() {}\n",
            ),
        ],
        calls,
    )
}

#[test]
fn a_name_several_symbols_share_lists_the_others_and_a_path_prefix_picks_one() {
    let answers = answers_over_copies(&[
        ("find_callers", json!({"name": "util.Parse", "hops": 1})),
        (
            "find_callers",
            json!({"name": "util.Parse", "hops": 1, "path_prefix": "b/"}),
        ),
        (
            "find_callers",
            json!({"name": "util.Parse", "path_prefix": "c/"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callers of="util.Parse" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Parse" qn="util.Parse" loc="a/util/util.go:3" count="2" "#,
                    r#"reach_band="low" unexpanded="2" passed_over="1">"#,
                ),
                r#"<ref name="One" qn="util.One" loc="a/util/util.go:5" role="consumer" hops="1" at="a/util/util.go:5"/>"#,
                r#"<ref name="Two" qn="util.Two" loc="a/util/util.go:7" role="consumer" hops="1" at="a/util/util.go:7"/>"#,
                r#"<candidate name="Parse" qn="util.Parse" kind="function" loc="b/util/util.go:3" callers="1"/>"#,
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="util.Parse" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Parse" qn="util.Parse" loc="b/util/util.go:3" count="1" "#,
                    r#"reach_band="low" unexpanded="1">"#,
                ),
                concat!(
                    r#"<ref name="Three" qn="util.Three" loc="b/util/util.go:5" role="consumer" hops="1" "#,
                    r#"at="b/util/util.go:5"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="util.Parse" matches="0">"#,
                "no definition of this name is under path_prefix - 2 are elsewhere:",
                r#"<candidate name="Parse" qn="util.Parse" kind="function" loc="a/util/util.go:3" lang="go"/>"#,
                r#"<candidate name="Parse" qn="util.Parse" kind="function" loc="b/util/util.go:3" lang="go"/>"#,
                "</callers>",
            ]),
        ]
    );
}

/// The answers to `calls` over two copies of a package: `a`'s `Run` has a caller and reads `Limit` once, `b`'s `Run`
/// has none and `b` reads its `Limit` on two lines.
fn answers_over_run_and_limit_copies(calls: &[(&str, Value)]) -> Vec<String> {
    answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            (
                "a/util/util.go",
                "package util\n\nvar Limit = 1\n\nfunc Helper() {}\n\nfunc Run() { Helper(); _ = Limit }\n\n\
                 func Caller() { Run() }\n",
            ),
            (
                "b/util/util.go",
                "package util\n\nvar Limit = 2\n\nfunc Check() {}\n\nfunc Run() { Check() }\n\n\
                 func Other() {\n\t_ = Limit\n\t_ = Limit\n}\n",
            ),
        ],
        calls,
    )
}

#[test]
fn callees_of_a_shared_name_list_the_other_copy_and_a_path_prefix_picks_it() {
    let answers = answers_over_run_and_limit_copies(&[
        ("find_callees", json!({"name": "util.Run", "hops": 1})),
        (
            "find_callees",
            json!({"name": "util.Run", "hops": 1, "path_prefix": "b/"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callees of="util.Run" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Run" qn="util.Run" loc="a/util/util.go:7" count="1" unexpanded="1" "#,
                    r#"passed_over="1">"#,
                ),
                concat!(
                    r#"<ref name="Helper" qn="util.Helper" loc="a/util/util.go:5" role="consumer" hops="1" "#,
                    r#"at="a/util/util.go:7"/>"#,
                ),
                r#"<candidate name="Run" qn="util.Run" kind="function" loc="b/util/util.go:7" callers="0"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="util.Run" matches="1">"#,
                r#"<match lang="go" of="Run" qn="util.Run" loc="b/util/util.go:7" count="1" unexpanded="1">"#,
                concat!(
                    r#"<ref name="Check" qn="util.Check" loc="b/util/util.go:5" role="consumer" hops="1" "#,
                    r#"at="b/util/util.go:7"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
        ]
    );
}

#[test]
fn references_of_a_shared_name_list_the_other_copy_and_a_path_prefix_picks_it() {
    let answers = answers_over_run_and_limit_copies(&[
        ("find_references", json!({"name": "util.Limit"})),
        (
            "find_references",
            json!({"name": "util.Limit", "path_prefix": "a/"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="util.Limit" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Limit" qn="util.Limit" loc="b/util/util.go:3" kind="variable" "#,
                    r#"count="2" files="1" reads="2" passed_over="1">"#,
                ),
                r#"<file path="b/util/util.go" sites="2">"#,
                r#"<ref name="Other" qn="util.Other" line="9" role="consumer" site="read" at="10,11"/>"#,
                "</file>",
                r#"<candidate name="Limit" qn="util.Limit" kind="variable" loc="a/util/util.go:3" sites="1"/>"#,
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="util.Limit" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Limit" qn="util.Limit" loc="a/util/util.go:3" kind="variable" "#,
                    r#"count="1" files="1" reads="1">"#,
                ),
                r#"<file path="a/util/util.go" sites="1">"#,
                r#"<ref name="Run" qn="util.Run" line="7" role="consumer" site="read" at="7"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ]
    );
}

#[test]
fn implementations_of_a_shared_name_list_the_other_type_and_a_path_prefix_picks_it() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            (
                "a/alpha/base.go",
                "package alpha\n\ntype Base interface{}\n\ntype Wide interface{ Base }\n\ntype Deep interface{ Base }\n",
            ),
            (
                "b/beta/base.go",
                "package beta\n\ntype Base interface{}\n\ntype Narrow interface{ Base }\n",
            ),
        ],
        &[
            ("find_implementations", json!({"name": "Base"})),
            (
                "find_implementations",
                json!({"name": "Base", "path_prefix": "b/"}),
            ),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<implementations of="Base" count="2" passed_over="1">"#,
                r#"<root name="Base" qn="alpha.Base" lang="go" loc="a/alpha/base.go:3"/>"#,
                concat!(
                    r#"<candidate name="Base" qn="beta.Base" kind="interface" loc="b/beta/base.go:3" lang="go" "#,
                    r#"implementors="1"/>"#,
                ),
                r#"<type name="Wide" qn="alpha.Wide" lang="go" loc="a/alpha/base.go:5"/>"#,
                r#"<type name="Deep" qn="alpha.Deep" lang="go" loc="a/alpha/base.go:7"/>"#,
                "</implementations>",
            ]),
            lines(&[
                r#"<implementations of="Base" count="1">"#,
                r#"<type name="Narrow" qn="beta.Narrow" lang="go" loc="b/beta/base.go:5"/>"#,
                "</implementations>",
            ]),
        ]
    );
}

#[test]
fn a_call_path_from_a_shared_name_starts_where_from_path_prefix_picks() {
    let answers = answers_over_run_and_limit_copies(&[
        (
            "find_call_path",
            json!({"from": "util.Run", "to": "util.Check"}),
        ),
        (
            "find_call_path",
            json!({"from": "util.Run", "to": "util.Check", "from_path_prefix": "b/"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<call_path from="util.Run" to="util.Check" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="util.Run" to="util.Check" hops="0" max_hops="8" "#,
                    r#"passed_over_from="1" found="false">"#,
                ),
                concat!(
                    "no call chain within 8 calls - the graph misses dynamic and unresolved calls, so this does not ",
                    "show that the code holds none",
                ),
                concat!(
                    r#"<candidate name="Run" qn="util.Run" kind="function" loc="b/util/util.go:7" end="from" "#,
                    r#"callers="0"/>"#,
                ),
                "</match>",
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="util.Run" to="util.Check" matches="1">"#,
                r#"<match lang="go" from="util.Run" to="util.Check" hops="1" max_hops="8">"#,
                r#"<ref name="Run" qn="util.Run" loc="b/util/util.go:7" role="consumer" call="b/util/util.go:7"/>"#,
                "</match>",
                "</call_path>",
            ]),
        ]
    );
}

#[test]
fn a_name_that_picks_no_one_of_several_lists_them_and_implementations_name_the_type_they_took() {
    let answers = answers_over_copies(&[
        ("find_references", json!({"name": "Store.Close"})),
        ("find_implementations", json!({"name": "Base"})),
        (
            "find_call_path",
            json!({"from": "Three", "to": "util.Parse", "to_path_prefix": "b/"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Store.Close" matches="0">"#,
                concat!(
                    "2 definitions end with this name and it picks none of them - a longer name, or path_prefix ",
                    "with one's file, picks one:",
                ),
                r#"<candidate name="Close" qn="alpha.Store.Close" kind="method" loc="a/alpha/store.go:9" lang="go"/>"#,
                r#"<candidate name="Close" qn="beta.Store.Close" kind="method" loc="b/beta/store.go:7" lang="go"/>"#,
                "</references>",
            ]),
            lines(&[
                r#"<implementations of="Base" count="1" passed_over="1">"#,
                r#"<root name="Base" qn="alpha.Base" lang="go" loc="a/alpha/store.go:3"/>"#,
                concat!(
                    r#"<candidate name="Base" qn="beta.Base" kind="interface" loc="b/beta/store.go:3" lang="go" "#,
                    r#"implementors="0"/>"#,
                ),
                r#"<type name="Impl" qn="alpha.Impl" lang="go" loc="a/alpha/store.go:5"/>"#,
                "</implementations>",
            ]),
            lines(&[
                r#"<call_path from="Three" to="util.Parse" matches="1">"#,
                r#"<match lang="go" from="util.Three" to="util.Parse" hops="1" max_hops="8">"#,
                r#"<ref name="Three" qn="util.Three" loc="b/util/util.go:5" role="consumer" call="b/util/util.go:5"/>"#,
                "</match>",
                "</call_path>",
            ]),
        ]
    );
}
