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

const NOT_HELD: &str =
    "no indexed language claims this file, so no code graph holds it - read or grep it";

const GO_COUNTS: &str = "<note>the go graph counts the files whose import names this file's package, the same ones \
                         for every file of the package and none for a _test.go file</note>";

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
                LIMIT_CUT,
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
fn read_definitions_marks_each_line_it_cut_and_counts_them() {
    let tree = go_module();
    let first = format!("\ts := \"{}\"", "a".repeat(1092));
    let second = format!("\treturn s + \"{}\"", "b".repeat(1100));
    std::fs::create_dir_all(tree.path().join("long")).expect("the directory");
    std::fs::write(
        tree.path().join("long/long.go"),
        format!("package long\n\nfunc Long() string {{\n{first}\n\ts += \"c\"\n{second}\n}}\n"),
    )
    .expect("the file");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("read_definitions", &json!({"names": ["Long"]}));

    assert_eq!(
        answer,
        lines(&[
            r#"<definitions count="1">"#,
            r#"<name of="Long" total="1">"#,
            concat!(
                r#"<definition name="Long" qn="long.Long" kind="function" lang="go" loc="long/long.go:3-7" "#,
                r#"scope="other" cut_lines="2">"#,
            ),
            "     3\tfunc Long() string {",
            &format!("     4\t{} [... omitted 76 bytes]", &first[..1024]),
            "     5\t\ts += \"c\"",
            &format!("     6\t{} [... omitted 90 bytes]", &second[..1024]),
            "     7\t}",
            "</definition>",
            "</name>",
            "</definitions>",
        ])
    );
}

#[test]
fn read_definitions_names_the_lines_past_its_500_line_cap() {
    let tree = go_module();
    let mut source = String::from("package long\n\nfunc Long() {\n");
    for at in 0..600 {
        writeln!(source, "\t_ = {at}").expect("a string takes the text");
    }
    source.push_str("}\n");
    std::fs::create_dir_all(tree.path().join("long")).expect("the directory");
    std::fs::write(tree.path().join("long/long.go"), source).expect("the file");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("read_definitions", &json!({"names": ["Long"]}));

    assert!(
        answer.contains(r#"loc="long/long.go:3-604" scope="other" truncated="true">"#),
        "{answer}"
    );
    assert!(
        answer.contains(
            "\n   502\t\t_ = 498\na fixed cap cut this body - a read of lines 503-604 of its file lists the rest\n\
             </definition>"
        ),
        "{answer}"
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
fn read_definitions_ends_each_name_its_limit_cut_with_the_line_that_names_limit() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            ("a/a.go", "package a\n\nfunc Open() {}\n"),
            ("b/b.go", "package b\n\nfunc Open() {}\n"),
            ("c/c.go", "package c\n\nfunc Shut() {}\n"),
        ],
        &[
            (
                "read_definitions",
                json!({"names": ["Open", "Shut"], "limit": 1}),
            ),
            ("read_definitions", json!({"names": ["Open"], "limit": 2})),
        ],
    );

    let (cut, whole) = (&answers[0], &answers[1]);
    assert!(
        cut.contains(r#"<name of="Open" total="2" shown="1">"#),
        "{cut}"
    );
    assert_eq!(cut.matches("<definition ").count(), 2, "{cut}");
    assert!(
        cut.contains(&format!(
            "</definition>\n{LIMIT_CUT}\n</name>\n<name of=\"Shut\" total=\"1\">"
        )),
        "{cut}"
    );
    assert_eq!(
        cut.matches(LIMIT_CUT).count(),
        1,
        "the name the limit did not cut ends with no line: {cut}"
    );
    assert!(
        whole.contains(r#"<name of="Open" total="2">"#) && !whole.contains(LIMIT_CUT),
        "{whole}"
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
                concat!(
                    r#"<suggestion name="Greet" qn="pkg.Service.Greet" kind="method" lang="go" "#,
                    r#"loc="pkg/service.go:12-14"/>"#,
                ),
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
                r#"<symbol name="Service" qn="pkg.Service" kind="struct" lines="4"/>"#,
                r#"<symbol name="New" qn="pkg.New" kind="function" lines="7-9" sig="func New() *Service"/>"#,
                concat!(
                    r#"<symbol name="Greet" qn="pkg.Service.Greet" owner="Service" kind="method" lines="12-14" "#,
                    r#"sig="func (s *Service) Greet(name string) string"/>"#,
                ),
                "</outline>",
            ]),
            format!(r#"<outline of="README.md" symbols="0">{NOT_HELD}</outline>"#),
            lines(&[
                r#"<importers of="pkg/service.go" count="1">"#,
                GO_COUNTS,
                r#"<file path="cmd/app/main.go"/>"#,
                "</importers>",
            ]),
            format!(r#"<importers of="README.md" count="0">{NOT_HELD}</importers>"#),
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
            LIMIT_CUT,
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
    assert!(
        answer.ends_with(
            "/>\na fixed cap cut this list - no parameter or call lists the rest\n</implementations>"
        ),
        "{answer}"
    );
}

#[test]
fn each_symbol_tool_names_find_symbols_for_the_candidates_past_its_5() {
    let packages: Vec<(String, String)> = (0..7)
        .map(|at| {
            (
                format!("p{at}/p.go"),
                format!(
                    "package p{at}\n\ntype Shape interface{{}}\n\ntype Box struct{{}}\n\nvar _ Shape = Box{{}}\n\n\
                     func Parse() {{}}\n\nfunc Run() {{\n\tParse()\n}}\n"
                ),
            )
        })
        .collect();
    let mut files = vec![
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        ("z/z.go", "package z\n"),
    ];
    files.extend(
        packages
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str())),
    );
    let answers = answers_over(
        &files,
        &[
            ("find_callers", json!({"name": "Parse"})),
            ("find_callees", json!({"name": "Run"})),
            ("find_references", json!({"name": "Parse"})),
            ("find_implementations", json!({"name": "Shape"})),
            ("find_call_path", json!({"from": "Run", "to": "Parse"})),
            (
                "find_callers",
                json!({"name": "Parse", "path_prefix": "z/"}),
            ),
            ("find_symbols", json!({"query": "Parse", "limit": 7})),
            ("find_callers", json!({"name": "Parsx"})),
            ("find_symbols", json!({"query": "Parsx"})),
        ],
    );

    let cut = |name: &str, over: &str| {
        format!(
            "/>\na fixed cap cut these candidates - find_symbols of {name} with {over}limit 7 lists the rest\n"
        )
    };
    let go = r#"languages ["go"] and "#;
    for answer in &answers[..4] {
        assert!(answer.contains(r#"passed_over="6""#), "{answer}");
        assert!(answer.contains(&cut("this name", go)), "{answer}");
    }
    let path = &answers[4];
    for end in ["the from name", "the to name"] {
        assert!(path.contains(&cut(end, go)), "{path}");
    }
    let elsewhere = &answers[5];
    assert!(
        elsewhere.contains("7 are elsewhere:") && elsewhere.contains(&cut("this name", "")),
        "{elsewhere}"
    );
    assert!(
        answers[6].starts_with(r#"<symbols of="Parse" count="7">"#),
        "{}",
        answers[6]
    );
    for (near, tag) in answers[7..].iter().zip(["callers", "symbols"]) {
        assert!(
            near.contains(r#"one_edit="7">"#) && near.contains(r#"<suggestion name="Parse" "#),
            "{near}"
        );
        assert!(
            near.ends_with(&format!(
                "/>\na fixed cap cut these suggestions - find_symbols of a suggested name with limit 7 lists its \
                 namesakes, and no call lists the other names\n</{tag}>"
            )),
            "{near}"
        );
    }
}

/// The answers to `calls` over a tree that defines a function `Node` in seven Go packages and three Rust modules.
fn answers_over_node_namesakes(calls: &[(&str, Value)]) -> Vec<String> {
    let packages: Vec<(String, String)> = (0..7)
        .map(|at| {
            (
                format!("p{at}/p.go"),
                format!("package p{at}\n\nfunc Node() {{}}\n"),
            )
        })
        .collect();
    let mut files = vec![
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        ("z/z.go", "package z\n"),
        ("Cargo.toml", "[package]\nname = \"r\"\n"),
        (
            "src/lib.rs",
            "mod a {\n    pub fn Node() {}\n}\nmod b {\n    pub fn Node() {}\n}\nmod c {\n    pub fn Node() {}\n}\n",
        ),
    ];
    files.extend(
        packages
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str())),
    );
    answers_over(&files, calls)
}

#[test]
fn the_candidates_line_names_the_family_whose_namesakes_its_limit_counts() {
    let answers = answers_over_node_namesakes(&[
        ("find_callers", json!({"name": "Node"})),
        (
            "find_symbols",
            json!({"query": "Node", "languages": ["go"], "limit": 7}),
        ),
        ("find_symbols", json!({"query": "Node", "limit": 7})),
        (
            "find_callers",
            json!({"name": "Node", "path_prefix": "z/", "languages": ["go"]}),
        ),
        ("find_callers", json!({"name": "Node", "path_prefix": "z/"})),
        (
            "find_call_path",
            json!({"from": "Node", "from_path_prefix": "z/", "to": "Node", "languages": ["go"]}),
        ),
    ]);

    let cut = |way: &str| {
        format!(
            "/>\na fixed cap cut these candidates - find_symbols of this name {way} lists the rest\n"
        )
    };
    for (answer, way) in [
        (&answers[0], r#"with languages ["go"] and limit 7"#),
        (&answers[3], r#"with languages ["go"] and limit 7"#),
        (&answers[4], "with limit 10"),
        (&answers[5], r#"with languages ["go"] and limit 7"#),
    ] {
        assert!(answer.contains(&cut(way)), "{answer}");
    }
    let go_namesakes = |answer: &str| {
        (0..7)
            .filter(|at| answer.contains(&format!(r#"loc="p{at}/p.go:3""#)))
            .count()
    };
    assert_eq!(go_namesakes(&answers[1]), 7, "{}", answers[1]);
    assert!(go_namesakes(&answers[2]) < 7, "{}", answers[2]);
}

#[test]
fn the_suggestions_line_names_the_languages_a_filtered_call_searched() {
    let answers = answers_over_node_namesakes(&[
        ("find_callers", json!({"name": "Nodf", "languages": ["go"]})),
        ("find_callers", json!({"name": "Nodf"})),
        (
            "find_callers",
            json!({"name": "Nodf", "languages": ["rust", "go"]}),
        ),
    ]);

    for (near, way) in [
        (&answers[0], r#"with languages ["go"] and limit 7"#),
        (&answers[1], "with limit 10"),
        (&answers[2], r#"with languages ["rust", "go"] and limit 10"#),
    ] {
        assert!(
            near.contains(&format!(
                "/>\na fixed cap cut these suggestions - find_symbols of a suggested name {way} lists its \
                 namesakes, and no call lists the other names\n"
            )),
            "{near}"
        );
    }
}

#[test]
fn find_implementations_of_a_parent_outside_the_graph_says_no_call_lists_the_types_past_500() {
    let mut python = String::from("from lib import External\n");
    for at in 0..501 {
        let _ = writeln!(python, "class P{at}(External):\n    pass");
    }
    let tree = tree(&[("app.py", &python)]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_implementations", &json!({"name": "External"}));

    assert!(
        answer.starts_with(
            r#"<implementations of="External" count="500" total="501" outside_graph="true">"#
        ),
        "{answer}"
    );
    assert!(
        answer.ends_with(
            "/>\na fixed cap cut this list - no parameter or call lists the rest\n</implementations>"
        ),
        "{answer}"
    );
}

/// A Go module whose packages import each other in a chain: `e` imports `d`, ..., `b` imports `a`.
fn import_chain() -> tempfile::TempDir {
    tree(&[
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
    ])
}

#[test]
fn find_importers_counts_the_importers_it_did_not_follow_past_three_hops() {
    let chain = import_chain();
    let mut client = Client::start(&[]);
    client.index(chain.path());

    assert_eq!(
        client.answer("find_importers", &json!({"file": "a/a.go"})),
        lines(&[
            r#"<importers of="a/a.go" count="3" unexpanded="1">"#,
            GO_COUNTS,
            r#"<file path="b/b.go"/>"#,
            r#"<file path="c/c.go" depth="2"/>"#,
            r#"<file path="d/d.go" depth="3"/>"#,
            "</importers>",
        ])
    );
}

#[test]
fn find_importers_follows_the_importers_as_many_hops_as_asked() {
    let chain = import_chain();
    let mut client = Client::start(&[]);
    client.index(chain.path());

    assert_eq!(
        [
            client.answer("find_importers", &json!({"file": "a/a.go", "hops": 1})),
            client.answer("find_importers", &json!({"file": "a/a.go", "hops": 5})),
        ],
        [
            lines(&[
                r#"<importers of="a/a.go" count="1" unexpanded="1">"#,
                GO_COUNTS,
                r#"<file path="b/b.go"/>"#,
                "</importers>",
            ]),
            lines(&[
                r#"<importers of="a/a.go" count="4">"#,
                GO_COUNTS,
                r#"<file path="b/b.go"/>"#,
                r#"<file path="c/c.go" depth="2"/>"#,
                r#"<file path="d/d.go" depth="3"/>"#,
                r#"<file path="e/e.go" depth="4"/>"#,
                "</importers>",
            ]),
        ]
    );
}

const CPP_COUNTS: &str = "<note>the cpp graph counts the files whose #include names this file, and none through a \
                          #include of a macro</note>";

/// A tree of `a.h` and the `count` files `i000.c`, `i001.c`, ... that each include it.
fn included_by(count: usize) -> tempfile::TempDir {
    let includers: Vec<(String, &str)> = (0..count)
        .map(|at| (format!("i{at:03}.c"), "#include \"a.h\"\n"))
        .collect();
    let mut files = vec![("a.h", "int a(void);\n")];
    files.extend(includers.iter().map(|(path, text)| (path.as_str(), *text)));
    tree(&files)
}

#[test]
fn find_importers_lists_its_nearest_importers_up_to_the_limit_and_counts_every_one() {
    let tree = included_by(3);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        [
            client.answer("find_importers", &json!({"file": "a.h", "limit": 2})),
            client.answer("find_importers", &json!({"file": "a.h", "limit": 3})),
        ],
        [
            lines(&[
                r#"<importers of="a.h" count="3" shown="2">"#,
                CPP_COUNTS,
                r#"<file path="i000.c"/>"#,
                r#"<file path="i001.c"/>"#,
                "the limit cut this list - a higher limit lists the rest",
                "</importers>",
            ]),
            lines(&[
                r#"<importers of="a.h" count="3">"#,
                CPP_COUNTS,
                r#"<file path="i000.c"/>"#,
                r#"<file path="i001.c"/>"#,
                r#"<file path="i002.c"/>"#,
                "</importers>",
            ]),
        ]
    );
}

#[test]
fn find_importers_lists_200_importers_by_default() {
    let tree = included_by(201);
    let mut client = Client::start(&[]);
    client.index(tree.path());
    let answer = client.answer("find_importers", &json!({"file": "a.h"}));

    assert!(
        answer.starts_with(r#"<importers of="a.h" count="201" shown="200">"#),
        "{answer}"
    );
    assert_eq!(answer.matches("<file ").count(), 200, "{answer}");
    assert!(answer.contains(r#"path="i199.c""#), "{answer}");
    assert!(!answer.contains(r#"path="i200.c""#), "{answer}");
}

#[test]
fn find_importers_says_what_the_language_counts_as_an_importer() {
    const COUNTS: &str = "the rust graph counts the files whose bodiless mod declaration loads this file, so a \
                          crate root has none, and not a file that only names its items in a use declaration or a \
                          path";
    let crate_tree = tree(&[
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ("src/lib.rs", "mod render;\nmod server;\n"),
        ("src/render.rs", "pub fn draw() {}\n"),
        (
            "src/server.rs",
            "use crate::render::draw;\n\npub fn serve() { draw() }\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(crate_tree.path());

    assert_eq!(
        [
            client.answer("find_importers", &json!({"file": "src/render.rs"})),
            client.answer("find_importers", &json!({"file": "src/lib.rs"})),
        ],
        [
            lines(&[
                r#"<importers of="src/render.rs" count="1">"#,
                &format!("<note>{COUNTS}</note>"),
                r#"<file path="src/lib.rs"/>"#,
                "</importers>",
            ]),
            format!(r#"<importers of="src/lib.rs" count="0">{COUNTS}</importers>"#),
        ]
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
                concat!(
                    r#"<match lang="go" from="main.main" from_loc="cmd/app/main.go:5-8" to="pkg.Service.Greet" "#,
                    r#"to_loc="pkg/service.go:12-14" hops="2" max_hops="8">"#,
                ),
                r#"<ref name="main" qn="main.main" loc="cmd/app/main.go:5-8" role="entry" at="7"/>"#,
                r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10-12" role="consumer" at="11"/>"#,
                "</match>",
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="main" to="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="main.main" from_loc="cmd/app/main.go:5-8" to="pkg.Service.Greet" "#,
                    r#"to_loc="pkg/service.go:12-14" max_hops="1" "#,
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
                r#"<ref name="New" qn="pkg.New" lines="7-9" role="consumer" site="type" at="7,8"/>"#,
                r#"<ref name="Greet" qn="pkg.Service.Greet" lines="12-14" role="consumer" site="type" at="12"/>"#,
                "</file>",
                r#"<file path="cmd/app/main.go" sites="1">"#,
                r#"<ref name="run" qn="main.run" lines="10-12" role="consumer" site="type" at="10"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12-14" kind="method" "#,
                    r#"count="1" files="1" calls="1">"#,
                ),
                concat!(
                    "<note>a call through a value of the owner's type resolves only where the graph can name ",
                    "that type, so some of those calls can be missing</note>",
                ),
                r#"<file path="cmd/app/main.go" sites="1">"#,
                r#"<ref name="run" qn="main.run" lines="10-12" role="consumer" site="call" at="11"/>"#,
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
        r#"<ref name="Load" qn="pkg.Load" lines="7-11" role="consumer" site="write" at="8"/>"#,
        r#"<ref name="Load" qn="pkg.Load" lines="7-11" role="consumer" site="update" at="9"/>"#,
    ];
    assert_eq!(
        every,
        lines(&[
            r#"<references of="pkg.Config.Timeout" matches="1">"#,
            &open.replace("{count}", "5"),
            r#"<file path="cmd/main.go" sites="3">"#,
            r#"<ref name="main" qn="main.main" lines="5-10" role="entry" site="write" at="7"/>"#,
            r#"<ref name="main" qn="main.main" lines="5-10" role="entry" site="read" at="8,9"/>"#,
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
            r#"<ref name="main" qn="main.main" lines="5-10" role="entry" site="write" at="7"/>"#,
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
                    r#"<ref name="Read" qn="probe.Read" lines="9-11" role="consumer" site="read" at="10" "#,
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
                r#"<ref name="cfg" qn="probe.cfg" lines="7" role="consumer" site="write" at="7"/>"#,
                "</file>",
                r#"<file path="use.go" sites="1">"#,
                r#"<ref name="Read" qn="probe.Read" lines="9-11" role="consumer" site="read" at="10"/>"#,
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
                    r#"<ref name="Call" qn="probe.Call" lines="3-5" role="consumer" site="read" at="4" "#,
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
                r#"<ref name="use030" qn="pkg.use030" lines="3-13" role="consumer" site="read" "#,
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
            r#"<ref name="use000" qn="pkg.use000" lines="3-8" role="consumer" site="read" at="4,5,6,7"/>"#,
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
            r#"<ref name="use022" qn="pkg.use022" lines="3-8" role="consumer" site="read" at="4,5,6"/>"#,
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
fn find_references_sites_under_lists_the_sites_past_the_named_files_and_counts_what_it_left_out() {
    let readers: Vec<(String, String)> = (0..130)
        .map(|file| {
            (
                format!("pkg/use{file:03}.go"),
                format!("package pkg\n\nfunc use{file:03}(c Config) {{\n\t_ = c.Timeout\n\t_ = c.Timeout\n}}\n"),
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
    let answers = answers_over(
        &files,
        &[
            ("find_references", json!({"name": "pkg.Config.Timeout"})),
            (
                "find_references",
                json!({"name": "pkg.Config.Timeout", "sites_under": "pkg/use12"}),
            ),
        ],
    );

    assert!(
        !answers[0].contains("pkg/use120.go"),
        "the default answer names no file past its 100 sites and 50 more files: {}",
        answers[0]
    );
    let records: Vec<&str> = answers[1].lines().collect();
    assert_eq!(
        records[..2],
        [
            r#"<references of="pkg.Config.Timeout" sites_under="pkg/use12" matches="1">"#,
            concat!(
                r#"<match lang="go" of="Timeout" qn="pkg.Config.Timeout" loc="pkg/config.go:4" kind="field" "#,
                r#"count="260" shown="20" files="130" files_shown="10" excluded="240" files_excluded="120" "#,
                r#"reads="260">"#,
            ),
        ],
        "the counts cover every site, and excluded= and files_excluded= count what sites_under left out: {}",
        answers[1]
    );
    let listed: Vec<&str> = records
        .iter()
        .filter_map(|record| record.strip_prefix(r#"<file path=""#))
        .filter_map(|record| record.split('"').next())
        .collect();
    let under: Vec<String> = (120..130).map(|file| format!("pkg/use{file}.go")).collect();
    assert_eq!(
        listed, under,
        "only the files under sites_under are listed: {}",
        answers[1]
    );
    assert_eq!(
        records[2..5],
        [
            r#"<file path="pkg/use120.go" sites="2">"#,
            r#"<ref name="use120" qn="pkg.use120" lines="3-6" role="consumer" site="read" at="4,5"/>"#,
            "</file>",
        ],
        "each file under sites_under lists its sites: {}",
        answers[1]
    );
}

#[test]
fn find_references_sites_under_filters_the_file_scope_users_and_says_when_it_keeps_nothing() {
    let scripts: Vec<(String, &str)> = (0..51)
        .map(|at| (format!("bin/run{at:02}.sh"), ". lib/common.sh\nshared\n"))
        .collect();
    let mut files: Vec<(&str, &str)> = scripts
        .iter()
        .map(|(path, text)| (path.as_str(), *text))
        .collect();
    files.push(("lib/common.sh", "shared() {\n\techo shared\n}\n"));
    files.push(("tools/use.sh", ". lib/common.sh\nuse() {\n\tshared\n}\n"));
    let answers = answers_over(
        &files,
        &[
            (
                "find_references",
                json!({"name": "shared", "sites_under": "tools/"}),
            ),
            (
                "find_references",
                json!({"name": "shared", "sites_under": "bin/run0"}),
            ),
            (
                "find_references",
                json!({"name": "shared", "sites_under": "lib/"}),
            ),
        ],
    );

    let file_scope = |answer: &str| -> Vec<String> {
        answer
            .lines()
            .filter_map(|line| line.strip_prefix(r#"<file_scope path=""#))
            .filter_map(|line| line.split('"').next())
            .map(str::to_owned)
            .collect()
    };
    assert!(
        answers[0].contains(r#"file_scope="51" file_scope_shown="0" file_scope_excluded="51""#)
            && answers[0].contains(r#"<file path="tools/use.sh" sites="1">"#)
            && file_scope(&answers[0]).is_empty(),
        "the site under sites_under is listed, and no file-scope user outside it: {}",
        answers[0]
    );
    let first_ten: Vec<String> = (0..10).map(|at| format!("bin/run{at:02}.sh")).collect();
    assert!(
        answers[1].contains(r#"file_scope="51" file_scope_shown="10" file_scope_excluded="41""#)
            && answers[1].contains(r#"excluded="1" files_excluded="1""#)
            && !answers[1].contains("<file "),
        "the file-scope users under sites_under are listed, the site outside it counted: {}",
        answers[1]
    );
    assert_eq!(file_scope(&answers[1]), first_ten, "{}", answers[1]);
    assert!(
        answers[2].contains(
            "no recorded site and no file_scope file is under sites_under - excluded= and file_scope_excluded= \
             count them, and a call without sites_under lists them"
        ),
        "a sites_under that keeps nothing says so: {}",
        answers[2]
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
                r#"<ref name="Second" qn="app.Second" lines="11-17" role="consumer" site="write" at="15"/>"#,
                r#"<ref name="Second" qn="app.Second" lines="11-17" role="consumer" site="read" at="16"/>"#,
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
                r#"<ref name="First" qn="app.First" lines="3-9" role="consumer" site="write" at="7"/>"#,
                r#"<ref name="First" qn="app.First" lines="3-9" role="consumer" site="read" at="8"/>"#,
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
                concat!(
                    r#"<match lang="go" of="viaDefined" qn="log.viaDefined" loc="log/log.go:7-9" count="1" "#,
                    r#"unexpanded="1">"#,
                ),
                concat!(
                    r#"<ref name="Info" qn="internal.Logger.Info" loc="internal/logger.go:4" role="consumer" "#,
                    r#"hops="1" at="8"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="viaConversion" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="viaConversion" qn="log.viaConversion" loc="log/log.go:15-17" count="2" "#,
                    r#"unexpanded="2">"#,
                ),
                concat!(
                    r#"<ref name="Info" qn="internal.Logger.Info" loc="internal/logger.go:4" role="consumer" "#,
                    r#"hops="1" at="16"/>"#,
                ),
                r#"<ref name="DepthLogger" qn="log.DepthLogger" loc="log/log.go:5" role="consumer" hops="1" at="16"/>"#,
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
                    r#"<match lang="go" of="use" qn="p.use" loc="p.go:9-11" count="0" unresolved="1">no callees - no "#,
                    "call in its body binds a definition the graph holds; a call into a library or a builtin, ",
                    "through a function value, or by dynamic dispatch binds none</match>",
                ),
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="convert" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="convert" qn="p.convert" loc="p.go:13-15" count="1" unexpanded="1" "#,
                    r#"unresolved="1">"#,
                ),
                r#"<ref name="T" qn="p.T" loc="p.go:7" role="consumer" hops="1" at="14"/>"#,
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
                r#"<match lang="go" of="viaDefined" qn="p.viaDefined" loc="a.go:19-22" count="1" unexpanded="1">"#,
                concat!(
                    r#"<ref name="Promoted" qn="p.Inner.Promoted" loc="a.go:5" role="consumer" hops="1" "#,
                    r#"at="20"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<references of="S.X" matches="1">"#,
                r#"<match lang="go" of="X" qn="p.S.X" loc="a.go:9" kind="field" count="2" files="1" reads="2">"#,
                r#"<file path="a.go" sites="2">"#,
                r#"<ref name="viaStruct" qn="p.viaStruct" lines="14-17" role="consumer" site="read" at="16"/>"#,
                r#"<ref name="viaDefined" qn="p.viaDefined" lines="19-22" role="consumer" site="read" at="21"/>"#,
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
            r#"<ref name="Second" qn="app.Second" lines="10-19" role="consumer" site="write" at="15"/>"#,
            r#"<ref name="Second" qn="app.Second" lines="10-19" role="consumer" site="update" at="16"/>"#,
            r#"<ref name="Second" qn="app.Second" lines="10-19" role="consumer" site="read" at="17"/>"#,
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
            r#"<ref name="run" lines="3-6" role="consumer" site="write" at="4"/>"#,
            r#"<ref name="run" lines="3-6" role="consumer" site="read" at="5"/>"#,
            "</file>",
            r#"<file path="src/config.ts" sites="1">"#,
            r#"<ref name="grow" qn="Config.grow" lines="3-5" role="consumer" site="update" at="4"/>"#,
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
            r#"<ref name="cap" lines="7" role="consumer" site="read" at="7" confidence="likely"/>"#,
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
            r#"<ref name="__init__" qn="pkg.config.Config.__init__" lines="5-6" role="entry" site="write" at="6"/>"#,
            r#"<ref name="grow" qn="pkg.config.Config.grow" lines="8-9" role="consumer" site="update" at="9"/>"#,
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
            r#"<ref name="run" qn="main.run" lines="4-6" role="consumer" site="read" at="6"/>"#,
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
                r#"<ref name="Devices" qn="devices.Devices" lines="1-4" role="consumer" site="read" at="4"/>"#,
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
            r#"<ref name="grow" qn="Widget::grow" lines="3-5" role="consumer" site="update" at="4"/>"#,
            r#"<ref name="paint" lines="7-11" role="consumer" site="write" at="8"/>"#,
            r#"<ref name="paint" lines="7-11" role="consumer" site="read" at="9"/>"#,
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
                    r#"<match lang="rust" of="Watcher" loc="src/watch.rs:1-3" kind="struct" count="0">no recorded "#,
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
                r#"<ref name="start" lines="3-5" role="consumer" site="call" at="4"/>"#,
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

fn read_site(qualified: &str, lines: &str, at: u32) -> String {
    let name = qualified.rsplit("::").next().unwrap_or(qualified);
    format!(
        r#"<ref name="{name}" qn="{qualified}" lines="{lines}" role="consumer" site="read" at="{at}"/>"#
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
        [Vec::new(), vec![read_site("second::read", "8-10", 9)]],
        "the read binds its own module's item, written last"
    );
    assert_eq!(
        sites_of_each(&second_first, &names),
        [Vec::new(), vec![read_site("second::read", "4-6", 5)]],
        "the read binds its own module's item, written first"
    );
    assert_eq!(
        sites_of_each(&in_first, &names),
        [vec![read_site("first::read", "4-6", 5)], Vec::new()],
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
        [vec![read_site("inner::read", "5-7", 6)]],
        "a module's item hides the file root's, written before it"
    );
    assert_eq!(
        sites_of_each(module_first, &["inner::X"]),
        [Vec::<String>::new()],
        "a read at the file root reaches no inline module's item"
    );
    assert_eq!(
        sites_of_each(beside, &["outer::X", "outer::inner::X"]),
        [vec![read_site("outer::read", "6-8", 7)], Vec::new()],
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
        [vec![read_site("test::run", "6-8", 7)]],
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
            vec![read_site("gate::read", "4-6", 5)],
            vec![read_site("gate::read", "12-14", 13)]
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
                    r#"<match lang="python" of="Spare" qn="models.Spare" loc="models.py:5-6" kind="class" "#,
                    r#"count="0">no recorded site - resolved calls can omit dynamic or unresolved ones, and uses "#,
                    "other than calls are recorded only where the graph records them, so this does not show that ",
                    "nothing uses the symbol</match>",
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="Config" matches="1">"#,
                concat!(
                    r#"<match lang="python" of="Config" qn="models.Config" loc="models.py:1-2" kind="class" "#,
                    r#"count="1" files="1" types="1">"#,
                ),
                r#"<file path="models.py" sites="1">"#,
                r#"<ref name="load" qn="models.load" lines="9-11" role="consumer" site="type" at="10"/>"#,
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
                    r#"<match lang="jsts" of="EventType" loc="events.ts:1-3" kind="enum" count="1" files="1" "#,
                    r#"reads="1">"#,
                ),
                concat!(
                    "<note>the jsts graph records no use of a type where code names it as one, such as a ",
                    "declaration, a signature, a generic argument or an import, so every such use is missing from ",
                    "this answer</note>",
                ),
                r#"<file path="card.ts" sites="1">"#,
                r#"<ref name="kind" lines="3-5" role="consumer" site="read" at="4" confidence="likely"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Palette" matches="1">"#,
                concat!(
                    r#"<match lang="python" of="Palette" qn="palette.Palette" loc="palette.py:1-2" kind="class" "#,
                    r#"count="2" files="1" types="1" reads="1">"#,
                ),
                r#"<file path="palette.py" sites="2">"#,
                r#"<ref name="pick" qn="palette.pick" lines="5-6" role="consumer" site="type" at="5"/>"#,
                r#"<ref name="pick" qn="palette.pick" lines="5-6" role="consumer" site="read" at="6"/>"#,
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
    let site = |lines: &str, kind: &str, at: u32| {
        format!(
            r#"<ref name="pick" qn="palette.pick" lines="{lines}" role="consumer" site="{kind}" at="{at}"/>"#
        )
    };
    let counts = r#"files="1" types="1" reads="1">"#;
    let head = r#"<match lang="python" of="Palette" qn="palette.Palette" loc="palette.py:1-2" kind="class" "#;

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Palette" matches="1">"#,
                &format!(r#"{head}count="1" {counts}"#),
                r#"<file path="palette.py" sites="1">"#,
                &site("5-6", "type", 5),
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Palette" matches="1">"#,
                &format!(r#"{head}count="2" {counts}"#),
                r#"<file path="palette.py" sites="2">"#,
                &site("5-6", "type", 5),
                &site("5-6", "read", 6),
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
                r#"<ref name="clean" lines="2-5" role="consumer" site="read" at="4"/>"#,
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
fn find_references_says_that_no_read_of_a_function_s_local_is_recorded() {
    let answers = answers_over(
        &[(
            "src/useSearch.ts",
            "export async function useSearch(url: string) {\n    const resp = await fetch(url);\n    \
             const done = make();\n    done();\n    return resp.ok;\n}\n",
        )],
        &[
            ("find_references", json!({"name": "resp"})),
            ("find_references", json!({"name": "done"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="resp" matches="1">"#,
                concat!(
                    r#"<match lang="jsts" of="resp" loc="src/useSearch.ts:2" kind="constant" count="0">no "#,
                    "recorded site - the jsts graph records no read or write of a constant local to a function ",
                    "body, so this does not show that nothing uses the constant; grep finds where code names it",
                    "</match>",
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="done" matches="1">"#,
                concat!(
                    r#"<match lang="jsts" of="done" loc="src/useSearch.ts:3" kind="constant" count="1" "#,
                    r#"files="1" calls="1">"#,
                ),
                concat!(
                    "<note>the jsts graph records no read or write of a constant local to a function body, so every ",
                    "such use is missing from this answer</note>",
                ),
                r#"<file path="src/useSearch.ts" sites="1">"#,
                r#"<ref name="useSearch" lines="1-6" role="consumer" site="call" at="4"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a body's local lists no read, and the answer says that no read of a function's local is recorded"
    );
}

#[test]
fn find_references_says_that_no_use_of_a_function_s_local_function_as_a_value_is_recorded() {
    let answers = answers_over(
        &[
            (
                "src/lib.rs",
                "pub fn outer(v: Vec<i32>) -> Vec<i32> {\n    fn double(x: i32) -> i32 {\n        x * 2\n    }\n    \
                 let first = double(1);\n    v.into_iter().map(double).chain([first]).collect()\n}\n\n\
                 fn triple(x: i32) -> i32 {\n    x * 3\n}\n\n\
                 pub fn control(v: Vec<i32>) -> Vec<i32> {\n    let first = triple(1);\n    \
                 v.into_iter().map(triple).chain([first]).collect()\n}\n",
            ),
            (
                "m.py",
                "def outer(v):\n    def double(x):\n        return x * 2\n\n    first = double(1)\n    \
                 return list(map(double, v)) + [first]\n",
            ),
        ],
        &[
            ("find_references", json!({"name": "double"})),
            ("find_references", json!({"name": "triple"})),
        ],
    );

    let note = |family: &str| {
        format!(
            "<note>the {family} graph records no use as a value of a function local to a function body, so every \
             such use is missing from this answer</note>"
        )
    };
    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="double" matches="2">"#,
                r#"<match lang="rust" of="double" loc="src/lib.rs:2-4" kind="function" count="1" files="1" calls="1">"#,
                &note("rust"),
                r#"<file path="src/lib.rs" sites="1">"#,
                r#"<ref name="outer" lines="1-7" role="consumer" site="call" at="5"/>"#,
                "</file>",
                "</match>",
                concat!(
                    r#"<match lang="python" of="double" qn="m.double" loc="m.py:2-3" kind="function" count="1" "#,
                    r#"files="1" calls="1">"#,
                ),
                &note("python"),
                r#"<file path="m.py" sites="1">"#,
                r#"<ref name="outer" qn="m.outer" lines="1-6" role="consumer" site="call" at="5"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="triple" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="triple" loc="src/lib.rs:9-11" kind="function" count="2" files="1" "#,
                    r#"calls="1" reads="1">"#,
                ),
                r#"<file path="src/lib.rs" sites="2">"#,
                r#"<ref name="control" lines="13-16" role="consumer" site="call" at="14"/>"#,
                r#"<ref name="control" lines="13-16" role="consumer" site="read" at="15"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a body's local function lists its calls and says that its uses as a value are not recorded, where a module's \
         function lists its read"
    );
}

#[test]
fn find_references_says_that_no_use_of_a_function_s_local_type_is_recorded() {
    let answers = answers_over(
        &[(
            "m.py",
            "def outer():\n    class Inner:\n        def __init__(self):\n            self.depth = 1\n\n        \
                 def size(self):\n            return self.depth\n\n    p = Inner()\n    q: Inner = p\n    \
                 if isinstance(q, Inner):\n        return p.size()\n    return 0\n\n\n\
                 class Outer:\n    def __init__(self):\n        self.width = 1\n\n\n\
                 def control():\n    p = Outer()\n    q: Outer = p\n    if isinstance(q, Outer):\n        \
                 return p.width\n    return 0\n",
        )],
        &[
            ("find_references", json!({"name": "Inner"})),
            ("find_references", json!({"name": "Outer"})),
            ("find_references", json!({"name": "size"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Inner" matches="1">"#,
                concat!(
                    r#"<match lang="python" of="Inner" qn="m.Inner" loc="m.py:2-7" kind="class" count="1" files="1" "#,
                    r#"calls="1">"#,
                ),
                concat!(
                    "<note>the python graph records no use as a type or a value of a type local to a function body, ",
                    "so every such use is missing from this answer</note>",
                ),
                r#"<file path="m.py" sites="1">"#,
                r#"<ref name="outer" qn="m.outer" lines="1-13" role="consumer" site="call" at="9"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Outer" matches="1">"#,
                concat!(
                    r#"<match lang="python" of="Outer" qn="m.Outer" loc="m.py:16-18" kind="class" count="3" "#,
                    r#"files="1" "#,
                    r#"calls="1" types="1" reads="1">"#,
                ),
                r#"<file path="m.py" sites="3">"#,
                r#"<ref name="control" qn="m.control" lines="21-26" role="consumer" site="call" at="22"/>"#,
                r#"<ref name="control" qn="m.control" lines="21-26" role="consumer" site="type" at="23"/>"#,
                r#"<ref name="control" qn="m.control" lines="21-26" role="consumer" site="read" at="24"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="size" matches="1">"#,
                concat!(
                    r#"<match lang="python" of="size" qn="m.Inner.size" loc="m.py:6-7" kind="method" count="1" "#,
                    r#"files="1" calls="1">"#,
                ),
                concat!(
                    "<note>a call through a value of the owner's type resolves only where the graph can name that ",
                    "type, so some of those calls can be missing</note>",
                ),
                r#"<file path="m.py" sites="1">"#,
                r#"<ref name="outer" qn="m.outer" lines="1-13" role="consumer" site="call" at="12"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
        ],
        "a body's local class lists its calls and says that its uses as a type or a value are not recorded, where a \
         module's class lists them, and the local class's method keeps its call with no such statement"
    );
}

#[test]
fn find_references_states_a_go_body_s_local_type_and_a_rust_body_s_local_struct() {
    let answers = answers_over(
        &[
            (
                "m.go",
                "package m\n\nfunc outer() int {\n\ttype Shape struct{ depth int }\n\tvar s Shape\n\t\
                 return s.depth\n}\n",
            ),
            (
                "src/lib.rs",
                "pub fn outer() -> i32 {\n    struct Shape {\n        depth: i32,\n    }\n    \
                 let s: Shape = Shape { depth: 1 };\n    s.depth\n}\n",
            ),
        ],
        &[("find_references", json!({"name": "Shape"}))],
    );

    assert_eq!(
        answers,
        [lines(&[
            r#"<references of="Shape" matches="2">"#,
            concat!(
                r#"<match lang="go" of="Shape" qn="m.Shape" loc="m.go:4" kind="struct" count="0">no recorded "#,
                "site - the go graph records no use as a type or a value of a type local to a function body, so ",
                "this does not show that nothing uses the type; grep finds where code names it</match>",
            ),
            concat!(
                r#"<match lang="rust" of="Shape" loc="src/lib.rs:2-4" kind="struct" count="0">no recorded site - "#,
                "the rust graph records no use as a type or a value of a type local to a function body, so this ",
                "does not show that nothing uses the type; grep finds where code names it</match>",
            ),
            "</references>",
        ])],
        "a Go body's local struct states that its uses are not recorded, and so does a Rust one, which the extraction \
         does not tell from a unit or a tuple struct"
    );
}

#[test]
fn find_references_states_both_gaps_of_a_rust_body_s_value_struct_and_the_type_gap_of_a_module_s() {
    let answers = answers_over(
        &[(
            "src/lib.rs",
            "pub struct Unit;\n\npub fn module() -> Unit {\n    let made = Unit;\n    made\n}\n\n\
             pub fn local() -> usize {\n    struct Inner;\n    let made = Inner;\n    \
             std::mem::size_of_val(&made)\n}\n\n\
             pub fn pair() -> u8 {\n    struct Pair(u8);\n    enum Mode {\n        On,\n    }\n    \
             let _ = Mode::On;\n    let make = Pair;\n    make(1).0 + Pair(2).0\n}\n",
        )],
        &[
            ("find_references", json!({"name": "Inner"})),
            ("find_references", json!({"name": "Unit"})),
            ("find_references", json!({"name": "Pair"})),
            ("find_references", json!({"name": "Mode"})),
        ],
    );

    let type_statement = "the rust graph records no use of a type where code names it as one, such as a declaration, \
                          a signature, a generic argument or an import";
    let local_statement =
        "the rust graph records no use as a type or a value of a type local to a function body";
    let no_site = |statement: &str| {
        format!(
            "no recorded site - {statement}, so this does not show that nothing uses the type; grep finds where code \
             names it</match>"
        )
    };
    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="Inner" matches="1">"#,
                &format!(
                    r#"<match lang="rust" of="Inner" loc="src/lib.rs:9" kind="struct" count="0">{}"#,
                    no_site(local_statement)
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="Unit" matches="1">"#,
                r#"<match lang="rust" of="Unit" loc="src/lib.rs:1" kind="struct" count="1" files="1" reads="1">"#,
                &format!(
                    "<note>{type_statement}, so every such use is missing from this answer</note>"
                ),
                r#"<file path="src/lib.rs" sites="1">"#,
                r#"<ref name="module" lines="3-6" role="consumer" site="read" at="4"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Pair" matches="1">"#,
                r#"<match lang="rust" of="Pair" loc="src/lib.rs:15" kind="struct" count="1" files="1" calls="1">"#,
                &format!(
                    "<note>{local_statement}, so every such use is missing from this answer</note>"
                ),
                r#"<file path="src/lib.rs" sites="1">"#,
                r#"<ref name="pair" lines="14-22" role="consumer" site="call" at="21"/>"#,
                "</file>",
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="Mode" matches="1">"#,
                &format!(
                    r#"<match lang="rust" of="Mode" loc="src/lib.rs:16-18" kind="enum" count="0">{}"#,
                    no_site(type_statement)
                ),
                "</references>",
            ]),
        ],
        "a body's unit or tuple struct names its uses as a type and as a value, a module's unit struct lists its read \
         under the statement every Rust type takes, and a body's enum, which is no value, keeps that statement"
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
        (
            "impact_of_edit",
            json!({"file": "pkg/service.go", "start_line": 12, "end_line": null}),
        ),
    ]);

    assert_eq!(
        answers[3], answers[0],
        "a null end_line reads as the end of the file"
    );
    assert_eq!(
        answers[..3],
        [
            lines(&[
                r#"<impact of="pkg/service.go" start_line="12" end_line="end" symbols="1">"#,
                concat!(
                    r#"<symbol name="Greet" qn="pkg.Service.Greet" kind="method" lines="12-14" callers="1" "#,
                    r#"callees="0" reach="2" reach_files="1" reach_band="low" "#,
                    r#"sig="func (s *Service) Greet(name string) string">"#,
                ),
                r#"<callers shown="1" of="1">"#,
                r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10-12" role="consumer" at="11"/>"#,
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
fn impact_of_edit_lists_10_symbols_by_default_the_first_among_equal_reaches() {
    let mut source = String::from("package lib\n");
    for at in 0..11 {
        write!(source, "\nfunc F{at:02}() {{}}\n").expect("a string takes the text");
    }
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            ("lib.go", &source),
        ],
        &[("impact_of_edit", json!({"file": "lib.go", "start_line": 1}))],
    );

    let answer = &answers[0];
    assert!(answer.contains(r#"symbols="11" shown="10">"#), "{answer}");
    let listed: Vec<&str> = answer
        .lines()
        .filter_map(|line| line.strip_prefix(r#"<symbol name=""#))
        .filter_map(|line| line.split('"').next())
        .collect();
    let first: Vec<String> = (0..10).map(|at| format!("F{at:02}")).collect();
    assert_eq!(listed, first, "the first ten in source order: {answer}");
}

#[test]
fn impact_of_edit_names_the_call_that_lists_the_callers_and_callees_past_its_50() {
    let mut source = String::from("package lib\n\nfunc hub() {\n");
    for at in 0..51 {
        writeln!(source, "\tdone{at:02}()").expect("a string takes the text");
    }
    source.push_str("}\n");
    for at in 0..51 {
        write!(
            source,
            "\nfunc done{at:02}() {{}}\n\nfunc call{at:02}() {{\n\thub()\n}}\n"
        )
        .expect("a string takes the text");
    }
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            ("lib.go", &source),
        ],
        &[
            (
                "impact_of_edit",
                json!({"file": "lib.go", "start_line": 3, "end_line": 3}),
            ),
            (
                "find_callers",
                json!({"name": "hub", "path_prefix": "lib.go", "line": 3, "hops": 1, "limit": 51}),
            ),
            (
                "find_callees",
                json!({"name": "hub", "path_prefix": "lib.go", "line": 3}),
            ),
        ],
    );

    let impact = &answers[0];
    assert!(
        impact.contains(
            "\na fixed cap cut this list - find_callers of this symbol with path_prefix this file, line 3, hops 1 \
             and limit 51 lists the rest\n</callers>"
        ),
        "{impact}"
    );
    assert!(
        impact.contains(
            "\na fixed cap cut this list - find_callees of this symbol with path_prefix this file and line 3 lists \
             the rest\n</callees>"
        ),
        "{impact}"
    );
    for (rest, tag) in [(&answers[1], "callers"), (&answers[2], "callees")] {
        assert!(rest.contains(r#"count="51""#), "{rest}");
        assert!(!rest.contains("shown="), "{tag} past the cap: {rest}");
    }
}

#[test]
fn impact_of_edit_names_the_call_that_lists_a_variable_s_users_past_its_50() {
    let mut source = String::from("package lib\n\nvar Limit = 1\n");
    for at in 0..51 {
        write!(source, "\nfunc read{at:02}() int {{\n\treturn Limit\n}}\n")
            .expect("a string takes the text");
    }
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            ("lib.go", &source),
        ],
        &[
            (
                "impact_of_edit",
                json!({"file": "lib.go", "start_line": 3, "end_line": 3}),
            ),
            (
                "find_references",
                json!({"name": "Limit", "path_prefix": "lib.go", "line": 3}),
            ),
        ],
    );

    let (impact, rest) = (&answers[0], &answers[1]);
    assert!(
        impact.contains(
            "\na fixed cap cut this list - find_references of this symbol with path_prefix this file and line 3 \
             lists the rest\n</users>"
        ),
        "{impact}"
    );
    assert!(
        rest.contains(r#"count="51""#) && !rest.contains("shown="),
        "{rest}"
    );
}

#[test]
fn impact_of_edit_holds_a_whole_file_under_50_000_characters_by_cutting_every_list_alike() {
    let mut source = String::from("package lib\n");
    for hub in 0..10 {
        write!(source, "\nfunc Hub{hub}() {{}}\n").expect("a string takes the text");
    }
    let padding = "x".repeat(120);
    for hub in 0..10 {
        for at in 0..50 {
            write!(
                source,
                "\nfunc caller{hub}_{at:02}_{padding}() {{\n\tHub{hub}()\n}}\n"
            )
            .expect("a string takes the text");
        }
    }
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            ("lib.go", &source),
        ],
        &[("impact_of_edit", json!({"file": "lib.go", "start_line": 1}))],
    );

    let answer = &answers[0];
    assert!(
        answer.chars().count() < 50_000,
        "{} characters",
        answer.chars().count()
    );
    let shown: Vec<&str> = answer
        .lines()
        .filter_map(|line| line.strip_prefix(r#"<callers shown=""#))
        .filter_map(|line| line.strip_suffix(r#"" of="50">"#))
        .collect();
    assert_eq!(shown.len(), 10, "every hub lists its callers: {answer}");
    assert!(
        shown
            .iter()
            .all(|&count| count == shown[0] && count != "50"),
        "the same cut for every list: {shown:?}"
    );
    assert!(
        answer.contains(
            "\nthe answer's size cut this list - find_callers of this symbol with path_prefix this file, line 3, \
             hops 1 and limit 50 lists the rest\n</callers>"
        ),
        "{answer}"
    );
}

#[test]
fn impact_of_change_names_the_call_that_lists_a_seed_s_callers_past_its_10() {
    let mut lib = String::from("package lib\n\nfunc Few() {}\n\nfunc Many() {}\n");
    for at in 0..11 {
        write!(lib, "\nfunc few{at:02}() {{\n\tFew()\n}}\n").expect("a string takes the text");
    }
    let packages: Vec<(String, String)> = (0..11)
        .map(|at| {
            let mut text = format!("package p{at:02}\n\nimport \"example.com/m/lib\"\n");
            for caller in 0..3 {
                write!(text, "\nfunc Call{caller}() {{\n\tlib.Many()\n}}\n")
                    .expect("a string takes the text");
            }
            (format!("p{at:02}/p.go"), text)
        })
        .collect();
    let mut files = vec![
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        ("lib/lib.go", lib.as_str()),
    ];
    files.extend(
        packages
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str())),
    );
    let answers = answers_over(
        &files,
        &[(
            "impact_of_change",
            json!({"regions": [{"file": "lib/lib.go", "start_line": 3, "end_line": 5}]}),
        )],
    );

    let change = &answers[0];
    assert!(
        change.contains(
            "\na fixed cap cut this list - find_callers of this seed with path_prefix its file, line 3, hops 1 and \
             limit 11 lists the rest\n</callers>"
        ),
        "{change}"
    );
    assert!(
        change.contains(
            "\na fixed cap cut this list - find_callers of this seed with path_prefix its file, line 5, hops 1 and \
             limit 33 lists the rest\n</caller_dirs>"
        ),
        "{change}"
    );
}

#[test]
fn impact_of_edit_lists_the_symbols_of_widest_reach_and_the_first_files_up_to_its_limit() {
    let importer = |package: &str, calls: &str| {
        format!("package {package}\n\nimport \"example.com/m/lib\"\n\nfunc Run() {{\n{calls}}}\n")
    };
    let (x, y, z) = (
        importer("x", "\tlib.A()\n\tlib.B()\n\tlib.C()\n"),
        importer("y", "\tlib.B()\n\tlib.C()\n"),
        importer("z", "\tlib.C()\n"),
    );
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            (
                "lib/lib.go",
                "package lib\n\nfunc A() {}\n\nfunc B() {}\n\nfunc C() {}\n",
            ),
            ("x/x.go", &x),
            ("y/y.go", &y),
            ("z/z.go", &z),
        ],
        &[
            (
                "impact_of_edit",
                json!({"file": "lib/lib.go", "start_line": 1, "limit": 2}),
            ),
            (
                "impact_of_edit",
                json!({"file": "lib/lib.go", "start_line": 1}),
            ),
        ],
    );

    let (cut, whole) = (&answers[0], &answers[1]);
    let listed = |answer: &str| -> Vec<String> {
        answer
            .lines()
            .filter_map(|line| line.strip_prefix(r#"<symbol name=""#))
            .filter_map(|line| line.split('"').next())
            .map(str::to_owned)
            .collect()
    };
    assert!(
        cut.starts_with(
            r#"<impact of="lib/lib.go" start_line="1" end_line="end" symbols="3" shown="2">"#
        ),
        "{cut}"
    );
    assert_eq!(
        listed(cut),
        ["B", "C"],
        "the widest reach, in source order: {cut}"
    );
    assert!(
        cut.contains(&format!("</symbol>\n{LIMIT_CUT}\n<includes")),
        "{cut}"
    );
    assert!(
        cut.contains(&format!(
            "<included-by count=\"3\" shown=\"2\">\n<file path=\"x/x.go\"/>\n<file path=\"y/y.go\"/>\n\
             {LIMIT_CUT}\n</included-by>"
        )),
        "{cut}"
    );
    assert_eq!(listed(whole), ["A", "B", "C"], "{whole}");
    assert!(
        whole.contains(r#"symbols="3">"#)
            && whole.contains(r#"<included-by count="3">"#)
            && !whole.contains(LIMIT_CUT),
        "{whole}"
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
                    r#"<match lang="bash" of="setup" loc="bin/run.sh:4-6" count="0" reach_band="low" "#,
                    r#"file_scope="1">"#,
                ),
                r#"<file_scope path="bin/run.sh"/>"#,
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="shared" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="shared" loc="lib/common.sh:1-3" count="0" reach_band="low" "#,
                    r#"file_scope="1">"#,
                ),
                r#"<file_scope path="bin/run.sh" confidence="likely"/>"#,
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="shared" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="shared" loc="lib/common.sh:1-3" count="0" reach_band="low" "#,
                    r#"below_confidence="1">no callers at this confidence floor - 1 caller binds below it, and "#,
                    r#"min_confidence "any" lists it</match>"#,
                ),
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="setup" role="entry" matches="1">"#,
                concat!(
                    r#"<match lang="bash" of="setup" loc="bin/run.sh:4-6" count="0" reach_band="low" "#,
                    r#"file_scope="1" consumers="0" tests="0" entries="0" generated="0"/>"#,
                ),
                "</callers>",
            ]),
        ]
    );
}

const LIMIT_CUT: &str = "the limit cut this list - a higher limit lists the rest";

#[test]
fn find_callers_and_find_references_list_the_first_50_file_scope_users_and_count_every_one() {
    let scripts: Vec<(String, &str)> = (0..51)
        .map(|at| (format!("bin/run{at:02}.sh"), ". lib/common.sh\nshared\n"))
        .collect();
    let mut files: Vec<(&str, &str)> = scripts
        .iter()
        .map(|(path, text)| (path.as_str(), *text))
        .collect();
    files.push(("lib/common.sh", "shared() {\n\techo shared\n}\n"));
    let answers = answers_over(
        &files,
        &[
            ("find_callers", json!({"name": "shared"})),
            ("find_references", json!({"name": "shared"})),
            ("find_callers", json!({"name": "shared", "role": "entry"})),
            (
                "find_references",
                json!({"name": "shared", "sites_under": "bin/run5"}),
            ),
        ],
    );

    let first: Vec<&str> = scripts[..50]
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    let cuts = [
        "no parameter lists the rest, and grep of the name's last segment finds them among its other uses",
        "a narrower sites_under lists the rest under it",
    ];
    for (answer, way) in answers[..2].iter().zip(cuts) {
        assert!(
            answer.contains(r#"file_scope="51" file_scope_shown="50""#),
            "{answer}"
        );
        let listed: Vec<&str> = answer
            .lines()
            .filter_map(|line| line.strip_prefix(r#"<file_scope path=""#))
            .filter_map(|line| line.split('"').next())
            .collect();
        assert_eq!(listed, first, "the first 50 in path order: {answer}");
        assert!(
            answer.contains(&format!(
                "<file_scope path=\"bin/run49.sh\" confidence=\"likely\"/>\na fixed cap cut these file_scope \
                 records - {way}\n"
            )),
            "{answer}"
        );
    }
    assert!(
        answers[3].contains(r#"file_scope="51""#)
            && answers[3].contains(r#"file_scope_excluded="50""#)
            && answers[3].contains(r#"<file_scope path="bin/run50.sh""#),
        "the sites_under the cut line names lists the user past 50: {}",
        answers[3]
    );
    assert!(
        answers[2].contains(r#"file_scope="51" consumers="#),
        "{}",
        answers[2]
    );
    assert!(
        !answers[2].contains("file_scope_shown") && !answers[2].contains("<file_scope "),
        "a role lists no file-scope record: {}",
        answers[2]
    );

    let at_the_cap = answers_over(
        &files[1..],
        &[
            ("find_callers", json!({"name": "shared"})),
            ("find_references", json!({"name": "shared"})),
        ],
    );
    for answer in &at_the_cap {
        assert!(
            answer.contains(r#"file_scope="50""#)
                && answer.contains(r#"<file_scope path="bin/run50.sh""#)
                && !answer.contains("file_scope_shown")
                && !answer.contains("a fixed cap cut these file_scope records"),
            "50 users fit the cap whole: {answer}"
        );
    }
}

#[test]
fn find_callers_lists_the_nearest_callers_up_to_its_limit_and_counts_every_one() {
    let mut callers =
        String::from("package lib\n\nfunc helper() {}\n\nfunc Top() {\n\tc000()\n}\n");
    for at in 0..101 {
        write!(callers, "\nfunc c{at:03}() {{\n\thelper()\n}}\n").expect("a string takes the text");
    }
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
            ("lib.go", &callers),
        ],
        &[
            ("find_callers", json!({"name": "helper"})),
            ("find_callers", json!({"name": "helper", "limit": 102})),
        ],
    );

    let (cut, whole) = (&answers[0], &answers[1]);
    assert!(
        cut.contains(r#"count="102" shown="100" reach_band="#),
        "{cut}"
    );
    assert_eq!(cut.matches("<ref ").count(), 100, "{cut}");
    assert!(
        !cut.contains(r#"hops="2""#),
        "the nearest callers come first: {cut}"
    );
    assert!(
        cut.ends_with(&format!("\n{LIMIT_CUT}\n</match>\n</callers>")),
        "{cut}"
    );
    assert!(whole.contains(r#"count="102" reach_band="#), "{whole}");
    assert_eq!(whole.matches("<ref ").count(), 102, "{whole}");
    assert!(whole.contains(r#"hops="2""#), "{whole}");
    assert!(!whole.contains(LIMIT_CUT), "{whole}");
}

#[test]
fn find_callers_spends_its_limit_on_the_callers_its_role_keeps() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/lib\n\ngo 1.22\n"),
            (
                "lib.go",
                "package lib\n\nfunc helper() {}\n\nfunc A() {\n\thelper()\n}\n\nfunc B() {\n\thelper()\n}\n",
            ),
            (
                "lib_test.go",
                "package lib\n\nimport \"testing\"\n\nfunc TestOne(t *testing.T) {\n\thelper()\n}\n\n\
                 func TestTwo(t *testing.T) {\n\thelper()\n}\n",
            ),
        ],
        &[(
            "find_callers",
            json!({"name": "helper", "role": "test", "limit": 1}),
        )],
    );

    let answer = &answers[0];
    assert!(
        answer.contains(r#"count="4" shown="1" reach_band="#)
            && answer.contains(r#"consumers="2" tests="2""#),
        "{answer}"
    );
    let refs: Vec<&str> = answer
        .lines()
        .filter(|line| line.starts_with("<ref "))
        .collect();
    assert!(
        refs.len() == 1 && refs[0].contains(r#" role="test""#),
        "{answer}"
    );
    assert!(answer.contains(LIMIT_CUT), "{answer}");
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
                r#"<match lang="go" of="walk" qn="lib.walk" loc="walk.go:3-8" count="1" reach_band="low"{unexpanded}>"#
            ),
            r#"<self_call at="7"/>"#,
            r#"<ref name="Run" qn="lib.Run" loc="walk.go:10-12" role="consumer" hops="1" at="11"/>"#,
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
                r#"<match lang="go" of="walk" qn="lib.walk" loc="walk.go:3-8" count="0">"#,
                concat!(
                    "no callees but itself - no other call in its body binds a definition the graph holds; a call ",
                    "into a library or a builtin, through a function value, or by dynamic dispatch binds none",
                ),
                r#"<self_call at="7"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<references of="walk" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="walk" qn="lib.walk" loc="walk.go:3-8" kind="function" count="2" "#,
                    r#"files="1" calls="2">"#,
                ),
                r#"<file path="walk.go" sites="2">"#,
                r#"<ref name="walk" qn="lib.walk" lines="3-8" role="consumer" site="call" at="7"/>"#,
                r#"<ref name="Run" qn="lib.Run" lines="10-12" role="consumer" site="call" at="11"/>"#,
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
                r#"<match lang="go" of="spin" qn="lib.spin" loc="walk.go:14-16" count="0" reach_band="low">"#,
                "no callers but itself - entry point, dead code, or dynamic dispatch",
                r#"<self_call at="15"/>"#,
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callees of="spin" matches="1">"#,
                r#"<match lang="go" of="spin" qn="lib.spin" loc="walk.go:14-16" count="0">"#,
                concat!(
                    "no callees but itself - no other call in its body binds a definition the graph holds; a call ",
                    "into a library or a builtin, through a function value, or by dynamic dispatch binds none",
                ),
                r#"<self_call at="15"/>"#,
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

/// The answers to `calls` over one interface's method called through the interface in PHP, Go and TypeScript, each
/// implemented by `Kernel`, and in PHP by `Other` too, beside a `Plain` whose `boot` implements nothing.
fn answers_over_an_interface(calls: &[(&str, Value)]) -> Vec<String> {
    answers_over(
        &[
            (
                "composer.json",
                r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
            ),
            (
                "src/A.php",
                "<?php\nnamespace App;\ninterface Runs {\n    public function boot();\n}\n\
                 class Kernel implements Runs {\n    public function boot() {}\n}\n\
                 class Other implements Runs {\n    public function boot() {}\n}\n\
                 class Plain {\n    public function boot() {}\n}\n\
                 class Holder {\n    private Runs $k;\n    public function start() { $this->k->boot(); }\n}\n",
            ),
            ("go/go.mod", "module example.com/runs\n\ngo 1.22\n"),
            (
                "go/runs.go",
                "package runs\n\ntype Runs interface{ Boot() }\n\ntype Kernel struct{}\n\nfunc (Kernel) Boot() {}\n\n\
                 type Wide interface {\n\tBoot()\n\tHalt()\n}\n\nfunc start(r Runs) { r.Boot() }\n\n\
                 func stop(w Wide) { w.Boot() }\n",
            ),
            (
                "ts/runs.ts",
                "interface Runs { boot(): void }\nclass Kernel implements Runs { boot() {} }\n\
                 function start(r: Runs) { r.boot(); }\n",
            ),
        ],
        calls,
    )
}

#[test]
fn find_callers_lists_a_call_through_an_interface_the_owner_implements() {
    let answers = answers_over_an_interface(&[
        ("find_callers", json!({"name": "App\\Kernel::boot"})),
        ("find_callers", json!({"name": "runs.Kernel.Boot"})),
        (
            "find_callers",
            json!({"name": "Kernel.boot", "languages": ["jsts"]}),
        ),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<callers of="App\\Kernel::boot" matches="1">"#,
                concat!(
                    r#"<match lang="php" of="boot" qn="App\\Kernel::boot" loc="src/A.php:7" count="1" "#,
                    r#"reach_band="low">"#,
                ),
                concat!(
                    r#"<ref name="start" qn="App\\Holder::start" loc="src/A.php:17" role="consumer" "#,
                    r#"through="App\\Runs::boot" hops="1" at="17"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="runs.Kernel.Boot" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Boot" qn="runs.Kernel.Boot" loc="go/runs.go:7" count="1" "#,
                    r#"reach_band="low">"#,
                ),
                concat!(
                    r#"<ref name="start" qn="runs.start" loc="go/runs.go:14" role="consumer" "#,
                    r#"through="runs.Runs.Boot" hops="1" at="14"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="Kernel.boot" languages="jsts" matches="1">"#,
                concat!(
                    r#"<match lang="jsts" of="boot" qn="Kernel.boot" loc="ts/runs.ts:2" count="1" "#,
                    r#"reach_band="low">"#,
                ),
                concat!(
                    r#"<ref name="start" loc="ts/runs.ts:3" role="consumer" through="Runs.boot" hops="1" "#,
                    r#"at="3"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
        ]
    );
}

#[test]
fn a_language_of_a_family_echoes_the_family_it_searched() {
    let answers = answers_over_an_interface(&[(
        "find_callers",
        json!({"name": "Kernel.boot", "languages": ["javascript"]}),
    )]);

    assert!(
        answers[0].starts_with(r#"<callers of="Kernel.boot" languages="jsts" matches="1">"#),
        "javascript searches the jsts graph, which holds ts/runs.ts: {}",
        answers[0]
    );
}

#[test]
fn find_callers_lists_no_call_through_an_interface_the_owner_does_not_implement() {
    let answers = answers_over_an_interface(&[
        ("find_callers", json!({"name": "App\\Plain::boot"})),
        ("find_callers", json!({"name": "App\\Runs::boot"})),
    ]);

    assert!(
        answers[0].contains(r#"count="0""#) && !answers[0].contains("through="),
        "Plain implements no interface: {}",
        answers[0]
    );
    assert!(
        answers[1].contains(r#"count="1""#) && !answers[1].contains("through="),
        "the interface's own callers bind it: {}",
        answers[1]
    );
}

#[test]
fn find_callers_reads_a_go_interface_as_implemented_only_by_a_type_with_its_every_method() {
    let answers =
        answers_over_an_interface(&[("find_callers", json!({"name": "runs.Kernel.Boot"}))]);

    assert!(
        !answers[0].contains(r#"name="stop""#),
        "Kernel has no Halt, so it is no Wide: {}",
        answers[0]
    );
}

#[test]
fn find_callers_reads_a_go_interface_that_embeds_one_as_implemented_by_a_type_with_both_method_sets()
 {
    let answers = answers_over(
        &[
            ("go.mod", "module m\n\ngo 1.22\n"),
            (
                "p/p.go",
                "package p\n\ntype Runner interface {\n\tRun()\n}\n\n\
                 type Service interface {\n\tRunner\n\tStop()\n}\n\n\
                 type T struct{}\n\nfunc (T) Run() {}\n\nfunc (T) Stop() {}\n\n\
                 type Half struct{}\n\nfunc (Half) Stop() {}\n\n\
                 func shut(s Service) {\n\ts.Stop()\n}\n",
            ),
        ],
        &[
            ("find_callers", json!({"name": "p.T.Stop"})),
            ("find_callers", json!({"name": "p.Half.Stop"})),
        ],
    );

    assert!(
        answers[0].contains(r#"name="shut""#) && answers[0].contains(r#"through="p.Service.Stop""#),
        "{}",
        answers[0]
    );
    assert!(
        answers[1].contains(r#"count="0""#),
        "Half has no Run, so it is no Runner and no Service: {}",
        answers[1]
    );
}

#[test]
fn find_callers_reads_a_go_interface_only_from_a_package_near_the_types() {
    let answers = answers_over(
        &[
            ("go.mod", "module m\n\ngo 1.22\n"),
            (
                "b/b.go",
                "package b\n\ntype Runner interface {\n\tRun() error\n}\n",
            ),
            (
                "c/c.go",
                "package c\n\nimport \"m/b\"\n\nfunc Start(r b.Runner) error {\n\treturn r.Run()\n}\n",
            ),
            (
                "d/d.go",
                "package d\n\nimport \"m/b\"\n\nvar _ b.Runner = U{}\n\ntype U struct{}\n\n\
                 func (U) Run() error { return nil }\n",
            ),
            (
                "e/e.go",
                "package e\n\ntype T struct{}\n\nfunc (T) Run() error { return nil }\n",
            ),
        ],
        &[
            ("find_callers", json!({"name": "d.U.Run"})),
            ("find_callers", json!({"name": "e.T.Run"})),
        ],
    );

    assert!(
        answers[0].contains(r#"name="Start""#) && answers[0].contains(r#"through="b.Runner.Run""#),
        "d imports b: {}",
        answers[0]
    );
    assert!(
        answers[1].contains(r#"count="0""#),
        "e and b import neither the other: {}",
        answers[1]
    );
}

#[test]
fn find_callers_lists_a_caller_that_also_calls_the_method_itself_once_as_a_direct_caller() {
    let answers = answers_over(
        &[
            (
                "composer.json",
                r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
            ),
            (
                "src/A.php",
                "<?php\nnamespace App;\ninterface Runs {\n    public function boot();\n}\n\
                 class Kernel implements Runs {\n    public function boot() {}\n}\n\
                 class Holder {\n    public function start(Runs $runs, Kernel $kernel) {\n        \
                 $runs->boot();\n        $kernel->boot();\n    }\n}\n",
            ),
        ],
        &[
            ("find_callers", json!({"name": "App\\Kernel::boot"})),
            (
                "impact_of_edit",
                json!({"file": "src/A.php", "start_line": 7, "end_line": 7}),
            ),
        ],
    );

    for answer in &answers {
        assert!(
            answer.matches(r#"name="start""#).count() == 1 && !answer.contains("through="),
            "{answer}"
        );
    }
}

#[test]
fn find_callers_lists_no_call_on_this_from_a_class_the_member_walk_sets_below_the_parent() {
    let answers = answers_over(
        &[
            (
                "base.nut",
                "let Base = class {\n  function close() {}\n}\n\n\
                 local Page = class (Base) {\n  function show() {}\n}\n\n\
                 return {\n  Base\n  Page\n}\n",
            ),
            (
                "form.nut",
                "let base = require(\"base.nut\")\n\n\
                 local Page = class (base.Page) {\n  function submit() {\n    this.close()\n  }\n}\n\n\
                 return { Page }\n",
            ),
            (
                "other.nut",
                "let { Base } = require(\"base.nut\")\n\n\
                 local Page = class (Base) {\n  function close() {}\n}\n\n\
                 return { Page }\n",
            ),
        ],
        &[(
            "find_callers",
            json!({"name": "close", "path_prefix": "other.nut"}),
        )],
    );

    assert!(
        !answers[0].contains(r#"name="submit""#),
        "this in form's Page is a Base below base's Page, never other's: {}",
        answers[0]
    );
}

#[test]
fn find_callers_reads_a_cpp_member_as_reached_only_through_the_virtual_one_it_overrides() {
    let answers = answers_over(
        &[(
            "runs.cpp",
            "struct Runs {\n    virtual void boot(int n) = 0;\n    virtual void visit(int* n) {}\n    \
             void plain() {}\n};\n\n\
             struct Kernel : Runs {\n    void boot(int count) override {}\n    void visit(char* c) {}\n    \
             void plain() {}\n};\n\n\
             void start(Runs& runs) {\n    runs.boot(1);\n    runs.visit(nullptr);\n    runs.plain();\n}\n",
        )],
        &[
            ("find_callers", json!({"name": "Kernel::boot"})),
            ("find_callers", json!({"name": "Kernel::visit"})),
            ("find_callers", json!({"name": "Kernel::plain"})),
        ],
    );

    assert!(
        answers[0].contains(r#"name="start""#) && answers[0].contains(r#"through="Runs::boot""#),
        "{}",
        answers[0]
    );
    for answer in &answers[1..] {
        assert!(
            answer.contains(r#"count="0""#),
            "another parameter list, or no virtual, overrides nothing: {answer}"
        );
    }
}

#[test]
fn find_callers_lists_no_call_from_within_another_branch_of_the_hierarchy_or_of_a_constructor() {
    let answers = answers_over(
        &[
            (
                "composer.json",
                r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
            ),
            (
                "src/B.php",
                "<?php\nnamespace App;\nclass Base {\n    public function __construct() {}\n    \
                 public function boot() {}\n    public function run() { $this->boot(); }\n}\n\
                 class Child extends Base {\n    public function __construct() { parent::__construct(); }\n    \
                 public function boot() { parent::boot(); }\n}\n\
                 class Sibling extends Base {\n    public function boot() {}\n}\n\
                 class Cousin extends Base {\n    public function go() { $this->boot(); }\n}\n\
                 class User {\n    public function make() { return new Base(); }\n}\n",
            ),
            (
                "py/app.py",
                "class Base:\n    def __init__(self):\n        pass\n\n    def boot(self):\n        pass\n\n\
                 class Child(Base):\n    def __init__(self):\n        Base.__init__(self)\n\n    \
                 def boot(self):\n        Base.boot(self)\n\n\
                 class Sibling(Base):\n    def __init__(self):\n        pass\n\n    def boot(self):\n        pass\n",
            ),
        ],
        &[
            ("find_callers", json!({"name": "App\\Sibling::boot"})),
            ("find_callers", json!({"name": "App\\Child::__construct"})),
            ("find_callers", json!({"name": "py.app.Sibling.boot"})),
            ("find_callers", json!({"name": "py.app.Sibling.__init__"})),
        ],
    );

    assert!(
        answers[0].contains(r#"name="run""#) && !answers[0].contains(r#"qn="App\\Child::boot""#),
        "parent::boot() runs Base's own boot: {}",
        answers[0]
    );
    assert!(
        !answers[0].contains(r#"name="go""#),
        "$this in Cousin is a Cousin, never a Sibling: {}",
        answers[0]
    );
    assert!(
        !answers[1].contains(r#"name="make""#),
        "new Base() constructs a Base: {}",
        answers[1]
    );
    for answer in &answers[2..] {
        assert!(
            answer.contains(r#"count="0""#),
            "Base.boot(self) and Base.__init__(self) run Base's own: {answer}"
        );
    }
}

#[test]
fn find_callers_reads_a_rust_impl_as_implementing_a_trait_and_no_namesake_struct() {
    let answers = answers_over(
        &[
            ("x/Cargo.toml", "[package]\nname = \"x\"\n"),
            (
                "x/src/lib.rs",
                "pub struct Module;\n\nimpl Module {\n    pub fn new() -> Self {\n        Module\n    }\n}\n\n\
                 pub fn build() -> Module {\n    Module::new()\n}\n",
            ),
            ("y/Cargo.toml", "[package]\nname = \"y\"\n"),
            (
                "y/src/lib.rs",
                "pub trait Module {\n    fn new() -> Self;\n    fn run(&self);\n}\n\npub struct Object;\n\n\
                 impl Module for Object {\n    fn new() -> Self {\n        Object\n    }\n\n    fn run(&self) {}\n}\n\n\
                 pub fn start(m: &dyn Module) {\n    m.run();\n}\n",
            ),
        ],
        &[
            ("find_callers", json!({"name": "Object::new"})),
            ("find_callers", json!({"name": "Object::run"})),
        ],
    );

    assert!(
        answers[0].contains(r#"count="0""#),
        "x's struct Module is no trait Object implements: {}",
        answers[0]
    );
    assert!(
        answers[1].contains(r#"name="start""#) && answers[1].contains(r#"through="Module::run""#),
        "{}",
        answers[1]
    );
}

#[test]
fn the_impact_tools_and_find_call_path_reach_a_method_through_an_interface() {
    let answers = answers_over_an_interface(&[
        (
            "impact_of_edit",
            json!({"file": "src/A.php", "start_line": 7}),
        ),
        (
            "find_call_path",
            json!({"from": "App\\Holder::start", "to": "App\\Kernel::boot"}),
        ),
    ]);

    assert!(
        answers[0].contains(r#"callers="1""#)
            && answers[0].contains(r#"through="App\\Runs::boot""#),
        "{}",
        answers[0]
    );
    assert!(
        answers[1].contains(r#"hops="1""#) && answers[1].contains(r#"through="App\\Runs::boot""#),
        "{}",
        answers[1]
    );
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

    let no_call_binds = |qn: &str, loc: &str| {
        let of = qn.rsplit("::").next().unwrap_or(qn);
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
            no_call_binds("CancelToken::cancel", "src/cancel.rs:4"),
            no_call_binds("Index::refresh_paths", "src/index.rs:4-6"),
            no_call_binds("Engine::refresh_paths", "src/engine.rs:4-6"),
            lines(&[
                r#"<callers of="CancelToken::is_cancelled" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="is_cancelled" qn="CancelToken::is_cancelled" loc="src/cancel.rs:6-8" "#,
                    r#"count="1" reach_band="low" unexpanded="1">"#,
                ),
                concat!(
                    r#"<ref name="stop" loc="src/run.rs:3-6" role="consumer" confidence="likely" hops="1" "#,
                    r#"at="5"/>"#,
                ),
                "</match>",
                "</callers>",
            ]),
            lines(&[
                r#"<callers of="Engine::refresh_paths_limit" matches="1">"#,
                concat!(
                    r#"<match lang="rust" of="refresh_paths_limit" qn="Engine::refresh_paths_limit" "#,
                    r#"loc="src/engine.rs:8-10" count="1" reach_band="low">"#,
                ),
                concat!(
                    r#"<ref name="refresh" loc="src/state.rs:3-8" role="consumer" confidence="likely" hops="1" "#,
                    r#"at="4"/>"#,
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
                    r#"<ref name="close" qn="Handle::close" loc="src/handle.rs:6-8" role="consumer" hops="1" "#,
                    r#"at="7"/>"#,
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
                "<match lang=\"rust\" of=\"refresh_paths\" qn=\"Index::refresh_paths\" loc=\"src/index.rs:4-6\" \
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
                    r#"<symbol name="cancel" qn="CancelToken::cancel" kind="method" lines="4" callers="0" "#,
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
                r#"<symbol name="Code" qn="wire.Error.Code" kind="field" lines="4" callers="0" users="3" "#,
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
    r#"<ref name="call" qn="wire.call" loc="wire/call.go:3-5" role="consumer" at="4"/>"#,
    r#"<ref name="Error" qn="wire.Error.Error" loc="wire/errors.go:7-9" role="consumer" at="8"/>"#,
    r#"<ref name="Unwrap" qn="wire.Error.Unwrap" loc="wire/errors.go:11-13" role="consumer" at="12"/>"#,
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
                r#"<symbol name="onDone" qn="hook.onDone" kind="variable" lines="3" callers="1" users="2" "#,
                r#"callees="0" reach="2" reach_files="0" reach_band="low">"#,
            ),
            r#"<users shown="2" of="2">"#,
            r#"<ref name="Set" qn="hook.Set" loc="hook/hook.go:5-7" role="consumer" at="6"/>"#,
            r#"<ref name="Fire" qn="hook.Fire" loc="hook/hook.go:9-11" role="consumer" at="10"/>"#,
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
                    r#"<match lang="go" of="main" qn="main.main" loc="cmd/app/main.go:5-8" count="2" "#,
                    r#"unexpanded="2">"#,
                ),
                concat!(
                    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10-12" role="consumer" hops="1" "#,
                    r#"at="7"/>"#,
                ),
                concat!(
                    r#"<ref name="New" qn="pkg.New" loc="pkg/service.go:7-9" role="consumer" hops="1" "#,
                    r#"at="6"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="main" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="main" qn="main.main" loc="cmd/app/main.go:5-8" count="3" "#,
                    r#"unexpanded="1">"#,
                ),
                concat!(
                    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10-12" role="consumer" hops="1" "#,
                    r#"at="7"/>"#,
                ),
                concat!(
                    r#"<ref name="New" qn="pkg.New" loc="pkg/service.go:7-9" role="consumer" hops="1" "#,
                    r#"at="6"/>"#,
                ),
                r#"<ref name="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12-14" role="consumer" hops="2"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Greet" qn="pkg.Service.Greet" loc="pkg/service.go:12-14" count="0">"#,
                    "no callees - no call in its body binds a definition the graph holds; a call into a library or a ",
                    "builtin, through a function value, or by dynamic dispatch binds none</match>",
                ),
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="Gret" matches="0" one_edit="1">"#,
                "no symbol matches - did you mean:",
                concat!(
                    r#"<suggestion name="Greet" qn="pkg.Service.Greet" kind="method" lang="go" "#,
                    r#"loc="pkg/service.go:12-14"/>"#,
                ),
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
                    r#"<match lang="go" of="Run" qn="lib.Run" loc="run.go:5-8" count="1" unexpanded="1" "#,
                    r#"unresolved="1">"#,
                ),
                concat!(
                    r#"<ref name="helper" qn="lib.helper" loc="helper.go:3" role="consumer" confidence="likely" "#,
                    r#"hops="1" at="7"/>"#,
                ),
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="Run" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Run" qn="lib.Run" loc="run.go:5-8" count="0" below_confidence="1" "#,
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
                    r#"below_confidence="2">no recorded site at this confidence floor - 2 uses bind below it, "#,
                    r#"and min_confidence "any" lists them</match>"#,
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="limit" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="limit" qn="lib.limit" loc="helper.go:3" kind="variable" count="0" "#,
                    r#"below_confidence="1">no recorded site at this confidence floor - 1 use binds below it, and "#,
                    r#"min_confidence "any" without access lists it</match>"#,
                ),
                "</references>",
            ]),
            lines(&[
                r#"<references of="Stop" matches="1">"#,
                concat!(
                    r#"<match lang="go" of="Stop" qn="lib.Stop" loc="stop.go:3-5" kind="function" count="0">no "#,
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
                    r#"<match lang="go" from="lib.Run" from_loc="run.go:3-5" to="lib.helper" to_loc="helper.go:3" "#,
                    r#"max_hops="8" below_confidence="2" "#,
                    r#"found="false">no call chain within 8 calls at this confidence floor - 2 callers bind below "#,
                    r#"it, and min_confidence "any" follows them</match>"#,
                ),
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="Run" to="Stop" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="lib.Run" from_loc="run.go:3-5" to="lib.Stop" to_loc="stop.go:3-5" "#,
                    r#"max_hops="8" below_confidence="1" "#,
                    r#"found="false">no call chain within 8 calls at this confidence floor - 1 caller binds below "#,
                    r#"it, and min_confidence "any" follows it</match>"#,
                ),
                "</call_path>",
            ]),
        ]
    );
}

#[test]
fn find_call_path_writes_hops_for_a_chain_alone_and_zero_for_one_symbol() {
    let answers = answers(&[
        ("find_call_path", json!({"from": "Greet", "to": "Greet"})),
        ("find_call_path", json!({"from": "Greet", "to": "main"})),
    ]);

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<call_path from="Greet" to="Greet" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="pkg.Service.Greet" from_loc="pkg/service.go:12-14" "#,
                    r#"to="pkg.Service.Greet" to_loc="pkg/service.go:12-14" hops="0" max_hops="8"/>"#,
                ),
                "</call_path>",
            ]),
            lines(&[
                r#"<call_path from="Greet" to="main" matches="1">"#,
                concat!(
                    r#"<match lang="go" from="pkg.Service.Greet" from_loc="pkg/service.go:12-14" to="main.main" "#,
                    r#"to_loc="cmd/app/main.go:5-8" max_hops="8" found="false">no call chain within 8 calls - the "#,
                    "graph misses dynamic and unresolved calls, so this does not show that the code holds none</match>",
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
                r#"<match lang="go" from="lib.helper" from_loc="helper.go:3" to="lib.Run" to_loc="run.go:3-5" "#,
                r#"max_hops="8" found="false">no call "#,
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
    r#"<ref name="run" qn="main.run" loc="cmd/app/main.go:10-12" role="consumer" at="11"/>"#,
    "</callers>",
    "</seed>",
];

const MAIN_HOTSPOT: [&str; 3] = [
    r#"<hotspots shown="1" of="1">"#,
    concat!(
        r#"<hotspot name="main" qn="main.main" kind="function" loc="cmd/app/main.go:5-8" role="entry" "#,
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
        r#"<ref name="main" qn="main.main" loc="cmd/app/main.go:5-8" role="entry" at="6"/>"#,
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
                            r#"footprint_files="1" unheld_regions="1" unclaimed_regions="1">"#,
                        ),
                        r#"<seeds shown="1" of="2">"#,
                    ][..],
                    &GREET_SEED,
                    &[LIMIT_CUT, "</seeds>"],
                    &MAIN_HOTSPOT,
                    &["</change>"],
                ]
                .concat()
            ),
            concat!(
                r#"<change regions="1" changed_symbols="0" changed_files="0" footprint="0" footprint_files="0" "#,
                r#"unheld_regions="1" unclaimed_regions="1">no symbols in these regions - they cover only "#,
                "file-scope code (imports, comments) or files no code graph holds (counted in unheld_regions); zero ",
                "symbols is not proof the change is safe, and find_importers lists the files that include a changed ",
                "file</change>",
            )
            .to_owned(),
        ]
    );
}

#[test]
fn impact_of_change_ends_the_hotspots_its_limit_cut_with_the_line_that_names_limit() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            (
                "app.go",
                "package app\n\nfunc A() {}\n\nfunc B() {}\n\nfunc X() {\n\tA()\n\tB()\n}\n\n\
                 func Y() {\n\tA()\n\tB()\n}\n\nfunc Z() {\n\tA()\n\tB()\n}\n",
            ),
        ],
        &[
            (
                "impact_of_change",
                json!({"regions": [{"file": "app.go", "start_line": 3, "end_line": 5}], "limit": 2}),
            ),
            (
                "impact_of_change",
                json!({"regions": [{"file": "app.go", "start_line": 3, "end_line": 5}], "limit": 3}),
            ),
        ],
    );

    let (cut, whole) = (&answers[0], &answers[1]);
    assert!(
        cut.contains(r#"<seeds shown="2" of="2">"#)
            && cut.contains(r#"<hotspots shown="2" of="3">"#),
        "{cut}"
    );
    assert!(
        cut.contains(&format!("/>\n{LIMIT_CUT}\n</hotspots>")),
        "{cut}"
    );
    assert_eq!(
        cut.matches(LIMIT_CUT).count(),
        1,
        "the seeds the limit did not cut end with no line: {cut}"
    );
    assert!(
        whole.contains(r#"<hotspots shown="3" of="3">"#) && !whole.contains(LIMIT_CUT),
        "{whole}"
    );
}

#[test]
fn impact_of_change_names_the_language_that_leaves_a_region_s_file_out_and_counts_the_paths_it_lacks()
 {
    let files = [
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("a.go", "package app\n\nfunc A() {}\n"),
        ("_skip.go", "package app\n\nfunc Skipped() {}\n"),
        (".hidden.go", "package app\n\nfunc Hidden() {}\n"),
        ("notes.txt", "notes\n"),
    ];
    let diff = "\
diff --git a/_skip.go b/_skip.go
--- a/_skip.go
+++ b/_skip.go
@@ -3 +3 @@
-func Skipped() {}
+func Skipped() { _ = 1 }
diff --git a/gone.go b/gone.go
--- a/gone.go
+++ b/gone.go
@@ -1 +1 @@
-package app
+package gone
";
    let answers = answers_over(
        &files,
        &[
            ("impact_of_change", json!({"diff": diff})),
            (
                "impact_of_change",
                json!({"regions": [
                    {"file": "a.go"},
                    {"file": "_skip.go"},
                    {"file": ".hidden.go"},
                    {"file": "notes.txt"},
                ]}),
            ),
        ],
    );
    let declined = |regions: usize| {
        format!(
            "<declined language=\"go\" regions=\"{regions}\">the go language leaves these regions' files out of its \
             graph: no Go build reads a file whose name starts with _ or . - read or grep them</declined>"
        )
    };

    assert_eq!(
        answers,
        [
            lines(&[
                concat!(
                    r#"<change regions="2" changed_symbols="0" changed_files="0" footprint="0" footprint_files="0" "#,
                    r#"unheld_regions="2" missing_regions="1">"#,
                ),
                concat!(
                    "no symbols in these regions - they cover only file-scope code (imports, comments) or files no ",
                    "code graph holds (counted in unheld_regions); zero symbols is not proof the change is safe, and ",
                    "find_importers lists the files that include a changed file",
                ),
                &declined(1),
                "</change>",
            ]),
            lines(&[
                concat!(
                    r#"<change regions="4" changed_symbols="1" changed_files="1" footprint="0" footprint_files="0" "#,
                    r#"unheld_regions="3" unclaimed_regions="1">"#,
                ),
                r#"<seeds shown="1" of="1">"#,
                r#"<seed name="A" qn="app.A" kind="function" loc="a.go:3" callers="0" reach="0" reach_band="low"/>"#,
                "</seeds>",
                &declined(2),
                "</change>",
            ]),
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
            client.answer("find_unused", &json!({"path_prefix": "go."})),
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
                LIMIT_CUT,
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
                r#"<unused path_prefix="go." count="0" live="0">no definition in a code graph lies under "#,
                "path_prefix - the paths it starts are in no graph, or define nothing</unused>",
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

#[test]
fn find_implementations_of_a_trait_outside_the_graph_lists_the_types_whose_parents_name_it() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("Cargo.toml", "[package]\nname = \"app\"\n"),
        ("src/lib.rs", "mod show;\nmod held;\n"),
        (
            "src/show.rs",
            "use std::fmt;\n\npub struct Module;\n\nimpl fmt::Display for Module {\n    fn fmt(&self, f: &mut \
             fmt::Formatter<'_>) -> fmt::Result { Ok(()) }\n}\n\npub struct Path;\n\nimpl std::fmt::Display for \
             Path {\n    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { Ok(()) }\n}\n",
        ),
        (
            "src/held.rs",
            "pub trait Shown {}\n\npub struct Plain;\n\nimpl Shown for Plain {}\n\npub fn Write() {}\n\npub struct \
             Sink;\n\nimpl std::io::Write for Sink {\n    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { \
             Ok(b.len()) }\n    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }\n}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let bare = client.answer("find_implementations", &json!({"name": "Display"}));
    let path = client.answer(
        "find_implementations",
        &json!({"name": "std::fmt::Display"}),
    );
    let narrowed = client.answer(
        "find_implementations",
        &json!({"name": "Display", "path_prefix": "src/"}),
    );
    let on_a_line = client.answer(
        "find_implementations",
        &json!({"name": "Display", "line": 3}),
    );
    let unwritten = client.answer("find_implementations", &json!({"name": "Debug"}));
    let a_function = client.answer("find_implementations", &json!({"name": "Write"}));

    assert!(
        bare.contains("outside_graph=\"true\"")
            && bare.contains("count=\"2\"")
            && bare.contains("name=\"Module\"")
            && bare.contains("name=\"Path\""),
        "the types whose impls name Display: {bare}"
    );
    assert!(
        path.contains("outside_graph=\"true\"")
            && path.contains("count=\"1\"")
            && path.contains("name=\"Path\""),
        "only the impl that writes the whole path: {path}"
    );
    assert!(
        !narrowed.contains("outside_graph")
            && !on_a_line.contains("outside_graph")
            && !unwritten.contains("outside_graph"),
        "a path prefix and a line look among definitions, and no parent writes Debug: {narrowed} {on_a_line} \
         {unwritten}"
    );
    assert!(
        a_function.contains("outside_graph=\"true\"") && a_function.contains("name=\"Sink\""),
        "a name the tree holds only as a function still lists the parent's implementors: {a_function}"
    );
}

#[test]
fn find_implementations_of_an_interface_outside_the_graph_lists_the_type_an_assertion_names() {
    let tree = tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("go.mod", "module m\n\ngo 1.22\n"),
        (
            "a/a.go",
            "package a\n\nimport \"io\"\n\ntype R struct{}\n\nfunc (R) Read(p []byte) (int, error) { return 0, nil \
             }\n\nvar _ io.Reader = R{}\n\ntype Reader struct{}\n",
        ),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("find_implementations", &json!({"name": "io.Reader"}));

    assert!(
        answer.contains("outside_graph=\"true\" namesakes=\"1\"")
            && answer.contains("name=\"R\" qn=\"a.R\"")
            && !answer.contains("name=\"Reader\""),
        "the type the assertion names, beside the package's own Reader: {answer}"
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
        r#"name="use" qn="a.use" lines="18" role="consumer" site="read""#,
        r#"name="set" qn="a.set" lines="20" role="consumer" site="write""#,
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
        (
            "impact_of_edit",
            json!({"file": "pkg/service.go", "start_line": 1, "end_line": 0}),
        ),
        ("find_symbols", json!({"query": "x", "languages": ["nut"]})),
        ("impact_of_change", json!({})),
        (
            "impact_of_change",
            json!({"regions": [{"file": "pkg/service.go", "start_line": 5, "end_line": 2}]}),
        ),
        (
            "impact_of_change",
            json!({"regions": [{"file": "pkg/service.go", "start_line": 1, "end_line": 0}]}),
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
            json!({"from": "One", "to": "util.Parse", "to_path_prefix": "a/alpha/"}),
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
                    r#"<match lang="go" from="util.One" from_loc="a/util/util.go:5" to="util.Parse" "#,
                    r#"to_loc="a/util/util.go:3" hops="1" max_hops="8" "#,
                    r#"passed_over_to="1">"#,
                ),
                r#"<ref name="One" qn="util.One" loc="a/util/util.go:5" role="consumer" at="5"/>"#,
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
            json!({"name": "util.Parse", "path_prefix": "a/alpha/"}),
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
                r#"<ref name="One" qn="util.One" loc="a/util/util.go:5" role="consumer" hops="1" at="5"/>"#,
                r#"<ref name="Two" qn="util.Two" loc="a/util/util.go:7" role="consumer" hops="1" at="7"/>"#,
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
                    r#"at="5"/>"#,
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
                r#"<ref name="a" loc="o.cpp:1" role="consumer" hops="1" at="4"/>"#,
                r#"<candidate name="f" qn="ns::f" kind="function" loc="o.cpp:5" callers="0"/>"#,
                "</match>",
                "</callees>",
            ]),
            lines(&[
                r#"<callees of="ns::f" matches="1">"#,
                r#"<match lang="cpp" of="f" qn="ns::f" loc="o.cpp:5" count="1" unexpanded="1">"#,
                r#"<ref name="b" loc="o.cpp:2" role="consumer" hops="1" at="5"/>"#,
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
                r#"<ref name="b" loc="o.cpp:2" role="consumer" hops="1" at="5"/>"#,
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
                r#"<match lang="cpp" from="ns::f" from_loc="o.cpp:5" to="b" to_loc="o.cpp:2" hops="1" max_hops="8">"#,
                r#"<ref name="f" qn="ns::f" loc="o.cpp:5" role="consumer" at="5"/>"#,
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
                    r#"at="7"/>"#,
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
                    r#"at="7"/>"#,
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
                r#"<ref name="Other" qn="util.Other" lines="9-12" role="consumer" site="read" at="10,11"/>"#,
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
                r#"<ref name="Run" qn="util.Run" lines="7" role="consumer" site="read" at="7"/>"#,
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
fn implementations_of_a_cpp_enum_answer_that_nothing_implements_it() {
    let answers = answers_over(
        &[(
            "e.cpp",
            "enum Plain { A };\nenum class Scoped { B };\ntypedef enum Pair { C } Pair;\nstruct Child : Plain {};\n",
        )],
        &[
            ("find_implementations", json!({"name": "Plain"})),
            ("find_implementations", json!({"name": "Scoped"})),
            ("find_implementations", json!({"name": "Pair"})),
            ("find_implementations", json!({"name": "Unheld"})),
        ],
    );

    assert_eq!(
        answers,
        [
            r#"<implementations of="Plain" count="0">the symbol is indexed but nothing implements or extends it</implementations>"#,
            r#"<implementations of="Scoped" count="0">the symbol is indexed but nothing implements or extends it</implementations>"#,
            r#"<implementations of="Pair" count="0">the symbol is indexed but nothing implements or extends it</implementations>"#,
            r#"<implementations of="Unheld" count="0">no symbol matches - find_symbols can pin the exact name</implementations>"#,
        ]
    );
}

#[test]
fn a_name_held_only_as_kinds_a_tool_refuses_names_its_kinds_and_definitions() {
    let answers = answers_over(
        &[
            ("go.mod", "module example.com/m\n\ngo 1.22\n"),
            (
                "g/g.go",
                "package g\n\ntype Celsius float64\n\ntype Base struct{}\n\ntype Boxed struct{ Base }\n\nfunc shared() {}\n",
            ),
            ("f.cpp", "void cfree() {}\nint cvar;\nint shared;\n"),
            ("Cargo.toml", "[package]\nname = \"r\"\n"),
            ("src/lib.rs", "pub enum Colour {\n    Crimson,\n}\n"),
            ("d/1.cpp", "void dup() {}\n"),
            ("d/2.cpp", "void dup() {}\n"),
            ("d/3.cpp", "void dup() {}\n"),
            ("d/4.cpp", "void dup() {}\n"),
            ("d/5.cpp", "void dup() {}\n"),
            ("d/6.cpp", "void dup() {}\n"),
        ],
        &[
            ("find_implementations", json!({"name": "cfree"})),
            (
                "find_implementations",
                json!({"name": "Celsius", "path_prefix": "f.cpp"}),
            ),
            ("find_implementations", json!({"name": "shared"})),
            ("find_references", json!({"name": "Crimson"})),
            ("find_implementations", json!({"name": "dup"})),
            ("find_implementations", json!({"name": "Base"})),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<implementations of="cfree" count="0">"#,
                "the name is held only as function (1) - this tool takes no root of that kind:",
                r#"<candidate name="cfree" kind="function" loc="f.cpp:1" lang="cpp"/>"#,
                "</implementations>",
            ]),
            lines(&[
                r#"<implementations of="Celsius" count="0">"#,
                "the name is held only as defined-type (1) - this tool takes no root of that kind:",
                r#"<candidate name="Celsius" qn="g.Celsius" kind="defined-type" loc="g/g.go:3" lang="go"/>"#,
                "</implementations>",
            ]),
            lines(&[
                r#"<implementations of="shared" count="0">"#,
                "the name is held only as function (1), variable (1) - this tool takes no root of those kinds:",
                r#"<candidate name="shared" qn="g.shared" kind="function" loc="g/g.go:9" lang="go"/>"#,
                r#"<candidate name="shared" kind="variable" loc="f.cpp:3" lang="cpp"/>"#,
                "</implementations>",
            ]),
            lines(&[
                r#"<references of="Crimson" matches="0">"#,
                "the name is held only as enum-variant (1) - this tool takes no root of that kind:",
                r#"<candidate name="Crimson" qn="Colour::Crimson" kind="enum-variant" loc="src/lib.rs:2" lang="rust"/>"#,
                "</references>",
            ]),
            lines(&[
                r#"<implementations of="dup" count="0">"#,
                concat!(
                    "the name is held only as function (6) - this tool takes no root of that kind, the first 5 of 6 ",
                    "listed:",
                ),
                r#"<candidate name="dup" kind="function" loc="d/1.cpp:1" lang="cpp"/>"#,
                r#"<candidate name="dup" kind="function" loc="d/2.cpp:1" lang="cpp"/>"#,
                r#"<candidate name="dup" kind="function" loc="d/3.cpp:1" lang="cpp"/>"#,
                r#"<candidate name="dup" kind="function" loc="d/4.cpp:1" lang="cpp"/>"#,
                r#"<candidate name="dup" kind="function" loc="d/5.cpp:1" lang="cpp"/>"#,
                "a fixed cap cut these candidates - find_symbols of this name with limit 6 lists the rest",
                "</implementations>",
            ]),
            lines(&[
                r#"<implementations of="Base" count="1">"#,
                r#"<type name="Boxed" qn="g.Boxed" lang="go" loc="g/g.go:7"/>"#,
                "</implementations>",
            ]),
        ]
    );
}

#[test]
fn an_impl_of_an_outside_trait_does_not_implement_an_enum_of_its_name() {
    let answers = answers_over(
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"r\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            ),
            ("src/lib.rs", "pub mod policy;\nmod surface;\n"),
            ("src/policy.rs", "pub enum Visit {\n    Keep,\n}\n"),
            (
                "src/surface.rs",
                "use syn::visit::Visit;\n\nstruct Surface;\n\nimpl<'ast> Visit<'ast> for Surface {}\n",
            ),
        ],
        &[("find_implementations", json!({"name": "Visit"}))],
    );

    assert_eq!(
        answers,
        [
            r#"<implementations of="Visit" count="0">the symbol is indexed but nothing implements or extends it</implementations>"#
        ]
    );
}

#[test]
fn a_cpp_base_names_a_class_past_a_nearer_enum_of_its_name() {
    let answers = answers_over(
        &[(
            "b.cpp",
            "struct Base {};\nnamespace ns {\nenum Base { A };\nstruct Child : Base {};\n}\n",
        )],
        &[(
            "find_implementations",
            json!({"name": "Base", "path_prefix": "b.cpp", "line": 1}),
        )],
    );

    assert_eq!(
        answers,
        [lines(&[
            r#"<implementations of="Base" count="1">"#,
            r#"<type name="Child" qn="ns::Child" lang="cpp" loc="b.cpp:4"/>"#,
            "</implementations>",
        ])]
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
                    r#"<match lang="go" from="util.Run" from_loc="a/util/util.go:7" to="util.Check" "#,
                    r#"to_loc="b/util/util.go:5" max_hops="8" passed_over_from="1" found="false">"#,
                ),
                concat!(
                    "no call chain within 8 calls - the graph misses dynamic and unresolved calls, so this does not ",
                    "show that the code holds none; only from_loc to to_loc was searched - from_path_prefix and ",
                    "from_line, or to_path_prefix and to_line, pick a candidate",
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
                concat!(
                    r#"<match lang="go" from="util.Run" from_loc="b/util/util.go:7" to="util.Check" "#,
                    r#"to_loc="b/util/util.go:5" hops="1" max_hops="8">"#,
                ),
                r#"<ref name="Run" qn="util.Run" loc="b/util/util.go:7" role="consumer" at="7"/>"#,
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
                concat!(
                    r#"<match lang="go" from="util.Three" from_loc="b/util/util.go:5" to="util.Parse" "#,
                    r#"to_loc="b/util/util.go:3" hops="1" max_hops="8">"#,
                ),
                r#"<ref name="Three" qn="util.Three" loc="b/util/util.go:5" role="consumer" at="5"/>"#,
                "</match>",
                "</call_path>",
            ]),
        ]
    );
}

#[test]
fn find_references_names_the_files_whose_top_level_reads_a_constant_and_sees_it() {
    let answers = answers_over(
        &[
            ("ts/a.ts", "export const LIMIT = 3;\n"),
            (
                "ts/b.ts",
                "import { LIMIT } from './a';\nconsole.log(LIMIT);\n\nexport function f() {\n  return LIMIT;\n}\n",
            ),
            ("ts/c.ts", "console.log(LIMIT);\n"),
        ],
        &[
            ("find_references", json!({"name": "LIMIT"})),
            (
                "find_references",
                json!({"name": "LIMIT", "access": "write"}),
            ),
            (
                "find_references",
                json!({"name": "LIMIT", "min_confidence": "certain"}),
            ),
        ],
    );

    assert_eq!(
        answers,
        [
            lines(&[
                r#"<references of="LIMIT" matches="1">"#,
                concat!(
                    r#"<match lang="jsts" of="LIMIT" loc="ts/a.ts:1" kind="constant" count="1" files="1" "#,
                    r#"reads="1" file_scope="1">"#,
                ),
                r#"<file path="ts/b.ts" sites="1">"#,
                r#"<ref name="f" lines="4-6" role="consumer" site="read" at="5" confidence="likely"/>"#,
                "</file>",
                r#"<file_scope path="ts/b.ts" confidence="likely"/>"#,
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="LIMIT" matches="1">"#,
                concat!(
                    r#"<match lang="jsts" of="LIMIT" loc="ts/a.ts:1" kind="constant" count="0" reads="1" "#,
                    r#"file_scope="1">"#,
                ),
                r#"<file_scope path="ts/b.ts" confidence="likely"/>"#,
                "</match>",
                "</references>",
            ]),
            lines(&[
                r#"<references of="LIMIT" matches="1">"#,
                concat!(
                    r#"<match lang="jsts" of="LIMIT" loc="ts/a.ts:1" kind="constant" count="0" "#,
                    r#"below_confidence="2">no recorded site at this confidence floor - 2 uses bind below it, "#,
                    r#"and min_confidence "any" lists them</match>"#,
                ),
                "</references>",
            ]),
        ],
        "b.ts imports LIMIT and reads it at its top level, which no access filters and the floor counts with f's \
         read; c.ts sees no LIMIT"
    );
}

#[test]
fn find_implementations_locates_each_type_by_its_span() {
    let answers = answers_over(
        &[(
            "shapes.ts",
            "interface Shape {\n  area(): number;\n}\n\nclass Square implements Shape {\n  area() {\n    return 1;\n  \
             }\n}\n\nclass Widget extends HTMLElement {\n  connect() {}\n}\n",
        )],
        &[
            ("find_implementations", json!({"name": "Shape"})),
            ("find_implementations", json!({"name": "HTMLElement"})),
        ],
    );

    let types: Vec<Vec<&str>> = answers
        .iter()
        .map(|answer| {
            answer
                .lines()
                .filter(|line| line.starts_with("<type "))
                .collect()
        })
        .collect();
    assert_eq!(
        types,
        [
            vec![r#"<type name="Square" lang="jsts" loc="shapes.ts:5-9"/>"#],
            vec![r#"<type name="Widget" lang="jsts" loc="shapes.ts:11-13"/>"#],
        ],
        "an implementor in the graph and one of a parent outside it each carry its span: {answers:?}"
    );
}
