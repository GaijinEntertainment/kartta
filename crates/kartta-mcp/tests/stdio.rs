//! The server driven over stdio as an agent's client drives it: the handshake, the tool list, the lifecycle, and the
//! answers of each tool, pinned in full.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test), so the client's helpers fail the test by a failed expectation"
)]

mod common;

use std::time::Duration;

use serde_json::json;

use common::{Client, go_module, rooted};

#[test]
fn a_grep_index_that_cannot_be_kept_costs_only_itself_and_stays_named_through_a_refresh() {
    let tree = go_module();
    let reason = "grep_index=\"off: cannot keep the content index in ";
    let mut client = Client::grep_indexed_in(&tree.path().join("gone"));

    let indexed = client.index(tree.path());
    let found = client.answer("grep", &json!({"patterns": ["Greet"]}));
    client.answer("refresh", &json!({}));
    let refreshed = client.answer("index_stats", &json!({}));

    assert!(indexed.contains(reason), "{indexed}");
    assert!(indexed.contains("<language name=\"go\""), "{indexed}");
    assert!(indexed.contains("<errors count=\"1\">"), "{indexed}");
    assert!(found.contains("func (s *Service) Greet"), "{found}");
    assert!(refreshed.contains(reason), "{refreshed}");
    assert!(!refreshed.contains("<errors"), "{refreshed}");
}

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
            "find_unused",
            "glob_files",
            "grep",
            "impact_of_change",
            "impact_of_edit",
            "index_repo",
            "index_stats",
            "list_dir",
            "read_definitions",
            "refresh"
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
        "cmd/app/main.go\n(truncated: the first 1 matching paths in path order; more match)\n\
         (excludes removed: 1)"
    );
    assert_eq!(
        client.answer(
            "glob_files",
            &json!({"globs": ["**"], "excludes": ["cmd"], "max_results": 2})
        ),
        "README.md\ngo.mod\n(truncated: the first 2 matching paths in path order; more match)\n\
         (excludes removed: 1)",
        "an excluded directory takes everything beneath it, and the cap holds"
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
    std::fs::create_dir(tree.path().join(".git")).expect("the root is a repository's top");
    let mut client = Client::start(&[]);

    let answer = client.index(tree.path());

    assert_eq!(
        rooted(&answer, tree.path()),
        "<index root=\"<root>\" files=\"4\" gitignore=\"in-tree\" watch=\"following\" symbols=\"5\" calls=\"3\">\n\
         <language name=\"go\" files=\"2\" symbols=\"5\" calls=\"3\" uses=\"4\" includes=\"1\"/>\n\
         </index>"
    );
    assert_eq!(client.answer("index_stats", &json!({})), answer);
}

#[test]
fn an_index_inside_a_repository_names_the_top_whose_rules_it_did_not_read_unless_it_applies_none() {
    let tree = common::tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        (".gitignore", "vendor/\n"),
        ("app/notes.txt", "notes\n"),
        ("app/vendor/lib.txt", "vendored\n"),
    ]);
    let root = tree.path().join("app");
    let top = tree.path().canonicalize().expect("the tree resolves");
    let mut client = Client::start(&[]);

    let answer = client.index(&root);

    let gitignore = format!(
        "in-tree: the root is inside the repository at {}, whose .gitignore files above the root were not read",
        top.display()
    );
    assert_eq!(
        answer,
        format!(
            "<index root={:?} files=\"2\" gitignore={gitignore:?} watch=\"following\" symbols=\"0\" calls=\"0\">the \
             code graph is empty</index>",
            root.display().to_string()
        ),
        "the rules of the top do not apply, so vendor/ is indexed"
    );
    let listed = client.request("tools/list", &json!({}));
    let description = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .find(|tool| tool["name"] == "index_stats")
        .and_then(|tool| tool["description"].as_str())
        .expect("index_stats is described");
    let quoted = gitignore.replace(&top.display().to_string(), "<top>");
    assert!(
        description.contains(&format!("gitignore={quoted:?}")),
        "the description quotes the answer: {description}"
    );
    assert_eq!(
        client.answer("index_repo", &json!({"root": root, "index_ignored": true})),
        format!(
            "<index root={:?} files=\"2\" gitignore=\"ignored\" watch=\"following\" symbols=\"0\" calls=\"0\">the \
             code graph is empty</index>",
            root.display().to_string()
        )
    );
}

#[test]
fn a_root_that_is_not_an_absolute_readable_directory_is_refused_and_the_index_stays() {
    let tree = go_module();
    let missing = tree.path().join("missing");
    let file = tree.path().join("go.mod");
    let mut client = Client::start(&[]);
    let indexed = client.index(tree.path());

    let refused: Vec<String> = [json!(missing), json!(file), json!("."), json!("")]
        .iter()
        .map(|root| {
            let answer = client.call("index_repo", &json!({"root": root}));
            assert!(answer.is_error, "{root} was indexed: {}", answer.text);
            answer.text
        })
        .collect();

    assert_eq!(client.answer("index_stats", &json!({})), indexed);
    for (refusal, root) in refused[..2].iter().zip([&missing, &file]) {
        assert!(
            refusal.starts_with(&format!("cannot index {}: ", root.display()))
                && refusal.ends_with(" - pass the absolute path of a readable directory"),
            "{refusal}"
        );
    }
    assert_eq!(
        refused[2..],
        [
            "the root \".\" is not an absolute path - pass the repository's absolute path",
            "the root \"\" is not an absolute path - pass the repository's absolute path",
        ]
    );
}

#[test]
fn index_stats_answers_an_envelope_over_nothing_indexed() {
    let mut client = Client::start(&[]);

    assert_eq!(
        client.answer("index_stats", &json!({})),
        "<index status=\"empty\">nothing is indexed - call index_repo with the repository's absolute root</index>"
    );
}

#[test]
fn index_stats_names_why_a_build_failed() {
    let tree = common::tree(&[]);
    let missing = tree.path().join("missing");
    let mut client = Client::start(&[&missing]);

    let stats = client.wait_for_index();

    let shown = rooted(&stats, &missing);
    assert!(
        shown.starts_with(
            "<index root=\"<root>\" status=\"failed\">cannot index <root>: cannot read directory <root>: "
        )
            && shown.ends_with(" - call index_repo again</index>"),
        "{stats}"
    );
}

#[test]
fn a_launch_build_that_fails_warns_on_stderr() {
    let tree = common::tree(&[]);
    let missing = tree.path().join("missing");

    let (_client, stderr) = Client::with_stderr(&[&missing]);

    let warning = stderr
        .recv_timeout(Duration::from_secs(60))
        .expect("the server warns");
    assert!(
        warning.starts_with(&format!(
            "kartta-mcp: the launch build ended without an index: cannot index {}: ",
            missing.display()
        )),
        "{warning}"
    );
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
        "go.mod\n(1 of 2 matching paths, best first)",
        "the count holds only what the excludes leave"
    );
    assert_eq!(
        client.answer(
            "find_files",
            &json!({"pattern": "go", "excludes": ["cmd", "go.mod"]})
        ),
        "pkg/service.go",
        "an excluded directory takes everything beneath it"
    );
    assert_eq!(
        client.answer("find_files", &json!({"pattern": "zzz"})),
        "no files match \"zzz\""
    );
    assert!(client.call("find_files", &json!({"pattern": " "})).is_error);
}

#[test]
fn find_files_counts_every_match_when_its_cap_cuts_the_list() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("find_files", &json!({"pattern": "go", "max_results": 2})),
        "go.mod\npkg/service.go\n(2 of 3 matching paths, best first)"
    );
    assert_eq!(
        client.answer("find_files", &json!({"pattern": "go", "max_results": 3})),
        "go.mod\npkg/service.go\ncmd/app/main.go",
        "a cap that cut nothing says nothing"
    );
}

#[test]
fn a_path_that_opens_with_a_parenthesis_is_quoted_so_it_cannot_read_as_the_closing_line() {
    let tree = common::tree(&[
        ("(x).txt", "x\n"),
        ("(x)/notes.md", "x\n"),
        ("a.txt", "x\n"),
    ]);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        [
            client.answer("glob_files", &json!({"globs": ["*.txt"], "max_results": 1})),
            client.answer("find_files", &json!({"pattern": "(x", "max_results": 1})),
        ],
        [
            "\"(x).txt\"\n(truncated: the first 1 matching paths in path order; more match)",
            "\"(x).txt\"\n(1 of 2 matching paths, best first)",
        ]
    );
}

#[test]
fn grep_quotes_an_unreadable_path_that_opens_with_a_parenthesis_and_not_the_count_after_it() {
    let names = [
        "(a).txt", "(b).txt", "(c).txt", "(d).txt", "(e).txt", "(f).txt",
    ];
    let tree = common::tree(&names.map(|name| (name, "x\n")));
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());
    for name in names {
        std::fs::remove_file(tree.path().join(name)).expect("the file is removed");
    }
    let missing = std::fs::File::open(tree.path().join(names[0]))
        .expect_err("the file is gone")
        .to_string();

    let listed: Vec<String> = names[..5]
        .iter()
        .map(|name| format!("{:?}", format!("{name}: {missing}")))
        .collect();
    assert_eq!(
        client.answer("grep", &json!({"patterns": ["x"]})),
        format!(
            "<grep patterns=\"x\" candidates=\"6\" files=\"0\" matches=\"0\">\n<failures count=\"6\">\n{}\n(+1 more)\n\
             </failures>\n</grep>",
            listed.join("\n")
        )
    );
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
        "<callers of=\"Gret\" matches=\"0\" one_edit=\"1\">\n\
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
