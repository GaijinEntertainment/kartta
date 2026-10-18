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
        [
            "file_outline",
            "find_call_path",
            "find_callees",
            "find_callers",
            "find_files",
            "find_implementations",
            "find_importers",
            "find_references",
            "find_symbols",
            "glob_files",
            "grep",
            "impact_of_change",
            "impact_of_edit",
            "index_repo",
            "index_stats",
            "list_dir",
            "read_definitions"
        ]
    );
}

#[test]
fn glob_files_lists_in_path_order_and_says_when_the_cap_cut_it() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["*.go"]})),
        "cmd/app/main.go\npkg/service.go"
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["**"], "max_results": 2})),
        "README.md\ncmd/app/main.go\n(truncated: the first 2 matching paths in path order; more match)"
    );
    assert_eq!(
        client.answer(
            "glob_files",
            &json!({"globs": ["**"], "excludes": ["*.md"], "max_results": 1})
        ),
        "cmd/app/main.go\n(truncated: 1 of 3 matching paths, in path order)\n(excludes removed: 1)"
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["*.rs"]})),
        "no files match *.rs"
    );
    assert_eq!(
        client.answer(
            "glob_files",
            &json!({"globs": ["*.md"], "excludes": ["README.md"]})
        ),
        "no files match *.md outside the excludes (excludes removed: 1)"
    );
    assert!(client.call("glob_files", &json!({"globs": []})).is_error);
}

#[test]
fn grep_answers_lines_with_context_and_counts_what_it_did_not_cover() {
    let tree = go_module();
    std::fs::create_dir_all(tree.path().join("vendor/lib")).expect("the directory is created");
    std::fs::write(
        tree.path().join("vendor/lib/lib.go"),
        "package lib // Greet\n",
    )
    .expect("the file is written");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("grep", &json!({"patterns": ["Greet"]})),
        "<grep patterns=\"Greet\" candidates=\"4\" files=\"2\" matches=\"3\" excluded=\"1\">\n\
         <file path=\"cmd/app/main.go\" matches=\"1\">\n\
         11:\ts.Greet(\"x\")\n\
         </file>\n\
         <file path=\"pkg/service.go\" matches=\"2\">\n\
         11:// Greet greets name.\n\
         12:func (s *Service) Greet(name string) string {\n\
         </file>\n\
         </grep>"
    );
    assert_eq!(
        client.answer(
            "grep",
            &json!({
                "patterns": ["greet"],
                "case_insensitive": true,
                "includes": ["pkg/**"],
                "context_lines": 1,
                "max_results": 1
            })
        ),
        "<grep patterns=\"greet\" candidates=\"1\" files=\"1\" matches=\"1\" truncated=\"true\">\n\
         <file path=\"pkg/service.go\" matches=\"1\">\n\
         2-\n\
         3:// Service greets.\n\
         4-type Service struct{}\n\
         </file>\n\
         </grep>"
    );
    assert_eq!(
        client.answer(
            "grep",
            &json!({"patterns": ["Greet.*World"], "excludes": []})
        ),
        "<grep patterns=\"Greet.*World\" candidates=\"5\" files=\"0\" matches=\"0\">no line matches - matching is \
         per-line: \".*\"/\".+\" cannot cross line breaks; search the most distinctive part alone</grep>"
    );
    assert_eq!(
        client.answer("grep", &json!({"patterns": ["x"], "includes": ["*.rs"]})),
        "<grep patterns=\"x\" candidates=\"0\" files=\"0\" matches=\"0\">the include/exclude globs selected no files \
         - glob_files shows what they match</grep>"
    );
    assert!(client.call("grep", &json!({"patterns": ["("]})).is_error);
    assert!(client.call("grep", &json!({"patterns": [" "]})).is_error);
}

#[test]
fn list_dir_counts_what_it_did_not_enter() {
    let tree = go_module();
    std::fs::create_dir_all(tree.path().join("vendor/lib")).expect("the directory is created");
    std::fs::write(tree.path().join("vendor/lib/lib.go"), "package lib\n")
        .expect("the file is written");
    std::fs::write(tree.path().join(".env"), "A=1\n").expect("the file is written");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("list_dir", &json!({})),
        "<dir path=\".\" entries=\"6\" total=\"11\">\n\
         <entry path=\".env\" kind=\"file\" size=\"4\"/>\n\
         <entry path=\"README.md\" kind=\"file\" size=\"6\"/>\n\
         <entry path=\"cmd\" kind=\"dir\" unlisted=\"2\"/>\n\
         <entry path=\"go.mod\" kind=\"file\" size=\"32\"/>\n\
         <entry path=\"pkg\" kind=\"dir\" unlisted=\"1\"/>\n\
         <entry path=\"vendor\" kind=\"dir\" unlisted=\"2\"/>\n\
         </dir>"
    );
    assert_eq!(
        client.answer(
            "list_dir",
            &json!({"path": ".", "max_depth": 3, "max_entries": 4, "skip_dotfiles": true})
        ),
        "<dir path=\".\" entries=\"4\" total=\"11\" excluded=\"1\" truncated=\"true\">\n\
         <entry path=\"README.md\" kind=\"file\" size=\"6\"/>\n\
         <entry path=\"cmd\" kind=\"dir\"/>\n\
         <entry path=\"cmd/app\" kind=\"dir\"/>\n\
         <entry path=\"cmd/app/main.go\" kind=\"file\" size=\"128\"/>\n\
         </dir>"
    );
    assert_eq!(
        client.answer("list_dir", &json!({"path": "vendor", "excludes": []})),
        "<dir path=\"vendor\" entries=\"1\" total=\"2\">\n\
         <entry path=\"vendor/lib\" kind=\"dir\" unlisted=\"1\"/>\n\
         </dir>"
    );
    assert!(
        client
            .call("list_dir", &json!({"path": "nowhere"}))
            .is_error
    );
    assert!(client.call("list_dir", &json!({"path": "go.mod"})).is_error);
    assert!(client.call("list_dir", &json!({"path": "/"})).is_error);
}

#[test]
fn grep_and_list_dir_say_why_they_show_nothing() {
    let tree = common::tree(&[("blob.bin", "Greet\0\0"), ("conf/.env", "A=1\n")]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer(
            "grep",
            &json!({"patterns": ["Greet"], "includes": ["*.bin"]})
        ),
        "<grep patterns=\"Greet\" candidates=\"1\" files=\"0\" matches=\"0\" binary=\"1\">no line matches in the files \
         read - binary and oversized files are skipped unread</grep>"
    );
    assert_eq!(
        client.answer("list_dir", &json!({"path": "conf", "skip_dotfiles": true})),
        "<dir path=\"conf\" entries=\"0\" total=\"1\" excluded=\"1\">no entry shown - skip_dotfiles excluded every \
         entry</dir>"
    );
}

#[cfg(unix)]
#[test]
fn grep_says_when_its_globs_selected_symlinks_alone() {
    let tree = go_module();
    std::os::unix::fs::symlink("pkg/service.go", tree.path().join("link.go"))
        .expect("the symlink is created");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("grep", &json!({"patterns": ["x"], "includes": ["link.go"]})),
        "<grep patterns=\"x\" candidates=\"0\" files=\"0\" matches=\"0\" symlinks=\"1\">the include/exclude globs \
         selected symlinks alone, which are never read through - include their targets</grep>"
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
