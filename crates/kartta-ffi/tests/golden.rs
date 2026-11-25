//! The wire, pinned: every file under `tests/golden` is one method's request on the fixture tree under
//! `tests/fixture` and the answer it must get, which the Go and Node bindings' tests read too. Each method has a file
//! named after it, and a method whose params or answer hold keys only a second request reaches has a file for that
//! request too, named after the method and what it pins, as `references-unmatched.json`, or `list-not_found.json` for
//! a request the method refuses. The requests run on one handle, the build first and the rest in the order of their
//! file names, so the goldens after the refreshes pin that a refresh of the unchanged fixture changes no answer. Every
//! value must match exactly, except a `score`, which must agree with the pinned one within a relative 1e-12.
//! `KARTTA_FFI_BLESS=1` writes the answers the dispatch gives now in place of the pinned ones.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test), so the workspace no-panic posture is waived here, where a failed expectation is the \
              assertion"
)]

use std::fs;
use std::path::{Path, PathBuf};

use kartta_ffi::Kartta;
use serde_json::{Value, json};

// constraint: search's scores pass through `ln_1p`, which Apple's libm and glibc round apart in the last bits
const SCORE_TOLERANCE: f64 = 1e-12;

fn here(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn pin_agreeing_scores(answer: &mut Value, golden: &Value) {
    match (answer, golden) {
        (Value::Object(answer), Value::Object(golden)) => {
            for (key, value) in answer {
                let Some(pinned) = golden.get(key) else {
                    continue;
                };
                if key == "score" && scores_agree(value, pinned) {
                    value.clone_from(pinned);
                } else {
                    pin_agreeing_scores(value, pinned);
                }
            }
        }
        (Value::Array(answer), Value::Array(golden)) => {
            for (item, pinned) in answer.iter_mut().zip(golden) {
                pin_agreeing_scores(item, pinned);
            }
        }
        _ => {}
    }
}

fn scores_agree(answer: &Value, pinned: &Value) -> bool {
    let (Some(answer), Some(pinned)) = (answer.as_f64(), pinned.as_f64()) else {
        return false;
    };
    (answer - pinned).abs() <= SCORE_TOLERANCE * answer.abs().max(pinned.abs())
}

fn opened() -> Kartta {
    let fixture = here("tests/fixture");
    Kartta::open(json!({ "root": fixture }).to_string().as_bytes()).expect("the fixture opens")
}

/// The golden files in the order they run.
fn goldens() -> Vec<PathBuf> {
    let mut goldens: Vec<PathBuf> = fs::read_dir(here("tests/golden"))
        .expect("the golden directory")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    goldens.sort_by(|left, right| {
        let after_build = |path: &PathBuf| path.file_stem() != Some("build".as_ref());
        (after_build(left), left).cmp(&(after_build(right), right))
    });
    goldens
}

/// The golden at `path`, and what `kartta` answers its request now.
fn replayed(kartta: &Kartta, path: &Path) -> (Value, Value) {
    let golden: Value = serde_json::from_slice(&fs::read(path).expect("a golden")).expect("JSON");
    let method = golden["method"].as_str().expect("a method");
    let params = golden["params"].to_string();
    let answer = serde_json::from_slice(&kartta.call(method, params.as_bytes())).expect("JSON");
    (golden, answer)
}

#[test]
fn every_method_answers_its_golden_request_as_pinned() {
    let kartta = opened();
    let bless = std::env::var_os("KARTTA_FFI_BLESS").is_some();

    let mut answered = Vec::new();
    for path in &goldens() {
        let (mut golden, mut answer) = replayed(&kartta, path);
        let method = golden["method"].as_str().expect("a method").to_owned();
        answered.push(method.clone());
        if bless {
            golden["answer"] = answer;
            let text = serde_json::to_string_pretty(&golden).expect("JSON") + "\n";
            fs::write(path, text).expect("the golden is written");
        } else {
            pin_agreeing_scores(&mut answer, &golden["answer"]);
            assert_eq!(answer, golden["answer"], "{method}");
        }
    }

    let version: Value = serde_json::from_slice(&kartta.call("version", b"")).expect("JSON");
    let methods: Vec<String> =
        serde_json::from_value(version["ok"]["methods"].clone()).expect("names");
    answered.sort();
    answered.dedup();
    assert_eq!(answered, methods, "a golden for every method");
}

const PINNED_SCORE: f64 = 3.579_907_375_813_987_4;
const PINNED_WEIGHT: f64 = 0.702_477_415_651_192_4;

fn agrees_with_golden(score: f64, weight: f64) -> bool {
    let golden = json!({ "hits": [{ "score": PINNED_SCORE, "weight": PINNED_WEIGHT }] });
    let mut answer = json!({ "hits": [{ "score": score, "weight": weight }] });
    pin_agreeing_scores(&mut answer, &golden);
    answer == golden
}

#[test]
fn a_score_that_differs_in_its_last_digit_agrees_with_the_pinned_one() {
    assert!(agrees_with_golden(3.579_907_375_813_988, PINNED_WEIGHT));
}

#[test]
fn a_score_that_differs_in_a_real_digit_does_not_agree() {
    assert!(!agrees_with_golden(3.579_907_375_913_988, PINNED_WEIGHT));
}

#[test]
fn a_number_outside_a_score_must_match_exactly() {
    assert!(!agrees_with_golden(PINNED_SCORE, 0.702_477_415_651_192_5));
}
