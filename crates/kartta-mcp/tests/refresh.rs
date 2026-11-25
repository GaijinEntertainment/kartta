//! The index following edits to its tree: the watch that takes each edit in by itself, and the `refresh` tool, over
//! the whole tree and at named paths.
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
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;

use common::{Client, go_module, rooted, tree};

/// How long a test waits for the watch to take an edit in, or for a rebuild to answer.
const PATIENCE: Duration = Duration::from_secs(20);

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
fn a_watched_index_takes_in_an_edit_by_itself() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    let indexed = client.index(tree.path());

    for name in ["Added", "AddedOnceTheWatchRuns"] {
        add_function(tree.path(), "pkg/service.go", name);
        let deadline = Instant::now() + PATIENCE;
        while !defines(&mut client, name) {
            assert!(
                Instant::now() < deadline,
                "the watch did not take {name} in"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(indexed.contains(" watch=\"following\" "), "{indexed}");
}

#[test]
fn an_unwatched_index_takes_in_an_edit_only_through_a_refresh_of_the_tree() {
    let tree = go_module();
    let mut client = Client::unwatched(&[]);
    let indexed = client.index(tree.path());
    add_function(tree.path(), "pkg/service.go", "Added");

    assert!(
        indexed.contains(" watch=\"off: turned off in the server configuration\" "),
        "{indexed}"
    );
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
    let mut client = Client::unwatched(&[]);
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
fn named_paths_past_the_limit_walk_the_tree_and_say_how_many_parts_they_came_to() {
    let tree = go_module();
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());
    let mut paths: Vec<String> = (0..51).map(|at| format!("pkg/gone{at}.go")).collect();
    paths.extend(["build/out/a.o".to_owned(), "build/b.o".to_owned()]);

    let answer = client.answer("refresh", &json!({ "paths": paths }));

    assert!(answer.starts_with("<refresh walked=\"tree\""), "{answer}");
    assert!(
        answer.contains(" widened=\"53\" parts=\"52\" limit=\"50\" "),
        "the two paths inside build/, which the index lacks, come to one part: {answer}"
    );
}

#[test]
fn a_refresh_that_finds_no_change_says_so() {
    let tree = go_module();
    let mut client = Client::unwatched(&[]);
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
    let mut client = Client::unwatched(&[]);

    let unindexed = client.call("refresh", &json!({}));
    client.index(tree.path());
    let absolute = client.call("refresh", &json!({"paths": ["/etc/hosts"]}));

    assert!(
        unindexed.is_error && unindexed.text.contains("call index_root first"),
        "{unindexed:?}"
    );
    assert!(
        absolute.is_error && absolute.text.contains("relative to the repository root"),
        "{absolute:?}"
    );
}

#[test]
fn a_refresh_refuses_a_path_spelled_the_windows_way_and_names_what_is_wrong() {
    let tree = go_module();
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());
    add_function(tree.path(), "pkg/service.go", "Added");

    let drive = client.call("refresh", &json!({"paths": ["C:\\repo\\pkg\\service.go"]}));
    let rooted = client.call("refresh", &json!({"paths": ["\\repo\\pkg\\service.go"]}));
    let backslash = client.call("refresh", &json!({"paths": ["pkg\\service.go"]}));
    let climbing = client.call("refresh", &json!({"paths": ["../app/pkg/service.go"]}));
    let refused = defines(&mut client, "Added");
    let slash = client.answer("refresh", &json!({"paths": ["pkg/service.go"]}));

    let fix = "pass paths relative to the repository root, with / between names, as find_files prints them";
    for (refusal, fault) in [
        (
            &drive,
            r#""C:\\repo\\pkg\\service.go" starts with the drive C:"#,
        ),
        (&rooted, r#""\\repo\\pkg\\service.go" is absolute"#),
        (
            &backslash,
            r#""pkg\\service.go" separates its names with \"#,
        ),
        (
            &climbing,
            r#""../app/pkg/service.go" holds a .. name, which may leave the root"#,
        ),
    ] {
        assert!(refusal.is_error, "{refusal:?}");
        assert_eq!(refusal.text, format!("{fault} - {fix}"));
    }
    assert!(!refused, "a refused path refreshes nothing");
    assert!(
        slash.starts_with("<refresh walked=\"1\" extracted=\"1\" "),
        "{slash}"
    );
    assert!(defines(&mut client, "Added"));
}

#[test]
fn indexing_another_root_while_the_first_is_watched_answers_for_the_second() {
    let first = go_module();
    let second = tree(&[("lib/only.go", "package lib\n\nfunc Only() {}\n")]);
    let mut client = Client::start(&[]);
    client.index(first.path());
    for at in 0..20 {
        add_function(first.path(), "pkg/service.go", &format!("Edit{at}"));
    }

    let (answered, answer) = mpsc::channel();
    let root = second.path().to_owned();
    std::thread::spawn(move || {
        let indexed = client.index(&root);
        let found = client.answer("find_files", &json!({"pattern": "go"}));
        let _ = answered.send((indexed, found));
    });
    let (indexed, found) = answer
        .recv_timeout(PATIENCE)
        .expect("the rebuild answers while the first index is watched");

    assert!(
        rooted(&indexed, second.path()).starts_with("<index root=\"<root>\" files=\"1\""),
        "{indexed}"
    );
    assert_eq!(found, "lib/only.go");
}
