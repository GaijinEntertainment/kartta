//! How a daslang extraction resolves against its file's path and the repository: the file a `require` or an `include`
//! names, and the flags a path gives a file's symbols.

use kartta_lang::{Extraction, Language, MemoryRepository, SymbolFlags};
use kartta_lang_daslang::Daslang;

const NOTHING: [&str; 0] = [];

fn resolve(files: &[&str], source: &str, path: &str) -> Extraction {
    let daslang = Daslang::new();
    let repository = MemoryRepository::of_paths(files);
    let resolver = daslang.resolver(&repository);

    resolver.resolve(daslang.extract(source.as_bytes()), path, &repository)
}

fn targets(files: &[&str], spec: &str, path: &str) -> Vec<String> {
    resolve(files, &format!("options gen2\nrequire {spec}\n"), path)
        .includes
        .into_iter()
        .map(|include| include.target)
        .collect()
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

fn modules(extraction: &Extraction) -> Vec<&str> {
    extraction
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect()
}

fn flags(path: &str) -> SymbolFlags {
    let extraction = resolve(&[], "options gen2\ndef helper() {}\n", path);

    extraction.symbols[0].flags
}

#[test]
fn a_directive_that_names_no_file_is_an_import_kept_as_written() {
    let extraction = resolve(
        &["game/main.das"],
        "options gen2\nrequire daslib/strings\nrequire math/vec as vmath\ninclude \"foo.das\"\n",
        "game/main.das",
    );

    assert_eq!(
        modules(&extraction),
        ["daslib/strings", "math/vec", "\"foo.das\""]
    );
    let lines: Vec<(u32, usize)> = extraction
        .imports
        .iter()
        .map(|import| (import.line, import.names.len()))
        .collect();
    assert_eq!(lines, [(2, 0), (3, 0), (4, 0)]);
    assert_eq!(includes(&extraction), NOTHING);
}

#[test]
fn a_directive_that_names_a_file_is_an_include_and_no_import() {
    let extraction = resolve(
        &["game/ai/pathing.das", "game/main.das"],
        "options gen2\nrequire pathing\nrequire math\n",
        "game/main.das",
    );

    assert_eq!(includes(&extraction), ["game/ai/pathing.das"]);
    assert_eq!(modules(&extraction), ["math"]);
}

#[test]
fn a_bare_require_names_the_one_file_named_for_the_module() {
    let files = ["game/ai/pathing.das", "game/main.das"];

    assert_eq!(
        targets(&files, "pathing", "game/main.das"),
        ["game/ai/pathing.das"]
    );
}

#[test]
fn a_bare_require_names_the_file_beside_its_own_of_those_named_for_the_module() {
    let files = ["tools/util.das", "game/util.das", "game/main.das"];

    assert_eq!(targets(&files, "util", "game/main.das"), ["game/util.das"]);
    assert_eq!(
        targets(&files, "util.das", "game/main.das"),
        ["game/util.das"]
    );
}

#[test]
fn a_bare_include_names_the_file_beside_its_own_of_those_of_its_name() {
    let files = ["tools/util.das", "game/util.das", "game/main.das"];

    let extraction = resolve(&files, "options gen2\ninclude util.das\n", "game/main.das");

    assert_eq!(includes(&extraction), ["game/util.das"]);
}

#[test]
fn a_bare_require_of_two_candidates_names_the_one_under_the_nearest_ancestor() {
    let files = [
        "game/server/profile.das",
        "game/server/actions/buy.das",
        "tools/profile.das",
    ];

    assert_eq!(
        targets(&files, "profile", "game/server/actions/buy.das"),
        ["game/server/profile.das"]
    );
}

#[test]
fn a_bare_include_of_two_candidates_names_the_one_under_the_nearest_ancestor() {
    let files = ["game/common.das", "game/server/buy.das", "tools/common.das"];

    let extraction = resolve(
        &files,
        "options gen2\ninclude common.das\n",
        "game/server/buy.das",
    );

    assert_eq!(includes(&extraction), ["game/common.das"]);
}

#[test]
fn a_file_without_the_extension_answers_no_module() {
    let files = [
        "game/profile",
        "game/server/a.das",
        "tools/profile.das",
        "other/profile.das",
    ];

    assert_eq!(targets(&files, "profile", "game/server/a.das"), NOTHING);
    assert_eq!(
        targets(&files, "game/profile", "game/server/a.das"),
        NOTHING
    );
}

#[test]
fn a_require_two_files_could_satisfy_names_neither() {
    let files = ["a/util.das", "b/util.das", "main.das"];

    let extraction = resolve(&files, "options gen2\nrequire util\n", "main.das");

    assert_eq!(includes(&extraction), NOTHING);
    assert_eq!(modules(&extraction), ["util"]);
}

#[test]
fn a_dotted_specifier_reads_its_dots_as_slashes_under_the_nearest_ancestor() {
    let files = [
        "prog/engine/core/mesh.das",
        "engine/core/mesh.das",
        "prog/game/a.das",
        "tools/game/a.das",
    ];

    assert_eq!(
        targets(&files, "engine.core.mesh", "prog/game/a.das"),
        ["prog/engine/core/mesh.das"]
    );
    assert_eq!(
        targets(&files, "engine/core/mesh", "prog/game/a.das"),
        ["prog/engine/core/mesh.das"]
    );
    assert_eq!(
        targets(&files, "engine/core/mesh", "tools/game/a.das"),
        ["engine/core/mesh.das"]
    );
}

#[test]
fn a_specifier_no_ancestor_holds_names_the_one_file_whose_path_ends_with_it() {
    let files = ["libs/daScript/daslib/strings_boost.das", "game/a.das"];

    assert_eq!(
        targets(&files, "daslib/strings_boost", "game/a.das"),
        ["libs/daScript/daslib/strings_boost.das"]
    );

    let ambiguous = [
        "libs/a/daslib/strings_boost.das",
        "libs/b/daslib/strings_boost.das",
        "game/a.das",
    ];
    assert_eq!(
        targets(&ambiguous, "daslib/strings_boost", "game/a.das"),
        NOTHING
    );
}

#[test]
fn a_specifier_of_two_candidates_names_the_one_that_ends_with_it_below_its_mount_point() {
    let files = [
        "modules/llvm_support/daslib/llvm_tune.das",
        "modules/llvm_support/tests/llvm_tune.das",
        "game/a.das",
    ];

    assert_eq!(
        targets(&files, "llvm/daslib/llvm_tune", "game/a.das"),
        ["modules/llvm_support/daslib/llvm_tune.das"]
    );
    assert_eq!(
        targets(&files, "llvm.daslib.llvm_tune", "game/a.das"),
        ["modules/llvm_support/daslib/llvm_tune.das"]
    );
}

#[test]
fn a_specifier_two_files_end_with_below_its_mount_point_names_neither() {
    let files = [
        "prog/libs/dm/dm_events.das",
        "skyquake/game/dm/dm_events.das",
        "game/a.das",
    ];

    let extraction = resolve(
        &files,
        "options gen2\nrequire danetlibs.dm.dm_events\n",
        "game/a.das",
    );

    assert_eq!(includes(&extraction), NOTHING);
    assert_eq!(modules(&extraction), ["danetlibs.dm.dm_events"]);
}

#[test]
fn a_path_that_matches_nothing_falls_back_to_its_module_name() {
    let files = ["engine/components/rigid_body.das", "game/a.das"];

    assert_eq!(
        targets(&files, "engine.core_components.rigid_body", "game/a.das"),
        ["engine/components/rigid_body.das"]
    );
}

#[test]
fn a_suffix_only_matches_at_a_directory_boundary() {
    let files = [
        "libs/mydaslib/strings.das",
        "libs/x/daslib/strings.das",
        "game/a.das",
    ];

    assert_eq!(
        targets(&files, "daslib/strings", "game/a.das"),
        ["libs/x/daslib/strings.das"]
    );
}

#[test]
fn a_relative_specifier_names_the_file_beside_the_importer() {
    let files = [
        "tools/common.das",
        "tools/build/gen.das",
        "tools/build/util.das",
    ];

    assert_eq!(
        targets(&files, "../common.das", "tools/build/gen.das"),
        ["tools/common.das"]
    );
    assert_eq!(
        targets(&files, "./util", "tools/build/gen.das"),
        ["tools/build/util.das"]
    );
    assert_eq!(
        targets(&files, "../../../common.das", "tools/build/gen.das"),
        NOTHING
    );
}

#[test]
fn a_relative_require_of_a_file_name_is_a_path_from_the_importer() {
    let files = ["tools/plugin.das_project", "tools/build/gen.das"];

    assert_eq!(
        targets(&files, "../plugin.das_project", "tools/build/gen.das"),
        ["tools/plugin.das_project"]
    );
}

#[test]
fn a_relative_require_without_the_extension_takes_it() {
    let gen_file = "tools/build/gen.das";

    assert_eq!(
        targets(
            &[gen_file, "tools/build/x", "tools/build/x.das"],
            "../x",
            gen_file
        ),
        ["tools/build/x.das"]
    );
    assert_eq!(
        targets(&[gen_file, "tools/build/x"], "../x", gen_file),
        NOTHING
    );
}

#[test]
fn a_relative_require_without_the_extension_reads_as_a_module_name_under_the_importer() {
    let gen_file = "tools/build/gen.das";

    assert_eq!(
        targets(&[gen_file, "tools/x.das"], "../x", gen_file),
        NOTHING
    );
    assert_eq!(targets(&[gen_file, "x.das"], "../../x", gen_file), NOTHING);
    assert_eq!(
        targets(
            &[gen_file, "tools/build/sub/x.das", "tools/sub/x.das"],
            "../sub/x",
            gen_file
        ),
        ["tools/build/sub/x.das"]
    );
    assert_eq!(
        targets(&[gen_file, "tools/build/a/b.das"], "./a.b", gen_file),
        ["tools/build/a/b.das"]
    );
    assert_eq!(targets(&["top.das", "x.das"], "../x", "top.das"), ["x.das"]);
}

#[test]
fn a_relative_include_names_the_file_as_written() {
    let source = "options gen2\ninclude ../x\n";
    let gen_file = "tools/build/gen.das";

    assert_eq!(
        includes(&resolve(
            &[gen_file, "tools/x", "tools/x.das"],
            source,
            gen_file
        )),
        ["tools/x"]
    );
    assert_eq!(
        includes(&resolve(&[gen_file, "tools/x.das"], source, gen_file)),
        NOTHING
    );
}

#[test]
fn an_include_written_with_backslashes_reads_them_as_slashes() {
    let files = ["tools/common.das", "tools/build/gen.das"];

    let extraction = resolve(
        &files,
        "options gen2\ninclude ..\\common.das\n",
        "tools/build/gen.das",
    );

    assert_eq!(includes(&extraction), ["tools/common.das"]);
}

#[test]
fn an_explicit_extension_is_not_doubled() {
    let files = ["tools/common.das", "tools/a.das"];

    assert_eq!(
        targets(&files, "common.das", "tools/a.das"),
        ["tools/common.das"]
    );
    assert_eq!(
        targets(&files, "tools/common.das", "tools/a.das"),
        ["tools/common.das"]
    );
}

#[test]
fn every_match_is_case_sensitive() {
    let files = ["game/ai/pathing.das", "game/main.das"];

    assert_eq!(targets(&files, "Pathing", "game/main.das"), NOTHING);
    assert_eq!(targets(&files, "ai/Pathing", "game/main.das"), NOTHING);
}

#[test]
fn a_module_no_file_provides_yields_no_include() {
    let extraction = resolve(
        &["game/main.das"],
        "options gen2\nrequire math\n",
        "game/main.das",
    );

    assert_eq!(includes(&extraction), NOTHING);
    assert_eq!(modules(&extraction), ["math"]);
}

#[test]
fn a_require_no_file_is_named_for_binds_its_module_outside_the_graph() {
    let extraction = resolve(
        &["game/main.das"],
        "options gen2\nrequire private_network\nrequire daslib/strings\nrequire daslib.fio.das\n",
        "game/main.das",
    );

    let bound: Vec<_> = (extraction.bindings.iter())
        .map(|binding| (binding.target_dir.as_deref(), binding.last_segment.as_str()))
        .collect();
    assert_eq!(
        bound,
        [(None, "private_network"), (None, "strings"), (None, "fio")]
    );
}

type Bound<'e> = (Option<&'e str>, Option<&'e str>, &'e str);

fn bound(extraction: &Extraction) -> Vec<Bound<'_>> {
    (extraction.bindings.iter())
        .map(|binding| {
            (
                binding.alias.as_deref(),
                binding.target_dir.as_deref(),
                binding.last_segment.as_str(),
            )
        })
        .collect()
}

#[test]
fn a_require_alias_of_a_file_binds_that_file() {
    let extraction = resolve(
        &["game/util/event.das", "game/main.das"],
        "options gen2\nrequire util/event as sub_event\n",
        "game/main.das",
    );

    assert_eq!(
        bound(&extraction),
        [(Some("sub_event"), Some("game/util/event.das"), "event")]
    );
    assert_eq!(includes(&extraction), ["game/util/event.das"]);
}

#[test]
fn a_require_alias_of_a_module_no_file_is_named_for_binds_both_names_outside_the_graph() {
    let extraction = resolve(
        &["game/main.das"],
        "options gen2\nrequire daslib/strings as sb\n",
        "game/main.das",
    );

    assert_eq!(
        bound(&extraction),
        [(None, None, "strings"), (Some("sb"), None, "strings")]
    );
}

#[test]
fn a_require_alias_two_files_could_satisfy_binds_nothing() {
    let files = ["a/util.das", "b/util.das", "main.das"];

    let extraction = resolve(&files, "options gen2\nrequire util as u\n", "main.das");

    assert_eq!(extraction.bindings, []);
}

#[test]
fn a_require_two_files_could_satisfy_binds_nothing_outside_the_graph() {
    let files = ["a/util.das", "b/util.das", "main.das"];

    let extraction = resolve(&files, "options gen2\nrequire util\n", "main.das");

    assert_eq!(extraction.bindings, []);
}

#[test]
fn an_include_that_names_no_file_binds_nothing_outside_the_graph() {
    let extraction = resolve(
        &["main.das"],
        "options gen2\ninclude shared.das\n",
        "main.das",
    );

    assert_eq!(modules(&extraction), ["shared.das"]);
    assert_eq!(
        extraction.bindings,
        [],
        "an include pastes text, so it names no module"
    );
}

#[test]
fn an_optional_require_includes_its_target_and_not_its_guard() {
    let files = ["libs/llvm_tune.das", "libs/llvm.das", "game/a.das"];

    assert_eq!(
        targets(&files, "?llvm_tune llvm", "game/a.das"),
        ["libs/llvm.das"]
    );
}

#[test]
fn an_include_resolves_its_file_name_as_written() {
    let extraction = resolve(
        &["lib/foo.das", "main.das"],
        "options gen2\ninclude \"foo.das\"\ninclude lib/foo.das\n",
        "main.das",
    );

    assert_eq!(includes(&extraction), ["lib/foo.das"]);
}

#[test]
fn a_test_path_marks_the_symbols_as_test_code() {
    for path in [
        "tests/thing.das",
        "game/test/thing.das",
        "game/Tests/thing.das",
        "thing_test.das",
        "test_thing.das",
    ] {
        assert!(flags(path).is_test(), "{path}");
    }
    for path in ["src/thing.das", "testing/thing.das", "contest.das"] {
        assert!(!flags(path).is_test(), "{path}");
    }
}

#[test]
fn a_generated_name_marks_the_symbols_as_generated_code() {
    for path in ["x.gen.das", "gen/x_gen.das"] {
        assert!(flags(path).is_generated(), "{path}");
    }
    assert!(!flags("gen/x.das").is_generated());
}
