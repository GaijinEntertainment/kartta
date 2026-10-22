//! How a Quirrel extraction resolves against its file's path and the repository: the `.sqconfig` that governs a
//! script, the file an import names, and the flags a path gives a file's symbols.

use kartta_lang::{Extraction, Language, MemoryRepository, SymbolFlags};
use kartta_lang_quirrel::Quirrel;

const NOTHING: [&str; 0] = [];

const UNREADABLE: &str = "\0unreadable";

fn resolve(files: &[(&str, &str)], source: &str, path: &str) -> Extraction {
    let quirrel = Quirrel::new();
    let readable = files
        .iter()
        .filter(|&&(_, contents)| contents != UNREADABLE);
    let unreadable = files
        .iter()
        .filter(|&&(_, contents)| contents == UNREADABLE);
    let repository =
        MemoryRepository::new(readable.copied()).with_paths(unreadable.map(|&(path, _)| path));
    let resolver = quirrel.resolver(&repository);

    resolver.resolve(quirrel.extract(source.as_bytes()), path, &repository)
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

fn target(files: &[(&str, &str)], spec: &str, path: &str) -> Option<String> {
    let mut extraction = resolve(files, &format!("require(\"{spec}\")\n"), path);
    assert_eq!(
        extraction.includes.len() + extraction.imports.len(),
        1,
        "{spec}"
    );

    extraction.includes.pop().map(|include| include.target)
}

fn flags(path: &str) -> SymbolFlags {
    let extraction = resolve(&[], "function helper() {}\n", path);

    extraction.symbols[0].flags
}

const GAME_A: &str = "mount = %darg=../../prog/scripts/sq/daRg\nmount = %ui=ui\nmount = %here=\n";

#[test]
fn a_mount_path_resolves_below_the_mounted_directory() {
    let files = [
        ("game_a/prog/.sqconfig", GAME_A),
        ("prog/scripts/sq/daRg/components/button.nut", ""),
        ("game_a/prog/ui/style.nut", ""),
        ("game_a/prog/hud/style.nut", ""),
    ];
    let target = |spec: &str| target(&files, spec, "game_a/prog/hud/main.nut");

    assert_eq!(
        target("%darg/components/button.nut").as_deref(),
        Some("prog/scripts/sq/daRg/components/button.nut")
    );
    assert_eq!(
        target("%ui/style.nut").as_deref(),
        Some("game_a/prog/ui/style.nut")
    );
    assert_eq!(
        target("%here/hud/style.nut").as_deref(),
        Some("game_a/prog/hud/style.nut")
    );
    assert_eq!(
        target("%darg/components\\\\button.nut").as_deref(),
        Some("prog/scripts/sq/daRg/components/button.nut")
    );
}

#[test]
fn a_mount_path_the_config_cannot_place_resolves_nothing() {
    let files = [
        ("game_a/prog/.sqconfig", GAME_A),
        ("prog/scripts/sq/daRg/components/button.nut", ""),
        ("game_a/prog/hud/style.nut", ""),
    ];
    let target = |spec: &str| target(&files, spec, "game_a/prog/hud/main.nut");

    assert_eq!(
        target("%hud/style.nut"),
        None,
        "a name the file does not mount"
    );
    assert_eq!(
        target("%darg/components/button"),
        None,
        "no extension is appended"
    );
    assert_eq!(target("%darg"), None);
    assert_eq!(target("%darg\\\\components\\\\button.nut"), None);
    assert_eq!(
        target("%DARG/components/button.nut"),
        None,
        "a mount name keeps its letter case"
    );
    assert_eq!(target("%darg/../../../../../x.nut"), None);
}

#[test]
fn a_nut_specifier_resolves_beside_the_script_then_under_each_base_path_in_order() {
    let files = [
        (
            "game/prog/.sqconfig",
            "base_path = ../../stubs\nbase_path = ../../prog/scripts/sq\n",
        ),
        ("game/prog/ui/std/frp.nut", ""),
        ("stubs/std/frp.nut", ""),
        ("prog/scripts/sq/std/frp.nut", ""),
        ("prog/scripts/sq/std/string.nut", ""),
        ("game/prog/ui/local.nut", ""),
    ];
    let target = |spec: &str| target(&files, spec, "game/prog/ui/main.nut");

    assert_eq!(
        target("std/frp.nut").as_deref(),
        Some("game/prog/ui/std/frp.nut")
    );
    assert_eq!(
        self::target(&files, "std/frp.nut", "game/prog/hud/main.nut").as_deref(),
        Some("stubs/std/frp.nut"),
        "the first base path that holds the file wins"
    );
    assert_eq!(
        target("std/string.nut").as_deref(),
        Some("prog/scripts/sq/std/string.nut")
    );
    assert_eq!(
        target("./local.nut").as_deref(),
        Some("game/prog/ui/local.nut")
    );
    assert_eq!(target("std/missing.nut"), None);
    assert_eq!(
        target("std/frp"),
        None,
        "a specifier without `.nut` is a native module"
    );
    assert_eq!(target("math"), None);
}

#[test]
fn a_target_in_another_letter_case_loads_its_sole_case_variant() {
    let files = [
        (
            "game/.sqconfig",
            "mount = %darg=../prog/daRg\nbase_path = stubs\n",
        ),
        ("prog/daRg/Components/Button.nut", ""),
        ("prog/daRg/Twin.nut", ""),
        ("prog/daRg/TWIN.nut", ""),
        ("prog/daRg/twice.nut", ""),
        ("prog/daRg/TWICE.nut", ""),
        ("game/stubs/Std/Frp.nut", ""),
        ("game/Local.nut", ""),
        ("game/stubs/local.nut", ""),
    ];
    let target = |spec: &str| target(&files, spec, "game/main.nut");

    assert_eq!(
        target("%darg/Twin.nut").as_deref(),
        Some("prog/daRg/Twin.nut")
    );
    assert_eq!(
        target("%darg/components/button.nut").as_deref(),
        Some("prog/daRg/Components/Button.nut")
    );
    assert_eq!(
        target("std/frp.nut").as_deref(),
        Some("game/stubs/Std/Frp.nut")
    );
    assert_eq!(
        target("local.nut").as_deref(),
        Some("game/Local.nut"),
        "the file beside the script loads before a base path's"
    );
    assert_eq!(target("%darg/Twice.nut"), None, "two case variants abstain");
    assert_eq!(
        target("Std/Frp.NUT"),
        None,
        "the `.nut` suffix keeps its letter case"
    );
}

#[test]
fn the_nearest_sqconfig_governs_and_another_file_contributes_nothing() {
    let files = [
        (".sqconfig", "mount = %root=r\n"),
        ("game/prog/.sqconfig", "mount = %game=g\n"),
        ("game/prog/ui/.sqconfig", "mount = %ui=\n"),
        ("game/prog/ui/settings.blk", "mount = %blk=b\n"),
        ("r/a.nut", ""),
        ("game/prog/g/a.nut", ""),
        ("game/prog/ui/a.nut", ""),
        ("game/prog/ui/b/a.nut", ""),
    ];

    assert_eq!(
        target(&files, "%ui/a.nut", "game/prog/ui/hud/main.nut").as_deref(),
        Some("game/prog/ui/a.nut")
    );
    assert_eq!(target(&files, "%game/a.nut", "game/prog/ui/main.nut"), None);
    assert_eq!(
        target(&files, "%game/a.nut", "game/prog/main.nut").as_deref(),
        Some("game/prog/g/a.nut")
    );
    assert_eq!(
        target(&files, "%game/a.nut", "game/prog/hud/deep/main.nut").as_deref(),
        Some("game/prog/g/a.nut")
    );
    assert_eq!(
        target(&files, "%root/a.nut", "other/main.nut").as_deref(),
        Some("r/a.nut")
    );
    assert_eq!(
        target(&files, "%root/a.nut", "main.nut").as_deref(),
        Some("r/a.nut")
    );
    assert_eq!(target(&files, "%blk/a.nut", "game/prog/ui/main.nut"), None);
}

#[test]
fn a_governing_sqconfig_never_falls_back_to_the_shape_of_the_specifier() {
    let files = [
        (
            "game_a/prog/.sqconfig",
            "mount = %darg=../../prog/scripts/sq/daRg\nmount = %ui=ui\nbase_path = ../../prog/scripts/sq/stubs/\n",
        ),
        ("prog/scripts/sq/daRg/components/button.nut", ""),
        ("prog/scripts/sq/stubs/frp.nut", ""),
        ("game_a/prog/ui/style.nut", ""),
        ("game_a/prog/hud/local.nut", ""),
        ("game_a/prog/scripts/lib.nut", ""),
        ("other/scripts/lib.nut", ""),
    ];

    let governed = resolve(
        &files,
        "require(\"%darg/components/button.nut\")\nimport \"%ui/style.nut\"\n\
         require(\"frp.nut\")\nrequire(\"./local.nut\")\nrequire(\"%scripts/lib.nut\")\n\
         require(\"ui/style\")\nrequire(\"math\")\nlet x = 1\n",
        "game_a/prog/hud/main.nut",
    );
    assert_eq!(
        includes(&governed),
        [
            "prog/scripts/sq/daRg/components/button.nut",
            "game_a/prog/ui/style.nut",
            "prog/scripts/sq/stubs/frp.nut",
            "game_a/prog/hud/local.nut",
        ]
    );
    let unresolved: Vec<&str> = governed
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect();
    assert_eq!(unresolved, ["%scripts/lib.nut", "ui/style", "math"]);

    let outside = resolve(&files, "require(\"%scripts/lib.nut\")\n", "other/main.nut");
    assert_eq!(includes(&outside), ["other/scripts/lib.nut"]);
}

#[test]
fn an_unreadable_sqconfig_governs_its_scripts_with_no_mount_and_no_base_path() {
    let files = [
        (".sqconfig", "mount = %ui=game/prog/ui\n"),
        ("game/prog/.sqconfig", UNREADABLE),
        ("game/prog/ui/style.nut", ""),
        ("game/prog/hud/local.nut", ""),
    ];

    let extraction = resolve(
        &files,
        "require(\"%ui/style.nut\")\nrequire(\"ui/style.nut\")\nrequire(\"local.nut\")\n",
        "game/prog/hud/main.nut",
    );

    assert_eq!(includes(&extraction), ["game/prog/hud/local.nut"]);
    assert_eq!(extraction.imports.len(), 2);
}

#[test]
fn without_a_sqconfig_a_mount_path_takes_the_nearest_ancestor_that_holds_it() {
    let files = [
        ("game/prog/ui/style.nut", ""),
        ("other/prog/ui/style.nut", ""),
        ("game/scripts/lib.nut", ""),
        ("game/prog/hud/panel.nut", ""),
    ];

    let extraction = resolve(
        &files,
        "require(\"%ui/style.nut\")\nimport \"%scripts/lib.nut\"\nrequire(\"ui/style\")\nimport \"%hud/panel\"\n",
        "game/prog/hud/main.nut",
    );

    assert_eq!(
        includes(&extraction),
        [
            "game/prog/ui/style.nut",
            "game/scripts/lib.nut",
            "game/prog/ui/style.nut",
            "game/prog/hud/panel.nut",
        ]
    );
}

#[test]
fn without_a_sqconfig_a_relative_specifier_names_the_file_beside_the_script() {
    let files = [("game/ui/sibling.nut", ""), ("game/shared/util.nut", "")];

    let extraction = resolve(
        &files,
        "require(\"./sibling.nut\")\nrequire(\"../shared/util\")\nrequire(\"../../../up.nut\")\n",
        "game/ui/main.nut",
    );

    assert_eq!(
        includes(&extraction),
        ["game/ui/sibling.nut", "game/shared/util.nut"]
    );
}

#[test]
fn native_and_missing_specifiers_stay_imports_without_an_include() {
    let extraction = resolve(
        &[("game/prog/ui/present.nut", "")],
        "require(\"math\")\nimport \"eventbus\"\nrequire(\"%ui/vanished.nut\")\nlet x = 1\n",
        "game/prog/main.nut",
    );

    assert_eq!(includes(&extraction), NOTHING);
    let modules: Vec<&str> = extraction
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect();
    assert_eq!(modules, ["math", "eventbus", "%ui/vanished.nut"]);
}

#[test]
fn an_import_keeps_the_names_it_takes_and_its_line() {
    let extraction = resolve(&[], "\nfrom \"math\" import sin, cos as c\n", "a.nut");

    let imports: Vec<(&str, &[String], u32)> = extraction
        .imports
        .iter()
        .map(|import| (import.module.as_str(), import.names.as_slice(), import.line))
        .collect();
    assert_eq!(
        imports,
        [("math", &["sin".to_owned(), "cos".to_owned()][..], 2)]
    );
}

#[test]
fn a_test_path_marks_the_symbols_as_test_code() {
    for path in [
        "tests/thing.nut",
        "ui/test/thing.nut",
        "thing_test.nut",
        "test_thing.nut",
    ] {
        assert!(flags(path).is_test(), "{path}");
    }
    for path in ["ui/thing.nut", "testing/thing.nut", "contest.nut"] {
        assert!(!flags(path).is_test(), "{path}");
    }
}

#[test]
fn a_generated_name_marks_the_symbols_as_generated_code() {
    for path in ["x.gen.nut", "ui/x_gen.nut"] {
        assert!(flags(path).is_generated(), "{path}");
    }
    assert!(!flags("gen/x.nut").is_generated());
}

const SQCONFIG: &str = "mount = %ui=ui\nbase_path = lib\n";

/// Whether the resolver built over a game whose `.sqconfig` reads `edited` resolves as the one built over the same
/// game with [`SQCONFIG`].
fn resolves_as_before(edited: &str) -> bool {
    let quirrel = Quirrel::new();
    let before = MemoryRepository::new([("game/.sqconfig", SQCONFIG), ("game/main.nut", "")]);
    let after = MemoryRepository::new([("game/.sqconfig", edited), ("game/main.nut", "")]);

    quirrel
        .resolver(&after)
        .resolves_as(&*quirrel.resolver(&before))
}

#[test]
fn a_sqconfig_comment_or_another_key_resolves_as_before() {
    assert!(resolves_as_before(&format!("; the game\n{SQCONFIG}")));
    assert!(resolves_as_before(&format!("{SQCONFIG}include = other\n")));
}

#[test]
fn a_changed_mount_or_base_path_resolves_otherwise() {
    assert!(!resolves_as_before(&SQCONFIG.replace("=ui", "=alt")));
    assert!(!resolves_as_before(&SQCONFIG.replace("lib", "src")));
}
