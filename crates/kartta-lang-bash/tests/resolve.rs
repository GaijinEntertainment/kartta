//! How a Bash extraction resolves against its file's path and the repository: the files its `source` commands name,
//! and the test code a path marks.

use std::io;

use kartta_lang::path::{base, dir};
use kartta_lang::{Extraction, Language, Repository};
use kartta_lang_bash::Bash;

struct Files(&'static [&'static str]);

impl Repository for Files {
    fn contains(&self, path: &str) -> bool {
        self.0.contains(&path)
    }

    fn files_in_dir<'a>(&'a self, in_dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let in_dir = in_dir.to_owned();
        Box::new(
            self.0
                .iter()
                .filter(move |file| dir(file) == in_dir)
                .map(|file| (*file).to_owned()),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        Box::new(
            self.0
                .iter()
                .filter(move |file| base(file) == name)
                .map(|file| (*file).to_owned()),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::NotFound, path.to_owned()))
    }
}

fn resolve(files: &Files, source: &[u8], path: &str) -> Extraction {
    let bash = Bash::new();
    let resolver = bash.resolver(files);

    resolver.resolve(bash.extract(source), path)
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

#[test]
fn a_relative_source_resolves_from_the_sourcing_script_with_or_without_its_extension() {
    let files = Files(&["scripts/lib/helpers.sh", "shared/env.sh"]);

    let extraction = resolve(
        &files,
        b"anchor() { :; }\nsource ./lib/helpers.sh\n. ../shared/env\n",
        "scripts/deploy.sh",
    );

    assert_eq!(
        includes(&extraction),
        ["scripts/lib/helpers.sh", "shared/env.sh"]
    );
}

#[test]
fn a_root_relative_source_resolves_as_written_then_by_a_segment_suffix() {
    let files = Files(&["lib/util.sh", "vendor/common/colors.bash"]);

    let extraction = resolve(
        &files,
        b"anchor() { :; }\nsource lib/util.sh\nsource common/colors\n",
        "bin/run.sh",
    );

    assert_eq!(
        includes(&extraction),
        ["lib/util.sh", "vendor/common/colors.bash"]
    );
}

#[test]
fn a_salvaged_tail_resolves_like_a_written_path() {
    let files = Files(&["lib/x.sh", "lib/y.sh", "lib/raw.sh"]);

    let extraction = resolve(
        &files,
        b"anchor() { :; }\nsource \"$DIR/lib/x.sh\"\nsource $ROOT/lib/y.sh\nsource 'lib/raw.sh'\n",
        "top.sh",
    );

    assert_eq!(
        includes(&extraction),
        ["lib/x.sh", "lib/y.sh", "lib/raw.sh"]
    );
}

#[test]
fn a_source_before_any_definition_still_resolves() {
    let files = Files(&["lib/util.sh", "run.sh"]);

    let extraction = resolve(&files, b"source lib/util.sh\n", "run.sh");

    assert_eq!(includes(&extraction), ["lib/util.sh"]);
}

#[test]
fn a_path_outside_the_repository_or_an_ambiguous_suffix_adds_no_include() {
    let files = Files(&["a/util.sh", "b/util.sh", "lib/x.sh"]);

    let extraction = resolve(
        &files,
        b"anchor() { :; }\nsource /etc/profile.d/global.sh\nsource util.sh\nsource ../../above.sh\n",
        "top.sh",
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_suffix_ambiguous_under_one_extension_stays_ambiguous_under_another() {
    let files = Files(&["a/lib.sh", "b/lib.sh", "c/lib.bash", "d/only.bash"]);

    let extraction = resolve(
        &files,
        b"anchor() { :; }\nsource lib\nsource only\n",
        "top.sh",
    );

    assert_eq!(includes(&extraction), ["d/only.bash"]);
}

#[test]
fn a_segment_suffix_never_matches_inside_a_path_segment() {
    let files = Files(&["pkg/xlib/x.sh"]);

    let extraction = resolve(&files, b"anchor() { :; }\nsource lib/x.sh\n", "top.sh");

    assert_eq!(includes(&extraction), Vec::<&str>::new());
}

#[test]
fn a_test_path_marks_every_symbol_as_test_code() {
    let source = b"run() {\n    :\n}\n";
    let files = Files(&[]);

    for path in [
        "foo_test.sh",
        "foo_test.bash",
        "test_foo.sh",
        "suite/tests/util.sh",
        "test/run.sh",
    ] {
        let extraction = resolve(&files, source, path);

        assert_eq!(extraction.symbols.len(), 1, "{path}");
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_test()),
            "{path}"
        );
    }
    let ordinary = resolve(&files, source, "bin/foo.sh");
    assert!(
        ordinary
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_test())
    );
}

#[test]
fn a_test_path_keeps_the_flags_the_bytes_gave() {
    let files = Files(&[]);

    let extraction = resolve(&files, b"# @generated\nrun() {\n    :\n}\n", "tests/gen.sh");

    assert!(extraction.symbols[0].flags.is_test());
    assert!(extraction.symbols[0].flags.is_generated());
}

#[test]
fn one_extraction_resolved_at_two_paths_differs_only_by_what_the_paths_show() {
    let bash = Bash::new();
    let files = Files(&["lib/util.sh"]);
    let resolver = bash.resolver(&files);
    let raw = bash.extract(b"run() {\n    :\n}\nsource lib/util.sh\n");

    let mut ordinary = resolver.resolve(raw.clone(), "bin/run.sh");
    let test = resolver.resolve(raw, "tests/run.sh");

    assert_eq!(ordinary.file, "bin/run.sh");
    assert_eq!(test.file, "tests/run.sh");
    assert!(!ordinary.symbols[0].flags.is_test());
    assert!(test.symbols[0].flags.is_test());
    ordinary.file.clone_from(&test.file);
    ordinary.symbols[0].flags = test.symbols[0].flags;
    assert_eq!(ordinary, test);
}
