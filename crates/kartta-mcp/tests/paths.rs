//! The path arguments of the file and symbol tools, read as the index spells a path.
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

use serde_json::{Value, json};

use common::{Client, go_module, tree};

/// A call that takes a path: its tool, its arguments with the path spelled `"{}"`, a path of the Go module it names,
/// what its error names the argument by, and the nearest path the index holds to a misspelling of it.
struct PathCall {
    tool: &'static str,
    arguments: &'static str,
    path: &'static str,
    label: &'static str,
    nearest: &'static str,
}

const PATH_CALLS: [PathCall; 13] = [
    PathCall {
        tool: "file_outline",
        arguments: r#"{"file": "{}"}"#,
        path: "pkg/service.go",
        label: "",
        nearest: "pkg/service.go",
    },
    PathCall {
        tool: "find_importers",
        arguments: r#"{"file": "{}"}"#,
        path: "pkg/service.go",
        label: "",
        nearest: "pkg/service.go",
    },
    PathCall {
        tool: "impact_of_edit",
        arguments: r#"{"file": "{}", "start_line": 7}"#,
        path: "pkg/service.go",
        label: "",
        nearest: "pkg/service.go",
    },
    PathCall {
        tool: "impact_of_change",
        arguments: r#"{"regions": [{"file": "{}"}]}"#,
        path: "pkg/service.go",
        label: "regions[0]: ",
        nearest: "pkg/service.go",
    },
    PathCall {
        tool: "list_dir",
        arguments: r#"{"path": "{}"}"#,
        path: "cmd/app",
        label: "",
        nearest: "cmd/app",
    },
    PathCall {
        tool: "find_unused",
        arguments: r#"{"path_prefix": "{}"}"#,
        path: "pkg/",
        label: "path_prefix: ",
        nearest: "pkg",
    },
    PathCall {
        tool: "find_callers",
        arguments: r#"{"name": "Greet", "path_prefix": "{}"}"#,
        path: "pkg/",
        label: "path_prefix: ",
        nearest: "pkg",
    },
    PathCall {
        tool: "find_callees",
        arguments: r#"{"name": "main", "path_prefix": "{}"}"#,
        path: "cmd/app/",
        label: "path_prefix: ",
        nearest: "cmd/app",
    },
    PathCall {
        tool: "find_references",
        arguments: r#"{"name": "New", "path_prefix": "{}"}"#,
        path: "pkg/service.go",
        label: "path_prefix: ",
        nearest: "pkg/service.go",
    },
    PathCall {
        tool: "find_implementations",
        arguments: r#"{"name": "Service", "path_prefix": "{}"}"#,
        path: "pkg/",
        label: "path_prefix: ",
        nearest: "pkg",
    },
    PathCall {
        tool: "find_call_path",
        arguments: r#"{"from": "main", "from_path_prefix": "{}", "to": "Greet"}"#,
        path: "cmd/app/",
        label: "from_path_prefix: ",
        nearest: "cmd/app",
    },
    PathCall {
        tool: "find_call_path",
        arguments: r#"{"from": "main", "to": "Greet", "to_path_prefix": "{}"}"#,
        path: "pkg/",
        label: "to_path_prefix: ",
        nearest: "pkg",
    },
    PathCall {
        tool: "read_definitions",
        arguments: r#"{"names": ["New"], "from": "{}"}"#,
        path: "pkg/service.go",
        label: "from: ",
        nearest: "pkg/service.go",
    },
];

/// The fix a fault's error names.
const FIX: &str =
    "pass the path from the repository root, with / between names, as find_files prints it";

/// What the error for a path nothing is indexed at says the index does not hold.
const UNHELD: &str = "the index holds no ignored path, nor one made since it last read the tree";

fn spelled(arguments: &str, path: &str) -> Value {
    let quoted = serde_json::to_string(path).expect("a path writes as JSON");
    serde_json::from_str(&arguments.replace(r#""{}""#, &quoted)).expect("the arguments are JSON")
}

fn indexed() -> (tempfile::TempDir, Client) {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());
    (tree, client)
}

#[test]
fn a_path_with_a_leading_dot_slash_answers_as_the_path_after_it() {
    let (_tree, mut client) = indexed();

    for PathCall {
        tool,
        arguments,
        path,
        ..
    } in PATH_CALLS
    {
        let plain = client.answer(tool, &spelled(arguments, path));
        let dotted = client.answer(tool, &spelled(arguments, &format!("./{path}")));

        assert_eq!(dotted, plain, "{tool} {arguments} with ./{path}");
    }
}

#[test]
fn a_misspelled_path_is_a_tool_error_naming_its_fault_and_the_nearest_held_path() {
    let (_tree, mut client) = indexed();

    for call in PATH_CALLS {
        let backslashed = call.path.replace('/', "\\");
        for (path, fault) in [
            (
                backslashed.clone(),
                format!("{backslashed:?} separates its names with \\"),
            ),
            (
                format!("C:\\repo\\{backslashed}"),
                format!(
                    "{:?} starts with the drive C:",
                    format!("C:\\repo\\{backslashed}")
                ),
            ),
            (
                format!("/repo/{}", call.path),
                format!("{:?} is absolute", format!("/repo/{}", call.path)),
            ),
            (
                format!("../app/{}", call.path),
                format!(
                    "{:?} holds a .. name, which may leave the root",
                    format!("../app/{}", call.path)
                ),
            ),
        ] {
            let answer = client.call(call.tool, &spelled(call.arguments, &path));

            assert!(answer.is_error, "{} {path}: {}", call.tool, answer.text);
            assert_eq!(
                answer.text.lines().next(),
                Some(
                    format!(
                        "{}{fault} - {FIX}; the nearest the index holds:",
                        call.label
                    )
                    .as_str()
                ),
                "{} {path}",
                call.tool
            );
            assert!(
                answer.text.lines().skip(1).any(|near| near == call.nearest),
                "{} {path}: {}",
                call.tool,
                answer.text
            );
        }
    }
}

#[test]
fn a_path_nothing_is_indexed_at_is_a_tool_error_naming_the_nearest_held_paths() {
    let (_tree, mut client) = indexed();

    let misspelled = client.call("file_outline", &json!({"file": "pkg/servce.go"}));
    let misplaced = client.call("find_importers", &json!({"file": "lib/service.go"}));
    let far = client.call(
        "impact_of_edit",
        &json!({"file": "zzz.qq", "start_line": 1}),
    );
    let prefix = client.call("find_unused", &json!({"path_prefix": "pkgs/"}));
    let dir = client.call("list_dir", &json!({"path": "cmd/ap"}));
    let shallow_dir = client.call("list_dir", &json!({"path": "cm"}));
    let under_held = client.call("find_unused", &json!({"path_prefix": "pkg/nope/"}));
    let root = client.call("list_dir", &json!({"path": "/"}));
    let a_dir = client.call("file_outline", &json!({"file": "pkg"}));

    for (answer, text) in [
        (
            &misspelled,
            format!(
                "nothing is indexed at \"pkg/servce.go\" - {UNHELD}; the nearest the index holds:\npkg/service.go"
            ),
        ),
        (
            &misplaced,
            format!(
                "nothing is indexed at \"lib/service.go\" - {UNHELD}; the nearest the index holds:\npkg/service.go"
            ),
        ),
        (
            &far,
            format!("nothing is indexed at \"zzz.qq\" - {UNHELD}; no indexed path is near it"),
        ),
        (
            &prefix,
            format!(
                "path_prefix: no indexed path starts with \"pkgs/\" - {UNHELD}; the nearest the index holds:\npkg"
            ),
        ),
        (
            &dir,
            format!(
                "nothing is indexed at \"cmd/ap\" - {UNHELD}; the nearest the index holds:\ncmd\ncmd/app"
            ),
        ),
        (
            &shallow_dir,
            format!("nothing is indexed at \"cm\" - {UNHELD}; the nearest the index holds:\ncmd"),
        ),
        (
            &under_held,
            format!(
                "path_prefix: no indexed path starts with \"pkg/nope/\" - {UNHELD}; the nearest the index holds:\npkg"
            ),
        ),
        (
            &root,
            format!("\"/\" is absolute - {FIX}; no indexed path is near it"),
        ),
        (
            &a_dir,
            "\"pkg\" is a directory - pass one of its files, which list_dir lists".to_owned(),
        ),
    ] {
        assert!(answer.is_error, "{answer:?}");
        assert_eq!(answer.text, text);
    }
}

#[test]
fn the_entries_beside_a_missed_name_rank_by_how_much_of_its_start_they_share() {
    let tree = tree(&[
        ("src/serde.rs", "fn a() {}\n"),
        ("src/server.rs", "fn b() {}\n"),
        ("src/service.rs", "fn c() {}\n"),
        ("src/zz.rs", "fn d() {}\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let missed = client.call("file_outline", &json!({"file": "src/servic.rs"}));

    assert!(missed.is_error, "{missed:?}");
    assert_eq!(
        missed.text.lines().skip(1).collect::<Vec<_>>(),
        ["src/service.rs", "src/server.rs", "src/serde.rs"]
    );
}

#[test]
fn a_miss_names_at_most_five_near_paths() {
    let tree = tree(&[
        ("a/x.rs", "fn a() {}\n"),
        ("b/x.rs", "fn b() {}\n"),
        ("c/x.rs", "fn c() {}\n"),
        ("d/x.rs", "fn d() {}\n"),
        ("e/x.rs", "fn e() {}\n"),
        ("f/x.rs", "fn f() {}\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let missed = client.call("file_outline", &json!({"file": "g/x.rs"}));

    assert!(missed.is_error, "{missed:?}");
    assert_eq!(
        missed.text.lines().skip(1).collect::<Vec<_>>(),
        ["a/x.rs", "b/x.rs", "c/x.rs", "d/x.rs", "e/x.rs"]
    );
}

#[test]
fn a_file_with_a_trailing_slash_is_no_prefix_it_starts() {
    let (_tree, mut client) = indexed();

    let missed = client.call(
        "find_callers",
        &json!({"name": "Greet", "path_prefix": "pkg/service.go/"}),
    );

    assert!(missed.is_error, "{missed:?}");
    assert_eq!(
        missed.text,
        format!(
            "path_prefix: no indexed path starts with \"pkg/service.go/\" - {UNHELD}; the nearest the index holds:\n\
             pkg\npkg/service.go"
        )
    );
    let doubled = client.call(
        "find_callers",
        &json!({"name": "Greet", "path_prefix": "pkg//"}),
    );
    assert!(doubled.is_error, "{doubled:?}");
}

#[test]
fn a_prefix_from_the_filesystem_root_is_a_tool_error() {
    let (_tree, mut client) = indexed();

    for prefix in ["/", "/pkg", "/pkg/"] {
        let missed = client.call(
            "find_callers",
            &json!({"name": "Greet", "path_prefix": prefix}),
        );

        assert!(missed.is_error, "{prefix}: {missed:?}");
        assert!(
            missed
                .text
                .starts_with(&format!("path_prefix: {prefix:?} is absolute - {FIX}")),
            "{}",
            missed.text
        );
    }
}

#[test]
fn a_prefix_that_starts_a_held_name_selects_as_it_did() {
    let (_tree, mut client) = indexed();

    let started = client.answer(
        "find_callers",
        &json!({"name": "Greet", "path_prefix": "pk"}),
    );
    let named = client.answer(
        "find_callers",
        &json!({"name": "Greet", "path_prefix": "pkg/serv"}),
    );
    let empty = client.answer("find_callers", &json!({"name": "Greet", "path_prefix": ""}));

    for answer in [&started, &named, &empty] {
        assert!(
            answer.starts_with(r#"<callers of="Greet" matches="1">"#),
            "{answer}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_held_name_with_a_backslash_answers_as_any_held_name() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("odd\\dir/odd\\name.go", "package odd\n\nfunc Odd() {}\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let outline = client.answer("file_outline", &json!({"file": "odd\\dir/odd\\name.go"}));
    let listing = client.answer("list_dir", &json!({"path": "odd\\dir"}));
    let unused = client.answer("find_unused", &json!({"path_prefix": "odd\\"}));

    assert!(outline.contains(r#"<symbol name="Odd""#), "{outline}");
    assert!(
        listing.contains(r#"path="odd\\dir/odd\\name.go""#),
        "{listing}"
    );
    assert!(unused.contains(r#"<symbol name="Odd""#), "{unused}");
}

#[cfg(unix)]
#[test]
fn a_file_answer_about_a_symlink_names_the_path_to_pass_instead() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/a.go", "package pkg\n\nfunc A() {}\n"),
        ("notes.txt", "notes\n"),
    ]);
    for (link, target) in [
        ("alink.go", "pkg/a.go"),
        ("link", "pkg"),
        ("flink", "notes.txt"),
        ("out.go", "../elsewhere.go"),
        ("chain.go", "alink.go"),
        ("slink", "sock"),
    ] {
        std::os::unix::fs::symlink(target, tree.path().join(link)).expect("a symlink");
    }
    let _socket =
        std::os::unix::net::UnixListener::bind(tree.path().join("sock")).expect("a socket");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("file_outline", &json!({"file": "slink"})),
        concat!(
            r#"<outline of="slink" symbols="0">this path is a symlink to "sock", the indexed "sock", which is "#,
            "neither a file nor a directory, and no code graph holds a symlink</outline>",
        ),
        "a link to a socket names no file to pass"
    );
    for (file, points, next) in [
        (
            "alink.go",
            r#""pkg/a.go", the indexed file "pkg/a.go""#,
            r#"pass "pkg/a.go""#,
        ),
        (
            "link",
            r#""pkg", the indexed directory "pkg""#,
            "pass one of that directory's files, which list_dir lists",
        ),
        (
            "flink",
            r#""notes.txt", the indexed file "notes.txt""#,
            r#"pass "notes.txt""#,
        ),
        (
            "out.go",
            r#""../elsewhere.go", which names nothing the index holds"#,
            "read it",
        ),
        (
            "chain.go",
            r#""alink.go", the indexed symlink "alink.go""#,
            r#"pass "alink.go""#,
        ),
    ] {
        let note = format!(
            "this path is a symlink to {points}, and no code graph holds a symlink - {next}"
        );
        assert_eq!(
            [
                client.answer("file_outline", &json!({"file": file})),
                client.answer("find_importers", &json!({"file": file})),
                client.answer("impact_of_edit", &json!({"file": file, "start_line": 1})),
            ],
            [
                format!(r#"<outline of="{file}" symbols="0">{note}</outline>"#),
                format!(r#"<importers of="{file}" count="0">{note}</importers>"#),
                format!(
                    r#"<impact of="{file}" start_line="1" end_line="end" symbols="0">{note}</impact>"#
                ),
            ]
        );
    }
    let change = client.answer(
        "impact_of_change",
        &json!({"regions": [{"file": "alink.go"}, {"file": "link"}, {"file": "pkg/a.go"}]}),
    );
    assert!(
        change.contains(r#"unheld_regions="2" symlink_regions="2">"#),
        "{change}"
    );
}

#[cfg(unix)]
#[test]
fn a_file_answer_about_a_socket_names_no_graph() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/a.go", "package pkg\n\nfunc A() {}\n"),
    ]);
    let _sockets = ["pipe.go", "_skip.go", "sock"].map(|name| {
        std::os::unix::net::UnixListener::bind(tree.path().join(name)).expect("a socket")
    });
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let note = "this path is a socket, a fifo or a device node, not a regular file, and no code graph holds one";
    for file in ["pipe.go", "_skip.go", "sock"] {
        assert_eq!(
            [
                client.answer("file_outline", &json!({"file": file})),
                client.answer("find_importers", &json!({"file": file})),
                client.answer("impact_of_edit", &json!({"file": file, "start_line": 1})),
            ],
            [
                format!(r#"<outline of="{file}" symbols="0">{note}</outline>"#),
                format!(r#"<importers of="{file}" count="0">{note}</importers>"#),
                format!(
                    r#"<impact of="{file}" start_line="1" end_line="end" symbols="0">{note}</impact>"#
                ),
            ],
            "a socket claimed by its name, declined by its name, and unclaimed"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_change_counts_its_regions_of_special_files_apart() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/a.go", "package pkg\n\nfunc A() {}\n"),
    ]);
    let _sockets = ["sock.go", "sock"].map(|name| {
        std::os::unix::net::UnixListener::bind(tree.path().join(name)).expect("a socket")
    });
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let change = client.answer(
        "impact_of_change",
        &json!({"regions": [{"file": "sock.go"}, {"file": "sock"}, {"file": "pkg/a.go"}]}),
    );
    assert!(
        change.contains(r#"unheld_regions="2" special_regions="2">"#),
        "{change}"
    );
}

#[test]
fn a_held_file_no_graph_answers_for_says_why() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/blob.go", "\0\0\0"),
        ("pkg/_skip.go", "package pkg\n\nfunc Skipped() {}\n"),
        ("README.md", "# app\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let unclaimed =
        "no indexed language claims this file, so no code graph holds it - read or grep it";
    let declined = "the go language leaves this file out of its graph: no Go build reads a file whose name starts \
                    with _ or . - read or grep it";
    let unheld = "the go graph claims this file and holds no record of it, one of the files index_stats counts in \
                  unheld - read or grep it";
    for (file, note) in [
        ("README.md", unclaimed),
        ("pkg/_skip.go", declined),
        ("pkg/blob.go", unheld),
    ] {
        assert_eq!(
            [
                client.answer("file_outline", &json!({"file": file})),
                client.answer("find_importers", &json!({"file": file})),
                client.answer("impact_of_edit", &json!({"file": file, "start_line": 1})),
            ],
            [
                format!(r#"<outline of="{file}" symbols="0">{note}</outline>"#),
                format!(r#"<importers of="{file}" count="0">{note}</importers>"#),
                format!(
                    r#"<impact of="{file}" start_line="1" end_line="end" symbols="0">{note}</impact>"#
                ),
            ]
        );
    }
}

#[test]
fn a_held_file_of_a_language_the_build_left_out_names_the_filter() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/app.go", "package pkg\n\nfunc Run() {}\n"),
        ("pkg/_skip.go", "package pkg\n\nfunc Skipped() {}\n"),
        ("lib/core.rs", "pub fn run() {}\n"),
        ("README.md", "# app\n"),
    ]);
    let mut client = Client::start(&[]);
    client.answer(
        "index_repo",
        &json!({"root": tree.path(), "languages": ["rust"]}),
    );

    let unbuilt = "the go language claims this file, and index_repo's languages filter left it out of this index, \
                   so no code graph holds it - read or grep it, or index_repo with go in languages";
    let declined = "the go language leaves this file out of its graph: no Go build reads a file whose name starts \
                    with _ or . - read or grep it";
    let unclaimed =
        "no indexed language claims this file, so no code graph holds it - read or grep it";
    for (file, note) in [
        ("pkg/app.go", unbuilt),
        ("pkg/_skip.go", declined),
        ("README.md", unclaimed),
    ] {
        assert_eq!(
            [
                client.answer("file_outline", &json!({"file": file})),
                client.answer("find_importers", &json!({"file": file})),
                client.answer("impact_of_edit", &json!({"file": file, "start_line": 1})),
            ],
            [
                format!(r#"<outline of="{file}" symbols="0">{note}</outline>"#),
                format!(r#"<importers of="{file}" count="0">{note}</importers>"#),
                format!(
                    r#"<impact of="{file}" start_line="1" end_line="end" symbols="0">{note}</impact>"#
                ),
            ]
        );
    }
}

#[test]
fn a_change_to_files_of_a_language_the_build_left_out_counts_them_under_it() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/app.go", "package pkg\n\nfunc Run() {}\n"),
        ("pkg/more.go", "package pkg\n\nfunc More() {}\n"),
        ("pkg/_skip.go", "package pkg\n\nfunc Skipped() {}\n"),
        ("lib/core.rs", "pub fn run() {}\n"),
        ("README.md", "# app\n"),
    ]);
    let mut client = Client::start(&[]);
    client.answer(
        "index_repo",
        &json!({"root": tree.path(), "languages": ["rust"]}),
    );

    let answer = client.answer(
        "impact_of_change",
        &json!({"regions": [
            {"file": "pkg/app.go"},
            {"file": "pkg/_skip.go"},
            {"file": "README.md"},
            {"file": "pkg/more.go"},
        ]}),
    );

    assert_eq!(
        answer,
        [
            concat!(
                r#"<change regions="4" changed_symbols="0" changed_files="0" footprint="0" footprint_files="0" "#,
                r#"unheld_regions="4" unclaimed_regions="1">"#,
            ),
            concat!(
                "no symbols in these regions - they cover only file-scope code (imports, comments) or files no code ",
                "graph holds (counted in unheld_regions); zero symbols is not proof the change is safe, and ",
                "find_importers lists the files that include a changed file",
            ),
            concat!(
                r#"<declined language="go" regions="1">the go language leaves these regions' files out of its "#,
                "graph: no Go build reads a file whose name starts with _ or . - read or grep them</declined>",
            ),
            concat!(
                r#"<unbuilt language="go" regions="2">the go language claims these regions' files, and "#,
                "index_repo's languages filter left it out of this index - read or grep them, or index_repo with ",
                "go in languages</unbuilt>",
            ),
            "</change>",
        ]
        .join("\n")
    );
}
