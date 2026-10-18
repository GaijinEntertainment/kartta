//! Which languages the set and a selection hold, in which order, and what a selection of an unknown name answers.

use kartta_languages::{NAMES, UnknownLanguages, all, select};

fn names(languages: &[Box<dyn kartta_lang::Language>]) -> Vec<&str> {
    languages.iter().map(|language| language.name()).collect()
}

#[test]
fn the_set_holds_every_language_under_its_listed_name_in_the_listed_order() {
    assert_eq!(names(&all()), NAMES);
    assert_eq!(
        NAMES,
        [
            "go",
            "cpp",
            "typescript",
            "tsx",
            "javascript",
            "daslang",
            "quirrel",
            "blk",
            "rust",
            "python",
            "bash",
            "php",
            "ruby",
            "lua",
            "puppet",
            "dshl",
        ]
    );
}

#[test]
fn each_file_of_the_set_goes_to_the_one_language_that_lists_its_extension() {
    let by_extension = [
        ("go", ["go"].as_slice()),
        (
            "cpp",
            &[
                "c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx", "inl", "inc",
            ],
        ),
        ("typescript", &["ts", "mts", "cts"]),
        ("tsx", &["tsx"]),
        ("javascript", &["js", "jsx", "mjs", "cjs"]),
        ("daslang", &["das"]),
        ("quirrel", &["nut"]),
        ("blk", &["blk"]),
        ("rust", &["rs"]),
        ("python", &["py", "pyi"]),
        ("bash", &["sh", "bash"]),
        ("php", &["php", "phtml"]),
        ("ruby", &["rb"]),
        ("lua", &["lua"]),
        ("puppet", &["pp"]),
        ("dshl", &["dshl"]),
    ];
    let languages = all();

    for (owner, extensions) in by_extension {
        for extension in extensions {
            for path in [
                format!("src/a.{extension}"),
                format!("SRC/A.{}", extension.to_uppercase()),
            ] {
                let claimants: Vec<&str> = (languages.iter())
                    .filter(|language| language.claims(&path))
                    .map(|language| language.name())
                    .collect();
                assert_eq!(claimants, [owner], "{path}");
            }
        }
    }
}

#[test]
fn a_selection_follows_the_listed_order_and_holds_each_language_once() {
    let selected = select(["rust", "go", "tsx", "rust"]).expect("every name is known");

    assert_eq!(names(&selected), ["go", "tsx", "rust"]);
}

#[test]
fn a_selection_takes_owned_names() {
    let given: Vec<String> = vec!["lua".to_owned(), "dshl".to_owned()];

    assert_eq!(
        names(&select(&given).expect("every name is known")),
        ["lua", "dshl"]
    );
}

#[test]
fn selecting_no_name_selects_no_language() {
    let selected = select::<[&str; 0]>([]).expect("no name is unknown");

    assert_eq!(names(&selected), [""; 0]);
}

#[test]
fn selecting_every_name_selects_the_set() {
    assert_eq!(names(&select(NAMES).expect("every name is known")), NAMES);
}

#[test]
fn an_unknown_name_fails_the_selection_and_the_error_lists_the_known_names() {
    let error = select(["go", "golang", "Go", "golang"]).expect_err("two names are unknown");

    assert_eq!(
        error.to_string(),
        "unknown languages \"golang\", \"Go\"; the known languages are go, cpp, typescript, tsx, javascript, \
         daslang, quirrel, blk, rust, python, bash, php, ruby, lua, puppet, dshl"
    );
}

#[test]
fn one_unknown_name_is_named_alone() {
    let error: UnknownLanguages = select(["c++"]).expect_err("the name is unknown");

    let message = error.to_string();
    assert!(
        message.starts_with("unknown language \"c++\"; the known languages are go, cpp,"),
        "{message}"
    );
}
