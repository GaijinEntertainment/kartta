//! The dispatch's contract: what `open` refuses, what a build and a walk over calls answer, which error code each
//! failure carries, and which build a cancel stops.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use kartta_ffi::Kartta;
use serde_json::{Value, json};

fn tree() -> tempfile::TempDir {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("main.go"),
        "package main\n\nfunc main() {\n\trun()\n}\n",
    )
    .expect("main.go");
    fs::write(
        tree.path().join("run.go"),
        "package main\n\nfunc run() {\n\thelper()\n}\n\nfunc helper() {}\n",
    )
    .expect("run.go");
    tree
}

fn open(root: &Path) -> Kartta {
    Kartta::open(json!({ "root": root }).to_string().as_bytes()).expect("the tree opens")
}

fn call(kartta: &Kartta, method: &str, params: &Value) -> Value {
    serde_json::from_slice(&kartta.call(method, params.to_string().as_bytes()))
        .expect("the answer is JSON")
}

fn ok(kartta: &Kartta, method: &str, params: &Value) -> Value {
    let answer = call(kartta, method, params);
    answer
        .get("ok")
        .cloned()
        .unwrap_or_else(|| panic!("{method} answered {answer}"))
}

fn code(kartta: &Kartta, method: &str, params: &Value) -> String {
    let answer = call(kartta, method, params);
    answer["err"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("{method} answered {answer}"))
        .to_owned()
}

fn refused(params: &[u8]) -> Value {
    let error = Kartta::open(params).expect_err("open refuses the params");
    serde_json::from_slice(&error.to_json()).expect("the error is JSON")
}

fn mentions(error: &Value, word: &str) -> bool {
    error["message"]
        .as_str()
        .is_some_and(|message| message.contains(word))
}

#[test]
fn open_refuses_a_root_that_is_not_a_directory_with_a_structured_error() {
    let tree = tree();
    let file = tree.path().join("main.go");

    let not_a_directory = refused(json!({ "root": file }).to_string().as_bytes());
    let missing = refused(
        json!({ "root": tree.path().join("gone") })
            .to_string()
            .as_bytes(),
    );
    let unknown_field = refused(br#"{"root": "/", "gitignore": true}"#);
    let not_json = refused(b"{");

    assert_eq!(not_a_directory["code"], "invalid_root");
    assert!(
        mentions(&not_a_directory, "not a directory"),
        "{not_a_directory}"
    );
    assert_eq!(missing["code"], "invalid_root");
    assert_eq!(unknown_field["code"], "invalid_params");
    assert!(mentions(&unknown_field, "gitignore"), "{unknown_field}");
    assert_eq!(not_json["code"], "invalid_request");
}

#[test]
fn a_query_before_any_build_answers_not_built() {
    let tree = tree();
    let kartta = open(tree.path());

    assert_eq!(code(&kartta, "stats", &Value::Null), "not_built");
    assert_eq!(
        code(&kartta, "callers", &json!({ "name": "helper" })),
        "not_built"
    );
}

#[test]
fn an_unknown_method_names_the_methods_there_are() {
    let tree = tree();
    let kartta = open(tree.path());

    let answer = call(&kartta, "engine/callers", &Value::Null);

    assert_eq!(answer["err"]["code"], "unknown_method");
    let message = answer["err"]["message"].as_str().expect("a message");
    assert!(message.contains("callers, cancel"), "{message}");
}

#[test]
fn a_file_query_refuses_what_it_cannot_take_with_its_own_code() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    assert_eq!(
        code(&kartta, "glob", &json!({ "globs": ["[a"] })),
        "invalid_glob"
    );
    assert_eq!(
        code(&kartta, "grep", &json!({ "patterns": ["("] })),
        "invalid_pattern"
    );
    assert_eq!(
        code(&kartta, "list", &json!({ "path": "gone" })),
        "not_found"
    );
    assert_eq!(
        code(&kartta, "list", &json!({ "path": "main.go" })),
        "not_a_directory"
    );
}

#[test]
fn access_keeps_the_uses_as_a_type_alone_or_among_the_reads() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("main.go"),
        "package main\n\ntype T struct{}\n\nfunc (T) m() {}\n\nfunc f(t T) {\n\tT.m(t)\n}\n",
    )
    .expect("main.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &json!({ "languages": ["go"] }));
    let accesses = |access: &str| {
        let params = json!({ "name": "T", "access": access });
        let part = ok(&kartta, "references", &params)["matches"][0].clone();
        let sites = part["sites"].as_array().expect("the sites");
        let mut accesses: Vec<String> = sites
            .iter()
            .map(|site| site["access"].as_str().expect("a word").to_owned())
            .collect();
        accesses.sort();
        (accesses, part["counts"].clone())
    };

    let (types, counts) = accesses("type");
    assert_eq!(types, ["type", "type"]);
    assert_eq!(
        (&counts["reads"], &counts["types"]),
        (&json!(1), &json!(2)),
        "{counts}"
    );

    let (reads, _) = accesses("read");
    assert_eq!(reads, ["read", "type", "type"]);
}

#[test]
fn a_fuzzy_limit_of_0_sets_no_cap_and_one_left_out_takes_the_default() {
    let tree = tree();
    for at in 0..60 {
        fs::write(tree.path().join(format!("gen{at}.go")), "package main\n").expect("a file");
    }
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let every = ok(&kartta, "fuzzy", &json!({ "query": "go", "limit": 0 }));
    let default = ok(&kartta, "fuzzy", &json!({ "query": "go" }));

    assert_eq!(
        every["matches"].as_array().map(Vec::len),
        Some(62),
        "{every}"
    );
    assert_eq!(every["truncated"], false);
    assert_eq!(default["matches"].as_array().map(Vec::len), Some(50));
    assert_eq!(default["truncated"], true);
}

#[test]
fn a_fuzzy_answer_counts_its_matches_unless_the_query_is_empty() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let found = ok(&kartta, "fuzzy", &json!({ "query": "go", "limit": 1 }));
    assert_eq!(
        (&found["truncated"], &found["matched"]),
        (&json!(true), &json!(2)),
        "{found}"
    );

    let nothing = ok(&kartta, "fuzzy", &json!({ "query": "zz" }));
    assert_eq!(
        (&nothing["matches"], &nothing["matched"]),
        (&json!([]), &json!(0)),
        "a query with terms is counted, 0 included: {nothing}"
    );

    let everything = ok(&kartta, "fuzzy", &json!({ "query": "", "limit": 1 }));
    assert_eq!(
        everything.get("matched"),
        None,
        "an empty query is not counted: {everything}"
    );
    assert_eq!(everything["truncated"], true, "{everything}");

    let exact = ok(&kartta, "fuzzy", &json!({ "query": "", "limit": 2 }));
    assert_eq!(
        (
            exact["matches"].as_array().map(Vec::len),
            &exact["truncated"]
        ),
        (Some(2), &json!(false)),
        "exactly the limit left nothing out: {exact}"
    );
}

#[test]
fn a_segment_left_open_ends_on_the_last_line_a_held_symbol_reaches() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let whole = ok(&kartta, "analyze_segment", &json!({ "file": "run.go" }));
    assert_eq!(whole["end_line"], 7, "{whole}");

    let past = ok(
        &kartta,
        "analyze_segment",
        &json!({ "file": "run.go", "start_line": 50 }),
    );
    assert_eq!(
        (&past["start_line"], &past["end_line"]),
        (&json!(50), &json!(50)),
        "a start past every symbol stays a range: {past}"
    );

    let asked = ok(
        &kartta,
        "analyze_segment",
        &json!({ "file": "run.go", "start_line": 3, "end_line": 40 }),
    );
    assert_eq!(asked["end_line"], 40, "a line the caller names: {asked}");
}

/// What a file answer says beside its `file` and `held`: the keys that tell its state apart.
fn file_state(answer: &Value) -> Value {
    json!({
        "graph": answer.get("graph"),
        "held": answer["held"],
        "missing": answer.get("missing"),
        "declined": answer.get("declined"),
    })
}

#[test]
fn a_file_answer_tells_a_missing_path_an_unclaimed_file_and_a_declined_one_apart() {
    let tree = tree();
    fs::write(
        tree.path().join("_skip.go"),
        "package main\n\nfunc skip() {}\n",
    )
    .expect("_skip.go");
    fs::write(tree.path().join("notes.txt"), "notes\n").expect("notes.txt");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    for method in ["outline", "dependents", "analyze_segment"] {
        let state = |file: &str| file_state(&ok(&kartta, method, &json!({ "file": file })));
        assert_eq!(
            state("run.go"),
            json!({ "graph": "go", "held": true, "missing": null, "declined": null }),
            "{method} of a file the graph holds"
        );
        assert_eq!(
            state("gone.go"),
            json!({ "graph": null, "held": false, "missing": true, "declined": null }),
            "{method} of a path the index does not hold"
        );
        assert_eq!(
            state("_gone.go"),
            json!({ "graph": null, "held": false, "missing": true, "declined": null }),
            "{method} of a path the index does not hold, named as a language leaves a file out"
        );
        assert_eq!(
            state("notes.txt"),
            json!({ "graph": null, "held": false, "missing": null, "declined": null }),
            "{method} of a held file no language claims"
        );
        assert_eq!(
            state("_skip.go"),
            json!({
                "graph": null,
                "held": false,
                "missing": null,
                "declined": {
                    "language": "go",
                    "reason": "no Go build reads a file whose name starts with _ or ."
                },
            }),
            "{method} of a held file a language leaves out"
        );
    }
}

#[test]
fn a_file_answer_tells_a_directory_from_a_held_file() {
    let tree = tree();
    for dir in ["pkg", "_dir.go"] {
        fs::create_dir(tree.path().join(dir)).expect(dir);
        fs::write(tree.path().join(dir).join("notes.txt"), "notes\n").expect("notes.txt");
    }
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    for method in ["outline", "dependents", "analyze_segment"] {
        for dir in ["pkg", "_dir.go"] {
            assert_eq!(
                ok(&kartta, method, &json!({ "file": dir })),
                json!({ "file": dir, "held": false, "directory": true }),
                "{method} of a directory, which no graph claims and no language leaves out"
            );
        }
    }
}

#[test]
fn a_graphs_stats_count_its_uses_by_access() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("main.go"),
        "package main\n\ntype T struct{}\n\nvar v = 1\n\nfunc f(t T) int {\n\tv = 2\n\tv += 1\n\treturn v\n}\n",
    )
    .expect("main.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &json!({ "languages": ["go"] }));

    let stats = ok(&kartta, "stats", &Value::Null);

    let graph = &stats["graphs"][0];
    assert_eq!(
        (
            &graph["reads"],
            &graph["writes"],
            &graph["updates"],
            &graph["types"]
        ),
        (&json!(1), &json!(1), &json!(1), &json!(1)),
        "{graph}"
    );
    assert_eq!(graph["uses"], 4, "the four counts together: {graph}");
}

#[test]
fn empty_text_whitespace_and_null_each_read_as_no_params() {
    let tree = tree();
    let kartta = open(tree.path());
    let empty_object = kartta.call("version", b"{}");
    assert!(empty_object.starts_with(br#"{"ok":"#));

    for params in [&b""[..], b" \n", b"null", b" null\n"] {
        assert_eq!(kartta.call("version", params), empty_object, "{params:?}");
    }
}

#[test]
fn params_that_are_not_the_method_s_answer_invalid_params() {
    let tree = tree();
    let kartta = open(tree.path());

    assert_eq!(
        code(&kartta, "build", &json!({ "jobs": 4 })),
        "invalid_params"
    );
    assert_eq!(
        code(&kartta, "build", &json!({ "threads": 0 })),
        "invalid_params"
    );
    assert_eq!(
        code(&kartta, "cancel", &json!({ "build": "first" })),
        "invalid_params"
    );
    assert_eq!(
        code(&kartta, "cancel", &json!({ "build": 1, "by": "slot" })),
        "invalid_params"
    );
    let not_json: Value = serde_json::from_slice(&kartta.call("version", b"{")).expect("JSON");
    assert_eq!(not_json["err"]["code"], "invalid_request");
    assert_eq!(
        code(&kartta, "version", &json!({ "verbose": true })),
        "invalid_params"
    );
    assert_eq!(call(&kartta, "version", &json!({}))["ok"]["abi"], 1);
    assert_eq!(
        code(&kartta, "build", &json!({ "languages": ["go", "golang"] })),
        "invalid_params"
    );
}

#[test]
fn params_that_are_not_an_object_answer_invalid_params_naming_what_they_are() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let refusals = [
        (
            call(&kartta, "build", &json!(["ignored"]))["err"].clone(),
            "an array",
        ),
        (
            call(&kartta, "version", &json!([]))["err"].clone(),
            "an array",
        ),
        (call(&kartta, "grep", &json!(-1))["err"].clone(), "a number"),
        (
            call(&kartta, "stats", &json!("x"))["err"].clone(),
            "a string",
        ),
        (
            call(&kartta, "cancel", &json!(false))["err"].clone(),
            "a boolean",
        ),
        (refused(br#"["/"]"#), "an array"),
    ];

    for (error, kind) in refusals {
        assert_eq!(error["code"], "invalid_params", "{error}");
        assert!(mentions(&error, &format!("not {kind}")), "{error}");
    }
    let malformed: Value =
        serde_json::from_slice(&kartta.call("version", b"[1")).expect("the answer is JSON");
    assert_eq!(
        malformed["err"]["code"], "invalid_request",
        "an array that does not parse is a malformed request, not a wrong shape"
    );
}

#[test]
fn a_build_answers_what_it_indexed_and_what_each_graph_holds() {
    let tree = tree();
    let kartta = open(tree.path());

    let built = ok(
        &kartta,
        "build",
        &json!({ "languages": ["go"], "content_index": { "storage": "memory" } }),
    );

    assert_eq!(built["entries"], 2);
    assert_eq!(
        built["languages"],
        json!([{"name": "go", "family": "go", "claimed": 2, "read": 2, "cached": 0, "held": 2}])
    );
    assert_eq!(built["graphs"][0]["name"], "go");
    assert_eq!(built["graphs"][0]["symbols"], 3);
    assert_eq!(built["graphs"][0]["kinds"], json!({ "func": 3 }));
    assert_eq!(built["content_index"]["files"], 2);
    assert_eq!(built["errors"], json!({ "total": 0, "listed": [] }));
    assert_eq!(ok(&kartta, "stats", &Value::Null), built);
}

#[test]
fn a_list_of_0_answers_the_counts_alone() {
    let tree = tree();
    fs::write(
        tree.path().join("dead.go"),
        "package main\n\nfunc unused() {\n\tmissing()\n}\n",
    )
    .expect("dead.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    let lists_nothing = |method: &str, params: &Value, list: &str, count: &str| {
        let answer = ok(&kartta, method, params);
        let part = &answer["matches"][0];
        assert_eq!(part[list], json!([]), "{method}: {answer}");
        assert_eq!(part[count], 1, "{method}: {answer}");
    };

    let helper = json!({ "name": "helper", "limit": 0 });
    lists_nothing("references", &helper, "sites", "total");
    lists_nothing("definitions", &helper, "found", "total");
    lists_nothing("sources", &helper, "found", "total");
    let search = json!({ "query": "helper", "limit": 0 });
    lists_nothing("search", &search, "hits", "total");
    let graph = json!({ "lang": "go", "limit": 0 });
    lists_nothing("dead_code", &graph, "found", "total");
    lists_nothing("resolution", &graph, "unbound", "unlisted");
}

#[test]
fn an_implementations_limit_of_0_answers_the_total_alone() {
    let (_tree, kartta) = rust_crate();

    let answer = ok(
        &kartta,
        "implementations",
        &json!({ "name": "Paint", "limit": 0 }),
    );

    assert_eq!(answer["matches"][0]["found"], json!([]), "{answer}");
    assert_eq!(answer["matches"][0]["total"], 1, "{answer}");
}

#[test]
fn a_depth_of_0_follows_nothing_and_counts_what_it_left() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let walk = ok(&kartta, "callers", &json!({ "name": "helper", "depth": 0 }));
    let path = ok(
        &kartta,
        "call_path",
        &json!({ "from": "main", "to": "helper", "depth": 0 }),
    );
    let dependents = ok(
        &kartta,
        "dependents",
        &json!({ "file": "run.go", "depth": 0 }),
    );
    let segment = ok(
        &kartta,
        "analyze_segment",
        &json!({ "file": "run.go", "depth": 0 }),
    );
    let change = ok(
        &kartta,
        "analyze_change",
        &json!({ "regions": [{ "file": "run.go" }], "depth": 0 }),
    );

    assert_eq!(walk["matches"][0]["reached"], json!([]), "{walk}");
    assert_eq!(walk["matches"][0]["unexpanded"], 1, "{walk}");
    assert_eq!(path["matches"][0]["found"], false, "{path}");
    assert_eq!(dependents["found"], json!([]), "{dependents}");
    assert_eq!(dependents["unexpanded"], 1, "{dependents}");
    assert_eq!(segment["symbols"][0]["blast"]["reach"], 0, "{segment}");
    assert_eq!(change["footprint"], 0, "{change}");
}

#[test]
fn an_implementations_depth_of_0_finds_none() {
    let (_tree, kartta) = rust_crate();

    let answer = ok(
        &kartta,
        "implementations",
        &json!({ "name": "Paint", "depth": 0 }),
    );

    assert_eq!(answer["matches"][0]["found"], json!([]), "{answer}");
    assert_eq!(answer["matches"][0]["unexpanded"], 1, "{answer}");
}

#[test]
fn a_symbol_cap_of_0_lifts_the_cap_and_one_left_out_takes_the_default() {
    let tree = tree();
    let reached = 600;
    fs::write(
        tree.path().join("many.go"),
        format!(
            "package main\n\n{}",
            callers_of("many", "helper()", reached)
        ),
    )
    .expect("many.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    let walk = |params: &Value| {
        let answer = ok(&kartta, "callers", params);
        (
            answer["matches"][0]["reached"].as_array().map(Vec::len),
            answer["matches"][0]["truncated"].clone(),
        )
    };

    let lifted = walk(&json!({ "name": "helper", "depth": 1, "max_symbols": 0 }));
    let default = walk(&json!({ "name": "helper", "depth": 1 }));

    assert_eq!(lifted, (Some(reached + 1), json!(false)));
    assert_eq!(default, (Some(499), json!(true)));
}

#[test]
fn a_grep_byte_cap_of_0_lifts_it_and_one_left_out_takes_the_default() {
    let tree = tree();
    let line = format!("needle{}\n", "x".repeat(2048));
    fs::write(tree.path().join("long.txt"), &line).expect("long.txt");
    fs::write(
        tree.path().join("big.txt"),
        line.repeat(4 << 20 >> 11).into_bytes(),
    )
    .expect("big.txt");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    let glob = |file: &str, caps: Value| {
        let mut params = json!({ "patterns": ["needle"], "globs": [file], "limit": 1 });
        params
            .as_object_mut()
            .expect("an object")
            .extend(caps.as_object().expect("an object").clone());
        ok(&kartta, "grep", &params)
    };

    let whole = glob("long.txt", json!({ "max_line_bytes": 0 }));
    let cut = glob("long.txt", json!({}));
    let read = glob("big.txt", json!({ "max_file_bytes": 0 }));
    let skipped = glob("big.txt", json!({}));

    assert_eq!(
        whole["files"][0]["lines"][0].get("cut"),
        None,
        "the line comes whole"
    );
    assert!(cut["files"][0]["lines"][0].get("cut").is_some(), "{cut}");
    assert_eq!(read["oversized"], 0, "the file is read");
    assert_eq!(read["lines"], 1);
    assert_eq!(skipped["oversized"], 1, "the default skips it");
}

#[test]
fn a_range_end_of_0_reads_as_left_out() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let zeros = ok(
        &kartta,
        "analyze_segment",
        &json!({ "file": "run.go", "start_line": 0, "end_line": 0 }),
    );
    let left_out = ok(&kartta, "analyze_segment", &json!({ "file": "run.go" }));

    assert_eq!(zeros, left_out);
    assert_eq!(zeros["start_line"], 1);
}

#[test]
fn a_line_of_0_selects_nothing_and_says_where_the_name_starts() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let answer = ok(&kartta, "callers", &json!({ "name": "helper", "line": 0 }));

    assert_eq!(answer["matches"], json!([]), "{answer}");
    assert_eq!(answer["unmatched"]["kind"], "off_line", "{answer}");
}

#[test]
fn an_id_of_0_names_a_call_and_a_cancel_of_it_stops_no_other() {
    let tree = tree();
    let kartta = open(tree.path());

    let cancel = ok(&kartta, "cancel", &json!({ "build": 0 }));
    let built = code(&kartta, "build", &json!({ "id": 0 }));
    ok(&kartta, "build", &json!({ "id": 1 }));

    assert_eq!(cancel["stopped"], false);
    assert_eq!(built, "build_cancelled");
}

#[test]
fn a_search_only_the_fill_up_matches_answers_the_graph_with_its_fuzzy_hits() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let search = ok(&kartta, "search", &json!({ "query": "hlpr" }));

    let matched = &search["matches"][0];
    assert_eq!(matched["graph"], "go");
    assert_eq!(matched["total"], 0);
    assert_eq!(matched["fuzzy_total"], 1);
    assert_eq!(matched["hits"][0]["definition"]["name"], "helper");
    assert_eq!(matched["hits"][0]["fuzzy"], true);
}

#[test]
fn resolution_names_a_read_past_its_bound_and_counts_it_only_where_one_exists() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    let counts = ok(&kartta, "resolution", &json!({ "limit": 1 }));
    assert!(counts["matches"][0]["misses"].get("past_bound").is_none());

    fs::write(
        tree.path().join("cycle.go"),
        "package main\n\nvar a = b\nvar b = a\n\nfunc cycle() {\n\ta.Set()\n}\n",
    )
    .expect("cycle.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let answer = ok(&kartta, "resolution", &json!({ "limit": 1 }));

    let matched = &answer["matches"][0];
    assert_eq!(matched["misses"]["past_bound"], 1);
    assert_eq!(matched["misses"]["not_admitted"], 0);
    assert_eq!(matched["unbound"][0]["miss"], "past_bound");
}

#[test]
fn callers_answers_each_graph_s_walk_with_what_it_left_out() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let callers = ok(&kartta, "callers", &json!({ "name": "helper" }));
    let one_deep = ok(
        &kartta,
        "callers",
        &json!({ "name": "helper", "lang": "go", "depth": 1 }),
    );

    let walk = &callers["matches"][0];
    assert_eq!(callers["matches"].as_array().map(Vec::len), Some(1));
    assert_eq!(walk["graph"], "go");
    assert_eq!(walk["root"]["qualified_name"], "main.helper");
    assert_eq!(walk["root"]["file"], "run.go");
    let reached: Vec<(&Value, &Value)> = walk["reached"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|at| (&at["symbol"]["name"], &at["depth"]))
        .collect();
    assert_eq!(
        reached,
        [(&json!("run"), &json!(1)), (&json!("main"), &json!(2))]
    );
    assert_eq!(walk["truncated"], false);
    assert_eq!(walk["unexpanded"], 0);
    assert_eq!(
        one_deep["matches"][0]["reached"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(one_deep["matches"][0]["unexpanded"], 1);
    assert_eq!(
        ok(&kartta, "callers", &json!({ "name": "nothing" })),
        json!({ "matches": [] })
    );
    assert!(
        walk.get("file_scope").is_none(),
        "a walk no file's top level calls into carries no file_scope"
    );
}

fn callers_of(target: &str, call: &str, count: usize) -> String {
    (0..count)
        .map(|at| format!("func {target}Caller{at}() {{\n\t{call}\n}}\n\n"))
        .collect::<Vec<_>>()
        .concat()
}

#[test]
fn callers_grades_the_reach_as_a_change_to_the_symbol_does_and_callees_does_not() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("lib.go"),
        format!(
            "package lib\n\ntype Port interface{{}}\n\nfunc Shared() {{}}\n\nfunc local() {{}}\n\n{}{}{}",
            callers_of("Shared", "Shared()", 20),
            callers_of("local", "local()", 20),
            callers_of("Port", "_ = Port(nil)", 50),
        ),
    )
    .expect("lib.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let band =
        |name: &str| ok(&kartta, "callers", &json!({ "name": name }))["matches"][0]["band"].clone();
    let segment = ok(
        &kartta,
        "analyze_segment",
        &json!({ "file": "lib.go", "start_line": 5, "end_line": 5 }),
    );
    let capped = ok(
        &kartta,
        "callers",
        &json!({ "name": "Port", "max_symbols": 11 }),
    );

    assert_eq!(band("Port"), "critical");
    assert_eq!(capped["matches"][0]["truncated"], true);
    assert_eq!(
        capped["matches"][0]["band"], "medium",
        "a walk the cap cut grades the callers it reached"
    );
    assert_eq!(band("Shared"), "high");
    assert_eq!(band("local"), "medium");
    assert_eq!(band("SharedCaller0"), "low");
    assert_eq!(
        segment["symbols"][0]["blast"]["band"],
        band("Shared"),
        "a segment grades the same symbol alike"
    );
    assert!(
        ok(&kartta, "callees", &json!({ "name": "Shared" }))["matches"][0]
            .get("band")
            .is_none(),
        "a band grades the callers a change reaches, so a walk over callees carries none"
    );
}

#[test]
fn callers_answers_the_files_whose_top_level_calls_the_root() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::create_dir_all(tree.path().join("bin")).expect("bin/");
    fs::create_dir_all(tree.path().join("lib")).expect("lib/");
    fs::write(
        tree.path().join("bin/run.sh"),
        "#!/bin/sh\n. lib/common.sh\n\nsetup() {\n\techo ready\n}\n\nsetup\nshared\n",
    )
    .expect("bin/run.sh");
    fs::write(
        tree.path().join("lib/common.sh"),
        "shared() {\n\techo shared\n}\n",
    )
    .expect("lib/common.sh");
    fs::write(
        tree.path().join("bin/check.sh"),
        "#!/bin/sh\n. lib/common.sh\n\nshared\n",
    )
    .expect("bin/check.sh");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let own = ok(&kartta, "callers", &json!({ "name": "setup" }));
    let included = ok(&kartta, "callers", &json!({ "name": "shared" }));
    let floored = ok(
        &kartta,
        "callers",
        &json!({ "name": "shared", "min_confidence": 75 }),
    );

    assert_eq!(
        own["matches"][0]["file_scope"],
        json!([{ "file": "bin/run.sh", "confidence": 80 }])
    );
    assert_eq!(own["matches"][0]["reached"], json!([]));
    assert_eq!(
        included["matches"][0]["file_scope"],
        json!([
            { "file": "bin/check.sh", "confidence": 70 },
            { "file": "bin/run.sh", "confidence": 70 }
        ]),
        "the files come in path order"
    );
    assert!(floored["matches"][0].get("file_scope").is_none());
    assert_eq!(floored["matches"][0]["below_confidence"], 2);
}

/// A built handle over two copies of a Go package: each defines `Parse` and `Base`, and `Start`, which calls `Helper`.
/// `a` calls `Parse` from `One`, `Start` from `Kick`, and embeds `Base` in `Impl`; `b` calls `Parse` from `Two`. The
/// tree directory is returned to keep it alive.
fn two_copies() -> (tempfile::TempDir, Kartta) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (dir, caller, embedder, kicker) in [
        (
            "a",
            "One",
            "type Impl interface{ Base }\n\n",
            "\nfunc Kick() { Start() }\n",
        ),
        ("b", "Two", "", ""),
    ] {
        fs::create_dir_all(tree.path().join(dir)).expect("a package directory");
        fs::write(
            tree.path().join(dir).join("util.go"),
            format!(
                "package util\n\ntype Base interface{{}}\n\n{embedder}func Parse() {{}}\n\nfunc {caller}() {{ Parse() }}\n\n\
                 func Helper() {{}}\n\nfunc Start() {{ Helper() }}\n{kicker}"
            ),
        )
        .expect("util.go");
    }
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    (tree, kartta)
}

/// The file of the symbol under `at` in an answer's first match.
fn file(answer: &Value, at: &str) -> Value {
    answer["matches"][0][at]["file"].clone()
}

#[test]
fn a_path_prefix_picks_one_of_two_copies_and_an_answer_lists_the_copy_it_passed_over() {
    let (_tree, kartta) = two_copies();

    let either = ok(&kartta, "references", &json!({ "name": "util.Parse" }));
    let in_b = ok(
        &kartta,
        "references",
        &json!({ "name": "util.Parse", "path_prefix": "b/" }),
    );
    let path = ok(
        &kartta,
        "call_path",
        &json!({ "from": "One", "to": "util.Parse" }),
    );
    let into_b = ok(
        &kartta,
        "call_path",
        &json!({ "from": "Two", "from_path_prefix": "b/", "to": "util.Parse", "to_path_prefix": "b/" }),
    );
    let from_a = ok(
        &kartta,
        "call_path",
        &json!({ "from": "util.Start", "to": "util.Helper" }),
    );
    let from_b = ok(
        &kartta,
        "call_path",
        &json!({ "from": "util.Start", "from_path_prefix": "b/", "to": "util.Helper", "to_path_prefix": "b/" }),
    );

    assert_eq!(file(&either, "symbol"), "a/util.go");
    assert_eq!(
        either["matches"][0]["candidates"][0]["symbol"]["file"],
        "b/util.go"
    );
    assert_eq!(
        either["matches"][0]["candidates"][0]["count"], 1,
        "the passed-over copy's count is its sites: Two calls it once"
    );
    assert_eq!(file(&in_b, "symbol"), "b/util.go");
    assert!(
        in_b["matches"][0].get("candidates").is_none(),
        "the prefix leaves no copy to pass over"
    );
    let ends = &path["matches"][0]["candidates"];
    assert_eq!(ends["from"], json!([]), "One has no copy");
    assert_eq!(ends["to"][0]["symbol"]["file"], "b/util.go");
    assert_eq!(
        (file(&into_b, "from"), file(&into_b, "to")),
        (json!("b/util.go"), json!("b/util.go"))
    );
    assert_eq!(into_b["matches"][0]["found"], true);
    assert!(
        into_b["matches"][0].get("candidates").is_none(),
        "neither end passed a copy over"
    );
    assert_eq!(
        file(&from_a, "from"),
        "a/util.go",
        "the Start with a caller ranks first"
    );
    assert_eq!(
        from_a["matches"][0]["candidates"]["from"][0]["symbol"]["file"],
        "b/util.go"
    );
    assert_eq!(
        (file(&from_b, "from"), file(&from_b, "to")),
        (json!("b/util.go"), json!("b/util.go"))
    );
    assert_eq!(from_b["matches"][0]["found"], true);
}

#[test]
fn a_path_prefix_picks_the_copy_a_walk_or_an_implementations_search_starts_from() {
    let (_tree, kartta) = two_copies();

    let incoming = ok(
        &kartta,
        "callers",
        &json!({ "name": "util.Parse", "path_prefix": "b/" }),
    );
    let outgoing = ok(
        &kartta,
        "callees",
        &json!({ "name": "util.Parse", "path_prefix": "b/" }),
    );
    let either = ok(&kartta, "implementations", &json!({ "name": "util.Base" }));
    let in_b = ok(
        &kartta,
        "implementations",
        &json!({ "name": "util.Base", "path_prefix": "b/" }),
    );

    assert_eq!(file(&incoming, "root"), "b/util.go");
    assert_eq!(
        incoming["matches"][0]["reached"][0]["symbol"]["name"],
        "Two"
    );
    assert_eq!(file(&outgoing, "root"), "b/util.go");
    assert_eq!(
        file(&either, "root"),
        "a/util.go",
        "the copy with an implementor ranks first"
    );
    assert_eq!(
        either["matches"][0]["candidates"][0]["symbol"]["file"],
        "b/util.go"
    );
    assert_eq!(file(&in_b, "root"), "b/util.go");
}

#[test]
fn a_line_picks_one_of_two_overloads_in_one_file() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("o.cpp"),
        "void a() {}\nvoid b() {}\nnamespace ns {\nvoid f(int x) { a(); }\nvoid f(double x) { b(); }\n}\n\
         namespace p {\nstruct S {};\n}\nnamespace q {\nstruct S {};\n}\n",
    )
    .expect("o.cpp");
    let kartta = open(tree.path());
    ok(&kartta, "build", &json!({ "languages": ["cpp"] }));
    let at = |line: u32| json!({ "name": "ns::f", "path_prefix": "o.cpp", "line": line });
    let line = |answer: &Value, at: &str| answer["matches"][0][at]["line"].clone();

    let by_file = ok(
        &kartta,
        "callees",
        &json!({ "name": "ns::f", "path_prefix": "o.cpp" }),
    );
    let outgoing = ok(&kartta, "callees", &at(5));
    let incoming = ok(&kartta, "callers", &at(5));
    let used = ok(&kartta, "references", &at(5));
    let path = ok(
        &kartta,
        "call_path",
        &json!({ "from": "ns::f", "from_path_prefix": "o.cpp", "from_line": 5, "to": "b", "to_line": 2 }),
    );
    let into = ok(
        &kartta,
        "call_path",
        &json!({ "from": "ns::f", "from_path_prefix": "o.cpp", "to": "ns::f", "to_line": 5 }),
    );
    let types = ok(
        &kartta,
        "implementations",
        &json!({ "name": "S", "path_prefix": "o.cpp", "line": 11 }),
    );
    let off = ok(&kartta, "callees", &at(3));

    assert_eq!(line(&by_file, "root"), 4);
    assert_eq!(by_file["matches"][0]["candidates"][0]["symbol"]["line"], 5);
    assert_eq!(line(&outgoing, "root"), 5);
    assert_eq!(outgoing["matches"][0]["reached"][0]["symbol"]["name"], "b");
    assert_eq!(outgoing["matches"][0]["passed_over"], 0);
    assert_eq!(line(&incoming, "root"), 5);
    assert_eq!(line(&used, "symbol"), 5);
    assert_eq!(
        (line(&path, "from"), path["matches"][0]["found"].clone()),
        (json!(5), json!(true))
    );
    assert_eq!(
        (line(&into, "from"), line(&into, "to")),
        (json!(4), json!(5))
    );
    assert_eq!(line(&types, "root"), 11);
    assert_eq!(types["matches"][0]["passed_over"], 0);
    assert_eq!(off["matches"], json!([]), "no ns::f starts on line 3");
    assert_eq!(off["unmatched"]["kind"], "off_line");
}

/// A built handle over a Rust crate: `Index::rebuild` calls `Index::refresh`, and `Index` implements
/// `render::Paint`. The tree directory is returned to keep it alive.
fn rust_crate() -> (tempfile::TempDir, Kartta) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");
    fs::create_dir(tree.path().join("src")).expect("src");
    fs::write(
        tree.path().join("src/lib.rs"),
        "pub mod render {\n    pub trait Paint {\n        fn paint(&self);\n    }\n}\n\npub struct Index;\n\n\
         impl Index {\n    pub fn refresh(&self) {}\n\n    pub fn rebuild(&self) {\n        self.refresh();\n    }\n}\n\n\
         impl render::Paint for Index {\n    fn paint(&self) {}\n}\n",
    )
    .expect("lib.rs");
    let kartta = open(tree.path());
    ok(&kartta, "build", &json!({ "languages": ["rust"] }));
    (tree, kartta)
}

/// Each method that takes one symbol's name, with the params that name `callable`, or `parent` for implementations.
fn by_name(callable: &str, parent: &str) -> [(&'static str, Value); 6] {
    [
        ("callers", json!({ "name": callable })),
        ("callees", json!({ "name": callable })),
        ("references", json!({ "name": callable })),
        ("definitions", json!({ "name": callable })),
        ("sources", json!({ "name": callable })),
        ("implementations", json!({ "name": parent })),
    ]
}

/// The keys of an answer.
fn keys(answer: &Value) -> Vec<&str> {
    answer
        .as_object()
        .expect("an answer is an object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn a_rust_name_written_with_dots_answers_as_its_own_spelling_and_names_the_reading() {
    let (_tree, kartta) = rust_crate();

    let written = by_name("Index.rebuild", "render.Paint");
    for ((method, written), (_, own)) in written
        .into_iter()
        .zip(by_name("Index::rebuild", "render::Paint"))
    {
        let mut expected = ok(&kartta, method, &own);
        assert!(
            expected["matches"]
                .as_array()
                .is_some_and(|matches| !matches.is_empty()),
            "{method} {own}: {expected}"
        );
        expected["read_as"] = own["name"].clone();
        assert_eq!(
            ok(&kartta, method, &written),
            expected,
            "{method} {written}"
        );
    }

    let mut expected = ok(
        &kartta,
        "call_path",
        &json!({ "from": "Index::rebuild", "to": "Index::refresh" }),
    );
    assert_eq!(expected["matches"][0]["found"], true, "{expected}");
    expected["to_read_as"] = json!("Index::refresh");
    let one_end = json!({ "from": "Index::rebuild", "to": "Index.refresh" });
    assert_eq!(
        ok(&kartta, "call_path", &one_end),
        expected,
        "an end that selects as written is not read"
    );
    expected["from_read_as"] = json!("Index::rebuild");
    let both = json!({ "from": "Index.rebuild", "to": "Index.refresh" });
    assert_eq!(ok(&kartta, "call_path", &both), expected);
}

#[test]
fn a_name_that_selects_as_written_or_in_neither_spelling_answers_no_reading() {
    let (_tree, kartta) = rust_crate();

    for (method, params) in by_name("Index::rebuild", "render::Paint")
        .into_iter()
        .chain(by_name("Index.missing", "render.Missing"))
    {
        let answer = ok(&kartta, method, &params);
        assert_eq!(keys(&answer), ["matches"], "{method} {params}: {answer}");
    }
    let path = ok(
        &kartta,
        "call_path",
        &json!({ "from": "Index::rebuild", "to": "Index::refresh" }),
    );
    assert_eq!(keys(&path), ["matches"], "{path}");

    let unmatched_end = ok(
        &kartta,
        "call_path",
        &json!({ "from": "Index.rebuild", "to": "Index.missing" }),
    );
    assert_eq!(
        unmatched_end,
        json!({ "matches": [], "from_read_as": "Index::rebuild" }),
        "the end its reading selects says so, though the other end selects nothing"
    );
}

/// A built handle over one Go package per name of `packages`, each defining `Store.Close`, the interface `Closer` and
/// `Run`, which calls `Close`. The tree directory is returned to keep it alive.
fn stores(packages: &[&str]) -> (tempfile::TempDir, Kartta) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for package in packages {
        fs::create_dir_all(tree.path().join(package)).expect("a package directory");
        fs::write(
            tree.path().join(package).join("store.go"),
            format!(
                "package {package}\n\ntype Store struct{{}}\n\nfunc (Store) Close() {{}}\n\n\
                 type Closer interface{{ Close() }}\n\nfunc Run() {{ Store{{}}.Close() }}\n"
            ),
        )
        .expect("store.go");
    }
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    (tree, kartta)
}

/// The kind, the count and the files of the definitions an answer's `key` names in place of a match.
fn unmatched_in(answer: &Value, key: &str) -> (Value, Value, Vec<Value>) {
    let unmatched = &answer[key];
    let files = unmatched["listed"]
        .as_array()
        .unwrap_or_else(|| panic!("no {key} in {answer}"))
        .iter()
        .map(|listed| {
            assert_eq!(listed["graph"], "go", "{listed}");
            listed["definition"]["file"].clone()
        })
        .collect();
    (unmatched["kind"].clone(), unmatched["total"].clone(), files)
}

/// The qualified names of the definitions an answer's `key` names in place of a match.
fn qualified_in(answer: &Value, key: &str) -> Vec<Value> {
    answer[key]["listed"]
        .as_array()
        .unwrap_or_else(|| panic!("no {key} in {answer}"))
        .iter()
        .map(|listed| listed["definition"]["qualified_name"].clone())
        .collect()
}

/// `name` as packages `a` and `b` of [`stores`] qualify it.
fn in_each_package(name: &str) -> [Value; 2] {
    ["a", "b"].map(|package| json!(format!("{package}.{name}")))
}

#[test]
fn a_name_several_definitions_end_with_answers_them_in_place_of_a_match() {
    let (_tree, kartta) = stores(&["a", "b"]);

    for method in ["callers", "callees", "references"] {
        let answer = ok(&kartta, method, &json!({ "name": "Store.Close" }));

        assert_eq!(answer["matches"], json!([]), "{method}: {answer}");
        assert_eq!(
            unmatched_in(&answer, "unmatched"),
            (
                json!("several"),
                json!(2),
                vec![json!("a/store.go"), json!("b/store.go")]
            ),
            "{method}"
        );
    }
    let read = ok(&kartta, "callers", &json!({ "name": "Store::Close" }));
    assert_eq!(read["read_as"], "Store.Close", "{read}");
    assert_eq!(
        read["unmatched"]["kind"], "several",
        "the reading's own: {read}"
    );
}

#[test]
fn a_name_defined_only_outside_its_path_prefix_says_so_and_counts_the_definitions() {
    let (_tree, kartta) = stores(&["a", "b"]);

    for (method, name) in [
        ("callers", "Store.Close"),
        ("callees", "Store.Close"),
        ("references", "Store.Close"),
        ("implementations", "Closer"),
    ] {
        let answer = ok(
            &kartta,
            method,
            &json!({ "name": name, "path_prefix": "c/" }),
        );

        assert_eq!(answer["matches"], json!([]), "{method}: {answer}");
        assert_eq!(
            unmatched_in(&answer, "unmatched"),
            (
                json!("outside_prefix"),
                json!(2),
                vec![json!("a/store.go"), json!("b/store.go")]
            ),
            "{method}"
        );
    }
    let one = ok(
        &kartta,
        "callers",
        &json!({ "name": "a.Store.Close", "path_prefix": "b/" }),
    );
    assert_eq!(
        unmatched_in(&one, "unmatched"),
        (json!("outside_prefix"), json!(1), vec![json!("a/store.go")])
    );
}

#[test]
fn a_call_path_end_that_selects_nothing_says_what_it_names_and_one_that_selects_says_nothing() {
    let (_tree, kartta) = stores(&["a", "b"]);

    let several = ok(
        &kartta,
        "call_path",
        &json!({ "from": "a.Run", "to": "Store.Close" }),
    );
    let outside = ok(
        &kartta,
        "call_path",
        &json!({ "from": "Run", "from_path_prefix": "c/", "to": "a.Store.Close" }),
    );
    let both = ok(
        &kartta,
        "call_path",
        &json!({ "from": "Store.Close", "to": "Run", "to_path_prefix": "c/" }),
    );

    assert_eq!(keys(&several), ["matches", "to_unmatched"], "{several}");
    assert_eq!(
        unmatched_in(&several, "to_unmatched"),
        (
            json!("several"),
            json!(2),
            vec![json!("a/store.go"), json!("b/store.go")]
        )
    );
    assert_eq!(keys(&outside), ["from_unmatched", "matches"], "{outside}");
    assert_eq!(
        unmatched_in(&outside, "from_unmatched"),
        (
            json!("outside_prefix"),
            json!(2),
            vec![json!("a/store.go"), json!("b/store.go")]
        )
    );
    assert_eq!(
        keys(&both),
        ["from_unmatched", "matches", "to_unmatched"],
        "{both}"
    );
    assert_eq!(
        unmatched_in(&both, "from_unmatched"),
        unmatched_in(&several, "to_unmatched"),
        "each end its own"
    );
    assert_eq!(
        unmatched_in(&both, "to_unmatched"),
        unmatched_in(&outside, "from_unmatched"),
        "each end its own"
    );
    let read = ok(
        &kartta,
        "call_path",
        &json!({ "from": "a.Run", "to": "Store::Close" }),
    );
    assert_eq!(
        keys(&read),
        ["matches", "to_read_as", "to_unmatched"],
        "{read}"
    );
    assert_eq!(read["to_read_as"], "Store.Close");
    assert_eq!(
        unmatched_in(&read, "to_unmatched"),
        unmatched_in(&several, "to_unmatched"),
        "the reading's own"
    );
}

#[test]
fn a_name_that_names_more_than_five_definitions_lists_the_first_five_and_counts_them_all() {
    let (_tree, kartta) = stores(&["a", "b", "c", "d", "e", "f"]);

    let answer = ok(&kartta, "callers", &json!({ "name": "Store.Close" }));

    assert_eq!(
        unmatched_in(&answer, "unmatched"),
        (
            json!("several"),
            json!(6),
            ["a", "b", "c", "d", "e"]
                .map(|package| json!(format!("{package}/store.go")))
                .to_vec()
        )
    );
}

#[test]
fn a_name_nothing_defines_answers_the_definitions_one_edit_from_it() {
    let (_tree, kartta) = stores(&["a", "b"]);
    let both = || {
        (
            json!("one_edit"),
            json!(2),
            vec![json!("a/store.go"), json!("b/store.go")],
        )
    };

    for prefix in [None, Some("c/")] {
        for (method, name, meant) in [
            ("callers", "Stroe.Close", "Store.Close"),
            ("callees", "Store.Clsoe", "Store.Close"),
            ("references", "Store.Closee", "Store.Close"),
            ("implementations", "Closr", "Closer"),
        ] {
            let answer = ok(
                &kartta,
                method,
                &json!({ "name": name, "path_prefix": prefix }),
            );

            assert_eq!(answer["matches"], json!([]), "{method}: {answer}");
            assert_eq!(
                unmatched_in(&answer, "unmatched"),
                both(),
                "{method} {prefix:?}"
            );
            assert_eq!(
                qualified_in(&answer, "unmatched"),
                in_each_package(meant),
                "{method} {prefix:?}"
            );
        }
        let path = ok(
            &kartta,
            "call_path",
            &json!({ "from": "a.Run", "to": "Rnu", "to_path_prefix": prefix }),
        );
        assert_eq!(keys(&path), ["matches", "to_unmatched"], "{path}");
        assert_eq!(unmatched_in(&path, "to_unmatched"), both(), "{prefix:?}");
        assert_eq!(
            qualified_in(&path, "to_unmatched"),
            in_each_package("Run"),
            "{prefix:?}"
        );
    }
}

#[test]
fn definitions_and_sources_of_a_name_nothing_defines_answer_the_definitions_one_edit_from_it() {
    let (_tree, kartta) = stores(&["a", "b"]);

    for method in ["definitions", "sources"] {
        let misspelled = ok(&kartta, method, &json!({ "name": "Stroe.Close" }));
        let matched = ok(&kartta, method, &json!({ "name": "Store.Close" }));
        let unknown = ok(&kartta, method, &json!({ "name": "Missing" }));

        assert_eq!(misspelled["matches"], json!([]), "{method}: {misspelled}");
        assert_eq!(
            unmatched_in(&misspelled, "unmatched"),
            (
                json!("one_edit"),
                json!(2),
                vec![json!("a/store.go"), json!("b/store.go")]
            ),
            "{method}"
        );
        assert_eq!(
            qualified_in(&misspelled, "unmatched"),
            in_each_package("Store.Close"),
            "{method}"
        );
        assert_eq!(keys(&matched), ["matches"], "{method}: {matched}");
        assert_eq!(keys(&unknown), ["matches"], "{method}: {unknown}");
    }
}

#[test]
fn a_name_that_matches_or_names_nothing_answers_no_unmatched_key() {
    let (_tree, kartta) = stores(&["a", "b"]);

    for (method, name) in [
        ("callers", "a.Store.Close"),
        ("callers", "Missing"),
        ("callees", "Store.Missing"),
        ("references", "b.Store.Close"),
        ("implementations", "a.Closer"),
        ("implementations", "Missing"),
    ] {
        let answer = ok(&kartta, method, &json!({ "name": name }));
        assert_eq!(keys(&answer), ["matches"], "{method} {name}: {answer}");
    }
    for (from, to) in [("a.Run", "b.Store.Close"), ("a.Run", "Missing")] {
        let answer = ok(&kartta, "call_path", &json!({ "from": from, "to": to }));
        assert_eq!(keys(&answer), ["matches"], "{from} {to}: {answer}");
    }
}

/// A built handle over a tree that defines `Index` in Rust and in Go and names it in a signature of each, Rust's
/// `open(index: &Index)` and Go's `Open(index Index)`. The tree directory is returned to keep it alive.
fn index_in_rust_and_go() -> (tempfile::TempDir, Kartta) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("Cargo.toml"),
        "[package]\nname = \"index\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");
    fs::create_dir(tree.path().join("src")).expect("src");
    fs::write(
        tree.path().join("src/lib.rs"),
        "pub struct Index;\n\npub fn open(index: &Index) {}\n",
    )
    .expect("lib.rs");
    fs::write(
        tree.path().join("index.go"),
        "package index\n\ntype Index struct{}\n\nfunc Open(index Index) {}\n",
    )
    .expect("index.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    (tree, kartta)
}

/// The part of a graph query's answer that `graph` answered.
fn part<'a>(answer: &'a Value, graph: &str) -> &'a Value {
    answer["matches"]
        .as_array()
        .and_then(|parts| parts.iter().find(|part| part["graph"] == graph))
        .unwrap_or_else(|| panic!("no {graph} part in {answer}"))
}

/// The keys of a references part with no candidates, a symbol's uses as a type recorded or no type asked.
const REFERENCES_KEYS: [&str; 8] = [
    "ambiguous",
    "below_confidence",
    "counts",
    "graph",
    "passed_over",
    "sites",
    "symbol",
    "total",
];

#[test]
fn a_rust_type_s_references_say_that_its_uses_as_a_type_go_unrecorded() {
    let (_tree, kartta) = index_in_rust_and_go();

    let answer = ok(&kartta, "references", &json!({ "name": "Index" }));

    let rust = part(&answer, "rust");
    assert_eq!(rust["symbol"]["kind"], "struct", "{rust}");
    assert_eq!(
        rust["total"], 0,
        "the signature names it unrecorded: {rust}"
    );
    assert_eq!(rust["type_uses_unrecorded"], true, "{rust}");
}

/// One file of each language that states a data kind unrecorded, reading and writing a symbol of each kind it states.
const UNRECORDED_DATA: [(&str, &str); 6] = [
    (
        "fields.rs",
        "pub struct Point { pub x: u32 }\n\
         pub fn shift(p: &mut Point) -> u32 {\n    p.x += 1;\n    p.x\n}\n",
    ),
    (
        "script.sh",
        "LOG=/tmp/log\nrun() {\n  echo \"$LOG\" >> \"${LOG}\"\n  LOG=other\n}\n",
    ),
    (
        "data.das",
        "options gen2\nlet {\n  LIMIT = 10\n}\nvar {\n  counter : int\n}\nstruct Pair {\n  first : int\n}\n\
         def bump(var p : Pair) : int {\n  counter += LIMIT\n  p.first = counter\n  return p.first\n}\n",
    ),
    (
        "fog.dshl",
        "shader s { int mode = 1; if (mode == 1) { dont_render; } }\n",
    ),
    (
        "feed.php",
        "<?php\nconst TOP = 1;\nclass Feed {\n    const MAX = 10;\n    private $count = 0;\n\
         public function bump() {\n        $this->count += self::MAX + TOP;\n        return Feed::MAX;\n    }\n}\n",
    ),
    (
        "feed.rb",
        "class Stream\n  CAP = 10\n  def limit\n    CAP + Stream::CAP\n  end\nend\n",
    ),
];

#[test]
fn every_data_kind_a_language_states_unrecorded_flags_its_references() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (file, source) in UNRECORDED_DATA {
        fs::write(tree.path().join(file), source).expect("a source file");
    }
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    for (graph, name, kind) in [
        ("rust", "x", "field"),
        ("bash", "LOG", "var"),
        ("daslang", "LIMIT", "const"),
        ("daslang", "counter", "var"),
        ("daslang", "first", "field"),
        ("dshl", "mode", "field"),
        ("php", "TOP", "const"),
        ("php", "MAX", "const"),
        ("php", "count", "field"),
        ("ruby", "CAP", "const"),
    ] {
        let answer = ok(
            &kartta,
            "references",
            &json!({ "name": name, "lang": graph }),
        );

        let found = part(&answer, graph);
        assert_eq!(found["symbol"]["kind"], kind, "{graph} {name}: {found}");
        assert_eq!(found["total"], 0, "{graph} {name}: {found}");
        assert_eq!(
            found["data_uses_unrecorded"], true,
            "{graph} {name}: {found}"
        );
    }
}

#[test]
fn a_rust_field_with_no_recorded_site_is_told_from_an_unused_constant() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("point.rs"),
        "pub const UNUSED: u32 = 0;\npub struct Point { pub x: u32 }\n\
         pub fn read(p: &Point) -> u32 {\n    p.x\n}\n",
    )
    .expect("point.rs");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let field = ok(&kartta, "references", &json!({ "name": "x" }));
    let constant = ok(&kartta, "references", &json!({ "name": "UNUSED" }));

    let field = part(&field, "rust");
    assert_eq!(field["total"], 0, "{field}");
    assert_eq!(field["data_uses_unrecorded"], true, "{field}");
    let constant = part(&constant, "rust");
    assert_eq!(constant["total"], 0, "{constant}");
    assert_eq!(keys(constant), REFERENCES_KEYS, "{constant}");
}

#[test]
fn a_type_of_a_language_that_states_a_data_kind_carries_only_the_type_flag() {
    let (_tree, kartta) = index_in_rust_and_go();

    let answer = ok(&kartta, "references", &json!({ "name": "Index" }));

    let rust = part(&answer, "rust");
    assert_eq!(rust["type_uses_unrecorded"], true, "{rust}");
    assert_eq!(rust.get("data_uses_unrecorded"), None, "{rust}");
}

#[test]
fn a_field_of_a_language_that_records_field_uses_carries_no_data_flag() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(
        tree.path().join("point.go"),
        "package point\n\ntype Point struct{ X int }\n\nfunc Read(p Point) int { return p.X }\n",
    )
    .expect("point.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let answer = ok(&kartta, "references", &json!({ "name": "X" }));

    let go = part(&answer, "go");
    assert_eq!(go["symbol"]["kind"], "field", "{go}");
    assert_eq!(go["total"], 1, "{go}");
    assert_eq!(keys(go), REFERENCES_KEYS, "{go}");
}

#[test]
fn a_go_type_s_references_and_a_rust_function_s_carry_no_new_key() {
    let (_tree, kartta) = index_in_rust_and_go();

    let index = ok(&kartta, "references", &json!({ "name": "Index" }));
    let open = ok(
        &kartta,
        "references",
        &json!({ "name": "open", "lang": "rust" }),
    );

    let go = part(&index, "go");
    assert_eq!(go["total"], 1, "Go records the signature's use: {go}");
    assert_eq!(keys(go), REFERENCES_KEYS, "{go}");
    let function = part(&open, "rust");
    assert_eq!(function["symbol"]["kind"], "func", "{function}");
    assert_eq!(keys(function), REFERENCES_KEYS, "{function}");
}

#[test]
fn a_language_the_build_did_not_hold_is_refused_with_the_ones_it_did() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &json!({ "languages": ["go", "python"] }));

    let answer = call(
        &kartta,
        "callers",
        &json!({ "name": "helper", "lang": "cpp" }),
    );

    assert_eq!(answer["err"]["code"], "invalid_params");
    let message = answer["err"]["message"].as_str().expect("a message");
    assert!(message.contains("go, python"), "{message}");
}

#[test]
fn a_cancel_naming_a_build_before_it_starts_stops_that_build_and_no_other() {
    let tree = tree();
    let kartta = open(tree.path());

    let ahead = ok(&kartta, "cancel", &json!({ "build": 7 }));
    let cancelled = call(&kartta, "build", &json!({ "id": 7 }));
    let next = call(&kartta, "build", &json!({ "id": 8 }));

    assert_eq!(ahead, json!({ "stopped": false }));
    assert_eq!(cancelled["err"]["code"], "build_cancelled");
    assert!(next.get("ok").is_some(), "{next}");
}

#[test]
fn a_cancel_naming_a_grep_before_it_starts_stops_that_grep_and_no_other() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let ahead = ok(&kartta, "cancel", &json!({ "grep": 3 }));
    let cancelled = ok(&kartta, "grep", &json!({ "patterns": ["func"], "id": 3 }));
    let next = ok(&kartta, "grep", &json!({ "patterns": ["func"], "id": 4 }));

    assert_eq!(ahead, json!({ "stopped": false }));
    assert_eq!(cancelled["cancelled"], true);
    assert_eq!(cancelled["files"], json!([]), "no file was read");
    assert_eq!(cancelled["unsearched"], 2);
    assert_eq!(next["cancelled"], false);
    assert_eq!(next["files"].as_array().map(Vec::len), Some(2));
}

#[test]
fn a_cancel_naming_a_build_leaves_a_grep_of_that_id_alone() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    ok(&kartta, "cancel", &json!({ "build": 5 }));
    let found = ok(&kartta, "grep", &json!({ "patterns": ["func"], "id": 5 }));

    assert_eq!(found["cancelled"], false);
}

#[test]
fn a_cancel_by_one_name_of_a_build_never_stops_a_build_by_the_other() {
    let tree = tree();
    let kartta = open(tree.path());

    ok(&kartta, "cancel", &json!({ "build": 7 }));
    ok(
        &kartta,
        "cancel",
        &json!({ "build": 8, "by": "binding_id" }),
    );
    let by_binding_id = call(&kartta, "build", &json!({ "binding_id": 7 }));
    let by_id = call(&kartta, "build", &json!({ "id": 8 }));
    let by_both = call(&kartta, "build", &json!({ "id": 7, "binding_id": 8 }));

    assert!(by_binding_id.get("ok").is_some(), "{by_binding_id}");
    assert!(by_id.get("ok").is_some(), "{by_id}");
    assert_eq!(by_both["err"]["code"], "build_cancelled");
}

#[test]
fn a_cancel_while_no_build_runs_leaves_the_next_build_alone() {
    let tree = tree();
    let kartta = open(tree.path());

    let stray = ok(&kartta, "cancel", &Value::Null);
    let built = call(&kartta, "build", &Value::Null);
    let found = ok(&kartta, "grep", &json!({ "patterns": ["func"] }));

    assert_eq!(stray, json!({ "stopped": false }));
    assert!(built.get("ok").is_some(), "{built}");
    assert_eq!(
        found["cancelled"], false,
        "a cancel naming no call stops only what is in flight"
    );
}

#[test]
fn a_build_cancelled_before_its_walk_reached_an_entry_leaves_nothing_behind() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    ok(&kartta, "cancel", &json!({ "build": 1 }));
    assert_eq!(
        code(&kartta, "build", &json!({ "id": 1 })),
        "build_cancelled"
    );

    assert_eq!(code(&kartta, "stats", &Value::Null), "not_built");
    assert_eq!(
        code(&kartta, "glob", &json!({ "globs": ["**"] })),
        "not_built"
    );
}

#[test]
fn a_build_that_fails_leaves_the_file_queries_not_built() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    let root = tree.path().to_owned();
    tree.close().expect("the tree goes");

    let failed = call(&kartta, "build", &Value::Null);

    assert_eq!(failed["err"]["code"], "build_failed", "{}", root.display());
    assert_eq!(
        code(&kartta, "glob", &json!({ "globs": ["**"] })),
        "not_built"
    );
}

#[test]
fn a_query_racing_a_build_to_its_end_never_answers_not_built() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    let rebuilt = AtomicBool::new(false);

    let unexpected: Vec<String> = thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..50 {
                ok(&kartta, "build", &Value::Null);
            }
            rebuilt.store(true, Ordering::Release);
        });
        let mut unexpected = Vec::new();
        while !rebuilt.load(Ordering::Acquire) {
            let answer = call(&kartta, "stats", &Value::Null);
            if let Some(code) = answer["err"]["code"].as_str()
                && code != "building"
            {
                unexpected.push(code.to_owned());
            }
        }
        unexpected
    });

    assert!(unexpected.is_empty(), "{unexpected:?}");
}

/// A handle over `tree`, built with stamps.
fn stamped(tree: &Path) -> Kartta {
    let kartta = open(tree);
    ok(&kartta, "build", &json!({ "stamps": true }));
    kartta
}

/// The paths of the index a glob of `globs` matches.
fn globbed(kartta: &Kartta, globs: &[&str]) -> Value {
    ok(kartta, "glob", &json!({ "globs": globs }))["paths"].clone()
}

#[test]
fn a_build_with_stamps_refreshes_one_named_file_alone() {
    let tree = tree();
    let kartta = stamped(tree.path());
    fs::write(
        tree.path().join("run.go"),
        "package main\n\nfunc run() {\n\thelper()\n}\n\nfunc helper() {}\n\nfunc extra() {}\n",
    )
    .expect("run.go");

    let refreshed = ok(&kartta, "refresh", &json!({ "paths": ["run.go"] }));

    assert_eq!(
        (
            &refreshed["walked"],
            &refreshed["tree"],
            &refreshed["extracted"]
        ),
        (&json!(1), &json!(false), &json!(1)),
        "main.go is not read again: {refreshed}"
    );
    assert_eq!(
        (
            &refreshed["added"],
            &refreshed["removed"],
            &refreshed["dropped"]
        ),
        (&json!(0), &json!(0), &json!(0)),
        "{refreshed}"
    );
    assert!(refreshed.get("widened").is_none(), "{refreshed}");
    let extra = ok(&kartta, "definitions", &json!({ "name": "extra" }));
    assert_eq!(
        extra["matches"][0]["found"][0]["definition"]["file"], "run.go",
        "{extra}"
    );
}

#[test]
fn a_refresh_without_stamps_answers_no_stamps_naming_the_fix() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    for params in [
        json!({}),
        json!({ "paths": ["run.go"] }),
        json!({ "paths": [] }),
    ] {
        let answer = call(&kartta, "refresh", &params);

        assert_eq!(answer["err"]["code"], "no_stamps", "{params}: {answer}");
        assert!(
            mentions(&answer["err"], r#"build {"stamps": true}"#),
            "{answer}"
        );
    }
}

#[test]
fn a_whole_refresh_counts_an_added_and_a_removed_file() {
    let tree = tree();
    let kartta = stamped(tree.path());
    fs::write(
        tree.path().join("new.go"),
        "package main\n\nfunc fresh() {}\n",
    )
    .expect("new.go");
    fs::remove_file(tree.path().join("main.go")).expect("main.go goes");

    let refreshed = ok(&kartta, "refresh", &json!({}));

    assert_eq!(
        (&refreshed["walked"], &refreshed["tree"]),
        (&json!(1), &json!(true)),
        "{refreshed}"
    );
    assert_eq!(
        (&refreshed["added"], &refreshed["removed"]),
        (&json!(1), &json!(1)),
        "{refreshed}"
    );
    assert_eq!(globbed(&kartta, &["*.go"]), json!(["new.go", "run.go"]));
}

#[test]
fn named_paths_past_the_limit_walk_the_tree_and_say_so() {
    let tree = tree();
    let kartta = stamped(tree.path());
    let named: Vec<String> = (0..51).map(|at| format!("new-{at:02}.go")).collect();
    for path in &named {
        fs::write(tree.path().join(path), "package main\n").expect("a new file");
    }

    let refreshed = ok(&kartta, "refresh", &json!({ "paths": named }));

    assert_eq!(
        (
            &refreshed["walked"],
            &refreshed["tree"],
            &refreshed["added"]
        ),
        (&json!(1), &json!(true), &json!(51)),
        "{refreshed}"
    );
    assert_eq!(
        refreshed["widened"],
        json!({ "named": 51, "parts": 51, "limit": 50 }),
        "the fewest parts a refresh walks apart is 50"
    );
}

#[test]
fn an_empty_list_of_paths_walks_nothing() {
    let tree = tree();
    let kartta = stamped(tree.path());
    fs::write(tree.path().join("new.go"), "package main\n").expect("new.go");

    let refreshed = ok(&kartta, "refresh", &json!({ "paths": [] }));

    assert_eq!(
        (
            &refreshed["walked"],
            &refreshed["tree"],
            &refreshed["added"]
        ),
        (&json!(0), &json!(false), &json!(0)),
        "{refreshed}"
    );
    assert_eq!(globbed(&kartta, &["new.go"]), json!([]));
}

#[test]
fn a_refresh_cancelled_ahead_answers_refresh_cancelled_and_leaves_the_index() {
    let tree = tree();
    let kartta = stamped(tree.path());
    fs::write(tree.path().join("new.go"), "package main\n").expect("new.go");

    let ahead = ok(&kartta, "cancel", &json!({ "refresh": 9 }));
    let cancelled = call(&kartta, "refresh", &json!({ "id": 9 }));

    assert_eq!(ahead, json!({ "stopped": false }));
    assert_eq!(cancelled["err"]["code"], "refresh_cancelled", "{cancelled}");
    assert_eq!(
        globbed(&kartta, &["new.go"]),
        json!([]),
        "the index is as it was"
    );
    let next = ok(&kartta, "refresh", &json!({ "id": 10 }));
    assert_eq!(next["added"], 1, "{next}");
}

#[test]
fn a_cancel_naming_a_refresh_leaves_a_build_and_a_grep_of_that_id_alone() {
    let tree = tree();
    let kartta = stamped(tree.path());

    ok(&kartta, "cancel", &json!({ "refresh": 4 }));
    let found = ok(&kartta, "grep", &json!({ "patterns": ["func"], "id": 4 }));
    let built = call(&kartta, "build", &json!({ "id": 4, "stamps": true }));

    assert_eq!(found["cancelled"], false);
    assert!(built.get("ok").is_some(), "{built}");
}

#[test]
fn refresh_before_a_build_answers_not_built() {
    let tree = tree();
    let kartta = open(tree.path());

    assert_eq!(code(&kartta, "refresh", &json!({})), "not_built");
}

#[test]
fn absolute_or_escaping_paths_answer_invalid_params() {
    let tree = tree();
    let kartta = stamped(tree.path());
    fs::write(tree.path().join("new.go"), "package main\n").expect("new.go");

    for path in ["/etc", "../outside.go", "src/../../outside.go"] {
        let answer = call(&kartta, "refresh", &json!({ "paths": ["new.go", path] }));

        assert_eq!(answer["err"]["code"], "invalid_params", "{path}: {answer}");
        assert!(mentions(&answer["err"], path), "{answer}");
    }
    assert_eq!(
        globbed(&kartta, &["new.go"]),
        json!([]),
        "nothing was walked"
    );
}

#[cfg(unix)]
#[test]
fn a_refresh_that_reads_a_failed_file_takes_it_out_of_stats_errors() {
    use std::os::unix::fs::PermissionsExt;

    let tree = tree();
    let locked = tree.path().join("run.go");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }
    let kartta = stamped(tree.path());
    let failed = ok(&kartta, "stats", &Value::Null)["errors"].clone();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("unlock");

    let refused = code(
        &kartta,
        "refresh",
        &json!({ "paths": [failed["listed"][0]["path"]] }),
    );
    let refreshed = ok(
        &kartta,
        "refresh",
        &json!({ "paths": [failed["listed"][0]["file"]] }),
    );

    assert_eq!(failed["total"], 1, "{failed}");
    assert_eq!(failed["listed"][0]["kind"], "read", "{failed}");
    assert_eq!(failed["listed"][0]["file"], "run.go", "{failed}");
    assert_eq!(refused, "invalid_params");
    assert!(
        failed["listed"][0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("run.go")),
        "{failed}"
    );
    assert!(failed["listed"][0]["message"].is_string(), "{failed}");
    assert_eq!(refreshed["errors"], json!({ "total": 0, "listed": [] }));
    assert_eq!(
        ok(&kartta, "stats", &Value::Null)["errors"],
        json!({ "total": 0, "listed": [] })
    );
}

#[cfg(unix)]
#[test]
fn a_content_index_counts_a_file_it_could_not_read_until_a_refresh_reads_it() {
    use std::os::unix::fs::PermissionsExt;

    let tree = tree();
    let locked = tree.path().join("run.go");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock");
    if fs::read(&locked).is_ok() {
        return; // running as a user permissions do not bind, such as root
    }
    let kartta = open(tree.path());
    let built = ok(
        &kartta,
        "build",
        &json!({ "stamps": true, "content_index": { "storage": "memory" } }),
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).expect("unlock");

    ok(&kartta, "refresh", &json!({ "paths": ["run.go"] }));

    assert_eq!(built["content_index"]["files"], 1);
    assert_eq!(built["content_index"]["unindexed"], 1);
    let refreshed = &ok(&kartta, "stats", &Value::Null)["content_index"];
    assert_eq!(refreshed["files"], 2);
    assert_eq!(
        refreshed.get("unindexed"),
        None,
        "absent at zero: {refreshed}"
    );
}

#[test]
fn a_content_index_that_cannot_be_kept_leaves_the_graphs_and_says_why_through_a_refresh() {
    let tree = tree();
    let missing = tree.path().join("no such directory");
    let kartta = open(tree.path());

    let built = ok(
        &kartta,
        "build",
        &json!({
            "languages": ["go"],
            "stamps": true,
            "content_index": { "storage": "file", "dir": missing },
        }),
    );

    assert_eq!(built["graphs"][0]["symbols"], 3);
    assert_eq!(built.get("content_index"), None, "{built}");
    let failure = built["content_index_failure"]
        .as_str()
        .unwrap_or_else(|| panic!("no reason: {built}"));
    assert!(failure.contains("no such directory"), "{failure}");
    assert_eq!(built["errors"]["total"], 1);
    assert_eq!(built["errors"]["listed"][0]["kind"], "content_index");
    assert_eq!(built["errors"]["listed"][0]["message"], failure);
    assert_eq!(ok(&kartta, "stats", &Value::Null), built);
    let found = ok(&kartta, "grep", &json!({ "patterns": ["helper"] }));
    assert_eq!(found["files"].as_array().map(Vec::len), Some(1), "{found}");

    let refreshed = ok(&kartta, "refresh", &Value::Null);
    let stats = ok(&kartta, "stats", &Value::Null);

    assert_eq!(refreshed["errors"]["total"], 0, "{refreshed}");
    assert_eq!(stats["errors"]["total"], 0, "{stats}");
    assert_eq!(stats["content_index_failure"], failure);
    assert_eq!(stats.get("content_index"), None, "{stats}");
}

#[test]
fn a_source_whose_file_left_the_tree_answers_why_in_its_place() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    fs::remove_file(tree.path().join("run.go")).expect("run.go is removed");

    let answer = ok(&kartta, "sources", &json!({ "name": "helper" }));

    let found = &answer["matches"][0]["found"][0];
    assert!(found["error"].is_string(), "{found}");
    assert!(found.get("source").is_none(), "{found}");
}

#[test]
fn a_grep_names_a_file_it_could_not_read() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    fs::remove_file(tree.path().join("run.go")).expect("run.go is removed");

    let answer = ok(&kartta, "grep", &json!({ "patterns": ["helper"] }));

    let failure = &answer["failures"][0];
    assert_eq!(failure["path"], "run.go", "{answer}");
    assert!(failure["message"].is_string(), "{answer}");
}

#[cfg(unix)]
#[test]
fn a_listing_names_where_a_symlink_points() {
    let tree = tree();
    std::os::unix::fs::symlink("run.go", tree.path().join("link.go")).expect("link.go");
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let listing = ok(&kartta, "list", &json!({ "path": "" }));

    let entries = listing["entries"].as_array().expect("the entries");
    let link = entries
        .iter()
        .find(|entry| entry["path"] == "link.go")
        .expect("the symlink");
    assert_eq!(link["kind"], "symlink", "{link}");
    assert_eq!(link["target"], "run.go", "{link}");
}

#[test]
fn a_second_build_reads_what_the_content_cache_kept() {
    let tree = tree();
    let cache = tempfile::tempdir().expect("a temporary directory");
    let kartta = open(tree.path());
    let params = json!({ "languages": ["go"], "content_cache": cache.path() });
    ok(&kartta, "build", &params);

    let built = ok(&kartta, "build", &params);

    assert_eq!(built["languages"][0]["cached"], 2, "{built}");
}

#[test]
fn a_build_that_names_no_ignore_applies_the_tree_s_gitignore() {
    let tree = tree();
    fs::write(tree.path().join(".gitignore"), "ignored.go\n").expect(".gitignore");
    fs::write(tree.path().join("ignored.go"), "package main\n").expect("ignored.go");
    let kartta = open(tree.path());
    let indexed = |params: &Value| {
        ok(&kartta, "build", params);
        globbed(&kartta, &["ignored.go"])
    };

    assert_eq!(indexed(&Value::Null), json!([]));
    assert_eq!(indexed(&json!({ "ignore": "in_tree" })), json!([]));
    assert_eq!(indexed(&json!({ "ignore": "none" })), json!(["ignored.go"]));
}

#[test]
fn a_build_answers_the_threads_it_named_and_the_ambient_pool_s_when_it_named_none() {
    let tree = tree();
    let kartta = open(tree.path());

    let named = ok(&kartta, "build", &json!({ "threads": 3 }));
    let ambient = ok(&kartta, "build", &Value::Null);

    assert_eq!(named["threads"], 3);
    assert!(
        ambient["threads"]
            .as_u64()
            .is_some_and(|threads| threads >= 1),
        "{ambient}"
    );
}

/// The file `definitions` answers for `name`, once it answers one, or `Null` once 20 s pass.
fn defined_soon(kartta: &Kartta, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found = ok(kartta, "definitions", &json!({ "name": name }));
        let file = &found["matches"][0]["found"][0]["definition"]["file"];
        if !file.is_null() || Instant::now() > deadline {
            return file.clone();
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_build_that_watches_takes_an_edit_in_without_a_refresh() {
    let tree = tree();
    let kartta = open(tree.path());

    let built = ok(&kartta, "build", &json!({ "watch": true }));
    fs::write(
        tree.path().join("run.go"),
        "package main\n\nfunc run() {\n\thelper()\n}\n\nfunc helper() {}\n\nfunc extra() {}\n",
    )
    .expect("run.go");

    // constraint: FSEvents can report the tree's own making, a moment before the build, after the watch began; that
    // names the root, which the watch takes in by a restart, so the restarts it counts vary
    assert_eq!(built["watch"]["following"], true, "{built}");
    assert_eq!(defined_soon(&kartta, "extra"), "run.go");
    let stats = ok(&kartta, "stats", &Value::Null);
    assert_eq!(stats["watch"]["following"], true, "{stats}");
}

#[test]
fn a_build_that_watches_keeps_stamps_and_refuses_to_build_without_them() {
    let tree = tree();
    let kartta = open(tree.path());

    let refused = call(&kartta, "build", &json!({ "watch": true, "stamps": false }));
    ok(&kartta, "build", &json!({ "watch": true }));
    let refreshed = ok(&kartta, "refresh", &json!({ "paths": [] }));

    assert_eq!(refused["err"]["code"], "invalid_params", "{refused}");
    assert!(mentions(&refused["err"], "stamps"), "{refused}");
    assert_eq!(refreshed["watch"]["following"], true, "{refreshed}");
}

#[test]
fn a_build_that_does_not_watch_follows_nothing_and_gives_no_reason() {
    let tree = tree();
    let kartta = stamped(tree.path());

    let refreshed = ok(&kartta, "refresh", &json!({ "paths": [] }));

    assert_eq!(
        ok(&kartta, "stats", &Value::Null)["watch"],
        json!({ "following": false })
    );
    assert_eq!(refreshed["watch"], json!({ "following": false }));
}
