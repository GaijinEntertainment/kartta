//! What the arguments of a DSHL macro call bind: the shader compiler refuses a call that passes any other count than
//! the macro's parameters, so a macro of another count is never the one called.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

const NO_PARAMETERS: &str = "macro PICK()\n  dont_render;\nendmacro\n";

const ONE_PARAMETER: &str = "macro PICK(a)\n  dont_render;\nendmacro\n";

const TWO_PARAMETERS: &str = "macro PICK(a, b)\n  dont_render;\nendmacro\n";

const USE_ONE_ARGUMENT: &str = "shader lit\n{\n  PICK(1)\n}\n";

const USE_NO_ARGUMENTS: &str = "shader lit\n{\n  PICK()\n}\n";

const USE: &str = r#"include "near.dshl"
include "far/wide.dshl"

shader lit
{
  PICK(1, 2)
}
"#;

fn tree(files: &[(&str, &str)]) -> Engine {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["dshl"]).expect("the language is known");

    Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds")
        .engine
}

/// The file and the confidence of each macro `shader` calls, and the calls left unresolved.
fn bound(tree: &Engine, path: &str, shader: &str) -> (Vec<(String, u8)>, usize) {
    let (_, graph) = tree.graph_of(path).expect("DSHL claims the file");
    let walk = graph
        .callees(shader, Limits::new(1))
        .expect("the shader is defined");
    let reached = (walk.reached.iter())
        .map(|reached| (reached.symbol.file.to_owned(), reached.confidence))
        .collect();
    (reached, walk.unresolved)
}

fn callees(tree: &Engine, path: &str, shader: &str) -> Vec<String> {
    let (reached, _) = bound(tree, path, shader);
    reached.into_iter().map(|(file, _)| file).collect()
}

#[test]
fn a_call_binds_the_included_macro_that_takes_its_arguments() {
    let tree = tree(&[
        ("near.dshl", NO_PARAMETERS),
        ("far/wide.dshl", TWO_PARAMETERS),
        ("use.dshl", USE),
    ]);

    assert_eq!(callees(&tree, "use.dshl", "lit"), ["far/wide.dshl"]);
}

#[test]
fn a_call_binds_past_its_own_file_s_fallback_of_another_count() {
    let tree = tree(&[
        ("wide.dshl", ONE_PARAMETER),
        (
            "use.dshl",
            "include \"wide.dshl\"\n\ndefine_macro_if_not_defined PICK()\nendmacro\n\nshader lit\n{\n  PICK(1)\n}\n",
        ),
    ]);

    assert_eq!(callees(&tree, "use.dshl", "lit"), ["wide.dshl"]);
}

#[test]
fn a_call_that_no_macro_of_its_name_takes_binds_nothing() {
    let tree = tree(&[("near.dshl", NO_PARAMETERS), ("use.dshl", USE_ONE_ARGUMENT)]);

    assert_eq!(bound(&tree, "use.dshl", "lit"), (vec![], 1));
}

#[test]
fn a_call_of_no_arguments_binds_the_macro_of_no_parameters() {
    let tree = tree(&[
        ("a/narrow.dshl", NO_PARAMETERS),
        ("b/wide.dshl", ONE_PARAMETER),
        ("c/use.dshl", USE_NO_ARGUMENTS),
    ]);

    assert_eq!(
        bound(&tree, "c/use.dshl", "lit"),
        (vec![("a/narrow.dshl".to_owned(), 60)], 0)
    );
}

#[test]
fn a_call_of_no_arguments_binds_no_macro_of_parameters() {
    let tree = tree(&[("wide.dshl", ONE_PARAMETER), ("use.dshl", USE_NO_ARGUMENTS)]);

    assert_eq!(bound(&tree, "use.dshl", "lit"), (vec![], 1));
}

#[test]
fn the_one_macro_that_fits_binds_as_its_name_s_one_callable() {
    let tree = tree(&[
        ("a/narrow.dshl", NO_PARAMETERS),
        ("b/wide.dshl", ONE_PARAMETER),
        ("c/use.dshl", USE_ONE_ARGUMENT),
    ]);

    assert_eq!(
        bound(&tree, "c/use.dshl", "lit"),
        (vec![("b/wide.dshl".to_owned(), 60)], 0)
    );
}

#[test]
fn the_one_macro_that_fits_a_name_many_files_define_binds_as_its_one_callable() {
    let copies: Vec<String> = (0..64).map(|copy| format!("a/{copy}.dshl")).collect();
    let mut files: Vec<(&str, &str)> = (copies.iter())
        .map(|path| (path.as_str(), NO_PARAMETERS))
        .collect();
    files.extend([
        ("b/wide.dshl", ONE_PARAMETER),
        ("c/use.dshl", USE_ONE_ARGUMENT),
    ]);
    let tree = tree(&files);

    assert_eq!(
        bound(&tree, "c/use.dshl", "lit"),
        (vec![("b/wide.dshl".to_owned(), 60)], 0)
    );
}

#[test]
fn a_macro_that_takes_fewer_elsewhere_leaves_the_nearest_fit_at_its_nearness() {
    let tree = tree(&[
        ("a/narrow.dshl", NO_PARAMETERS),
        ("b/global.dshl", ONE_PARAMETER),
        ("c/global.dshl", ONE_PARAMETER),
        ("c/use.dshl", USE_ONE_ARGUMENT),
    ]);

    assert_eq!(
        bound(&tree, "c/use.dshl", "lit"),
        (vec![("c/global.dshl".to_owned(), 50)], 0)
    );
}

#[test]
fn a_macro_as_near_that_takes_more_leaves_the_nearest_fit_bound() {
    let tree = tree(&[
        ("a/global.dshl", ONE_PARAMETER),
        ("b/global.dshl", ONE_PARAMETER),
        ("b/wider.dshl", TWO_PARAMETERS),
        ("b/use.dshl", USE_ONE_ARGUMENT),
    ]);

    assert_eq!(
        bound(&tree, "b/use.dshl", "lit"),
        (vec![("b/global.dshl".to_owned(), 50)], 0)
    );
}

#[test]
fn a_nearer_copy_of_another_count_leaves_the_macro_past_the_first_include_bound() {
    let tree = tree(&[
        ("games/a/shaders/own.dshl", NO_PARAMETERS),
        (
            "games/a/shaders/top.dshl",
            "include \"shared.dshl\"\n\nshader lit\n{\n  PICK(1, 2)\n}\n",
        ),
        ("libs/shaders/shared.dshl", "include \"deep.dshl\"\n"),
        ("libs/shaders/deep.dshl", TWO_PARAMETERS),
    ]);

    assert_eq!(
        callees(&tree, "games/a/shaders/top.dshl", "lit"),
        ["libs/shaders/deep.dshl"]
    );
}
