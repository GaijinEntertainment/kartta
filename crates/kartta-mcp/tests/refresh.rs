//! The index following edits to its tree: the `refresh` tool, over the whole tree and at named paths.
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

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde_json::json;

use common::{Client, go_module};

/// Appends a function named `name` to the Go file at `path` under `root`.
fn add_function(root: &Path, path: &str, name: &str) {
    let mut file = OpenOptions::new()
        .append(true)
        .open(root.join(path))
        .expect("the file opens");
    writeln!(file, "\nfunc {name}() {{}}").expect("the file is written");
}

fn defines(client: &mut Client, name: &str) -> bool {
    client
        .answer("find_symbols", &json!({"query": name}))
        .contains(&format!("<hit name=\"{name}\""))
}

#[test]
fn a_refresh_of_the_tree_takes_in_an_edit_and_counts_it() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());
    add_function(tree.path(), "pkg/service.go", "Added");

    assert!(
        !defines(&mut client, "Added"),
        "an edit is not seen before a refresh"
    );
    let answer = client.answer("refresh", &json!({}));

    assert!(answer.starts_with("<refresh walked=\"tree\""), "{answer}");
    assert!(answer.contains(" extracted=\""), "{answer}");
    assert!(defines(&mut client, "Added"));
}

#[test]
fn a_refresh_of_named_paths_walks_those_alone() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());
    add_function(tree.path(), "pkg/service.go", "Added");

    let answer = client.answer(
        "refresh",
        &json!({"paths": ["pkg/service.go", "README.md"]}),
    );

    assert!(
        answer.starts_with("<refresh walked=\"2\" extracted=\"1\" elapsed_ms=\""),
        "{answer}"
    );
    assert!(defines(&mut client, "Added"));
}

#[test]
fn a_refresh_that_finds_no_change_says_so() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer("refresh", &json!({"paths": ["README.md"]}));

    assert!(
        answer.ends_with(">nothing changed - the index already matched the tree</refresh>"),
        "{answer}"
    );
}

#[test]
fn a_refresh_names_the_fix_for_an_absolute_path_and_a_missing_index() {
    let tree = go_module();
    let mut client = Client::start(&[]);

    let unindexed = client.call("refresh", &json!({}));
    client.index(tree.path());
    let absolute = client.call("refresh", &json!({"paths": ["/etc/hosts"]}));

    assert!(
        unindexed.is_error && unindexed.text.contains("call index_repo first"),
        "{unindexed:?}"
    );
    assert!(
        absolute.is_error && absolute.text.contains("relative to the repository root"),
        "{absolute:?}"
    );
}
