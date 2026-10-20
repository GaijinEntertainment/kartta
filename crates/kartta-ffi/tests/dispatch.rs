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
    assert!(
        walk.get("file_scope").is_none(),
        "a walk no file's top level calls into carries no file_scope"
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
