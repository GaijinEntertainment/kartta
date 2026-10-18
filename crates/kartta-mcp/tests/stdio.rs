//! The server driven over stdio as an agent's client drives it: the handshake, the tool list, the lifecycle, and the
//! answers of each tool, pinned in full.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test), so the client's helpers fail the test by a failed expectation"
)]

mod common;

use serde_json::json;

use common::{Client, go_module, rooted};

#[test]
fn the_server_lists_its_tools() {
    let mut client = Client::start(&[]);

    let listed = client.request("tools/list", &json!({}));
    let mut names: Vec<&str> = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a named tool"))
        .collect();
    names.sort_unstable();

    assert_eq!(
        names,
        ["find_callers", "find_files", "index_repo", "index_stats"]
    );
}

#[test]
fn a_query_before_index_repo_names_the_fix() {
    let mut client = Client::start(&[]);

    let answer = client.call("find_files", &json!({"pattern": "main"}));

    assert!(answer.is_error);
    assert_eq!(answer.text, "no repository indexed: call index_repo first");
}

#[test]
fn index_repo_reports_each_graph() {
    let tree = go_module();
    let mut client = Client::start(&[]);

    let answer = client.index(tree.path());

    assert_eq!(
        rooted(&answer, tree.path()),
        "<index root=\"<root>\" files=\"4\" gitignore=\"respected\" symbols=\"5\" calls=\"3\">\n\
         <language name=\"go\" files=\"2\" symbols=\"5\" calls=\"3\" uses=\"4\" includes=\"1\"/>\n\
         </index>"
    );
    assert_eq!(client.answer("index_stats", &json!({})), answer);
}

#[test]
fn a_root_given_at_launch_builds_without_index_repo() {
    let tree = go_module();
    let mut client = Client::start(&[tree.path()]);

    let stats = client.wait_for_index();

    assert!(stats.starts_with("<index root="), "{stats}");
    assert_eq!(
        client.answer("find_files", &json!({"pattern": "service"})),
        "pkg/service.go"
    );
}

#[test]
fn an_unknown_language_lists_the_names_taken() {
    let tree = go_module();
    let mut client = Client::start(&[]);

    let answer = client.call(
        "index_repo",
        &json!({"root": tree.path(), "languages": ["golang"]}),
    );

    assert!(answer.is_error);
    assert!(
        answer
            .text
            .starts_with("unknown language \"golang\"; valid: go, cpp,"),
        "{}",
        answer.text
    );
}

#[test]
fn find_files_ranks_file_names_and_drops_the_excluded() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_files", &json!({"pattern": "go"})),
        "go.mod\npkg/service.go\ncmd/app/main.go"
    );
    assert_eq!(
        client.answer(
            "find_files",
            &json!({"pattern": "go", "excludes": ["cmd/**"], "max_results": 1})
        ),
        "go.mod"
    );
    assert_eq!(
        client.answer("find_files", &json!({"pattern": "zzz"})),
        "no files match \"zzz\""
    );
    assert!(client.call("find_files", &json!({"pattern": " "})).is_error);
}

#[test]
fn find_callers_walks_callers_with_the_line_of_each_call() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_callers", &json!({"name": "Greet"})),
        "<callers of=\"Greet\" matches=\"1\">\n\
         <match lang=\"go\" of=\"Greet\" qn=\"pkg.Service.Greet\" loc=\"pkg/service.go:12\" count=\"2\" \
         reach_band=\"low\">\n\
         <ref name=\"run\" qn=\"main.run\" loc=\"cmd/app/main.go:10\" role=\"consumer\" hops=\"1\" \
         at=\"cmd/app/main.go:11\"/>\n\
         <ref name=\"main\" qn=\"main.main\" loc=\"cmd/app/main.go:5\" role=\"entry\" hops=\"2\" \
         at=\"cmd/app/main.go:7\"/>\n\
         </match>\n\
         </callers>"
    );
    assert_eq!(
        client.answer(
            "find_callers",
            &json!({"name": "Greet", "hops": 1, "role": "entry"})
        ),
        "<callers of=\"Greet\" role=\"entry\" matches=\"1\">\n\
         <match lang=\"go\" of=\"Greet\" qn=\"pkg.Service.Greet\" loc=\"pkg/service.go:12\" count=\"1\" \
         reach_band=\"low\" unexpanded=\"1\" consumers=\"1\" tests=\"0\" entries=\"0\" generated=\"0\"/>\n\
         </callers>"
    );
}

#[test]
fn find_callers_offers_alternatives_to_an_unknown_name() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_callers", &json!({"name": "Gret"})),
        "<callers of=\"Gret\" matches=\"0\">\n\
         no symbol matches - did you mean:\n\
         <suggestion name=\"Greet\" qn=\"pkg.Service.Greet\" kind=\"method\" lang=\"go\" loc=\"pkg/service.go:12\"/>\n\
         </callers>"
    );
    assert_eq!(
        client.answer("find_callers", &json!({"name": "Greet", "languages": []})),
        "<callers of=\"Greet\" languages=\"[]\" matches=\"0\">no language selected - an empty languages list \
         searches no graph; omit languages to search every indexed language</callers>"
    );
}

#[test]
fn indexing_another_repository_replaces_the_first() {
    let first = go_module();
    let second = common::tree(&[("lib/only.go", "package lib\n\nfunc Only() {}\n")]);
    let mut client = Client::start(&[first.path()]);

    let answer = client.index(second.path());

    assert!(
        rooted(&answer, second.path()).starts_with("<index root=\"<root>\" files=\"1\""),
        "{answer}"
    );
    assert_eq!(
        client.answer("find_files", &json!({"pattern": "go"})),
        "lib/only.go"
    );
    assert_eq!(
        client.index(second.path()),
        answer,
        "a second build of the same root answers the same"
    );
}
