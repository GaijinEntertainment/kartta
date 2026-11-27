//! The server driven over stdio as an agent's client drives it: the handshake, the tool list, the lifecycle, and the
//! answers of each tool, pinned in full.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test), so the client's helpers fail the test by a failed expectation"
)]

mod common;

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};

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
    assert!(indexed.contains("<errors total=\"1\">"), "{indexed}");
    assert!(found.contains("func (s *Service) Greet"), "{found}");
    assert!(refreshed.contains(reason), "{refreshed}");
    assert!(!refreshed.contains("<errors"), "{refreshed}");
}

// constraint: only unix mode bits close a directory to its owner
#[cfg(unix)]
#[test]
fn the_index_answer_says_no_call_lists_the_errors_past_its_5() {
    use std::os::unix::fs::PermissionsExt;

    let tree = go_module();
    let closed: Vec<std::path::PathBuf> = (0..6)
        .map(|at| tree.path().join(format!("closed{at}")))
        .collect();
    for dir in &closed {
        std::fs::create_dir(dir).expect("the directory");
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000))
            .expect("the permissions");
    }
    // constraint: root reads a directory whatever its mode, so no listing fails to give the walk an error
    if std::fs::read_dir(&closed[0]).is_ok() {
        return;
    }
    let mut client = Client::start(&[]);

    let indexed = client.index(tree.path());
    for dir in &closed {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
            .expect("the permissions");
    }

    assert!(
        indexed.contains("<errors total=\"6\" shown=\"5\" truncated=\"true\">"),
        "{indexed}"
    );
    assert!(
        indexed.contains(
            "\n(+1 more) a fixed cap cut this list - no parameter or call lists the rest\n</errors>"
        ),
        "{indexed}"
    );
}

#[test]
fn a_server_told_nothing_shares_a_cache_under_the_user_s_and_narrows_grep_by_an_index() {
    let tree = go_module();
    let home = tempfile::tempdir().expect("a temporary directory");
    let mut client = Client::defaulted_in(home.path());
    let cache = if cfg!(target_os = "macos") {
        home.path().join("Library").join("Caches").join("kartta")
    } else if cfg!(windows) {
        home.path().join("kartta")
    } else {
        home.path().join(".cache").join("kartta")
    };

    client.index(tree.path());
    let found = client.answer("grep", &json!({"patterns": ["Greet"]}));
    let published = std::fs::read_dir(&cache).map_or(0, Iterator::count);

    assert!(found.contains("ruled_out=\"2\""), "{found}");
    assert!(published > 0, "nothing in {}", cache.display());
}

#[test]
fn the_server_lists_its_tools() {
    let mut client = Client::start(&[]);

    let listed = client.request("tools/list", &json!({}));
    let mut hinted: Vec<String> = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| {
            let name = tool["name"].as_str().expect("a named tool");
            format!("{name} {}", tool["annotations"])
        })
        .collect();
    hinted.sort_unstable();

    let read_only = r#"{"readOnlyHint":true}"#;
    let idempotent = r#"{"idempotentHint":true}"#;
    assert_eq!(
        hinted,
        [
            format!("file_outline {read_only}"),
            format!("find_call_path {read_only}"),
            format!("find_callees {read_only}"),
            format!("find_callers {read_only}"),
            format!("find_files {read_only}"),
            format!("find_implementations {read_only}"),
            format!("find_importers {read_only}"),
            format!("find_references {read_only}"),
            format!("find_symbols {read_only}"),
            format!("find_unused {read_only}"),
            format!("glob_files {read_only}"),
            format!("grep {read_only}"),
            format!("impact_of_change {read_only}"),
            format!("impact_of_edit {read_only}"),
            format!("index_root {idempotent}"),
            format!("index_stats {read_only}"),
            format!("list_dir {read_only}"),
            format!("read_definitions {read_only}"),
            format!("refresh {idempotent}"),
        ],
        "a tool that changes nothing is read-only, and one that changes the index is safe to repeat"
    );
}

/// The most characters a tool description holds: what the tool answers, when to call it or a neighbor, and the limits
/// that change that choice.
const DESCRIPTION_CHARACTERS: usize = 1200;
/// The most bytes one tool's `tools/list` entry takes, as compact JSON.
const TOOL_BYTES: usize = 4 * 1024;
/// The most bytes the whole `tools/list` result takes, as compact JSON: a client without tool search pays it in every
/// session.
const LIST_BYTES: usize = 42 * 1024;
/// The most characters the instructions hold: Claude Code cuts a server's instructions past 4,096.
const INSTRUCTIONS_CHARACTERS: usize = 4096;

#[test]
fn the_tool_list_stays_within_its_budget() {
    let mut client = Client::start(&[]);

    let listed = client.request("tools/list", &json!({}));
    let over: Vec<String> = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .filter_map(|tool| {
            let characters = tool["description"]
                .as_str()
                .expect("a described tool")
                .chars()
                .count();
            let bytes = tool.to_string().len();
            (characters > DESCRIPTION_CHARACTERS || bytes > TOOL_BYTES).then(|| {
                format!(
                    "{}: {characters} description characters, {bytes} bytes",
                    tool["name"]
                )
            })
        })
        .collect();
    let bytes = listed.to_string().len();

    assert!(over.is_empty(), "past a tool's budget: {over:?}");
    assert!(bytes <= LIST_BYTES, "tools/list takes {bytes} bytes");
    assert!(
        client.instructions.chars().count() <= INSTRUCTIONS_CHARACTERS,
        "the instructions take {} characters",
        client.instructions.chars().count()
    );
}

#[test]
fn the_instructions_send_an_agent_to_index_stats_before_it_builds() {
    let client = Client::start(&[]);

    assert!(
        client.instructions.contains(
            "index_stats without a root shows each root held, built, building or failed. Call index_root with the \
             repository's absolute root when index_stats shows no index of it."
        ),
        "{}",
        client.instructions
    );
}

#[test]
fn the_instructions_state_the_vocabulary_every_answer_shares() {
    let client = Client::start(&[]);

    let unstated: Vec<&str> = [
        "total=",
        "shown=",
        "truncated=",
        "hops=",
        "loc=",
        "lines=",
        "role=",
        "confidence=",
        "one_edit=",
        "at=",
        "through=",
        "passed_over=",
        "unexpanded=",
        "turned_away=",
        "below_confidence=",
        "ambiguous=",
        "excluded=",
        "unresolved=",
    ]
    .into_iter()
    .filter(|word| !client.instructions.contains(word))
    .collect();

    assert!(
        unstated.is_empty(),
        "{unstated:?} missing from {}",
        client.instructions
    );
}

#[test]
fn the_instructions_name_every_tool() {
    let mut client = Client::start(&[]);

    let listed = client.request("tools/list", &json!({}));
    let unnamed: Vec<&str> = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a named tool"))
        .filter(|name| !client.instructions.contains(name))
        .collect();

    assert!(
        unnamed.is_empty(),
        "{unnamed:?} missing from {}",
        client.instructions
    );
}

/// Whether a property's JSON schema admits null: a `null` type, or a branch of an `anyOf` that does.
fn admits_null(shape: &Value) -> bool {
    shape["type"] == "null"
        || shape["type"]
            .as_array()
            .is_some_and(|types| types.contains(&json!("null")))
        || shape["anyOf"]
            .as_array()
            .is_some_and(|shapes| shapes.iter().any(admits_null))
}

#[test]
fn every_optional_argument_reads_null_as_omitted() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());
    let required = json!({
        "root": tree.path(),
        "pattern": "serv",
        "globs": ["**"],
        "patterns": ["Greet"],
        "query": "Greet",
        "name": "Greet",
        "names": ["Greet"],
        "from": "main",
        "to": "Greet",
        "file": "pkg/service.go",
        "start_line": 1,
    });

    let listed = client.request("tools/list", &json!({}));
    for tool in listed["tools"].as_array().expect("a tool list") {
        let name = tool["name"].as_str().expect("a named tool");
        let schema = &tool["inputSchema"];
        let needed = schema["required"].as_array().cloned().unwrap_or_default();
        let mut omitted = Map::new();
        let mut nulled = Map::new();
        for (property, shape) in schema["properties"].as_object().expect("the properties") {
            if needed.contains(&json!(property)) {
                omitted.insert(property.clone(), required[property].clone());
                nulled.insert(property.clone(), required[property].clone());
            } else {
                assert!(
                    admits_null(shape),
                    "{name}'s {property} schema admits no null: {shape}"
                );
                nulled.insert(property.clone(), Value::Null);
            }
        }

        let without = client.call_as_sent(name, &Value::Object(omitted));
        let with = client.call_as_sent(name, &Value::Object(nulled));

        assert!(
            !with.text.starts_with("failed to deserialize"),
            "{name}: {}",
            with.text
        );
        assert_eq!(
            with.is_error, without.is_error,
            "{name}: {} against {}",
            with.text, without.text
        );
        assert_eq!(untimed(&with.text), untimed(&without.text), "{name}");
    }
}

#[test]
fn a_wrong_argument_names_its_parameter_and_its_path_inside_the_arguments() {
    let mut client = Client::start(&[]);
    let refused = "failed to deserialize parameters:";

    for (tool, arguments, error) in [
        (
            "find_callers",
            json!({"name": "B", "hops": 0}),
            "hops: invalid value: integer `0`, expected a nonzero u32",
        ),
        (
            "file_outline",
            json!({"file": "a.go", "limit": 0}),
            "limit: invalid value: integer `0`, expected a nonzero usize",
        ),
        (
            "find_importers",
            json!({"file": "a.go", "hops": 0}),
            "hops: invalid value: integer `0`, expected a nonzero u32",
        ),
        (
            "find_importers",
            json!({"file": "a.go", "limit": 0}),
            "limit: invalid value: integer `0`, expected a nonzero usize",
        ),
        (
            "find_unused",
            json!({"limit": 0}),
            "limit: invalid value: integer `0`, expected a nonzero usize",
        ),
        (
            "impact_of_edit",
            json!({"file": "a.go", "start_line": 1, "limit": 0}),
            "limit: invalid value: integer `0`, expected a nonzero usize",
        ),
        (
            "impact_of_change",
            json!({"regions": [{"file": "a.go", "start_line": 1}, {"file": "a.go", "start_line": 0}]}),
            "regions[1].start_line: invalid value: integer `0`, expected a nonzero u32",
        ),
        (
            "find_callers",
            json!({"name": 5}),
            "name: invalid type: integer `5`, expected a string",
        ),
        (
            "find_references",
            json!({"name": "B", "access": "x"}),
            "access: unknown variant `x`, expected one of `read`, `write`, `type`",
        ),
        (
            "grep",
            json!({"patterns": ["a", 7]}),
            "patterns[1]: invalid type: integer `7`, expected a string",
        ),
        (
            "impact_of_change",
            json!({"regions": [{"file": "a.go", "line": 3}]}),
            "regions[0].line: unknown field `line`, expected one of `file`, `start_line`, `end_line`",
        ),
        (
            "find_callers",
            json!({"root": "/r", "hops": 2}),
            "missing field `name`",
        ),
        ("find_callers", json!({"name": "B"}), "missing field `root`"),
    ] {
        let answer = client.call(tool, &arguments);

        assert!(answer.is_error, "{tool} {arguments}: {}", answer.text);
        assert_eq!(
            answer.text,
            format!("{refused} {error}"),
            "{tool} {arguments}"
        );
    }
}

/// `text` without its `elapsed_ms` attribute, the one part of a refresh answer two calls never share.
fn untimed(text: &str) -> String {
    match text.split_once(" elapsed_ms=\"") {
        Some((head, tail)) => format!(
            "{head}{}",
            tail.split_once('"').map_or("", |(_, rest)| rest)
        ),
        None => text.to_owned(),
    }
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
        "no files match \"*.rs\""
    );
    assert_eq!(
        client.answer(
            "glob_files",
            &json!({"globs": ["*.md"], "excludes": ["README.md"]})
        ),
        "no files match \"*.md\" outside the excludes (excludes removed: 1)"
    );
    assert!(client.call("glob_files", &json!({"globs": []})).is_error);
}

#[test]
fn an_empty_glob_answer_tells_one_glob_from_two() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["x, y"]})),
        "no files match \"x, y\""
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["x", "y"]})),
        "no files match \"x\", \"y\""
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["x\ny", "\"x\", \"y\""]})),
        "no files match \"x\\ny\", \"\\\"x\\\", \\\"y\\\"\"",
        "a glob may hold a line break or a quote, which the echo escapes"
    );
}

#[test]
fn an_empty_file_answer_names_the_globs_that_name_a_directory() {
    let tree = go_module();
    let mut literal = vec!["p[k]"];
    if cfg!(unix) {
        literal.push("a\\b");
    }
    for dir in &literal {
        std::fs::create_dir_all(tree.path().join(dir)).expect("the directory is created");
        std::fs::write(tree.path().join(dir).join("x.txt"), "x\n").expect("the file is written");
    }
    let mut client = Client::start(&[]);
    client.index(tree.path());
    let one = format!("{DIRECTORY_FIX} \"pkg/**\" in place of \"pkg\"");

    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["pkg"]})),
        format!("no files match \"pkg\" - {one}")
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["cmd/app/", "pkg"]})),
        format!(
            "no files match \"cmd/app/\", \"pkg\" - {DIRECTORY_FIX} \"cmd/app/**\" in place of \"cmd/app/\", \
             \"pkg/**\" in place of \"pkg\""
        )
    );
    assert_eq!(
        client.answer(
            "glob_files",
            &json!({"globs": ["pkg", "*.md"], "excludes": ["README.md"]})
        ),
        format!(
            "no files match \"pkg\", \"*.md\" outside the excludes (excludes removed: 1) - {one}"
        )
    );
    assert_eq!(
        client.answer(
            "glob_files",
            &json!({"globs": ["pk?", "go.mod/", "nowhere", "/"]})
        ),
        "no files match \"pk?\", \"go.mod/\", \"nowhere\", \"/\"",
        "a glob character, a file and a path the index lacks name no directory"
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": literal})),
        format!(
            "no files match {}",
            literal
                .iter()
                .map(|glob| format!("{glob:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "a glob that holds a glob character or an escape means another path than the directory spelled as it"
    );
    assert_eq!(
        client.answer("grep", &json!({"patterns": ["x"], "includes": ["pkg"]})),
        format!(
            "<grep patterns=\"x\" candidates=\"0\" files=\"0\" total=\"0\">the include/exclude globs selected no \
             files - {one}</grep>"
        )
    );
    assert_eq!(
        client.answer(
            "grep",
            &json!({"patterns": ["zzz"], "includes": ["pkg/", "*.go"]})
        ),
        format!(
            "<grep patterns=\"zzz\" candidates=\"2\" files=\"0\" total=\"0\">no line matches - {DIRECTORY_FIX} \
             \"pkg/**\" in place of \"pkg/\"</grep>"
        )
    );
    assert_eq!(
        client.answer(
            "find_files",
            &json!({"pattern": "serv", "includes": ["pkg"]})
        ),
        format!("no files match \"serv\" - {one}")
    );
}

/// The start of the fix a file answer gives for a glob that names a directory.
const DIRECTORY_FIX: &str = "a glob selects files by their path from the root, so one that names a directory \
                             selects nothing: pass";

#[test]
fn a_listed_answer_and_a_name_below_the_root_name_the_glob_that_names_a_directory() {
    let tree = go_module();
    for dir in ["pkg/sub", "cmd/sub"] {
        std::fs::create_dir_all(tree.path().join(dir)).expect("the directory is created");
        std::fs::write(tree.path().join(dir).join("x.txt"), "x\n").expect("the file is written");
    }
    let mut client = Client::start(&[]);
    client.index(tree.path());
    let one = format!("{DIRECTORY_FIX} \"pkg/**\" in place of \"pkg\"");

    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["pkg", "*.md"]})),
        format!("README.md\n({one})"),
        "an answer another glob filled names the directory too"
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["app"]})),
        format!("no files match \"app\" - {DIRECTORY_FIX} \"cmd/app/**\" in place of \"app\""),
        "a name the root does not hold names the one directory below it named so"
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["sub/", "**/*.mod"]})),
        format!(
            "go.mod\n({DIRECTORY_FIX} \"**/sub/**\" (2 directories are named \"sub\") in place of \"sub/\")"
        ),
        "a name several directories below the root carry names them all, with their count"
    );
    assert_eq!(
        client.answer(
            "grep",
            &json!({"patterns": ["^module"], "includes": ["pkg", "go.mod"]})
        ),
        format!(
            "<grep patterns=\"^module\" candidates=\"1\" files=\"1\" total=\"1\">\n<file path=\"go.mod\" \
             total=\"1\">\n1:module example.com/app\n</file>\n<note>{one}</note>\n</grep>"
        )
    );
    assert_eq!(
        client.answer(
            "find_files",
            &json!({"pattern": "go.mod", "includes": ["pkg", "go.mod"]})
        ),
        format!("go.mod\n({one})")
    );
}

#[test]
fn a_glob_of_several_names_names_the_directories_whose_path_it_ends() {
    let tree = go_module();
    for dir in ["pkg/sub/deep", "cmd/sub/deep", "cmd/app/inner"] {
        std::fs::create_dir_all(tree.path().join(dir)).expect("the directory is created");
        std::fs::write(tree.path().join(dir).join("x.txt"), "x\n").expect("the file is written");
    }
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["app/inner"]})),
        format!(
            "no files match \"app/inner\" - {DIRECTORY_FIX} \"cmd/app/inner/**\" in place of \"app/inner\""
        ),
        "names that end one directory's path name that directory"
    );
    assert_eq!(
        client.answer("glob_files", &json!({"globs": ["sub/deep/", "**/*.mod"]})),
        format!(
            "go.mod\n({DIRECTORY_FIX} \"**/sub/deep/**\" (2 directories are named \"sub/deep\") in place of \
             \"sub/deep/\")"
        ),
        "names that end several directories' paths name them all, with their count"
    );
    assert_eq!(
        client.answer(
            "glob_files",
            &json!({"globs": ["md/app", "app/main.go", "inner/deep"]})
        ),
        "no files match \"md/app\", \"app/main.go\", \"inner/deep\"",
        "names end a directory's path only at a whole name, of a directory, in their order"
    );
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
        "<grep patterns=\"Greet\" candidates=\"4\" files=\"2\" total=\"3\" excluded=\"1\">\n\
         <file path=\"cmd/app/main.go\" total=\"1\">\n\
         11:\ts.Greet(\"x\")\n\
         </file>\n\
         <file path=\"pkg/service.go\" total=\"2\">\n\
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
        "<grep patterns=\"greet\" candidates=\"1\" files=\"1\" total=\"1\" truncated=\"true\">\n\
         <file path=\"pkg/service.go\" total=\"1\">\n\
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
        "<grep patterns=\"Greet.*World\" candidates=\"5\" files=\"0\" total=\"0\">no line matches - matching is \
         per-line: \".*\"/\".+\" cannot cross line breaks; search the most distinctive part alone</grep>"
    );
    assert_eq!(
        client.answer("grep", &json!({"patterns": ["x"], "includes": ["*.rs"]})),
        "<grep patterns=\"x\" candidates=\"0\" files=\"0\" total=\"0\">the include/exclude globs selected no files \
         - glob_files shows what they match</grep>"
    );
    assert!(client.call("grep", &json!({"patterns": ["("]})).is_error);
    assert!(client.call("grep", &json!({"patterns": [" "]})).is_error);
}

/// Line `number` of a 3,000-line file whose every thousandth line is a match of `needle`.
fn long_file_line(number: usize) -> String {
    if number.is_multiple_of(1000) {
        format!("needle {number}")
    } else {
        format!("filler line {number:>4} of the long generated file")
    }
}

/// The most context lines a side whose lines, the matches' with them, fit 40,000 bytes in the long file's answer.
fn long_file_context_that_fits() -> usize {
    let answered = |number: usize| format!("{number}-{}\n", long_file_line(number)).len();
    let mut spent: usize = [1000, 2000, 3000]
        .iter()
        .map(|&number| format!("{number}:{}\n", long_file_line(number)).len())
        .sum();
    (1..1000)
        .take_while(|&side| {
            spent += [
                1000 - side,
                1000 + side,
                2000 - side,
                2000 + side,
                3000 - side,
            ]
            .into_iter()
            .map(answered)
            .sum::<usize>();
            spent <= 40_000
        })
        .count()
}

#[test]
fn grep_cuts_every_match_s_context_to_the_most_lines_a_side_that_fit_its_answer() {
    let tree = go_module();
    let line = long_file_line;
    let text: String = (1..=3000).map(|number| line(number) + "\n").collect();
    std::fs::write(tree.path().join("long.txt"), text).expect("the file is written");
    let fits = long_file_context_that_fits();
    let mut client = Client::start(&[]);
    client.index(tree.path());
    let grep = |client: &mut Client, context_lines: usize| {
        client.answer(
            "grep",
            &json!({"patterns": ["needle"], "includes": ["long.txt"], "context_lines": context_lines}),
        )
    };

    let cut = grep(&mut client, 100_000);
    let shown: usize = cut
        .split_once("context_shown=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(shown, _)| shown.parse().expect("a count"))
        .expect("the answer says how much context it shows");
    let whole = grep(&mut client, shown);
    let past = grep(&mut client, shown + 1);

    assert!(cut.len() < 50_000, "{} bytes", cut.len());
    assert_eq!(
        shown, fits,
        "the most lines a side whose lines, the matches' with them, fit 40,000 bytes"
    );
    assert_eq!(
        cut.replacen(
            &format!(" context_lines=\"100000\" context_shown=\"{shown}\""),
            "",
            1
        )
        .replacen(
            "\nthe answer's size cut each match's context to context_shown lines a side - a narrower includes \
             or a lower max_results shows more of it",
            "",
            1
        ),
        whole,
        "the cut answer is the answer at the context it shows"
    );
    assert!(!whole.contains("context_shown"), "{whole}");
    for side in [1000 - fits, 2000 + fits, 3000 - fits] {
        assert!(
            whole.contains(&format!("\n{side}-{}\n", line(side))),
            "the context line {side} is shown"
        );
    }
    assert!(
        !whole.contains(&format!("\n{}-", 1000 - fits - 1)),
        "the line past the context is not"
    );
    assert!(
        past.contains(&format!(
            "context_lines=\"{}\" context_shown=\"{shown}\"",
            shown + 1
        )),
        "one line more a side does not fit"
    );
}

#[test]
fn grep_keeps_every_match_whose_lines_alone_pass_the_answer_s_size_and_shows_no_context() {
    let tree = go_module();
    let wide = format!("wide {}\nfiller\n", "x".repeat(1000)).repeat(50);
    std::fs::write(tree.path().join("wide.txt"), wide).expect("the file is written");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer(
        "grep",
        &json!({"patterns": ["wide"], "includes": ["wide.txt"], "context_lines": 1}),
    );

    assert!(
        answer.contains("total=\"50\" context_lines=\"1\" context_shown=\"0\""),
        "{answer}"
    );
    assert_eq!(
        answer.matches(":wide ").count(),
        50,
        "every match is listed"
    );
    assert!(!answer.contains("-filler"), "no context line fits");
}

#[test]
fn grep_echoes_each_pattern_on_a_line_of_its_own() {
    let tree = go_module();
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let answer = client.answer(
        "grep",
        &json!({"patterns": ["x, y", "zzz"], "includes": ["pkg/**"]}),
    );

    assert!(
        answer.starts_with(r#"<grep patterns="x, y\nzzz" candidates="1""#),
        "{answer}"
    );
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
        "<dir path=\".\" total=\"11\" shown=\"6\">\n\
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
        "<dir path=\".\" total=\"11\" shown=\"4\" excluded=\"1\" truncated=\"true\">\n\
         <entry path=\"README.md\" kind=\"file\" size=\"6\"/>\n\
         <entry path=\"cmd\" kind=\"dir\"/>\n\
         <entry path=\"cmd/app\" kind=\"dir\"/>\n\
         <entry path=\"cmd/app/main.go\" kind=\"file\" size=\"128\"/>\n\
         </dir>"
    );
    assert_eq!(
        client.answer("list_dir", &json!({"path": "vendor", "excludes": []})),
        "<dir path=\"vendor\" total=\"2\" shown=\"1\">\n\
         <entry path=\"vendor/lib\" kind=\"dir\" unlisted=\"1\"/>\n\
         </dir>"
    );
    assert_eq!(
        client.answer(
            "list_dir",
            &json!({"path": "vendor", "max_depth": 2, "excludes": ["vendor/lib"]})
        ),
        "<dir path=\"vendor\" total=\"2\" shown=\"1\">\n\
         <entry path=\"vendor/lib\" kind=\"dir\" unlisted=\"1\"/>\n\
         </dir>",
        "a name with the directories above it is matched as written"
    );
    let globbed = client.call("list_dir", &json!({"excludes": ["vendor", "**/lib"]}));
    assert!(globbed.is_error);
    assert_eq!(
        globbed.text,
        "excludes: \"**/lib\" holds the glob character * and names no indexed directory - list_dir excludes are \
         directory names, matched literally at any depth, such as \"vendor\" or \"pkg/deep\""
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
fn a_list_dir_exclude_excludes_a_held_directory_whatever_its_name_holds() {
    let tree = go_module();
    std::fs::create_dir_all(tree.path().join("cmd/odd[1]")).expect("the directory is created");
    std::fs::write(tree.path().join("cmd/odd[1]/a[2]"), "a\n").expect("the file is written");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer(
            "list_dir",
            &json!({"path": "cmd", "max_depth": 2, "excludes": ["odd[1]"]})
        ),
        "<dir path=\"cmd\" total=\"4\" shown=\"3\">\n\
         <entry path=\"cmd/app\" kind=\"dir\"/>\n\
         <entry path=\"cmd/app/main.go\" kind=\"file\" size=\"128\"/>\n\
         <entry path=\"cmd/odd[1]\" kind=\"dir\" unlisted=\"1\"/>\n\
         </dir>"
    );
    for name in ["dd[1]", "a[2]"] {
        assert!(
            client
                .call("list_dir", &json!({"excludes": [name]}))
                .is_error,
            "{name} ends a directory's name or names a file, and names no directory"
        );
    }
    let unheld = client.call("list_dir", &json!({"excludes": ["odd[1]", "odd[2]"]}));
    assert!(unheld.is_error);
    assert_eq!(
        unheld.text,
        "excludes: \"odd[2]\" holds the glob character [ and names no indexed directory - list_dir excludes are \
         directory names, matched literally at any depth, such as \"vendor\" or \"pkg/deep\""
    );
    let backslashed = client.call("list_dir", &json!({"excludes": ["cmd\\app"]}));
    assert!(backslashed.is_error);
    assert_eq!(
        backslashed.text,
        "excludes: \"cmd\\\\app\" holds a \\ and names no indexed directory - list_dir excludes are directory names, \
         matched literally at any depth, such as \"vendor\" or \"pkg/deep\""
    );
}

/// Windows cannot spell a name that holds a `\`, so the case exists only where a filename may hold one.
#[cfg(unix)]
#[test]
fn a_list_dir_exclude_excludes_a_held_directory_whose_name_holds_a_backslash() {
    let tree = go_module();
    std::fs::create_dir_all(tree.path().join("cmd/a\\b")).expect("the directory is created");
    std::fs::write(tree.path().join("cmd/a\\b/a.txt"), "a\n").expect("the file is written");
    let mut client = Client::start(&[]);
    client.index(tree.path());

    assert_eq!(
        client.answer(
            "list_dir",
            &json!({"path": "cmd", "max_depth": 2, "excludes": ["a\\b"]})
        ),
        "<dir path=\"cmd\" total=\"4\" shown=\"3\">\n\
         <entry path=\"cmd/a\\\\b\" kind=\"dir\" unlisted=\"1\"/>\n\
         <entry path=\"cmd/app\" kind=\"dir\"/>\n\
         <entry path=\"cmd/app/main.go\" kind=\"file\" size=\"128\"/>\n\
         </dir>"
    );
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
        "<grep patterns=\"Greet\" candidates=\"1\" files=\"0\" total=\"0\" binary=\"1\">no line matches in the files \
         read - binary and oversized files are skipped unread</grep>"
    );
    assert_eq!(
        client.answer("list_dir", &json!({"path": "conf", "skip_dotfiles": true})),
        "<dir path=\"conf\" total=\"1\" shown=\"0\" excluded=\"1\">no entry shown - skip_dotfiles excluded every \
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
        "<grep patterns=\"x\" candidates=\"0\" files=\"0\" total=\"0\" symlinks=\"1\">the include/exclude globs \
         selected symlinks alone, which are never read through - include their targets</grep>"
    );
    assert_eq!(
        client.answer(
            "grep",
            &json!({"patterns": ["x"], "includes": ["link.go", "cmd"]})
        ),
        "<grep patterns=\"x\" candidates=\"0\" files=\"0\" total=\"0\" symlinks=\"1\">the include/exclude globs \
         selected symlinks alone, which are never read through - include their targets - a glob selects files by \
         their path from the root, so one that names a directory selects nothing: pass \"cmd/**\" in place of \
         \"cmd\"</grep>"
    );
}

#[test]
fn a_query_before_index_root_names_the_fix() {
    let mut client = Client::start(&[]);

    let answer = client.call("find_files", &json!({"root": "/repo", "pattern": "main"}));

    assert!(answer.is_error);
    assert_eq!(
        answer.text,
        "this server has not indexed \"/repo\" - it holds no root; call index_root with root \"/repo\" to index it"
    );
}

#[test]
fn index_root_reports_each_graph() {
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
fn the_index_counts_the_worktrees_of_its_repository_it_holds_empty() {
    let tree = common::tree(&[
        (".git/HEAD", "ref: refs/heads/main\n"),
        (".git/worktrees/agent1/HEAD", "ref: refs/heads/agent\n"),
        (".git/worktrees/agent1/commondir", "../..\n"),
        (".git/worktrees/agent2/HEAD", "ref: refs/heads/agent\n"),
        (".git/worktrees/agent2/commondir", "../..\n"),
        (
            ".claude/worktrees/agent1/.git",
            "gitdir: ../../../.git/worktrees/agent1\n",
        ),
        (
            ".claude/worktrees/agent1/main.go",
            "package main\n\nfunc main() {}\n",
        ),
        (
            ".claude/worktrees/agent2/.git",
            "gitdir: ../../../.git/worktrees/agent2\n",
        ),
        (
            ".claude/worktrees/agent2/main.go",
            "package main\n\nfunc main() {}\n",
        ),
        ("main.go", "package main\n\nfunc main() {}\n"),
    ]);
    let mut client = Client::start(&[]);

    let answer = client.index(tree.path());

    assert_eq!(
        rooted(&answer, tree.path()),
        "<index root=\"<root>\" files=\"1\" pruned_worktrees=\"2\" gitignore=\"in-tree\" watch=\"following\" \
         symbols=\"1\" calls=\"0\">\n\
         <language name=\"go\" files=\"1\" symbols=\"1\" calls=\"0\" uses=\"0\" includes=\"0\"/>\n\
         </index>"
    );
    assert_eq!(client.answer("index_stats", &json!({})), answer);
    let other = go_module();
    let second = client.index(other.path());
    assert_eq!(
        client.call_as_sent("index_stats", &json!({})).text,
        format!("<roots total=\"2\">\n{answer}\n{second}\n</roots>"),
        "a listing of several roots keeps each root's pruned_worktrees in its own envelope"
    );
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
    assert_eq!(
        client.answer("index_root", &json!({"root": root, "index_ignored": true})),
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
            let answer = client.call("index_root", &json!({"root": root}));
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
        "<roots total=\"0\">nothing is indexed - call index_root with the repository's absolute root</roots>"
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
        shown
            .starts_with("<index root=\"<root>\" status=\"failed\">cannot read directory <root>: ")
            && shown.ends_with(" - call index_root again</index>"),
        "{stats}"
    );
    let refused = client.call("find_symbols", &json!({"query": "x"}));
    let shown = rooted(&refused.text, &missing);
    assert!(
        refused.is_error
            && shown.starts_with("indexing <root> failed: cannot read directory <root>: ")
            && shown.ends_with(" - call index_root again"),
        "{}",
        refused.text
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
            "kartta-mcp: the launch build of {0} ended without an index: cannot read directory {0}: ",
            missing.display()
        )),
        "{warning}"
    );
}

#[test]
fn a_root_given_at_launch_builds_without_index_root() {
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
        "index_root",
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
fn a_filtered_build_names_its_languages_on_the_index_and_on_a_miss() {
    let tree = common::tree(&[
        ("web/app.ts", "export function render() {}\n"),
        (
            "src/lib.rs",
            "pub fn helper() {}\n\npub fn run() { helper(); }\n",
        ),
    ]);
    let mut client = Client::start(&[]);

    let indexed = client.answer(
        "index_root",
        &json!({"root": tree.path(), "languages": ["typescript"]}),
    );
    let misses = [
        client.answer("find_callers", &json!({"name": "helper"})),
        client.answer("find_callees", &json!({"name": "helper"})),
        client.answer("find_references", &json!({"name": "helper"})),
        client.answer("find_implementations", &json!({"name": "helper"})),
        client.answer("find_symbols", &json!({"query": "helper"})),
        client.answer("read_definitions", &json!({"names": ["helper"]})),
        client.answer("find_call_path", &json!({"from": "run", "to": "helper"})),
    ];
    let stats = client.answer("index_stats", &json!({}));
    let hit = client.answer("find_symbols", &json!({"query": "render"}));
    let whole = client.index(tree.path());
    let none = client.answer("index_root", &json!({"root": tree.path(), "languages": []}));
    let none_miss = client.answer("find_callers", &json!({"name": "helper"}));

    assert!(none.contains(r#" languages="[]" files="2""#), "{none}");
    assert!(
        none_miss.starts_with(
            r#"<callers of="helper" total="0" index_languages="[]">no symbol matches"#
        ),
        "{none_miss}"
    );
    for answer in [&indexed, &stats] {
        assert!(
            answer.contains(r#" languages="typescript" files="2""#),
            "{answer}"
        );
    }
    for miss in &misses {
        assert!(miss.contains(r#" index_languages="typescript""#), "{miss}");
    }
    assert!(
        !hit.contains("index_languages"),
        "a hit holds what it found: {hit}"
    );
    assert!(
        !whole.contains(" languages="),
        "every language built: {whole}"
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
fn find_files_and_glob_files_say_when_their_default_cap_cut_the_list() {
    let names: Vec<String> = (0..201).map(|at| format!("notes/n{at:03}.txt")).collect();
    let files: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "x\n")).collect();
    let tree = common::tree(&files);
    let mut client = Client::start(&[]);
    client.index(tree.path());

    let found = client.answer("find_files", &json!({"pattern": "txt"}));
    let globbed = client.answer("glob_files", &json!({"globs": ["*.txt"]}));
    let capped = client.answer(
        "glob_files",
        &json!({"globs": ["*.txt"], "max_results": 200}),
    );
    let whole = [
        client.answer(
            "find_files",
            &json!({"pattern": "txt", "excludes": ["notes/n200.txt"]}),
        ),
        client.answer(
            "glob_files",
            &json!({"globs": ["*.txt"], "excludes": ["notes/n200.txt"]}),
        ),
    ];
    let every = [
        client.answer("find_files", &json!({"pattern": "txt", "max_results": 0})),
        client.answer("glob_files", &json!({"globs": ["*.txt"], "max_results": 0})),
    ];

    let marker = "(the default max_results of 200 cut this list - a higher max_results, or 0, lists the rest)";
    assert!(
        found.ends_with(&format!(
            "\n(200 of 201 matching paths, best first)\n{marker}"
        )),
        "{found}"
    );
    assert_eq!(found.lines().count(), 202, "{found}");
    assert!(
        globbed.ends_with(&format!(
            "\nnotes/n199.txt\n(truncated: the first 200 matching paths in path order; more match)\n{marker}"
        )),
        "{globbed}"
    );
    assert!(
        capped.ends_with(
            "\nnotes/n199.txt\n(truncated: the first 200 matching paths in path order; more match)"
        ),
        "a cap the call named takes no line for the default: {capped}"
    );
    assert!(
        every
            .iter()
            .all(|answer| answer.lines().count() == 201 && !answer.contains('(')),
        "0 lists every match: {every:?}"
    );
    assert!(
        whole.iter().all(|answer| answer.lines().count() <= 201
            && answer.contains("notes/n199.txt")
            && !answer.contains("default max_results")),
        "200 matches fill the default cap and nothing is cut: {whole:?}"
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
            "<grep patterns=\"x\" candidates=\"6\" files=\"0\" total=\"0\">\n\
             <failures total=\"6\" shown=\"5\" truncated=\"true\">\n{}\n(+1 more) a fixed cap \
             cut this list - grep with narrower includes lists the rest\n</failures>\n</grep>",
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
        "<callers of=\"Greet\" total=\"1\">\n\
         <match lang=\"go\" of=\"Greet\" qn=\"pkg.Service.Greet\" loc=\"pkg/service.go:12-14\" total=\"2\" \
         consumers=\"1\" entries=\"1\">\n\
         <ref name=\"run\" qn=\"main.run\" loc=\"cmd/app/main.go:10-12\" role=\"consumer\" hops=\"1\" \
         at=\"11\"/>\n\
         <ref name=\"main\" qn=\"main.main\" loc=\"cmd/app/main.go:5-8\" role=\"entry\" hops=\"2\" \
         at=\"7\"/>\n\
         </match>\n\
         </callers>"
    );
    assert_eq!(
        client.answer(
            "find_callers",
            &json!({"name": "Greet", "hops": 1, "role": "entry"})
        ),
        "<callers of=\"Greet\" role=\"entry\" total=\"1\">\n\
         <match lang=\"go\" of=\"Greet\" qn=\"pkg.Service.Greet\" loc=\"pkg/service.go:12-14\" total=\"1\" shown=\"0\" \
         unexpanded=\"1\" consumers=\"1\"/>\n\
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
        "<callers of=\"Gret\" total=\"0\" one_edit=\"1\">\n\
         no symbol matches - did you mean:\n\
         <suggestion name=\"Greet\" qn=\"pkg.Service.Greet\" kind=\"method\" lang=\"go\" \
         loc=\"pkg/service.go:12-14\"/>\n\
         </callers>"
    );
    assert_eq!(
        client.answer("find_callers", &json!({"name": "Greet", "languages": []})),
        "<callers of=\"Greet\" languages=\"[]\" total=\"0\">no language selected - an empty languages list \
         searches no graph; omit languages to search every indexed language</callers>"
    );
}

#[test]
fn indexing_another_root_leaves_the_first_served() {
    let first = go_module();
    let second = common::tree(&[("lib/only.go", "package lib\n\nfunc Only() {}\n")]);
    let mut client = Client::start(&[first.path()]);
    let launched = client.wait_for_index();

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
        client.answer(
            "find_files",
            &json!({"root": first.path(), "pattern": "service"})
        ),
        "pkg/service.go"
    );
    assert_eq!(
        client.call_as_sent("index_stats", &json!({})).text,
        format!("<roots total=\"2\">\n{launched}\n{answer}\n</roots>"),
        "index_stats without a root shows each root in the order the server was first asked to build them"
    );
    assert_eq!(
        client.index(second.path()),
        answer,
        "a second build of the same root answers the same"
    );
}

#[test]
fn an_index_root_past_the_bound_names_the_root_it_dropped() {
    let launched = go_module();
    let others =
        [(); 4].map(|()| common::tree(&[("lib/only.go", "package lib\n\nfunc Only() {}\n")]));
    let mut client = Client::unwatched(&[launched.path()]);
    client.wait_for_index();
    for tree in &others[..3] {
        client.index(tree.path());
    }
    assert!(
        client
            .call_as_sent("index_stats", &json!({}))
            .text
            .starts_with("<roots total=\"4\">"),
        "the launch root and three others fill the bound of 4"
    );

    let answer = client.index(others[3].path());

    let shown = launched.path().display().to_string();
    assert!(
        rooted(&answer, others[3].path()).starts_with("<index root=\"<root>\" files=\"1\""),
        "{answer}"
    );
    assert!(
        answer.ends_with(&format!(
            "\nthis index_root dropped {shown:?} to stay within KARTTA_MAX_ROOTS=4 roots, the root a call named least \
             recently - call index_root with root {shown:?} to build it again"
        )),
        "{answer}"
    );
    let dropped = client.call("index_stats", &json!({"root": launched.path()}));
    assert!(
        dropped.is_error
            && dropped
                .text
                .starts_with(&format!("this server has not indexed {shown:?}")),
        "{}",
        dropped.text
    );
}

#[test]
fn calls_side_by_side_each_answer_from_the_root_they_name() {
    let trees = [("First", go_module()), ("Second", go_module())];
    let mut client = Client::unwatched(&[]);
    for (name, tree) in &trees {
        let function = format!("package pkg\n\nfunc {name}() {{}}\n");
        std::fs::write(tree.path().join("pkg/named.go"), function).expect("the file is written");
        client.index(tree.path());
    }
    let calls: Vec<(&str, Value)> = (0..16)
        .map(|at| {
            let root = trees[at % 2].1.path();
            (
                "file_outline",
                json!({"root": root, "file": "pkg/named.go"}),
            )
        })
        .collect();

    let answers = client.calls_at_once(&calls);

    for (at, answer) in answers.iter().enumerate() {
        let (own, other) = if at % 2 == 0 {
            ("First", "Second")
        } else {
            ("Second", "First")
        };
        assert!(
            !answer.is_error && answer.text.contains(own) && !answer.text.contains(other),
            "call {at} asked for {own}: {}",
            answer.text
        );
    }
}

#[test]
fn index_stats_of_a_root_the_server_does_not_hold_names_the_roots_it_holds() {
    let tree = go_module();
    let other = common::tree(&[]);
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());

    let answer = client.call("index_stats", &json!({"root": other.path()}));

    assert!(answer.is_error, "{}", answer.text);
    assert_eq!(
        answer.text,
        format!(
            "this server has not indexed {other:?} - it holds {tree:?}; call index_root with root {other:?} to index \
             it",
            other = other.path().display().to_string(),
            tree = tree.path().display().to_string()
        )
    );
}

#[test]
fn a_relative_launch_root_is_named_by_its_absolute_path() {
    let tree = go_module();
    // constraint: the child resolves its working directory through macOS's /var link, and `canonicalize` writes a
    // Windows path in the extended-length syntax, as its documentation says, which the root's spelling would then carry
    let dir = if cfg!(windows) {
        tree.path().to_owned()
    } else {
        tree.path().canonicalize().expect("the tree resolves")
    };
    let mut client = Client::start_in(&dir, &[Path::new("pkg")]);

    let stats = client.wait_for_index();

    assert!(
        stats.starts_with(&format!(
            "<index root={:?}",
            dir.join("pkg").display().to_string()
        )),
        "{stats}"
    );
    assert_eq!(
        client.answer("find_files", &json!({"pattern": "service"})),
        "service.go"
    );
}

#[test]
fn an_agent_told_a_build_still_runs_is_sent_to_index_stats() {
    let mut client = Client::start(&[]);

    let listed = client.request("tools/list", &json!({}));
    let description = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .find(|tool| tool["name"] == "index_root")
        .and_then(|tool| tool["description"].as_str())
        .expect("index_root is described");

    for told in [description, &client.instructions] {
        assert!(
            told.contains("status=\"building\"")
                && told.contains("call index_stats until it answers the index"),
            "{told}"
        );
    }
}

#[test]
fn index_root_names_every_field_a_repeated_build_matches() {
    let mut client = Client::start(&[]);

    let listed = client.request("tools/list", &json!({}));
    let description = listed["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .find(|tool| tool["name"] == "index_root")
        .and_then(|tool| tool["description"].as_str())
        .expect("index_root is described");

    assert!(
        description
            .contains("a build of the same root, languages and index_ignored already under way"),
        "{description}"
    );
}
