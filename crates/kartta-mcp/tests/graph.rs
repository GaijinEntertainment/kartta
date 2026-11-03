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
fn read_definitions_names_vendored_code_as_its_scope() {
    let tree = go_module();
    let vendored = tree.path().join("vendor/lib");
    std::fs::create_dir_all(&vendored).expect("the directory");
    std::fs::write(
        vendored.join("lib.go"),
        "package lib\n\nfunc Carried() {}\n",
    )
    .expect("the file");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("read_definitions", &json!({"names": ["Carried"]}));

    assert!(answer.contains(r#"scope="vendored""#), "{answer}");
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
                r#"<name of="Gret" total="0" one_edit="1">"#,
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
fn find_references_lists_a_read_through_a_package_value_a_sibling_file_declares() {
    let module = tree(&[
        ("go.mod", "module example.com/probe\n\ngo 1.22\n"),
        (
            "cfg.go",
            "package probe\n\ntype Config struct {\n\tTimeout int\n}\n\nvar cfg = Config{Timeout: 5}\n",
        ),
        (
            "use.go",
            "package probe\n\ntype Client struct{}\n\nfunc (Client) Do() {}\n\nvar client Client\n\n\
             func Read() int {\n\treturn cfg.Timeout\n}\n",
        ),
        (
            "call.go",
            "package probe\n\nfunc Call() {\n\tclient.Do()\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(module.path());

    let answers = [
        client.answer("find_references", &json!({"name": "probe.cfg"})),
        client.answer("find_references", &json!({"name": "probe.Config.Timeout"})),
        client.answer("find_references", &json!({"name": "probe.client"})),
    ];

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="probe.cfg" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="cfg" qn="probe.cfg" loc="cfg.go:7" kind="variable" count="1" "#,
                    r#"files="1" reads="1">"#,
                ),
                r#"<file path="use.go" sites="1">"#,
                concat!(
                    r#"<ref name="Read" qn="probe.Read" line="9" role="consumer" site="read" at="10" "#,
                    r#"confidence="likely"/>"#,
                ),
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="probe.Config.Timeout" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Timeout" qn="probe.Config.Timeout" loc="cfg.go:4" kind="field" "#,
                    r#"count="2" files="2" reads="1" writes="1">"#,
                ),
                r#"<file path="cfg.go" sites="1">"#,
                r#"<ref name="cfg" qn="probe.cfg" line="7" role="consumer" site="write" at="7"/>"#,
                "</file>",
                r#"<file path="use.go" sites="1">"#,
                r#"<ref name="Read" qn="probe.Read" line="9" role="consumer" site="read" at="10"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="probe.client" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="client" qn="probe.client" loc="use.go:7" kind="variable" count="1" "#,
                    r#"files="1" reads="1">"#,
                ),
                r#"<file path="call.go" sites="1">"#,
                concat!(
                    r#"<ref name="Call" qn="probe.Call" line="3" role="consumer" site="read" at="4" "#,
                    r#"confidence="likely"/>"#,
                ),
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a selector on a value another file of the package declares reads the value and its member, and a call \
         through it reads the value"
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
fn a_go_field_read_binds_through_the_type_its_own_function_declares_and_not_a_namesake_elsewhere() {
    let first_and_second = "package app\n\nfunc First() int {\n\ttype fields struct {\n\t\tSize int\n\t}\n\
                            \tf := fields{Size: 1}\n\treturn f.Size\n}\n\nfunc Second() int {\n\
                            \ttype fields struct {\n\t\tLimit int\n\t}\n\tf := fields{Limit: 2}\n\
                            \treturn f.Limit\n}\n";
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            ("app/app.go", first_and_second),
        ],
        &[
            ("find_references", json!({"name": "Limit"})),
            ("find_references", json!({"name": "Size"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Limit" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Limit" qn="app.fields.Limit" loc="app/app.go:13" kind="field" "#,
                    r#"count="2" files="1" reads="1" writes="1">"#,
                ),
                r#"<file path="app/app.go" sites="2">"#,
                r#"<ref name="Second" qn="app.Second" line="11" role="consumer" site="write" at="15"/>"#,
                r#"<ref name="Second" qn="app.Second" line="11" role="consumer" site="read" at="16"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Size" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Size" qn="app.fields.Size" loc="app/app.go:5" kind="field" "#,
                    r#"count="2" files="1" reads="1" writes="1">"#,
                ),
                r#"<file path="app/app.go" sites="2">"#,
                r#"<ref name="First" qn="app.First" line="3" role="consumer" site="write" at="7"/>"#,
                r#"<ref name="First" qn="app.First" line="3" role="consumer" site="read" at="8"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "each function's read and write bind the field of the type that function declares"
    );
}

#[test]
fn a_go_type_defined_over_an_interface_has_the_interface_s_methods() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/p\n\ngo 1.22\n"),
            (
                "internal/logger.go",
                "package internal\n\ntype Logger interface {\n\tInfo()\n}\n\n\
                 type Other interface {\n\tInfo()\n}\n",
            ),
            (
                "log/log.go",
                "package log\n\nimport \"example.com/p/internal\"\n\ntype DepthLogger internal.Logger\n\n\
                 func viaDefined(l DepthLogger) {\n\tl.Info()\n}\n\n\
                 func viaNamed(l internal.Logger) {\n\tl.Info()\n}\n\n\
                 func viaConversion(l internal.Logger) {\n\tDepthLogger(l).Info()\n}\n",
            ),
        ],
        &[
            ("find_callees", json!({"name": "viaDefined"})),
            ("find_callees", json!({"name": "viaConversion"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callees of="viaDefined" matches="1">"#,
                r#"<match lang="go" of="viaDefined" qn="log.viaDefined" loc="log/log.go:7" count="1" unexpanded="1">"#,
                concat!(
                    r#"<ref name="Info" qn="internal.Logger.Info" loc="internal/logger.go:4" role="consumer" "#,
                    r#"hops="1" at="log/log.go:8"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="viaConversion" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="viaConversion" qn="log.viaConversion" loc="log/log.go:15" count="2" "#,
                    r#"unexpanded="2">"#,
                ),
                concat!(
                    r#"<ref name="Info" qn="internal.Logger.Info" loc="internal/logger.go:4" role="consumer" "#,
                    r#"hops="1" at="log/log.go:16"/>"#,
                ),
                r#"<ref name="DepthLogger" qn="log.DepthLogger" loc="log/log.go:5" role="consumer" hops="1" at="log/log.go:16"/>"#,
                "</match>",
                "</callees>",
            ]),
        ],
        "a call through the defined type or a conversion to it binds the interface's method"
    );
}

#[test]
fn a_go_type_defined_over_a_struct_takes_none_of_the_struct_s_methods() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/p\n\ngo 1.22\n"),
            (
                "p.go",
                "package p\n\ntype S struct{}\n\nfunc (S) M() {}\n\ntype T S\n\n\
                 func use(t T) {\n\tt.M()\n}\n\nfunc convert(s S) {\n\tT(s).M()\n}\n",
            ),
        ],
        &[
            ("find_callees", json!({"name": "use"})),
            ("find_callees", json!({"name": "convert"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callees of="use" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="use" qn="p.use" loc="p.go:9" count="0" unresolved="1">no callees - no "#,
                    "call in its body binds a definition the graph holds; a call into a library or a builtin, ",
                    "through a function value, or by dynamic dispatch binds none</match>",
                ),
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="convert" matches="1">"#,
                r#"<match lang="go" of="convert" qn="p.convert" loc="p.go:13" count="1" unexpanded="1" unresolved="1">"#,
                r#"<ref name="T" qn="p.T" loc="p.go:7" role="consumer" hops="1" at="p.go:14"/>"#,
                "</match>",
                "</callees>",
            ]),
        ],
        "neither a value of the defined type nor a conversion to it reaches the struct's method"
    );
}

#[test]
fn a_go_type_defined_over_a_struct_keeps_its_fields_and_what_they_promote() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/p\n\ngo 1.22\n"),
            (
                "a.go",
                "package p\n\ntype Inner struct{}\n\nfunc (Inner) Promoted() {}\n\ntype S struct {\n\tInner\n\
                 \tX int\n}\n\ntype T S\n\nfunc viaStruct(s S) int {\n\ts.Promoted()\n\treturn s.X\n}\n\n\
                 func viaDefined(t T) int {\n\tt.Promoted()\n\treturn t.X\n}\n",
            ),
        ],
        &[
            ("find_callees", json!({"name": "viaDefined"})),
            ("find_references", json!({"name": "S.X"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callees of="viaDefined" matches="1">"#,
                r#"<match lang="go" of="viaDefined" qn="p.viaDefined" loc="a.go:19" count="1" unexpanded="1">"#,
                concat!(
                    r#"<ref name="Promoted" qn="p.Inner.Promoted" loc="a.go:5" role="consumer" hops="1" "#,
                    r#"at="a.go:20"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<references of="S.X" matches="1">"#,
                r#"<match lang="go" of="X" qn="p.S.X" loc="a.go:9" kind="field" count="2" files="1" reads="2">"#,
                r#"<file path="a.go" sites="2">"#,
                r#"<ref name="viaStruct" qn="p.viaStruct" line="14" role="consumer" site="read" at="16"/>"#,
                r#"<ref name="viaDefined" qn="p.viaDefined" line="19" role="consumer" site="read" at="21"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a defined type holds the struct's fields and what its embedded fields promote"
    );
}

/// The `M` each of `callers` of package `a` binds, read from `find_callees` over `source`.
fn methods_bound_by(source: &str, callers: &[&str]) -> Vec<(String, Vec<String>)> {
    let tree = tree(&[
        ("go.mod", "module example.com/a\n\ngo 1.22\n"),
        ("a.go", source),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());
    callers
        .iter()
        .map(|caller| {
            let answer = client.answer("find_callees", &json!({"name": format!("a.{caller}")}));
            let bound = answer
                .split(r#"<ref name="M" qn=""#)
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .map(str::to_owned)
                .collect();
            ((*caller).to_owned(), bound)
        })
        .collect()
}

fn each_binds(cases: &[(&str, &str)]) -> Vec<(String, Vec<String>)> {
    cases
        .iter()
        .map(|&(caller, method)| (caller.to_owned(), vec![method.to_owned()]))
        .collect()
}

const GO_DECLARATIONS: &str = "package a

type E struct{}

func (E) M() {}

type Z struct{}

func (Z) M() {}

type L []E
type T L

func (L) M() {}

type F func() E
type G F

func (F) M() {}
func (G) M() {}
";

#[test]
fn a_go_value_reads_through_a_type_defined_over_a_slice_or_a_function() {
    let source = format!(
        "{GO_DECLARATIONS}
func index(t T) {{ t[0].M() }}
func direct(l L) {{ l[0].M() }}
func call(g G) {{ g().M() }}
func plain(f F) {{ f().M() }}
func dropped(t T) {{ t.M() }}
"
    );

    let mut expected = each_binds(&[
        ("index", "a.E.M"),
        ("direct", "a.E.M"),
        ("call", "a.E.M"),
        ("plain", "a.E.M"),
    ]);
    expected.push(("dropped".to_owned(), Vec::new()));
    assert_eq!(
        methods_bound_by(&source, &["index", "direct", "call", "plain", "dropped"]),
        expected,
    );
}

#[test]
fn a_go_conversion_to_a_function_type_yields_that_type() {
    let source = format!(
        "{GO_DECLARATIONS}
type A = F

func defined(f F) {{ G(f).M() }}
func plain(f func() E) {{ F(f).M() }}
func aliased(f func() E) {{ A(f).M() }}
func held(f F) {{
\tconverted := G(f)
\tconverted.M()
}}
func calledAfter(f F) {{
\tconverted := G(f)
\tconverted().M()
}}
"
    );

    assert_eq!(
        methods_bound_by(
            &source,
            &["defined", "plain", "aliased", "held", "calledAfter"]
        ),
        each_binds(&[
            ("defined", "a.G.M"),
            ("plain", "a.F.M"),
            ("aliased", "a.F.M"),
            ("held", "a.G.M"),
            ("calledAfter", "a.E.M"),
        ]),
    );
}

#[test]
fn a_go_conversion_to_a_type_its_function_declares_yields_that_type() {
    let source = "package a

type E struct{}

func (E) M() {}

type Z struct{}

func (Z) M() {}

type P struct{ E }

func useP(s struct{ E }) { P(s).M() }

func useL(s struct{ E }) {
\ttype L struct{ E }
\tL(s).M()
}

func held(s struct{ E }) {
\ttype L struct{ E }
\tconverted := L(s)
\tconverted.M()
}
";

    assert_eq!(
        methods_bound_by(source, &["useP", "useL", "held"]),
        each_binds(&[("useP", "a.E.M"), ("useL", "a.E.M"), ("held", "a.E.M")]),
    );
}

const GO_BODY_TYPES: &str = "package a

type E struct{}

func (E) M() {}

type Z struct{}

func (Z) M() {}

type P struct{ E }

func shadowed(s struct{ Z }) {
\ttype P struct{ Z }
\tP(s).M()
}

func declared() {
\ttype P struct{ Z }
\tvar p P
\tp.M()
}

func literal() {
\ttype P struct{ Z }
\tP{}.M()
}

func allocated() {
\ttype P struct{ Z }
\tnew(P).M()
}

func expression() {
\ttype P struct{ Z }
\tP.M(P{})
}

func before(s struct{ E }) {
\tP(s).M()
\ttype P struct{ Z }
\t_ = P{}
}

func past() {
\t{
\t\ttype P struct{ Z }
\t\t_ = P{}
\t}
\tP{}.M()
}

func siblings() {
\t{
\t\ttype B struct{ E }
\t\tB{}.M()
\t}
\t{
\t\ttype B struct{ Z }
\t\tB{}.M()
\t}
}

func embedded() {
\ttype I struct{ Z }
\ttype O struct{ I }
\tO{}.M()
}
";

#[test]
fn a_go_type_a_body_declares_reads_as_the_scope_at_the_site_gives_it() {
    let mut expected = each_binds(&[
        ("shadowed", "a.Z.M"),
        ("declared", "a.Z.M"),
        ("literal", "a.Z.M"),
        ("allocated", "a.Z.M"),
        ("expression", "a.Z.M"),
        ("before", "a.E.M"),
        ("past", "a.E.M"),
    ]);
    expected.push((
        "siblings".to_owned(),
        vec!["a.E.M".to_owned(), "a.Z.M".to_owned()],
    ));
    expected.push(("embedded".to_owned(), vec!["a.Z.M".to_owned()]));
    assert_eq!(
        methods_bound_by(
            GO_BODY_TYPES,
            &[
                "shadowed",
                "declared",
                "literal",
                "allocated",
                "expression",
                "before",
                "past",
                "siblings",
                "embedded",
            ]
        ),
        expected,
    );
}

#[test]
fn a_go_value_of_a_function_type_is_called_wherever_it_is_held() {
    let source = format!(
        "{GO_DECLARATIONS}
type A = F
type AG = G
type Literal = func() E
type LiteralTwo = func() (Z, E)

type S struct {{
\tG
\tNamed G
}}

type Two func() (Z, E)
type TwoDefined Two

var held G

func source() E {{ return E{{}} }}

func packageValue() {{ held().M() }}
func embedded(s S) {{ s.G().M() }}
func field(s S) {{ s.Named().M() }}
func alias(a A) {{ a().M() }}
func aliasOfDefined(a AG) {{ a().M() }}
func literal(l Literal) {{ l().M() }}
func literalSecond(l LiteralTwo) {{
\t_, e := l()
\te.M()
}}
func second(t TwoDefined) {{
\t_, e := t()
\te.M()
}}
func first(t TwoDefined) {{
\tz, _ := t()
\tz.M()
}}
func viaFunction() {{
\tf := source
\tf().M()
}}
"
    );

    assert_eq!(
        methods_bound_by(
            &source,
            &[
                "packageValue",
                "embedded",
                "field",
                "alias",
                "aliasOfDefined",
                "literal",
                "literalSecond",
                "second",
                "first",
                "viaFunction",
            ]
        ),
        each_binds(&[
            ("packageValue", "a.E.M"),
            ("embedded", "a.E.M"),
            ("field", "a.E.M"),
            ("alias", "a.E.M"),
            ("aliasOfDefined", "a.E.M"),
            ("literal", "a.E.M"),
            ("literalSecond", "a.E.M"),
            ("second", "a.E.M"),
            ("first", "a.Z.M"),
            ("viaFunction", "a.E.M"),
        ]),
    );
}

#[test]
fn a_go_call_of_a_function_type_with_unspelled_results_binds_none_of_its_own_methods() {
    let source = "package a

type E struct{}

func (E) M() {}

type F func() interface{ M() }

func (F) M() {}

type G func() E

func (G) M() {}

type Over F

func (Over) M() {}

type Generic[T interface{ M() }] func() T

func (Generic[T]) M() {}

type Literal func() struct{ E }

func (Literal) M() {}

type Funcs []func() E

func (Funcs) M() {}

type N func()

func (N) M() {}

func useF(f F) { f().M() }
func useG(g G) { g().M() }
func over(o Over) { o().M() }
func generic(g Generic[E]) { g().M() }
func literal(l Literal) { l().M() }
func element(c Funcs) { c[0]().M() }
func noResults(n N) {
\tn()
\tn.M()
}
";

    assert_eq!(
        methods_bound_by(
            source,
            &[
                "useF",
                "useG",
                "over",
                "generic",
                "literal",
                "element",
                "noResults",
            ]
        ),
        vec![
            ("useF".to_owned(), Vec::new()),
            ("useG".to_owned(), vec!["a.E.M".to_owned()]),
            ("over".to_owned(), Vec::new()),
            ("generic".to_owned(), Vec::new()),
            ("literal".to_owned(), Vec::new()),
            ("element".to_owned(), Vec::new()),
            ("noResults".to_owned(), vec!["a.N.M".to_owned()]),
        ],
    );
}

#[test]
fn a_go_function_type_s_results_read_past_an_alias_and_a_first_container() {
    let source = format!(
        "{GO_DECLARATIONS}
type AliasOfFunction = func() F
type AliasOfAlias = AliasOfFunction
type DefinedOverAlias AliasOfFunction

type Containers func() ([]E, Z)
type OverContainers Containers
type AliasOfContainers = func() ([]E, Z)
type OneContainer func() []E

func aliasOfFunction(a AliasOfFunction) {{ a().M() }}
func aliasOfAlias(a AliasOfAlias) {{ a().M() }}
func overAlias(d DefinedOverAlias) {{ d().M() }}
func laterResult(c Containers) {{
\t_, z := c()
\tz.M()
}}
func firstResult(c Containers) {{
\te, _ := c()
\te[0].M()
}}
func laterThroughDefined(c OverContainers) {{
\t_, z := c()
\tz.M()
}}
func laterThroughAlias(c AliasOfContainers) {{
\t_, z := c()
\tz.M()
}}
func oneContainer(c OneContainer) {{ c()[0].M() }}
"
    );

    assert_eq!(
        methods_bound_by(
            &source,
            &[
                "aliasOfFunction",
                "aliasOfAlias",
                "overAlias",
                "laterResult",
                "firstResult",
                "laterThroughDefined",
                "laterThroughAlias",
                "oneContainer",
            ]
        ),
        each_binds(&[
            ("aliasOfFunction", "a.F.M"),
            ("aliasOfAlias", "a.F.M"),
            ("overAlias", "a.F.M"),
            ("laterResult", "a.Z.M"),
            ("firstResult", "a.E.M"),
            ("laterThroughDefined", "a.Z.M"),
            ("laterThroughAlias", "a.Z.M"),
            ("oneContainer", "a.E.M"),
        ]),
    );
}

#[test]
fn a_go_element_reads_through_a_map_a_channel_an_array_and_an_alias_under_a_defined_type() {
    let source = format!(
        "{GO_DECLARATIONS}
type Mapped map[Z]E
type MappedDefined Mapped
type Chan chan E
type ChanDefined Chan
type Array [1]E
type ArrayDefined Array
type AliasOfDefined = T
type AliasOfSlice = []E
type OverAlias AliasOfSlice

func mapped(n MappedDefined) {{ n[Z{{}}].M() }}
func ranged(n MappedDefined) {{
\tfor _, value := range n {{
\t\tvalue.M()
\t}}
}}
func received(d ChanDefined) {{ (<-d).M() }}
func array(a ArrayDefined) {{ a[0].M() }}
func pointer(a *ArrayDefined) {{ a[0].M() }}
func alias(a AliasOfDefined) {{ a[0].M() }}
func overAlias(a OverAlias) {{ a[0].M() }}
func converted(l L) {{ T(l)[0].M() }}
"
    );

    assert_eq!(
        methods_bound_by(
            &source,
            &[
                "mapped",
                "ranged",
                "received",
                "array",
                "pointer",
                "alias",
                "overAlias",
                "converted",
            ]
        ),
        each_binds(&[
            ("mapped", "a.E.M"),
            ("ranged", "a.E.M"),
            ("received", "a.E.M"),
            ("array", "a.E.M"),
            ("pointer", "a.E.M"),
            ("alias", "a.E.M"),
            ("overAlias", "a.E.M"),
            ("converted", "a.E.M"),
        ]),
    );
}

#[test]
fn a_go_closure_reads_a_field_through_the_type_its_enclosing_function_declares() {
    let source = "package app\n\nfunc First() int {\n\ttype fields struct {\n\t\tSize int\n\t}\n\
                  \treturn fields{Size: 1}.Size\n}\n\nfunc Second() func() int {\n\ttype fields struct {\n\
                  \t\tSize int\n\t}\n\treturn func() int {\n\t\tf := fields{Size: 2}\n\t\tf.Size++\n\
                  \t\treturn f.Size\n\t}\n}\n";
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            ("app/app.go", source),
        ],
        &[("find_references", json!({"name": "app.fields.Size"}))],
    );

    assert_eq!(
        answers,
        [lines(&[
            r#"<references of="app.fields.Size" matches="1">"#,
            concat!(
                r#"<match lang="go" of="Size" qn="app.fields.Size" loc="app/app.go:12" kind="field" "#,
                r#"count="3" files="1" reads="1" writes="1" updates="1" passed_over="1">"#,
            ),
            r#"<file path="app/app.go" sites="3">"#,
            r#"<ref name="Second" qn="app.Second" line="10" role="consumer" site="write" at="15"/>"#,
            r#"<ref name="Second" qn="app.Second" line="10" role="consumer" site="update" at="16"/>"#,
            r#"<ref name="Second" qn="app.Second" line="10" role="consumer" site="read" at="17"/>"#,
            "</file>",
            r#"<candidate name="Size" qn="app.fields.Size" kind="field" loc="app/app.go:5" sites="2"/>"#,
            "</match>",
            "</references>",
        ])],
        "the closure's sites bind Second's field, and First's two sites stay on its own"
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
fn find_references_lists_the_read_a_python_class_body_makes_of_its_own_attribute() {
    let module = tree(&[(
        "devices.py",
        "class Devices:\n    ONE = \"one\"\n    TWO = \"two\"\n    KNOWN = (ONE, TWO)\n\n\n\
         def known():\n    return Devices.KNOWN\n\n\nMODULE = 1\n\n\ndef read_module():\n    return MODULE\n",
    )]);
    let mut client = Client::start(&[]);
    client.index(module.path());

    for (name, line) in [("ONE", 2), ("TWO", 3)] {
        assert_eq!(
            client.answer("find_references", &json!({"name": name})),
            lines(&[
                &format!(r#"<references of="{name}" matches="1">"#),
                &format!(
                    "<match lang=\"python\" of=\"{name}\" qn=\"devices.Devices.{name}\" loc=\"devices.py:{line}\" \
                     kind=\"field\" count=\"1\" files=\"1\" reads=\"1\">"
                ),
                concat!(
                    "<note>an attribute of a value whose class the extraction cannot name binds only where one field ",
                    "of the name is in its own file; ambiguous counts the rest</note>",
                ),
                r#"<file path="devices.py" sites="1">"#,
                r#"<ref name="Devices" qn="devices.Devices" line="1" role="consumer" site="read" at="4"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            "the class body reads {name}"
        );
    }
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
                r#"<ref name="start" line="3" role="consumer" site="call" at="4"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a type named only in a signature and an import has no recorded site, and a tuple struct's constructor call \
         is one, certain through the import"
    );
}

fn sites_of_each(source: &str, names: &[&str]) -> Vec<Vec<String>> {
    let calls: Vec<_> = names
        .iter()
        .map(|&name| ("find_references", json!({ "name": name })))
        .collect();
    answers_over(
        &[
            ("Cargo.toml", "[package]\nname = \"probe\"\n"),
            ("src/lib.rs", source),
        ],
        &calls,
    )
    .iter()
    .map(|answer| {
        answer
            .lines()
            .filter(|line| line.starts_with("<ref "))
            .map(str::to_owned)
            .collect()
    })
    .collect()
}

fn read_site(qualified: &str, line: u32, at: u32) -> String {
    let name = qualified.rsplit("::").next().unwrap_or(qualified);
    format!(
        r#"<ref name="{name}" qn="{qualified}" line="{line}" role="consumer" site="read" at="{at}"/>"#
    )
}

#[test]
fn find_references_credits_a_rust_bare_read_to_the_item_of_its_own_inline_module() {
    let declares = |module: &str, value: u8| {
        format!("pub mod {module} {{\n    pub static X: i32 = {value};\n}}\n")
    };
    let reads = |module: &str, value: u8| {
        let read = "    pub fn read() -> i32 {\n        X\n    }\n";
        format!("pub mod {module} {{\n    pub static X: i32 = {value};\n\n{read}}}\n")
    };
    let second_last = format!("{}\n{}", declares("first", 1), reads("second", 2));
    let second_first = format!("{}\n{}", reads("second", 2), declares("first", 1));
    let in_first = format!("{}\n{}", reads("first", 1), declares("second", 2));
    let names = ["first::X", "second::X"];

    assert_eq!(
        sites_of_each(&second_last, &names),
        [Vec::new(), vec![read_site("second::read", 8, 9)]],
        "the read binds its own module's item, written last"
    );
    assert_eq!(
        sites_of_each(&second_first, &names),
        [Vec::new(), vec![read_site("second::read", 4, 5)]],
        "the read binds its own module's item, written first"
    );
    assert_eq!(
        sites_of_each(&in_first, &names),
        [vec![read_site("first::read", 4, 5)], Vec::new()],
        "a read in the first module binds the first"
    );
}

#[test]
fn find_references_credits_a_rust_bare_read_to_the_nearest_module_that_holds_it_and_no_other() {
    let root_first = "static X: i32 = 0;

mod inner {
    pub static X: i32 = 1;
    pub fn read() -> i32 {
        X
    }
}
";
    let module_first = "mod inner {
    pub static X: i32 = 1;
}

static X: i32 = 0;

fn read() -> i32 {
    X
}
";
    let beside = "mod outer {
    pub static X: i32 = 1;
    pub mod inner {
        pub static X: i32 = 2;
    }
    pub fn read() -> i32 {
        X
    }
}
";
    let undeclared = "pub mod first {
    pub static X: i32 = 1;
}

pub mod third {
    pub fn read() -> i32 {
        X
    }
}
";

    assert_eq!(
        sites_of_each(root_first, &["inner::X"]),
        [vec![read_site("inner::read", 5, 6)]],
        "a module's item hides the file root's, written before it"
    );
    assert_eq!(
        sites_of_each(module_first, &["inner::X"]),
        [Vec::<String>::new()],
        "a read at the file root reaches no inline module's item"
    );
    assert_eq!(
        sites_of_each(beside, &["outer::X", "outer::inner::X"]),
        [vec![read_site("outer::read", 6, 7)], Vec::new()],
        "a module the read is outside of holds no item it reaches"
    );
    assert_eq!(
        sites_of_each(undeclared, &["first::X"]),
        [Vec::<String>::new()],
        "a module that declares no item of the name keeps no read of it"
    );
}

#[test]
fn find_references_credits_a_rust_bare_read_past_a_module_of_its_name_and_within_its_cfg_block() {
    let module_of_the_name = "fn handler() {}

mod test {
    use super::*;

    fn run() {
        let _ = handler;
    }

    mod handler {}
}
";
    assert_eq!(
        sites_of_each(module_of_the_name, &["handler"]),
        [vec![read_site("test::run", 6, 7)]],
        "a module is no value, so the item a glob brings in binds"
    );

    let blocks = "#[cfg(unix)]
mod gate {
    pub static X: i32 = 1;
    pub fn read() -> i32 {
        X
    }
}

#[cfg(not(unix))]
mod gate {
    pub static X: i32 = 2;
    pub fn read() -> i32 {
        X
    }
}
";
    let answers = answers_over(
        &[
            ("Cargo.toml", "[package]\nname = \"probe\"\n"),
            ("src/lib.rs", blocks),
        ],
        &[
            ("find_references", json!({"name": "gate::X", "line": 3})),
            ("find_references", json!({"name": "gate::X", "line": 11})),
        ],
    );
    let of_block = |answer: &str| {
        (answer.lines())
            .filter(|line| line.starts_with("<ref "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        [of_block(&answers[0]), of_block(&answers[1])],
        [
            vec![read_site("gate::read", 4, 5)],
            vec![read_site("gate::read", 12, 13)]
        ],
        "a read binds the item of the block of its module that holds it"
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
fn find_references_labels_a_member_read_through_a_type_a_read_and_not_a_use_as_a_type() {
    let answers = answers_over(
        &[
            ("events.ts", "export enum EventType {\n    Created,\n}\n"),
            (
                "card.ts",
                "import { EventType } from \"./events\";\n\nexport function kind() {\n    \
                 return EventType.Created;\n}\n",
            ),
            (
                "palette.py",
                "class Palette:\n    default = 1\n\n\ndef pick(palette: Palette):\n    \
                 return Palette.default\n",
            ),
        ],
        &[
            ("find_references", json!({"name": "EventType"})),
            ("find_references", json!({"name": "Palette"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="EventType" matches="1">"#,
                concat!(
                    r#"<match lang="jsts" of="EventType" loc="events.ts:1" kind="enum" count="1" files="1" "#,
                    r#"reads="1">"#,
                ),
                concat!(
                    "<note>the jsts graph records no use of a type where code names it as one, such as a ",
                    "declaration, a signature, a generic argument or an import, so every such use is missing from ",
                    "this answer</note>",
                ),
                r#"<file path="card.ts" sites="1">"#,
                r#"<ref name="kind" line="3" role="consumer" site="read" at="4" confidence="likely"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Palette" matches="1">"#,
                concat!(
                    r#"<match lang="python" of="Palette" qn="palette.Palette" loc="palette.py:1" kind="class" "#,
                    r#"count="2" files="1" types="1" reads="1">"#,
                ),
                r#"<file path="palette.py" sites="2">"#,
                r#"<ref name="pick" qn="palette.pick" line="5" role="consumer" site="type" at="5"/>"#,
                r#"<ref name="pick" qn="palette.pick" line="5" role="consumer" site="read" at="6"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a member taken through a type reads the type as a value, and an annotation names it as a type"
    );
}

#[test]
fn find_references_keeps_the_uses_as_a_type_alone_or_among_the_reads() {
    let answers = answers_over(
        &[(
            "palette.py",
            "class Palette:\n    default = 1\n\n\ndef pick(palette: Palette):\n    return Palette.default\n",
        )],
        &[
            (
                "find_references",
                json!({"name": "Palette", "access": "type"}),
            ),
            (
                "find_references",
                json!({"name": "Palette", "access": "read"}),
            ),
        ],
    );
    let site = |line: u32, kind: &str, at: u32| {
        format!(
            r#"<ref name="pick" qn="palette.pick" line="{line}" role="consumer" site="{kind}" at="{at}"/>"#
        )
    };
    let counts = r#"files="1" types="1" reads="1">"#;
    let head = r#"<match lang="python" of="Palette" qn="palette.Palette" loc="palette.py:1" kind="class" "#;

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Palette" matches="1">"#,
                &format!(r#"{head}count="1" {counts}"#),
                r#"<file path="palette.py" sites="1">"#,
                &site(5, "type", 5),
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Palette" matches="1">"#,
                &format!(r#"{head}count="2" {counts}"#),
                r#"<file path="palette.py" sites="2">"#,
                &site(5, "type", 5),
                &site(5, "read", 6),
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "type keeps the annotation alone, and read keeps it beside the read through the class"
    );
}

#[test]
fn find_references_names_the_language_that_records_no_read_or_write_of_the_symbol_s_kind() {
    let answers = answers_over(
        &[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("src/lib.rs", "mod shapes;\n"),
            (
                "src/shapes.rs",
                "pub struct Point {\n    pub x: u32,\n}\n\npub fn shift(p: &mut Point) {\n    p.x += 1;\n}\n",
            ),
            (
                "tools/run.sh",
                "LOG=/tmp/log\nrun() {\n  echo \"$LOG\"\n}\n",
            ),
            (
                "tools/clean.sh",
                "TMP=/tmp/work\nclean() {\n  rm -rf \"$TMP\"\n  trap TMP EXIT\n}\n",
            ),
        ],
        &[
            ("find_references", json!({"name": "Point::x"})),
            ("find_references", json!({"name": "LOG"})),
            ("find_references", json!({"name": "TMP"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Point::x" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="x" qn="Point::x" loc="src/shapes.rs:2" kind="field" count="0">no "#,
                    "recorded site - the rust graph records no read or write of a field, so this does not show that ",
                    "nothing uses the field; grep finds where code names it</match>",
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="LOG" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="LOG" loc="tools/run.sh:1" kind="variable" count="0">no recorded "#,
                    "site - the bash graph records no read or write of a variable, so this does not show that ",
                    "nothing uses the variable; grep finds where code names it</match>",
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="TMP" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="TMP" loc="tools/clean.sh:1" kind="variable" count="1" files="1" "#,
                    r#"reads="1">"#,
                ),
                concat!(
                    "<note>the bash graph records no read or write of a variable, so every such use is missing from ",
                    "this answer</note>",
                ),
                r#"<file path="tools/clean.sh" sites="1">"#,
                r#"<ref name="clean" line="2" role="consumer" site="read" at="4"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a field or a variable its language records no read or write of has no recorded site, and the answer says why; \
         a handler word that spells the name lists under a note that every read and write is missing"
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

/// The answers to `calls` over a Go package whose `walk` calls itself and is called by `Run`, and whose `spin` calls
/// itself alone.
fn answers_over_recursion(calls: &[(&str, Value)]) -> Vec<String> {
    answers_over(
        &[
            ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
            (
                "walk.go",
                "package lib\n\nfunc walk(n int) int {\n\tif n == 0 {\n\t\treturn 0\n\t}\n\treturn walk(n - 1)\n}\n\n\
                 func Run() {\n\twalk(3)\n}\n\nfunc spin() {\n\tspin()\n}\n",
            ),
        ],
        calls,
    )
}

#[test]
fn find_callers_find_callees_and_find_references_agree_on_a_function_that_calls_itself() {
    let answers = answers_over_recursion(&[
        ("find_callers", json!({"name": "walk", "hops": 1})),
        ("find_callees", json!({"name": "walk"})),
        ("find_references", json!({"name": "walk"})),
        ("find_callers", json!({"name": "walk"})),
    ]);

    let callers = |unexpanded: &str| {
        lines(&[
            r#"<callers of="walk" matches="1">"#,
            &format!(
                r#"<match lang="go" of="walk" qn="lib.walk" loc="walk.go:3" count="1" reach_band="low"{unexpanded}>"#
            ),
            r#"<self_call at="walk.go:7"/>"#,
            r#"<ref name="Run" qn="lib.Run" loc="walk.go:10" role="consumer" hops="1" at="walk.go:11"/>"#,
            "</match>",
            "</callers>",
        ])
    };
    assert_eq!(
        answers,
        [
            callers(r#" unexpanded="1""#),
            lines(&[
                r#"<callees of="walk" matches="1">"#,
                r#"<match lang="go" of="walk" qn="lib.walk" loc="walk.go:3" count="0">"#,
                concat!(
                    "no callees but itself - no other call in its body binds a definition the graph holds; a call ",
                    "into a library or a builtin, through a function value, or by dynamic dispatch binds none",
                ),
                r#"<self_call at="walk.go:7"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<references of="walk" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="walk" qn="lib.walk" loc="walk.go:3" kind="function" count="2" "#,
                    r#"files="1" calls="2">"#,
                ),
                r#"<file path="walk.go" sites="2">"#,
                r#"<ref name="walk" qn="lib.walk" line="3" role="consumer" site="call" at="7"/>"#,
                r#"<ref name="Run" qn="lib.Run" line="10" role="consumer" site="call" at="11"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            callers(""),
        ]
    );
}

#[test]
fn a_function_that_calls_itself_alone_has_no_caller_and_no_callee_but_itself() {
    let answers = answers_over_recursion(&[
        ("find_callers", json!({"name": "spin"})),
        ("find_callees", json!({"name": "spin"})),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callers of="spin" matches="1">"#,
                r#"<match lang="go" of="spin" qn="lib.spin" loc="walk.go:14" count="0" reach_band="low">"#,
                "no callers but itself - entry point, dead code, or dynamic dispatch",
                r#"<self_call at="walk.go:15"/>"#,
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callees of="spin" matches="1">"#,
                r#"<match lang="go" of="spin" qn="lib.spin" loc="walk.go:14" count="0">"#,
                concat!(
                    "no callees but itself - no other call in its body binds a definition the graph holds; a call ",
                    "into a library or a builtin, through a function value, or by dynamic dispatch binds none",
                ),
                r#"<self_call at="walk.go:15"/>"#,
                "</match>",
                "</callees>",
            ]),
        ]
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
/// follow, an element a slice parameter holds, and calls split between two methods of one name.
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
                "use crate::cancel::CancelToken;\n\npub fn stop(tokens: &[CancelToken]) -> bool { \
                 let token = &tokens[0];\n    token.cancel();\n    token.is_cancelled()\n}\n",
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
                "use crate::engine::Engine;\n\npub fn refresh(engines: &mut [Engine], paths: &[&str]) -> usize { \
                 let engine = &mut engines[0];\n    \
                 if paths.len() > engine.refresh_paths_limit() {\n        return 0;\n    }\n    \
                 engine.refresh_paths(paths)\n}\n",
            ),
            (
                "src/scan.rs",
                "use crate::index::Index;\n\npub fn scan(indexes: &mut [Index]) -> usize { \
                 let index = &mut indexes[0];\n    \
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
fn the_impact_tools_reach_the_symbols_that_read_or_write_a_field() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "wire/errors.go",
            "package wire\n\ntype Error struct {\n\tCode string\n}\n\nfunc (e *Error) Error() string {\n\t\
             return e.Code\n}\n\nfunc (e *Error) Unwrap() string {\n\treturn e.Code\n}\n",
        ),
        (
            "wire/call.go",
            "package wire\n\nfunc call() error {\n\treturn &Error{Code: \"x\"}\n}\n\nfunc Run() {\n\tcall()\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answers = [
        client.answer(
            "impact_of_edit",
            &json!({"file": "wire/errors.go", "start_line": 4, "end_line": 4}),
        ),
        client.answer(
            "impact_of_change",
            &json!({"regions": [{"file": "wire/errors.go", "start_line": 4, "end_line": 4}]}),
        ),
    ];

    let impact = [
        &[
            r#"<impact of="wire/errors.go" start_line="4" end_line="4" symbols="2">"#,
            concat!(
                r#"<symbol name="Error" qn="wire.Error" kind="struct" lines="3-5" callers="0" callees="0" "#,
                r#"reach="0" reach_files="0" reach_band="low"/>"#,
            ),
            concat!(
                r#"<symbol name="Code" qn="wire.Error.Code" kind="field" lines="4-4" callers="0" users="3" "#,
                r#"callees="0" reach="4" reach_files="1" reach_band="low">"#,
            ),
        ][..],
        &CODE_USERS,
        &[
            "</symbol>",
            r#"<includes count="0"/>"#,
            r#"<included-by count="0"/>"#,
            "</impact>",
        ],
    ];
    let change = [
        &[
            concat!(
                r#"<change regions="1" changed_symbols="2" changed_files="1" footprint="4" "#,
                r#"footprint_files="2">"#,
            ),
            r#"<seeds shown="2" of="2">"#,
            concat!(
                r#"<seed name="Code" qn="wire.Error.Code" kind="field" loc="wire/errors.go:4" callers="0" "#,
                r#"users="3" reach="4" reach_band="low">"#,
            ),
        ][..],
        &CODE_USERS,
        &[
            "</seed>",
            concat!(
                r#"<seed name="Error" qn="wire.Error" kind="struct" loc="wire/errors.go:3-5" callers="0" "#,
                r#"reach="0" reach_band="low"/>"#,
            ),
            "</seeds>",
            "</change>",
        ],
    ];

    assert_eq!(answers, [lines(&impact.concat()), lines(&change.concat())]);
}

/// The users of `wire.Error.Code` the impact tools list: the function that writes it and the two methods that read it.
const CODE_USERS: [&str; 5] = [
    r#"<users shown="3" of="3">"#,
    r#"<ref name="call" qn="wire.call" loc="wire/call.go:3" role="consumer" at="wire/call.go:4"/>"#,
    r#"<ref name="Error" qn="wire.Error.Error" loc="wire/errors.go:7" role="consumer" at="wire/errors.go:8"/>"#,
    r#"<ref name="Unwrap" qn="wire.Error.Unwrap" loc="wire/errors.go:11" role="consumer" at="wire/errors.go:12"/>"#,
    "</users>",
];

#[test]
fn impact_of_edit_lists_a_called_variable_s_callers_once_among_its_users() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            (
                "hook/hook.go",
                "package hook\n\nvar onDone func()\n\nfunc Set(f func()) {\n\tonDone = f\n}\n\nfunc Fire() {\n\t\
                 onDone()\n}\n",
            ),
        ],
        &[(
            "impact_of_edit",
            json!({"file": "hook/hook.go", "start_line": 3, "end_line": 3}),
        )],
    );

    assert_eq!(
        answers,
        [lines(&[
            r#"<impact of="hook/hook.go" start_line="3" end_line="3" symbols="1">"#,
            concat!(
                r#"<symbol name="onDone" qn="hook.onDone" kind="variable" lines="3-3" callers="1" users="2" "#,
                r#"callees="0" reach="2" reach_files="0" reach_band="low">"#,
            ),
            r#"<users shown="2" of="2">"#,
            r#"<ref name="Set" qn="hook.Set" loc="hook/hook.go:5" role="consumer" at="hook/hook.go:6"/>"#,
            r#"<ref name="Fire" qn="hook.Fire" loc="hook/hook.go:9" role="consumer" at="hook/hook.go:10"/>"#,
            "</users>",
            "</symbol>",
            r#"<includes count="0"/>"#,
            r#"<included-by count="0"/>"#,
            "</impact>",
        ])]
    );
}

#[test]
fn find_callees_binds_nothing_through_a_package_level_cycle() {
    const RUN: &str = "\nfunc run() {\n\ta.Set()\n}\n";

    let one_file = format!("package x\n\nvar a = b\nvar b = a\n{RUN}");
    let self_read = format!("package x\n\nvar a = a\n{RUN}");
    let first_of_two = format!("package x\n\nvar a = b\n{RUN}");
    let three_steps = format!("package x\n\nvar a = b\nvar b = c\nvar c = a\n{RUN}");
    let element = format!("package x\n\nvar a = b[0]\nvar b = a\n{RUN}");
    let trees: [&[(&str, &str)]; 5] = [
        &[("x.go", &one_file)],
        &[("x.go", &self_read)],
        &[
            ("x.go", &first_of_two),
            ("y.go", "package x\n\nvar b = a\n"),
        ],
        &[("x.go", &three_steps)],
        &[("x.go", &element)],
    ];
    let namesake = (
        "u.go",
        "package x\n\ntype U struct{}\n\nfunc (U) Set() {}\n",
    );

    for files in trees {
        let tree = tree(
            &[
                files,
                &[("go.mod", "module example.com/x\n\ngo 1.26\n"), namesake],
            ]
            .concat(),
        );
        let mut client = Client::start(&[]);
        client.index(tree.path());

        let answer = client.answer("find_callees", &json!({"name": "run"}));

        assert!(
            answer.contains(r#"count="0" unresolved="1""#),
            "{files:?} answered {answer}"
        );
    }
}

#[test]
fn find_callees_binds_nothing_through_a_cycle_across_packages() {
    let tree = tree(&[
        ("go.mod", "module example.com/x\n\ngo 1.26\n"),
        (
            "p/p.go",
            "package p\n\nimport \"example.com/x/q\"\n\nvar A = q.B\n\nfunc run() {\n\tA.Set()\n}\n",
        ),
        (
            "q/q.go",
            "package q\n\nimport \"example.com/x/p\"\n\nvar B = p.A\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_callees", &json!({"name": "run"}));

    assert!(answer.contains(r#"count="0" unresolved="1""#), "{answer}");
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
                r#"<callees of="Gret" matches="0" one_edit="1">"#,
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
            client.answer(
                "find_callees",
                &json!({"name": "helper", "min_confidence": "certain"})
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
                    r#"unresolved="1">no callees at this confidence floor - 1 callee binds below it, and "#,
                    r#"min_confidence "any" lists it</match>"#,
                ),
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="helper" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="helper" qn="lib.helper" loc="helper.go:3" count="0">no callees - no "#,
                    "call in its body binds a definition the graph holds; a call into a library or a builtin, ",
                    "through a function value, or by dynamic dispatch binds none</match>",
                ),
                "</callees>",
            ]),
        ],
        "the floor is named only where it dropped a callee"
    );
}

#[test]
fn find_references_counts_the_sites_below_the_floor_and_names_it_only_where_it_dropped_some() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
            (
                "helper.go",
                "package lib\n\nvar limit = 3\n\nfunc helper() {}\n",
            ),
            (
                "run.go",
                "package lib\n\nfunc Run() int {\n\thelper()\n\treturn limit\n}\n",
            ),
            ("stop.go", "package lib\n\nfunc Stop() {\n\thelper()\n}\n"),
        ],
        &[
            (
                "find_references",
                json!({"name": "helper", "min_confidence": "certain"}),
            ),
            (
                "find_references",
                json!({"name": "limit", "min_confidence": "certain", "access": "write"}),
            ),
            (
                "find_references",
                json!({"name": "Stop", "min_confidence": "certain"}),
            ),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="helper" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="helper" qn="lib.helper" loc="helper.go:5" kind="function" count="0" "#,
                    r#"below_confidence="2">no recorded site at this confidence floor - 2 sites bind below it, "#,
                    r#"and min_confidence "any" lists them</match>"#,
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="limit" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="limit" qn="lib.limit" loc="helper.go:3" kind="variable" count="0" "#,
                    r#"below_confidence="1">no recorded site at this confidence floor - 1 site binds below it, and "#,
                    r#"min_confidence "any" without access lists it</match>"#,
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="Stop" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Stop" qn="lib.Stop" loc="stop.go:3" kind="function" count="0">no "#,
                    "recorded site - resolved calls can omit dynamic or unresolved ones, and uses other than calls ",
                    "are recorded only where the graph records them, so this does not show that nothing uses the ",
                    "symbol</match>",
                ),
                "</references>",
            ]),
        ]
    );
}

#[test]
fn find_callers_names_the_floor_only_where_it_dropped_callers() {
    let answers = answers_over_shared_method_names(&[
        (
            "find_callers",
            json!({"name": "CancelToken::cancel", "min_confidence": "certain"}),
        ),
        (
            "find_callers",
            json!({"name": "CancelToken", "min_confidence": "certain"}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callers of="CancelToken::cancel" matches="1">"#,
                &format!(
                    "<match lang=\"rust\" of=\"cancel\" qn=\"CancelToken::cancel\" loc=\"src/cancel.rs:4\" \
                     count=\"0\" reach_band=\"low\" ambiguous=\"1\">{NO_CALL_BINDS}</match>"
                ),
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="CancelToken" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="CancelToken" loc="src/cancel.rs:1" count="0" reach_band="low">no "#,
                    "callers - the graph holds no call that names this struct; find_references lists its recorded ",
                    "uses</match>",
                ),
                "</callers>",
            ]),
        ]
    );
}

/// A Go package whose `Run` and `Stop` each call `helper`, and whose `Start` calls `Stop`, each from another file, a
/// call that binds `likely`.
const HELPER_CALLERS: [(&str, &str); 5] = [
    ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
    ("helper.go", "package lib\n\nfunc helper() {}\n"),
    ("run.go", "package lib\n\nfunc Run() {\n\thelper()\n}\n"),
    ("start.go", "package lib\n\nfunc Start() {\n\tStop()\n}\n"),
    ("stop.go", "package lib\n\nfunc Stop() {\n\thelper()\n}\n"),
];

#[test]
fn find_call_path_counts_the_callers_below_the_floor_that_it_did_not_follow() {
    let answers = answers_over(
        &HELPER_CALLERS,
        &[
            (
                "find_call_path",
                json!({"from": "Run", "to": "helper", "min_confidence": "certain"}),
            ),
            (
                "find_call_path",
                json!({"from": "Run", "to": "Stop", "min_confidence": "certain"}),
            ),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<call_path from="Run" to="helper" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="lib.Run" to="lib.helper" hops="0" max_hops="8" below_confidence="2" "#,
                    r#"found="false">no call chain within 8 calls at this confidence floor - 2 callers bind below "#,
                    r#"it, and min_confidence "any" follows them</match>"#,
                ),
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="Run" to="Stop" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="lib.Run" to="lib.Stop" hops="0" max_hops="8" below_confidence="1" "#,
                    r#"found="false">no call chain within 8 calls at this confidence floor - 1 caller binds below "#,
                    r#"it, and min_confidence "any" follows it</match>"#,
                ),
                "</call_path>",
            ]),
        ]
    );
}

#[test]
fn find_call_path_names_the_floor_only_where_it_dropped_callers() {
    let answers = answers_over(
        &HELPER_CALLERS,
        &[(
            "find_call_path",
            json!({"from": "helper", "to": "Run", "min_confidence": "certain"}),
        )],
    );

    assert_eq!(
        answers,
        [lines(&[
            r#"<call_path from="helper" to="Run" matches="1">"#,
            concat!(
                r#"<match lang="go" from="lib.helper" to="lib.Run" hops="0" max_hops="8" found="false">no call "#,
                "chain within 8 calls - the graph misses dynamic and unresolved calls, so this does not show that ",
                "the code holds none</match>",
            ),
            "</call_path>",
        ])]
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
fn find_unused_that_judged_everything_and_found_it_reached_points_at_no_exclusion() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "pkg/a.go",
            "package pkg\n\nfunc A() {\n\tb()\n}\n\nfunc b() {}\n",
        ),
        ("pkg/c.go", "package pkg\n\nfunc C() {\n\tA()\n}\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({"path_prefix": "pkg/a.go"}));

    assert_eq!(
        answer,
        concat!(
            r#"<unused path_prefix="pkg/a.go" count="0" live="2">no unused definition - every definition was "#,
            "judged, and a call or a use reaches each</unused>",
        )
    );
}

#[test]
fn find_unused_judges_go_fields_by_their_uses_and_counts_those_a_struct_tag_hands_to_reflection() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "main.go",
            "package main\n\ntype Seed struct {\n\tName string\n\tCount int `json:\"count\"`\n\t\
             Skipped int `json:\"-\"`\n\tSpare int\n}\n\nfunc main() {\n\ts := Seed{Name: \"a\"}\n\t_ = s.Name\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_unused", &json!({})),
        lines(&[
            r#"<unused count="2" live="2">"#,
            r#"<excluded entry="1" reflected="1"/>"#,
            r#"<symbol name="Skipped" qn="main.Seed.Skipped" kind="field" lang="go" loc="main.go:6"/>"#,
            r#"<symbol name="Spare" qn="main.Seed.Spare" kind="field" lang="go" loc="main.go:7"/>"#,
            "</unused>",
        ])
    );
}

#[test]
fn find_unused_counts_a_go_field_that_only_a_decoder_writes_as_reflected() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "live/live.go",
            "package live\n\nimport \"encoding/json\"\n\ntype Info struct {\n\tIssued string\n\tToken  string\n}\n\n\
             func Parse(data []byte) string {\n\tvar info Info\n\t_ = json.Unmarshal(data, &info)\n\t\
             return info.Token\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_unused", &json!({})),
        lines(&[
            r#"<unused count="1" live="2">"#,
            r#"<excluded unobservable="1" reflected="1"/>"#,
            r#"<symbol name="Parse" qn="live.Parse" kind="function" lang="go" loc="live/live.go:10-14"/>"#,
            "</unused>",
        ])
    );
}

#[test]
fn find_unused_lists_a_go_field_a_marshal_call_meets_only_through_a_func_type() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "b/a.go",
            "package b\n\nimport \"encoding/json\"\n\ntype S struct {\n\tName string\n}\n\n\
             type H func() S\n\nfunc run(h H) ([]byte, error) { return json.Marshal(h) }\n",
        ),
        (
            "b/c.go",
            "package b\n\ntype C struct {\n\tTag string\n}\n\ntype K func() C\n",
        ),
        (
            "b/d.go",
            "package b\n\nimport \"encoding/json\"\n\ntype D struct {\n\tKept string\n}\n\n\
             func show(d D) ([]byte, error) { return json.Marshal(d) }\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_unused", &json!({})),
        lines(&[
            r#"<unused count="5" live="4">"#,
            r#"<excluded reflected="1"/>"#,
            r#"<symbol name="Name" qn="b.S.Name" kind="field" lang="go" loc="b/a.go:6"/>"#,
            r#"<symbol name="run" qn="b.run" kind="function" lang="go" loc="b/a.go:11"/>"#,
            r#"<symbol name="Tag" qn="b.C.Tag" kind="field" lang="go" loc="b/c.go:4"/>"#,
            r#"<symbol name="K" qn="b.K" kind="defined-type" lang="go" loc="b/c.go:7"/>"#,
            r#"<symbol name="show" qn="b.show" kind="function" lang="go" loc="b/d.go:9"/>"#,
            "</unused>",
        ])
    );
}

#[test]
fn find_unused_lists_a_go_field_a_marshal_call_meets_only_through_a_channel() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "b/a.go",
            "package b\n\nimport \"encoding/json\"\n\ntype S struct {\n\tName string\n}\n\n\
             func run(h chan S) ([]byte, error) { return json.Marshal(h) }\n",
        ),
        (
            "b/c.go",
            "package b\n\ntype C struct {\n\tTag string\n}\n\nvar Keep chan C\n",
        ),
        (
            "b/d.go",
            "package b\n\nimport \"encoding/json\"\n\ntype D struct {\n\tKept string\n}\n\n\
             func show(d []D) ([]byte, error) { return json.Marshal(d) }\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_unused", &json!({})),
        lines(&[
            r#"<unused count="5" live="3">"#,
            r#"<excluded reflected="1"/>"#,
            r#"<symbol name="Name" qn="b.S.Name" kind="field" lang="go" loc="b/a.go:6"/>"#,
            r#"<symbol name="run" qn="b.run" kind="function" lang="go" loc="b/a.go:9"/>"#,
            r#"<symbol name="Tag" qn="b.C.Tag" kind="field" lang="go" loc="b/c.go:4"/>"#,
            r#"<symbol name="Keep" qn="b.Keep" kind="variable" lang="go" loc="b/c.go:7"/>"#,
            r#"<symbol name="show" qn="b.show" kind="function" lang="go" loc="b/d.go:9"/>"#,
            "</unused>",
        ])
    );
}

#[test]
fn a_go_range_over_a_channel_reads_its_value_as_the_element() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "b/a.go",
            "package b\n\nimport \"encoding/json\"\n\ntype S struct {\n\tName string\n}\n\n\
             func (s S) Run() {}\n\n\
             func run(h chan S) {\n\tfor v := range h {\n\t\t_, _ = json.Marshal(v)\n\t\tv.Run()\n\t}\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_unused", &json!({})),
        lines(&[
            r#"<unused count="1" live="2">"#,
            r#"<excluded reflected="1"/>"#,
            r#"<symbol name="run" qn="b.run" kind="function" lang="go" loc="b/a.go:11-16"/>"#,
            "</unused>",
        ])
    );
    let callers = client.answer("find_callers", &json!({"name": "b.S.Run"}));
    assert!(callers.contains(r#"qn="b.run""#), "{callers}");
}

#[test]
fn find_unused_counts_the_exports_a_go_plugin_s_host_looks_up_by_name_as_reflected() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "lib/adder/adder.go",
            "package main\n\nvar Selector = 1\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n\nfunc spare() {}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_unused", &json!({})),
        lines(&[
            r#"<unused count="1" live="0">"#,
            r#"<excluded reflected="2"/>"#,
            r#"<symbol name="spare" qn="main.spare" kind="function" lang="go" loc="lib/adder/adder.go:9"/>"#,
            "</unused>",
        ])
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
fn coverage_counts_a_call_through_a_type_over_an_import_as_external() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module example.com/b\n\ngo 1.22\n"),
        (
            "b.go",
            "package b\n\nimport \"net/http\"\n\ntype H http.Handler\n\n\
             func viaDefinedExternal(h H) {\n\th.ServeHTTP(nil, nil)\n}\n",
        ),
        (
            "held.go",
            "package b\n\ntype S struct{}\n\nfunc (S) M() {}\n\ntype T S\n\n\
             func viaDefinedHeld(t T) {\n\tt.M()\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("index_stats", &json!({"coverage": true}));

    assert!(
        answer.contains(
            r#"<coverage lang="go" sites="2" bound="0" edges="0" external_import="1" not_admitted="1" "#
        ),
        "the imported type's member lies outside the graph, and the held type refuses its own: {answer}"
    );
}

#[test]
fn coverage_counts_a_read_past_its_bound_under_a_reason_of_its_own() {
    let tree = tree(&[
        ("go.mod", "module example.com/x\n\ngo 1.26\n"),
        (
            "x.go",
            "package x\n\nvar a = b\nvar b = a\n\nfunc run() {\n\ta.Set()\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("index_stats", &json!({"coverage": true}));

    assert!(
        answer.contains(r#"<coverage lang="go" sites="1" bound="0" edges="0" past_bound="1"/>"#),
        "the graph stopped reading, which says nothing of what the call reaches: {answer}"
    );
}

fn go_coverage(source: &str) -> String {
    let tree = tree(&[
        ("go.mod", "module example.com/a\n\ngo 1.26\n"),
        ("a.go", source),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());
    client.answer("index_stats", &json!({"coverage": true}))
}

#[test]
fn coverage_counts_a_read_that_closes_a_cycle_of_aliases_as_a_stopped_read() {
    for source in [
        "package a\n\ntype A = B\ntype B = A\n\nfunc run(v A) { v.M() }\n",
        "package a\n\ntype A = A\n\nfunc run(v A) { v.M() }\n",
        "package a\n\ntype T = A\ntype A = B\ntype B = A\n\nfunc run(v T) { v.M() }\n",
        "package a\n\ntype A = B\ntype B = A\ntype S struct {\n\tA\n}\n\nfunc run(v S) { v.M() }\n",
        "package a\n\ntype A = B\ntype B = A\ntype T = A\ntype U = T\ntype S struct {\n\tA\n\tU\n}\n\n\
         func run(v S) { v.M() }\n",
        "package a\n\ntype A = B\ntype B = A\nfunc g() {}\ntype S struct {\n\tA\n\tg.x\n}\n\n\
         func run(v S) { v.M() }\n",
    ] {
        let answer = go_coverage(source);

        assert!(
            answer
                .contains(r#"<coverage lang="go" sites="1" bound="0" edges="0" past_bound="1"/>"#),
            "the cycle gives the read no type to judge the call by: {source}\n{answer}"
        );
    }
}

#[test]
fn coverage_keeps_a_cycle_a_defined_type_breaks_and_aliases_that_meet_as_not_admitted() {
    for source in [
        "package a\n\ntype Link = Node\ntype Node struct {\n\t*Link\n}\n\nfunc run(v Link) { v.M() }\n",
        "package a\n\ntype Top = Mid\ntype Mid = Base\ntype Side = Mid\ntype Base struct{}\n\
         type D struct {\n\tTop\n\tSide\n}\n\nfunc run(v D) { v.M() }\n",
    ] {
        let answer = go_coverage(source);

        assert!(
            answer.contains(r#"not_admitted="1" through_receiver="1""#)
                && !answer.contains("past_bound"),
            "the type holds no member of the name: {source}\n{answer}"
        );
    }
}

#[test]
fn coverage_counts_a_cycle_of_aliases_beside_an_import_as_external() {
    let answer = go_coverage(
        "package a\n\nimport \"net/http\"\n\ntype A = B\ntype B = A\n\
         type S struct {\n\tA\n\thttp.Client\n}\n\nfunc run(v S) { v.M() }\n",
    );

    assert!(
        answer.contains(r#"external_import="1""#) && !answer.contains("past_bound"),
        "the import may give the member: {answer}"
    );
}

#[test]
fn a_selector_past_a_cycle_of_aliases_binds_no_namesake() {
    let answer = go_coverage(
        "package a\n\ntype A = B\ntype B = A\ntype U struct{}\n\nfunc (U) M() {}\n\n\
         func run(v A) { v.f.M() }\n",
    );

    assert!(
        answer.contains(r#"<coverage lang="go" sites="1" bound="0" edges="0" past_bound="1"/>"#),
        "a selector the cycle stops is left to no tier that reads no type: {answer}"
    );
}

#[test]
fn find_callees_binds_a_call_through_a_field_embedded_beside_an_import() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module example.com/b\n\ngo 1.22\n"),
        (
            "b.go",
            "package b\n\nimport \"net/http\"\n\ntype Base struct{}\n\nfunc (Base) M() {}\n\n\
             type Outer struct {\n\thttp.Client\n\tBase\n}\n\n\
             type Plain struct {\n\tBase\n}\n\n\
             func run(o Outer) { o.Base.M() }\n\nfunc ctl(p Plain) { p.Base.M() }\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    for caller in ["run", "ctl"] {
        let answer = client.answer("find_callees", &json!({"name": caller}));

        assert!(
            answer.contains(r#"count="1""#) && answer.contains("b.Base.M"),
            "{caller} binds the embedded field's method: {answer}"
        );
    }
}

#[test]
fn find_callees_takes_no_member_from_an_interface_a_file_asserts() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module example.com/a\n\ngo 1.22\n"),
        (
            "a.go",
            "package a\n\ntype I interface{ M() }\n\ntype Deep struct{}\n\nfunc (Deep) M() {}\n\n\
             type Mid struct{ Deep }\n\ntype T struct{ Mid }\n\nvar _ I = (*T)(nil)\n\n\
             func run(t T) { t.M() }\n\ntype U struct{ Mid }\n\nfunc ctl(u U) { u.M() }\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    for caller in ["run", "ctl"] {
        let answer = client.answer("find_callees", &json!({"name": caller}));

        assert!(
            answer.contains(r#"count="1""#) && answer.contains("a.Deep.M"),
            "{caller} binds the method Go selects: {answer}"
        );
    }
}

#[test]
fn a_selector_reads_no_embedded_field_from_an_interface_a_file_asserts() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        ("base/b.go", "package base\n\ntype Named interface{ M() }\n"),
        (
            "a/a.go",
            "package a\n\nimport \"m/base\"\n\n\
             type Mid struct{ Named string }\n\ntype T struct{ Mid }\n\n\
             func (T) M() {}\n\nvar _ base.Named = T{}\n\n\
             func read(t T) string { return t.Named }\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let field = client.answer("find_references", &json!({"name": "a.Mid.Named"}));
    let interface = client.answer("find_references", &json!({"name": "base.Named"}));

    assert!(
        field.contains("a.read"),
        "the promoted field answers the read: {field}"
    );
    assert!(
        !interface.contains("a.read"),
        "an assertion names no field: {interface}"
    );
}

#[test]
fn coverage_reads_no_import_from_an_interface_a_file_asserts() {
    let answer = go_coverage(
        "package a\n\nimport \"io\"\n\ntype T struct{}\n\nvar _ io.Closer = T{}\n\n\
         func run(v T) { v.Close() }\n",
    );

    assert!(
        !answer.contains("external_import"),
        "an assertion embeds no imported type: {answer}"
    );
}

#[test]
fn find_implementations_keeps_a_type_asserted_where_it_is_declared_beside_a_namesake() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        (
            "a/a.go",
            "package a\n\ntype I interface{ M() }\n\ntype T struct{}\n\nfunc (T) M() {}\n",
        ),
        (
            "a/a_test.go",
            "package a_test\n\nimport \"m/a\"\n\ntype T struct{}\n\nfunc (T) M() {}\n\nvar _ a.I = T{}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_implementations", &json!({"name": "a.I"}));

    assert!(
        answer.contains("a/a_test.go") && !answer.contains("a/a.go\""),
        "the asserting file's own type implements the interface: {answer}"
    );
}

/// The answer to `find_implementations` of `Tr` over a crate whose `src/tr.rs` declares the trait, `src/t.rs` the types
/// `T` and `E`, `src/other.rs` a namesake `T` and `src/alias.rs` an associated type `Assoc`, with `implementing` as
/// `src/i.rs` and `src/nested.rs` re-exporting what `src/nested/inner.rs` declares and holding an inline module.
fn implementors_of_a_trait_a_third_file_implements(implementing: &str) -> String {
    implementors_over_a_third_file(implementing, &json!({"name": "Tr"}))
}

fn implementors_over_a_third_file(implementing: &str, arguments: &Value) -> String {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "src/lib.rs",
            "mod alias;\nmod i;\nmod nested;\nmod other;\nmod t;\nmod tr;\n",
        ),
        ("src/tr.rs", "pub trait Tr {\n    fn start(&self) {}\n}\n"),
        ("src/t.rs", "pub struct T;\npub enum E {}\n"),
        ("src/other.rs", "pub struct T;\npub struct Vec;\n"),
        (
            "src/alias.rs",
            "pub struct Holder;\n\npub trait Has {\n    type Assoc;\n}\n\nimpl Has for Holder {\n    \
             type Assoc = u8;\n}\n",
        ),
        (
            "src/nested.rs",
            "mod inner;\npub use inner::{Below, Deep};\n\npub mod inline {\n    pub struct Held;\n}\n",
        ),
        (
            "src/nested/inner.rs",
            "pub struct Below;\npub enum Deep {}\n",
        ),
        ("src/i.rs", implementing),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    client.answer("find_implementations", arguments)
}

#[test]
fn find_implementations_lists_a_rust_impl_in_a_file_that_does_not_declare_its_type() {
    for (implementing, (name, file)) in [
        (
            "use crate::t::T;\nuse crate::tr::Tr;\n\nimpl Tr for T {}\n",
            ("T", "src/t.rs"),
        ),
        (
            "use crate::t::T as Renamed;\nuse crate::tr::Tr;\n\nimpl Tr for &Renamed {}\n",
            ("T", "src/t.rs"),
        ),
        ("impl crate::tr::Tr for crate::t::E {}\n", ("E", "src/t.rs")),
        (
            "use crate::t::*;\nuse crate::tr::*;\n\nimpl Tr for T {}\n",
            ("T", "src/t.rs"),
        ),
        (
            "use crate::other::T;\nuse crate::tr::Tr;\n\nimpl Tr for T {}\n",
            ("T", "src/other.rs"),
        ),
        (
            "mod inside {\n    use crate::t::T;\n    use crate::tr::Tr;\n\n    impl Tr for T {}\n}\n",
            ("T", "src/t.rs"),
        ),
        (
            "use crate::nested::Below;\nuse crate::tr::Tr;\n\nimpl Tr for Below {}\n",
            ("Below", "src/nested/inner.rs"),
        ),
        (
            "use crate::nested::Deep;\nuse crate::tr::Tr;\n\nimpl Tr for Deep {}\n",
            ("Deep", "src/nested/inner.rs"),
        ),
        (
            "use crate::nested::inline::Held;\nuse crate::tr::Tr;\n\nimpl Tr for Held {}\n",
            ("Held", "src/nested.rs"),
        ),
    ] {
        let answer = implementors_of_a_trait_a_third_file_implements(implementing);
        let (named, located) = (format!("name=\"{name}\""), format!("loc=\"{file}:"));

        assert!(
            answer.contains("count=\"1\"") && answer.contains(&named) && answer.contains(&located),
            "{implementing} implements the trait for {name} of {file}: {answer}"
        );
    }
}

#[test]
fn find_implementations_lists_no_type_a_rust_impl_does_not_reach() {
    for implementing in [
        "use crate::tr::Tr;\n\nimpl Tr for u32 {}\n",
        "use crate::t::*;\nuse crate::tr::Tr;\n\nimpl<T> Tr for T {}\n",
        "use crate::tr::Tr;\n\nimpl<V> Tr for Vec<V> {}\n",
        "use outside::Held;\nuse crate::tr::Tr;\n\nimpl Tr for Held {}\n",
        "use crate::nested::Below;\n\nmod inside {\n    use crate::tr::Tr;\n\n    impl Tr for super::Below {}\n}\n",
        "use crate::tr::Tr;\n\nimpl Tr for std::path::T {}\n",
        "use crate::alias::Assoc;\nuse crate::tr::Tr;\n\nimpl Tr for Assoc {}\n",
        "use crate::t::T;\n\nimpl outside::Tr for T {}\n",
        "use crate::t::T;\n\nimpl Tr for T {}\n",
        "use crate::t::T;\n\nimpl crate::nested::inline::Tr for T {}\n",
        "use crate::tr::Tr;\n\npub struct Held;\n\nimpl Tr for crate::nested::inline::Held {}\n",
    ] {
        let answer = implementors_of_a_trait_a_third_file_implements(implementing);

        assert!(
            answer.contains("count=\"0\""),
            "{implementing} implements the trait for no type of the tree: {answer}"
        );
    }
}

#[test]
fn find_implementations_reads_a_trait_a_rust_file_declares_past_a_glob_that_holds_a_namesake() {
    let implementing = "use crate::tr::*;\n\npub trait Tr {}\n\nimpl Tr for crate::t::T {}\n";

    let globbed = implementors_over_a_third_file(
        implementing,
        &json!({"name": "Tr", "path_prefix": "src/tr.rs"}),
    );
    let declared = implementors_over_a_third_file(
        implementing,
        &json!({"name": "Tr", "path_prefix": "src/i.rs"}),
    );

    assert!(
        globbed.contains("count=\"0\"") && declared.contains("count=\"1\""),
        "the file's own item shadows the glob's: {globbed} {declared}"
    );
}

#[test]
fn find_implementations_lists_no_type_past_the_crates_a_rust_impl_is_built_with() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/lib.rs",
            "pub mod inline {\n    pub trait Tr {}\n}\n\npub trait Own {}\n\n\
             impl Own for crate::inline::Far {}\n",
        ),
        ("crates/far/Cargo.toml", "[package]\nname = \"far\"\n"),
        ("crates/far/src/lib.rs", "pub struct Far;\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_implementations", &json!({"name": "Own"}));

    assert!(
        answer.contains("count=\"0\""),
        "the crate is not built with the one type of the name: {answer}"
    );
}

#[test]
fn find_unused_counts_a_method_of_an_interface_outside_the_graph_its_file_asserts_as_dispatched() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        (
            "a.go",
            "package a\n\nimport \"example.com/lib\"\n\ntype handler struct{}\n\n\
             func (*handler) Serve() {}\n\n\
             var _ lib.Server = (*handler)(nil)\n\n\
             type other struct{}\n\nfunc (*other) stale() {}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({}));

    assert!(
        !answer.contains("Serve") && answer.contains(r#"dispatched="1""#),
        "the assertion says what dispatches the method: {answer}"
    );
    assert!(
        answer.contains("stale"),
        "it says nothing of the file's other types: {answer}"
    );
}

#[test]
fn find_unused_counts_a_method_asserted_on_a_generic_interface_or_through_a_plain_conversion() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module a\n\ngo 1.24\n"),
        (
            "a.go",
            "package a\n\nimport (\n\t\"example.com/ext\"\n\t\"io\"\n)\n\n\
             type Y []int\n\nfunc (Y) Close() error { return nil }\n\n\
             var _ io.Closer = (Y)(nil)\n\n\
             type X struct{}\n\nfunc (*X) Get() int { return 0 }\n\n\
             var _ ext.I[int] = (*X)(nil)\n\n\
             type Z []int\n\nfunc (Z) Write(p []byte) (int, error) { return 0, nil }\n\n\
             var _ io.Writer = Z(nil)\n\n\
             type Ctl struct{}\n\nfunc (*Ctl) Read(p []byte) (int, error) { return 0, nil }\n\n\
             var _ io.Reader = (*Ctl)(nil)\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({"languages": ["go"]}));

    assert!(
        answer.contains(r#"dispatched="4""#),
        "each assertion says what dispatches its method: {answer}"
    );
    for method in ["a.Y.Close", "a.X.Get", "a.Z.Write", "a.Ctl.Read"] {
        assert!(!answer.contains(method), "{method}: {answer}");
    }
}

#[test]
fn find_unused_counts_the_method_each_value_of_a_blank_spec_asserts() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module a\n\ngo 1.24\n"),
        (
            "a.go",
            "package a\n\nimport \"example.com/ext\"\n\n\
             type A struct{}\n\nfunc (A) Put() {}\n\n\
             type B struct{}\n\nfunc (B) Take() {}\n\n\
             var _, _ ext.Pair = A{}, B{}\n\n\
             type C struct{}\n\nfunc (C) Hold() {}\n\n\
             type D struct{}\n\nfunc (D) Keep() {}\n\n\
             var kept, _ ext.Named = C{}, D{}\n\n\
             type E struct{}\n\nfunc (E) Plain() {}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({"languages": ["go"]}));

    for asserted in ["a.A.Put", "a.B.Take", "a.D.Keep"] {
        assert!(!answer.contains(asserted), "{asserted}: {answer}");
    }
    for unread in ["a.C.Hold", "a.E.Plain"] {
        assert!(
            answer.contains(unread),
            "{unread}: a value given to a named variable asserts nothing: {answer}"
        );
    }
}

const GO_HANDLER: &str = "package a\n\nimport \"example.com/lib\"\n\ntype handler struct{}\n\n\
     func (*handler) Serve() {}\n";

#[test]
fn find_unused_counts_a_method_another_file_of_its_package_asserts_as_dispatched() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        ("a/a.go", GO_HANDLER),
        (
            "a/b.go",
            "package a\n\nimport \"example.com/lib\"\n\nvar _ lib.Server = (*handler)(nil)\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({}));

    assert!(
        !answer.contains("Serve") && answer.contains(r#"dispatched="1""#),
        "a package's files say the same thing: {answer}"
    );
}

#[test]
fn find_unused_lists_a_method_whose_namesake_type_in_another_package_is_asserted() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        ("a/a.go", GO_HANDLER),
        (
            "b/b.go",
            "package b\n\nimport \"example.com/lib\"\n\ntype handler struct{}\n\n\
             func (*handler) Serve() {}\n\n\
             var _ lib.Server = (*handler)(nil)\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({}));

    assert!(
        answer.contains(r#"loc="a/a.go:7""#) && answer.contains(r#"dispatched="1""#),
        "only b's method is the one its package asserts: {answer}"
    );
}

#[test]
fn find_unused_lists_a_method_whose_namesake_an_external_test_package_asserts() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        ("a/a.go", GO_HANDLER),
        (
            "a/a_test.go",
            "package a_test\n\nimport \"example.com/lib\"\n\ntype handler struct{}\n\n\
             func (*handler) Serve() {}\n\n\
             var _ lib.Server = (*handler)(nil)\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({}));

    assert!(
        answer.contains(r#"loc="a/a.go:7""#),
        "the external test package's handler is its own: {answer}"
    );
}

#[test]
fn find_unused_lists_a_method_no_interface_declares_whatever_its_type_is_named() {
    for name in ["Storage", "Store"] {
        let tree = tree(&[
            (".git/HEAD", "ref: refs/heads/main\n"),
            ("go.mod", "module x\n\ngo 1.22\n"),
            (
                "storage/s.go",
                "package storage\n\ntype Storage interface{ Read() }\n",
            ),
            (
                "impl/i.go",
                &format!(
                    "package impl\n\nimport \"x/storage\"\n\ntype {name} struct{{}}\n\n\
                     func (*{name}) Read()  {{}}\nfunc (*{name}) Extra() {{}}\n\n\
                     var _ storage.Storage = (*{name})(nil)\n"
                ),
            ),
            (
                "other/o.go",
                "package other\n\ntype Other struct{}\n\nfunc (*Other) Extra() {}\n",
            ),
        ]);
        let mut client = Client::start(&[]);
        client.index(tree.path());

        let answer = client.answer("find_unused", &json!({}));

        assert!(
            answer.contains(&format!(r#"qn="impl.{name}.Extra""#)),
            "the interface the assertion names declares no Extra: {answer}"
        );
        assert!(
            !answer.contains(r#"loc="impl/i.go:7""#),
            "it declares Read: {answer}"
        );
        assert!(
            answer.contains(r#"qn="storage.Storage.Read""#),
            "a struct is no second interface that declares Read: {answer}"
        );
        assert!(
            answer.contains(r#"qn="other.Other.Extra""#),
            "a struct's method counts for no interface: {answer}"
        );
        assert!(answer.contains(r#"dispatched="1""#), "{answer}");
    }
}

#[test]
fn find_unused_counts_a_method_a_library_interface_declares_as_dispatched() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        (
            "a/a.go",
            "package a\n\nimport \"sort\"\n\ntype L []int\n\n\
             func (l L) Len() int           { return len(l) }\n\
             func (l L) Less(i, j int) bool { return l[i] < l[j] }\n\
             func (l L) Swap(i, j int)      { l[i], l[j] = l[j], l[i] }\n\
             func (l L) Own()               {}\n\n\
             type E struct{ cause error }\n\n\
             func (e *E) Error() string { return \"e\" }\n\
             func (e *E) Unwrap() error { return e.cause }\n\n\
             type I interface{ Close() error }\n\n\
             func Run(l L) error {\n\tsort.Sort(l)\n\treturn &E{}\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({}));

    assert!(answer.contains(r#"dispatched="5""#), "{answer}");
    assert!(
        answer.contains(r#"qn="a.L.Own""#),
        "no interface declares Own: {answer}"
    );
    assert!(
        answer.contains(r#"qn="a.I.Close""#),
        "a library interface runs what implements it, not another interface's declaration: {answer}"
    );
}

#[test]
fn find_unused_lists_a_method_whose_namesake_embeds_a_type_outside_the_graph() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        (
            "a/a.go",
            "package a\n\nimport \"net/http\"\n\ntype T struct{ http.Server }\n\nfunc (*T) One() {}\n",
        ),
        (
            "b/b.go",
            "package b\n\ntype T struct{}\n\nfunc (*T) Two() {}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({}));

    assert!(
        answer.contains(r#"qn="b.T.Two""#) && !answer.contains(r#"qn="a.T.One""#),
        "only a's T embeds what the graph does not hold: {answer}"
    );
}

#[test]
fn find_unused_lists_a_method_whose_namesake_embeds_a_type_that_declares_it() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        (
            "a/a.go",
            "package a\n\ntype Base struct{}\n\nfunc (*Base) Run() {}\n\n\
             type T struct{ Base }\n\nfunc (*T) Run() {}\n",
        ),
        (
            "b/b.go",
            "package b\n\ntype Base struct{}\n\ntype T struct{ Base }\n\nfunc (*T) Run() {}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_unused", &json!({}));

    assert!(
        answer.contains(r#"qn="b.T.Run""#) && !answer.contains(r#"qn="a.T.Run""#),
        "only a's Base declares Run: {answer}"
    );
}

#[test]
fn find_unused_reads_what_an_asserted_interface_may_hold_past_the_graph() {
    for declared in [
        "type I interface{ lib.Fetcher }",
        "type Inner interface{ lib.Fetcher }\n\ntype I interface{ Inner }",
        "type I = lib.Fetcher",
        "type I lib.Fetcher",
    ] {
        let tree = tree(&[
            (".git/HEAD", "ref: refs/heads/main\n"),
            ("go.mod", "module m\n\ngo 1.22\n"),
            (
                "a/a.go",
                &format!(
                    "package a\n\nimport \"example.com/lib\"\n\n{declared}\n\ntype T struct{{}}\n\n\
                     func (*T) Fetch() {{}}\n\nvar _ I = (*T)(nil)\n"
                ),
            ),
        ]);
        let mut client = Client::start(&[]);
        client.index(tree.path());

        let answer = client.answer("find_unused", &json!({}));

        assert!(
            !answer.contains("a.T.Fetch") && answer.contains(r#"dispatched="1""#),
            "{declared} may declare Fetch: {answer}"
        );
    }
}

fn embedded_field_named_like_a_promoted_member() -> tempfile::TempDir {
    tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        (
            "base/b.go",
            "package base\n\ntype Config struct {\n\tConfig string\n\tName   string\n}\n\n\
             type Run func() int\n\n\
             type Plain struct{}\n\nfunc (Plain) Run() int { return 0 }\n\n\
             type Named interface {\n\tNamed() string\n}\n",
        ),
        (
            "a/c.go",
            "package a\n\nimport \"m/base\"\n\n\
             type C struct {\n\tbase.Config\n}\n\n\
             type Both struct {\n\tbase.Run\n\tbase.Plain\n}\n\n\
             type Embedding interface {\n\tbase.Named\n}\n\n\
             func use(c *C) *base.Config { return &c.Config }\n\n\
             func set(c *C, v base.Config) { c.Config = v }\n\n\
             func read(c *C) string { return c.Name }\n\n\
             func run(b Both) int { return b.Run() }\n\n\
             func name(e Embedding) string { return e.Named() }\n",
        ),
    ])
}

#[test]
fn find_references_reads_an_embedded_field_over_a_promoted_field_of_its_name() {
    let tree = embedded_field_named_like_a_promoted_member();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let promoted = client.answer("find_references", &json!({"name": "base.Config.Config"}));
    assert!(
        promoted.contains(r#"count="0""#) && !promoted.contains("<ref "),
        "the embedded field shadows the promoted one: {promoted}"
    );

    let control = client.answer("find_references", &json!({"name": "base.Config.Name"}));
    assert!(
        control.contains(r#"name="read""#),
        "a promoted field no embedded field shadows binds: {control}"
    );

    let named_type = client.answer("find_references", &json!({"name": "base.Config"}));
    for expected in [
        r#"name="use" qn="a.use" line="18" role="consumer" site="read""#,
        r#"name="set" qn="a.set" line="20" role="consumer" site="write""#,
    ] {
        assert!(
            named_type.contains(expected),
            "the type that names the field holds the use: {named_type}"
        );
    }
}

#[test]
fn find_callees_calls_no_type_through_an_embedded_field() {
    let tree = embedded_field_named_like_a_promoted_member();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_callees", &json!({"name": "a.run"}));

    assert!(
        answer.contains(r#"count="0""#) && !answer.contains("<ref "),
        "the embedded func field shadows Plain.Run, and its type is no callee: {answer}"
    );
}

#[test]
fn find_callees_reads_no_field_of_an_interface_embedded_in_an_interface() {
    let tree = embedded_field_named_like_a_promoted_member();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_callees", &json!({"name": "a.name"}));

    assert!(
        answer.contains("base.Named.Named"),
        "an interface holds no fields, so its embedded interface's method answers: {answer}"
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
                    "from_path_prefix with one's file and from_line with its line, picks one:",
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

/// The answers to `calls` over one C++ file and one Rust file, each defining a name twice: `ns::f` overloads, of which
/// the first calls `a` and the second `b`, and two `S::m` methods.
fn answers_over_overloads(calls: &[(&str, Value)]) -> Vec<String> {
    answers_over(
        &[
            (
                "o.cpp",
                "void a() {}\nvoid b() {}\nnamespace ns {\nvoid f(int x) { a(); }\nvoid f(double x) { b(); }\n}\n\
                 namespace p {\nstruct S {\nvoid m() {}\n};\n}\nnamespace q {\nstruct S {\nvoid m() {}\n};\n}\n",
            ),
            (
                "src/lib.rs",
                "pub mod a { pub struct S; impl S { pub fn m(&self) {} } }\n\
                 pub mod b { pub struct S; impl S { pub fn m(&self) {} } }\n",
            ),
        ],
        calls,
    )
}

#[test]
fn a_line_picks_one_of_several_symbols_of_a_name_in_one_file() {
    let answers = answers_over_overloads(&[
        (
            "find_callees",
            json!({"name": "ns::f", "path_prefix": "o.cpp"}),
        ),
        (
            "find_callees",
            json!({"name": "ns::f", "path_prefix": "o.cpp", "line": 5}),
        ),
        (
            "find_callees",
            json!({"name": "ns::f", "path_prefix": "o.cpp", "line": 3}),
        ),
        ("find_callees", json!({"name": "ns::f", "line": 5})),
        (
            "find_callers",
            json!({"name": "S::m", "languages": ["cpp"], "path_prefix": "o.cpp", "line": 14}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callees of="ns::f" matches="1">"#,
                concat!(
                    r#"<match lang="cpp" of="f" qn="ns::f" loc="o.cpp:4" count="1" unexpanded="1" "#,
                    r#"passed_over="1">"#,
                ),
                r#"<ref name="a" loc="o.cpp:1" role="consumer" hops="1" at="o.cpp:4"/>"#,
                r#"<candidate name="f" qn="ns::f" kind="function" loc="o.cpp:5" callers="0"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="ns::f" matches="1">"#,
                r#"<match lang="cpp" of="f" qn="ns::f" loc="o.cpp:5" count="1" unexpanded="1">"#,
                r#"<ref name="b" loc="o.cpp:2" role="consumer" hops="1" at="o.cpp:5"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="ns::f" matches="0">"#,
                "no definition of this name under path_prefix starts on line 3 - 2 are elsewhere:",
                r#"<candidate name="f" qn="ns::f" kind="function" loc="o.cpp:4" lang="cpp"/>"#,
                r#"<candidate name="f" qn="ns::f" kind="function" loc="o.cpp:5" lang="cpp"/>"#,
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="ns::f" matches="1">"#,
                r#"<match lang="cpp" of="f" qn="ns::f" loc="o.cpp:5" count="1" unexpanded="1">"#,
                r#"<ref name="b" loc="o.cpp:2" role="consumer" hops="1" at="o.cpp:5"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callers of="S::m" languages="cpp" matches="1">"#,
                concat!(
                    r#"<match lang="cpp" of="m" qn="S::m" loc="o.cpp:14" count="0" reach_band="low">"#,
                    "no callers - entry point, dead code, or dynamic dispatch</match>",
                ),
                "</callers>",
            ]),
        ]
    );
}

#[test]
fn a_line_picks_the_symbol_of_each_tool_that_selects_one() {
    let answers = answers_over_overloads(&[
        (
            "find_references",
            json!({"name": "ns::f", "path_prefix": "o.cpp", "line": 5}),
        ),
        (
            "find_implementations",
            json!({"name": "S", "languages": ["cpp"], "path_prefix": "o.cpp", "line": 13}),
        ),
        (
            "find_call_path",
            json!({"from": "ns::f", "from_path_prefix": "o.cpp", "from_line": 5, "to": "b", "to_line": 2}),
        ),
        (
            "find_call_path",
            json!({"from": "ns::f", "from_path_prefix": "o.cpp", "from_line": 5, "to": "b", "to_line": 3}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="ns::f" matches="1">"#,
                concat!(
                    r#"<match lang="cpp" of="f" qn="ns::f" loc="o.cpp:5" kind="function" count="0">no recorded "#,
                    "site - resolved calls can omit dynamic or unresolved ones, and uses other than calls are ",
                    "recorded only where the graph records them, so this does not show that nothing uses the ",
                    "symbol</match>",
                ),
                "</references>",
            ]),
            concat!(
                r#"<implementations of="S" languages="cpp" count="0">the symbol is indexed but nothing "#,
                "implements or extends it</implementations>",
            )
            .to_owned(),
            lines(&[
                r#"<call_path from="ns::f" to="b" matches="1">"#,
                r#"<match lang="cpp" from="ns::f" to="b" hops="1" max_hops="8">"#,
                r#"<ref name="f" qn="ns::f" loc="o.cpp:5" role="consumer" call="o.cpp:5"/>"#,
                "</match>",
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="ns::f" to="b" matches="0">"#,
                "<to>",
                "no definition of this name starts on to_line 3 - 2 are elsewhere:",
                r#"<candidate name="b" kind="function" loc="o.cpp:2" lang="cpp"/>"#,
                r#"<candidate name="b" kind="namespace" loc="src/lib.rs:2" lang="rust"/>"#,
                "</to>",
                "</call_path>",
            ]),
        ]
    );
}

#[test]
fn a_name_several_symbols_of_one_file_end_with_names_the_line_that_picks_one() {
    let answers = answers_over_overloads(&[(
        "find_callers",
        json!({"name": "S::m", "languages": ["rust"]}),
    )]);

    assert_eq!(
        answers,
        [lines(&[
            r#"<callers of="S::m" languages="rust" matches="0">"#,
            concat!(
                "2 definitions end with this name and it picks none of them - a longer name, or path_prefix ",
                "with one's file and line with its line, picks one:",
            ),
            r#"<candidate name="m" qn="a::S::m" kind="method" loc="src/lib.rs:1" lang="rust"/>"#,
            r#"<candidate name="m" qn="b::S::m" kind="method" loc="src/lib.rs:2" lang="rust"/>"#,
            "</callers>",
        ])]
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
fn a_c_typedef_of_a_struct_to_its_own_name_answers_as_the_struct_alone() {
    let answers = answers_over(
        &[(
            "s.h",
            "typedef struct S { int x; } S;\nstruct Child : S {};\n",
        )],
        &[
            ("find_implementations", json!({"name": "S"})),
            ("find_symbols", json!({"query": "S"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<implementations of="S" count="1">"#,
                r#"<type name="Child" lang="cpp" loc="s.h:2"/>"#,
                "</implementations>",
            ]),
            lines(&[
                r#"<symbols of="S" count="3">"#,
                r#"<hit name="S" kind="struct" lang="cpp" loc="s.h:1"/>"#,
                r#"<hit name="x" qn="S::x" owner="S" kind="field" lang="cpp" loc="s.h:1" sig="int x"/>"#,
                r#"<hit name="Child" kind="struct" lang="cpp" loc="s.h:2"/>"#,
                "</symbols>",
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
fn a_name_several_types_end_with_lists_them_in_callers_and_in_implementations_alike() {
    let answers = answers_over(
        &[
            ("a.h", "namespace a { namespace ns { class Node {}; } }\n"),
            (
                "b.h",
                "namespace b { namespace ns {\nclass Node {};\nclass Leaf : public Node {};\n} }\n",
            ),
        ],
        &[
            ("find_callers", json!({"name": "ns::Node"})),
            ("find_implementations", json!({"name": "ns::Node"})),
            (
                "find_implementations",
                json!({"name": "ns::Node", "path_prefix": "b.h"}),
            ),
        ],
    );

    let listing = [
        concat!(
            "2 definitions end with this name and it picks none of them - a longer name, or path_prefix ",
            "with one's file and line with its line, picks one:",
        ),
        r#"<candidate name="Node" qn="a::ns::Node" kind="class" loc="a.h:1" lang="cpp"/>"#,
        r#"<candidate name="Node" qn="b::ns::Node" kind="class" loc="b.h:2" lang="cpp"/>"#,
    ]
    .join("\n");
    assert_eq!(
        answers,
        [
            format!("<callers of=\"ns::Node\" matches=\"0\">\n{listing}\n</callers>"),
            format!("<implementations of=\"ns::Node\" count=\"0\">\n{listing}\n</implementations>"),
            lines(&[
                r#"<implementations of="ns::Node" count="1">"#,
                r#"<type name="Leaf" qn="b::ns::Leaf" lang="cpp" loc="b.h:3"/>"#,
                "</implementations>",
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
                    "with one's file and line with its line, picks one:",
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
