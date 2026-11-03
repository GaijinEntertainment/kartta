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

#[test]
fn a_held_file_no_graph_answers_for_says_why() {
    let tree = tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/blob.go", "\0\0\0"),
        ("README.md", "# app\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let unclaimed =
        "no indexed language claims this file, so no code graph holds it - read or grep it";
    let unheld = "the go graph claims this file and holds no record of it, one of the files index_stats counts in \
                  unheld - read or grep it";
    for (file, note) in [("README.md", unclaimed), ("pkg/blob.go", unheld)] {
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
