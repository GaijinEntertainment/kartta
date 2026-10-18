//! The wire, pinned: every file under `tests/golden` is one method's request on the fixture tree under
//! `tests/fixture` and the answer it must get, which the Go binding's tests read too. `KARTTA_FFI_BLESS=1` writes the
//! answers the dispatch gives now in place of the pinned ones.

use std::fs;
use std::path::{Path, PathBuf};

use kartta_ffi::Kartta;
use serde_json::{Value, json};

fn here(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

#[test]
fn every_method_answers_its_golden_request_as_pinned() {
    let kartta = Kartta::open(
        json!({ "root": here("tests/fixture") })
            .to_string()
            .as_bytes(),
    )
    .expect("the fixture opens");
    let bless = std::env::var_os("KARTTA_FFI_BLESS").is_some();
    let mut goldens: Vec<PathBuf> = fs::read_dir(here("tests/golden"))
        .expect("the golden directory")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    goldens.sort_by_key(|path| path.file_stem() != Some("build".as_ref()));

    let mut answered = Vec::new();
    for path in &goldens {
        let mut golden: Value =
            serde_json::from_slice(&fs::read(path).expect("a golden")).expect("JSON");
        let method = golden["method"].as_str().expect("a method").to_owned();
        let params = golden["params"].to_string();
        let answer: Value =
            serde_json::from_slice(&kartta.call(&method, params.as_bytes())).expect("JSON");
        answered.push(method.clone());
        if bless {
            golden["answer"] = answer;
            let text = serde_json::to_string_pretty(&golden).expect("JSON") + "\n";
            fs::write(path, text).expect("the golden is written");
        } else {
            assert_eq!(answer, golden["answer"], "{method}");
        }
    }

    let version: Value = serde_json::from_slice(&kartta.call("version", b"")).expect("JSON");
    let methods: Vec<String> =
        serde_json::from_value(version["ok"]["methods"].clone()).expect("names");
    answered.sort();
    assert_eq!(answered, methods, "one golden per method");
}
