//! How a DSHL extraction resolves against the repository: the file each `include` names, by the nearest path that
//! ends in its spelling, and an import where none does.

use kartta_lang::{Extraction, Language, MemoryRepository};
use kartta_lang_dshl::Dshl;

const TREE: [&str; 10] = [
    "game/shaders/shader_global.dshl",
    "game/shaders/water.dshl",
    "tool/shaders/shader_global.dshl",
    "tool/shaders/sky.dshl",
    "libs/render/shaders/gbuffer.dshl",
    "libs/render/shaders/decals/decal_inc.dshl",
    "libs/render/shaders/common.hlsl",
    "libs/render/shaders/fog.dshl",
    "other/a/shader_global.dshl",
    "shader_global.dshl",
];

fn resolve(source: &str, path: &str) -> Extraction {
    let dshl = Dshl::new();
    let tree = MemoryRepository::of_paths(TREE);
    let resolver = dshl.resolver(&tree);

    resolver.resolve(dshl.extract(source.as_bytes()), path, &tree)
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

fn imports(extraction: &Extraction) -> Vec<&str> {
    extraction
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect()
}

#[test]
fn a_name_only_one_file_has_resolves_to_it() {
    let extraction = resolve("include \"gbuffer.dshl\"\n", "game/shaders/water.dshl");

    assert_eq!(includes(&extraction), ["libs/render/shaders/gbuffer.dshl"]);
}

#[test]
fn of_several_files_of_one_name_the_nearest_wins() {
    let in_game = resolve(
        "include \"shader_global.dshl\"\n",
        "game/shaders/water.dshl",
    );
    let in_tool = resolve("include \"shader_global.dshl\"\n", "tool/shaders/sky.dshl");

    assert_eq!(includes(&in_game), ["game/shaders/shader_global.dshl"]);
    assert_eq!(includes(&in_tool), ["tool/shaders/shader_global.dshl"]);
}

#[test]
fn the_file_at_the_spelled_path_wins_only_by_nearness() {
    let in_game = resolve(
        "include \"shader_global.dshl\"\n",
        "game/shaders/water.dshl",
    );
    let at_root = resolve("include \"shader_global.dshl\"\n", "main.dshl");

    assert_eq!(includes(&in_game), ["game/shaders/shader_global.dshl"]);
    assert_eq!(includes(&at_root), ["shader_global.dshl"]);
}

#[test]
fn files_equally_near_resolve_to_neither() {
    let extraction = resolve(
        "include \"shader_global.dshl\"\n",
        "libs/render/shaders/fog.dshl",
    );

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(imports(&extraction), ["shader_global.dshl"]);
}

#[test]
fn a_spelling_with_directories_matches_the_paths_that_end_in_it() {
    let extraction = resolve(
        "include \"decals/decal_inc.dshl\"\ninclude \"./gbuffer.dshl\"\n",
        "game/shaders/water.dshl",
    );

    assert_eq!(
        includes(&extraction),
        [
            "libs/render/shaders/decals/decal_inc.dshl",
            "libs/render/shaders/gbuffer.dshl"
        ]
    );
}

#[test]
fn a_spelling_that_climbs_is_relative_to_the_including_file() {
    let extraction = resolve(
        "include \"../fog.dshl\"\ninclude \"../../water.dshl\"\n",
        "libs/render/shaders/decals/decal_inc.dshl",
    );

    assert_eq!(includes(&extraction), ["libs/render/shaders/fog.dshl"]);
    assert_eq!(imports(&extraction), ["../../water.dshl"]);
}

#[test]
fn an_hlsl_include_resolves_by_the_same_rule() {
    let extraction = resolve(
        "shader s { hlsl(ps) {\n#include \"common.hlsl\"\n#include <missing.hlsl>\n} }\n",
        "libs/render/shaders/fog.dshl",
    );

    assert_eq!(includes(&extraction), ["libs/render/shaders/common.hlsl"]);
    assert_eq!(imports(&extraction), ["missing.hlsl"]);
    assert_eq!(extraction.imports[0].line, 3);
}

#[test]
fn a_file_including_itself_adds_no_include() {
    let extraction = resolve("include \"water.dshl\"\n", "game/shaders/water.dshl");

    assert_eq!(includes(&extraction), Vec::<&str>::new());
    assert_eq!(imports(&extraction), Vec::<&str>::new());
}

#[test]
fn a_resolved_file_keeps_its_path_and_symbols() {
    let extraction = resolve("shader water {}\n", "game/shaders/water.dshl");

    assert_eq!(extraction.file, "game/shaders/water.dshl");
    assert_eq!(extraction.symbols[0].name, "water");
}
