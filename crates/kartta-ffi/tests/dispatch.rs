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

use kartta_ffi::{ErrorCode, Kartta};
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

#[test]
fn open_refuses_a_root_that_is_not_a_directory_with_a_structured_error() {
    let tree = tree();
    let file = tree.path().join("main.go");

    let not_a_directory =
        Kartta::open(json!({ "root": file }).to_string().as_bytes()).expect_err("a file");
    let missing = Kartta::open(
        json!({ "root": tree.path().join("gone") })
            .to_string()
            .as_bytes(),
    )
    .expect_err("none");
    let unknown_field =
        Kartta::open(br#"{"root": "/", "gitignore": true}"#).expect_err("unknown field");
    let not_json = Kartta::open(b"{").expect_err("not JSON");

    assert_eq!(not_a_directory.code(), ErrorCode::InvalidRoot);
    assert!(
        not_a_directory.message().contains("not a directory"),
        "{not_a_directory}"
    );
    assert_eq!(missing.code(), ErrorCode::InvalidRoot);
    assert_eq!(unknown_field.code(), ErrorCode::InvalidParams);
    assert!(
        unknown_field.message().contains("gitignore"),
        "{unknown_field}"
    );
    assert_eq!(not_json.code(), ErrorCode::InvalidRequest);
    let wire: Value = serde_json::from_slice(&missing.to_json()).expect("JSON");
    assert_eq!(wire["code"], "invalid_root");
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
fn version_lists_the_abi_the_methods_and_the_languages() {
    let tree = tree();
    let kartta = open(tree.path());

    let version = ok(&kartta, "version", &Value::Null);

    assert_eq!(version["abi"], kartta_ffi::ABI_VERSION);
    assert_eq!(
        version["methods"]
            .as_array()
            .map(|methods| methods[..3].to_vec()),
        Some(vec![
            json!("analyze_change"),
            json!("analyze_segment"),
            json!("build")
        ])
    );
    assert_eq!(version["languages"][0], "go");
}

#[test]
fn every_listed_method_is_answered_by_the_dispatch() {
    let tree = tree();
    let kartta = open(tree.path());
    let version = ok(&kartta, "version", &Value::Null);

    for method in version["methods"].as_array().expect("a list") {
        let method = method.as_str().expect("a name");
        let answer = call(&kartta, method, &json!({ "name": "helper" }));
        assert_ne!(answer["err"]["code"], "unknown_method", "{method}");
    }
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
fn a_fuzzy_limit_of_0_takes_the_default() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    let found = ok(&kartta, "fuzzy", &json!({ "query": "go", "limit": 0 }));

    assert_eq!(
        found["matches"].as_array().map(Vec::len),
        Some(2),
        "{found}"
    );
    assert_eq!(found["truncated"], false);
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
fn a_bound_of_0_takes_the_method_s_default() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);
    let zeros = json!({ "name": "helper", "depth": 0, "max_symbols": 0 });

    let callers = ok(&kartta, "callers", &zeros);
    let definitions = ok(
        &kartta,
        "definitions",
        &json!({ "name": "helper", "limit": 0 }),
    );
    let search = ok(&kartta, "search", &json!({ "query": "helper", "limit": 0 }));
    let segment = ok(
        &kartta,
        "analyze_segment",
        &json!({ "file": "run.go", "start_line": 0 }),
    );

    assert_eq!(
        callers["matches"][0]["reached"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(
        definitions["matches"][0]["found"].as_array().map(Vec::len),
        Some(1)
    );
    assert!(
        search["matches"][0]["hits"]
            .as_array()
            .is_some_and(|hits| !hits.is_empty())
    );
    assert_eq!(segment["start_line"], 1);
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
fn a_cancel_while_no_build_runs_leaves_the_next_build_alone() {
    let tree = tree();
    let kartta = open(tree.path());

    let stray = ok(&kartta, "cancel", &Value::Null);
    let built = call(&kartta, "build", &Value::Null);

    assert_eq!(stray, json!({ "stopped": false }));
    assert!(built.get("ok").is_some(), "{built}");
}

#[test]
fn a_cancelled_build_leaves_no_index_behind() {
    let tree = tree();
    let kartta = open(tree.path());
    ok(&kartta, "build", &Value::Null);

    ok(&kartta, "cancel", &json!({ "build": 1 }));
    assert_eq!(
        code(&kartta, "build", &json!({ "id": 1 })),
        "build_cancelled"
    );

    assert_eq!(code(&kartta, "stats", &Value::Null), "not_built");
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
