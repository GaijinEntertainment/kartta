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

use serde_json::Value;

use common::{Client, go_module};

/// Each call that takes a path, with the path spelled `{}`, and a path of the Go module it names.
const PATH_CALLS: [(&str, &str, &str); 13] = [
    ("file_outline", r#"{"file": "{}"}"#, "pkg/service.go"),
    ("find_importers", r#"{"file": "{}"}"#, "pkg/service.go"),
    (
        "impact_of_edit",
        r#"{"file": "{}", "start_line": 7}"#,
        "pkg/service.go",
    ),
    (
        "impact_of_change",
        r#"{"regions": [{"file": "{}"}]}"#,
        "pkg/service.go",
    ),
    ("list_dir", r#"{"path": "{}"}"#, "pkg"),
    ("find_unused", r#"{"path_prefix": "{}"}"#, "pkg/"),
    (
        "find_callers",
        r#"{"name": "Greet", "path_prefix": "{}"}"#,
        "pkg/",
    ),
    (
        "find_callees",
        r#"{"name": "main", "path_prefix": "{}"}"#,
        "cmd/",
    ),
    (
        "find_references",
        r#"{"name": "New", "path_prefix": "{}"}"#,
        "pkg/service.go",
    ),
    (
        "find_implementations",
        r#"{"name": "Service", "path_prefix": "{}"}"#,
        "pkg/",
    ),
    (
        "find_call_path",
        r#"{"from": "main", "from_path_prefix": "{}", "to": "Greet"}"#,
        "cmd/app/",
    ),
    (
        "find_call_path",
        r#"{"from": "main", "to": "Greet", "to_path_prefix": "{}"}"#,
        "pkg/",
    ),
    (
        "read_definitions",
        r#"{"names": ["New"], "from": "{}"}"#,
        "pkg/service.go",
    ),
];

fn spelled(arguments: &str, path: &str) -> Value {
    serde_json::from_str(&arguments.replace("{}", path)).expect("the arguments are JSON")
}

#[test]
fn a_path_with_a_leading_dot_slash_answers_as_the_path_after_it() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    for (tool, arguments, path) in PATH_CALLS {
        let plain = client.answer(tool, &spelled(arguments, path));
        let dotted = client.answer(tool, &spelled(arguments, &format!("./{path}")));

        assert_eq!(dotted, plain, "{tool} {arguments} with ./{path}");
    }
}
