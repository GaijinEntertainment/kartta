//! How a BLK extraction resolves against its file's path and the repository: the file an include names, relative to
//! the including file, to a root, or through the mount table the repository's `mountPoints` blocks state.

use std::io;

use kartta_lang::path::{base, dir};
use kartta_lang::{Extraction, Language, Repository};
use kartta_lang_blk::Blk;

struct Files<'a>(&'a [(&'a str, &'a str)]);

impl Repository for Files<'_> {
    fn contains(&self, path: &str) -> bool {
        self.0.iter().any(|&(file, _)| file == path)
    }

    fn files_in_dir<'a>(&'a self, in_dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let in_dir = in_dir.to_owned();
        Box::new(
            self.0
                .iter()
                .map(|&(file, _)| file)
                .filter(move |file| dir(file) == in_dir)
                .map(str::to_owned),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        Box::new(
            self.0
                .iter()
                .map(|&(file, _)| file)
                .filter(move |file| base(file) == name)
                .map(str::to_owned),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.0
            .iter()
            .find(|&&(file, _)| file == path)
            .map(|&(_, contents)| contents.as_bytes().to_vec())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

fn resolve(files: &[(&str, &str)], source: &[u8], path: &str) -> Extraction {
    let blk = Blk::new();
    let repository = Files(files);
    let resolver = blk.resolver(&repository);

    resolver.resolve(blk.extract(source), path)
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

fn imports(extraction: &Extraction) -> Vec<(&str, u32)> {
    extraction
        .imports
        .iter()
        .map(|import| (import.module.as_str(), import.line))
        .collect()
}

fn target(files: &[(&str, &str)], path: &str, including: &str) -> Option<String> {
    let mut extraction = resolve(files, format!("include \"{path}\"\n").as_bytes(), including);
    assert_eq!(
        extraction.includes.len() + extraction.imports.len(),
        1,
        "{path}"
    );

    extraction.includes.pop().map(|include| include.target)
}

#[test]
fn a_relative_path_resolves_against_the_including_file() {
    let source = b"include \"common.blk\"\ninclude ../shared/base.blk\ninclude sub\\x.blk\ninclude missing.blk\n";
    let files = [
        ("game/config/common.blk", ""),
        ("game/shared/base.blk", ""),
        ("game/config/sub/x.blk", ""),
    ];
    let extraction = resolve(&files, source, "game/config/settings.blk");

    assert_eq!(
        includes(&extraction),
        [
            "game/config/common.blk",
            "game/shared/base.blk",
            "game/config/sub/x.blk"
        ]
    );
    assert_eq!(imports(&extraction), [("missing.blk", 4)]);
    assert!(
        extraction
            .imports
            .iter()
            .all(|import| import.names.is_empty())
    );
    assert_eq!(extraction.file, "game/config/settings.blk");
}

#[test]
fn a_quoted_path_resolves_decoded_and_records_as_written() {
    let source = b"include \"gyro~~ps4.blk\"\n\
                   include \"say~\"hi~\".blk\"\n\
                   include 'it~'s.blk'\n\
                   include \"\"\"   \r\nmulti.blk\r\n\"\"\"\n\
                   include '''inline.blk'''\n\
                   include \"gone~~ps4.blk\"\n";
    let files = [
        ("cfg/gyro~ps4.blk", ""),
        ("cfg/say\"hi\".blk", ""),
        ("cfg/it's.blk", ""),
        ("cfg/multi.blk", ""),
        ("cfg/inline.blk", ""),
        ("cfg/gyro~~ps4.blk", ""),
    ];
    let extraction = resolve(&files, source, "cfg/a.blk");

    assert_eq!(imports(&extraction), [("gone~~ps4.blk", 8)]);
    assert_eq!(
        includes(&extraction),
        [
            "cfg/gyro~ps4.blk",
            "cfg/say\"hi\".blk",
            "cfg/it's.blk",
            "cfg/multi.blk",
            "cfg/inline.blk",
        ]
    );
}

#[test]
fn an_unquoted_path_resolves_without_its_comments_and_trailing_blanks() {
    let source =
        b"include a/*x*/b.blk\ninclude c.blk /* note */\ninclude d.blk\t;\ninclude e/*y*/f.blk\n";
    let files = [("cfg/ab.blk", ""), ("cfg/c.blk", ""), ("cfg/d.blk", "")];
    let extraction = resolve(&files, source, "cfg/a.blk");

    assert_eq!(imports(&extraction), [("e/*y*/f.blk", 4)]);
    assert_eq!(
        includes(&extraction),
        ["cfg/ab.blk", "cfg/c.blk", "cfg/d.blk"]
    );
}

#[test]
fn every_spelling_of_the_keyword_resolves() {
    let spellings: &[&[u8]] = &[
        b"include a.blk\n",
        b"InClUdE a.blk\n",
        b"'include' a.blk\n",
        b"\"incl~ude\" a.blk\n",
        b"'~I~N~C~L~U~D~E' a.blk\n",
        b"\"\"\"inc\rlude\"\"\" a.blk\n",
        b"'''\r\ni\rnclude\r\n''' a.blk\n",
    ];

    for source in spellings {
        let extraction = resolve(&[("cfg/a.blk", "")], source, "cfg/x.blk");
        assert_eq!(
            includes(&extraction),
            ["cfg/a.blk"],
            "{:?}",
            String::from_utf8_lossy(source)
        );
    }
}

#[test]
fn a_bare_root_or_mount_prefix_stays_unresolved() {
    let cases: &[(&[u8], &str)] = &[
        (b"include #\n", "#"),
        (b"include #", "#"),
        (b"include %\n", "%"),
        (b"include \"#\"\n", "#"),
        (b"include '%'\n", "%"),
    ];

    for &(source, path) in cases {
        let extraction = resolve(&[("cfg/a.blk", ""), ("x.blk", "")], source, "cfg/a.blk");
        assert_eq!(imports(&extraction), [(path, 1)]);
        assert!(extraction.includes.is_empty(), "{:?}", extraction.includes);
    }
}

#[test]
fn a_root_path_resolves_only_to_its_sole_candidate() {
    let source = b"include \"#/gamedata/sole.blk\"\ninclude \"#gamedata/twice.blk\"\ninclude \"#gamedata/none.blk\"\n";
    let files = [
        ("a/gamedata/sole.blk", ""),
        ("a/other_gamedata/sole.blk", ""),
        ("a/gamedata/twice.blk", ""),
        ("b/gamedata/twice.blk", ""),
    ];
    let extraction = resolve(&files, source, "a/config/settings.blk");

    assert_eq!(includes(&extraction), ["a/gamedata/sole.blk"]);
    assert_eq!(
        imports(&extraction),
        [("#gamedata/twice.blk", 2), ("#gamedata/none.blk", 3)]
    );
}

#[test]
fn a_root_path_at_the_repository_root_is_a_candidate() {
    let files = [("gamedata/x.blk", "")];

    assert_eq!(
        target(&files, "#gamedata/x.blk", "cfg/a.blk").as_deref(),
        Some("gamedata/x.blk")
    );
    assert_eq!(target(&files, "#../gamedata/x.blk", "cfg/a.blk"), None);
}

#[test]
fn unmapped_mount_absolute_and_working_directory_paths_stay_unresolved() {
    let source =
        b"include \"%gameBase/x.blk\"\ninclude /x.blk\ninclude \\x.blk\ninclude \":/x.blk\"\n\
                   include \"C:/x.blk\"\ninclude d:\\x.blk\n";
    let files = [
        ("gameBase/x.blk", ""),
        ("cfg/x.blk", ""),
        ("cfg/:/x.blk", ""),
        ("cfg/C:/x.blk", ""),
        ("cfg/d:/x.blk", ""),
        ("x.blk", ""),
    ];
    let extraction = resolve(&files, source, "cfg/a.blk");

    assert!(extraction.includes.is_empty(), "{:?}", extraction.includes);
    assert_eq!(
        imports(&extraction),
        [
            ("%gameBase/x.blk", 1),
            ("/x.blk", 2),
            ("\\x.blk", 3),
            (":/x.blk", 4),
            ("C:/x.blk", 5),
            ("d:\\x.blk", 6),
        ]
    );
}

#[test]
fn a_target_that_differs_only_in_letter_case_names_no_file() {
    let files = [
        ("cfg/gamedata/units/tank.blk", ""),
        ("gamedata/root.blk", ""),
    ];

    assert_eq!(target(&files, "gameData/Units/tank.blk", "cfg/a.blk"), None);
    assert_eq!(target(&files, "#/GameData/root.blk", "cfg/a.blk"), None);
}

const GAME_BASE: &str =
    "mountPoints {\n  \"%gameBase\"{ forSource:t=\"../prog/gameBase\"; forVromfs:t=\"\"; }\n}\n";

#[test]
fn a_mount_path_resolves_through_the_mount_table_as_the_engine_reads_it() {
    let source = b"include \"%gameBase/gameData/units/tank.blk\"\n\
                   include %gameBase/gameData/units/missing.blk\n\
                   include \"%game~Base/gameData/units/tank.blk\"\n";
    let files = [
        ("game/prog/gameBase/config/settings.blk", GAME_BASE),
        ("game/prog/gameBase/gameData/units/tank.blk", ""),
    ];
    let extraction = resolve(&files, source, "game/prog/gameBase/gameData/units/a.blk");

    assert_eq!(
        includes(&extraction),
        [
            "game/prog/gameBase/gameData/units/tank.blk",
            "game/prog/gameBase/gameData/units/tank.blk",
        ]
    );
    assert_eq!(
        imports(&extraction),
        [("%gameBase/gameData/units/missing.blk", 2)]
    );
}

#[test]
fn a_mount_target_matches_its_name_and_its_file_in_letter_case() {
    let files = [
        ("game/prog/gameBase/config/settings.blk", GAME_BASE),
        ("game/prog/gameBase/gamedata/units/tank.blk", ""),
    ];
    let including = "game/prog/gameBase/gamedata/units/a.blk";

    assert_eq!(
        target(&files, "%gameBase/gamedata/units/tank.blk", including).as_deref(),
        Some("game/prog/gameBase/gamedata/units/tank.blk")
    );
    assert_eq!(
        target(&files, "%gameBase/gameData/Units/tank.blk", including),
        None
    );
    assert_eq!(
        target(&files, "%gamebase/gamedata/units/tank.blk", including),
        None
    );
}

const WT_SETTINGS: &str = "wt/prog/gameBase/gamebase/config/settings.blk";

#[test]
fn a_source_directory_resolves_below_the_ancestor_of_the_stating_file_that_holds_the_target() {
    let settings = "mountPoints {\n  \"%gameBase\"{forSource:t=\"../prog/gameBase/gamebase\"; forVromfs:t=\"\";}\n  \
                    \"%gameLibs\"{forSource:t=\"..\\..\\prog\\gameLibs\"; forVromfs:t=\"gameLibs\";}\n}\n";
    let files = [
        (WT_SETTINGS, settings),
        ("wt/prog/gameBase/gamebase/gameData/units/tank.blk", ""),
        ("prog/gameLibs/render/shaders.blk", ""),
    ];

    assert_eq!(
        target(
            &files,
            "%gameBase/gameData/units/tank.blk",
            "wt/prog/gameBase/gamebase/gameData/units/a.blk"
        )
        .as_deref(),
        Some("wt/prog/gameBase/gamebase/gameData/units/tank.blk")
    );
    assert_eq!(
        target(&files, "%gameLibs/render\\shaders.blk", "wt/prog/x.blk").as_deref(),
        Some("prog/gameLibs/render/shaders.blk")
    );
}

#[test]
fn a_mount_parameter_resolves_against_the_directory_of_the_stating_file() {
    let config = "mountPoints{\n  \"%daslib\":t=\"../../prog/daslib\"\n  \"%appGame\":t=\"scripts/wt/game\"\n}\n";
    let files = [
        ("wt/prog/aot_config.blk", config),
        ("prog/daslib/a.blk", ""),
        ("wt/prog/scripts/wt/game/b.blk", ""),
    ];

    assert_eq!(
        target(&files, "%daslib/a.blk", "wt/prog/x.blk").as_deref(),
        Some("prog/daslib/a.blk")
    );
    assert_eq!(
        target(&files, "%appGame/b.blk", "wt/prog/x.blk").as_deref(),
        Some("wt/prog/scripts/wt/game/b.blk")
    );
}

#[test]
fn every_file_name_that_states_mounts_is_read() {
    let mounts =
        |name: &str| format!("mountPoints{{ \"%{name}\"{{ forSource:t=\"../{name}\"; }} }}\n");
    let (settings, application, aot, libs_aot, other) = (
        mounts("settings"),
        mounts("application"),
        mounts("aot"),
        mounts("libs_aot"),
        mounts("other"),
    );
    let files = [
        ("game/settings.blk", settings.as_str()),
        ("game/application.blk", application.as_str()),
        ("game/aot_config.blk", aot.as_str()),
        ("game/libs_aot_config.blk", libs_aot.as_str()),
        ("game/game.settings.blk", other.as_str()),
        ("settings/a.blk", ""),
        ("application/a.blk", ""),
        ("aot/a.blk", ""),
        ("libs_aot/a.blk", ""),
        ("other/a.blk", ""),
    ];

    for name in ["settings", "application", "aot", "libs_aot"] {
        assert_eq!(
            target(&files, &format!("%{name}/a.blk"), "game/x.blk"),
            Some(format!("{name}/a.blk"))
        );
    }
    assert_eq!(target(&files, "%other/a.blk", "game/x.blk"), None);
}

#[test]
fn the_mapping_stated_nearest_the_including_file_decides() {
    let files = [
        (
            WT_SETTINGS,
            "mountPoints{ \"%gameBase\"{ forSource:t=\"../prog/gameBase/gamebase\"; } }\n",
        ),
        (
            "wt/prog/gamebase_wtm/gamebase/config/settings.blk",
            "mountPoints{ \"%gameBase\"{ forSource:t=\"../../wt/prog/gamebase_wtm/gamebase\"; } }\n",
        ),
        ("wt/prog/gameBase/gamebase/config/units.blk", ""),
        ("wt/prog/gamebase_wtm/gamebase/config/units.blk", ""),
    ];
    let units = |including: &str| target(&files, "%gameBase/config/units.blk", including);

    assert_eq!(
        units("wt/prog/gameBase/gamebase/gameData/a.blk").as_deref(),
        Some("wt/prog/gameBase/gamebase/config/units.blk")
    );
    assert_eq!(
        units("wt/prog/gamebase_wtm/pkg_dev/a.blk").as_deref(),
        Some("wt/prog/gamebase_wtm/gamebase/config/units.blk")
    );
    assert_eq!(units("wt/prog/scripts/a.blk"), None);
}

#[test]
fn equally_near_mappings_that_agree_resolve_and_a_missing_place_abstains() {
    let files = [
        (
            "game/application.blk",
            "mountPoints{ \"%gameLibs\"{ forSource:t=\"../../prog/gameLibs\"; } \
             \"%tools\"{ forSource:t=\"../prog/tools\"; } }\n",
        ),
        (
            "game/aot_config.blk",
            "mountPoints{ \"%gameLibs\":t=\"../prog/gameLibs\"\n\"%tools\":t=\"prog/other\" }\n",
        ),
        ("prog/gameLibs/a.blk", ""),
        ("game/prog/tools/b.blk", ""),
    ];

    assert_eq!(
        target(&files, "%gameLibs/a.blk", "game/x.blk").as_deref(),
        Some("prog/gameLibs/a.blk")
    );
    assert_eq!(target(&files, "%tools/b.blk", "game/x.blk"), None);
}

#[test]
fn several_ancestors_holding_the_target_abstain() {
    let settings = (
        "game/prog/config/settings.blk",
        "mountPoints{ \"%gameLibs\"{ forSource:t=\"../../prog/gameLibs\"; } }\n",
    );
    let place = |files: &[(&str, &str)]| target(files, "%gameLibs/a.blk", "x.blk");

    assert_eq!(
        place(&[settings, ("prog/gameLibs/a.blk", "")]).as_deref(),
        Some("prog/gameLibs/a.blk")
    );
    assert_eq!(
        place(&[
            settings,
            ("prog/gameLibs/a.blk", ""),
            ("game/prog/gameLibs/a.blk", "")
        ]),
        None
    );
    assert_eq!(place(&[settings, ("other/prog/gameLibs/a.blk", "")]), None);
}

#[test]
fn a_source_directory_that_does_not_climb_takes_part_and_places_nothing() {
    let nearer = (
        "game/prog/config/settings.blk",
        "mountPoints{ \"%scripts\"{ forSource:t=\"scripts\"; } \"%abs\"{ forSource:t=\"/x\"; } \
         \"%drive\"{ forSource:t=\"C:/x\"; } \"%nested\"{ forSource:t=\"%appDir/x\"; } \
         \"%rooted\"{ forSource:t=\"/../x\"; } }\n",
    );
    let farther = (
        "settings.blk",
        "mountPoints{ \"%scripts\"{ forSource:t=\"../game/scripts\"; } }\n",
    );
    let files = [
        nearer,
        farther,
        ("game/scripts/a.blk", ""),
        ("game/prog/config/scripts/a.blk", ""),
        ("x/a.blk", ""),
        ("game/x/a.blk", ""),
        ("appDir/x/a.blk", ""),
    ];

    for name in ["abs", "drive", "nested", "rooted"] {
        assert_eq!(
            target(&files, &format!("%{name}/a.blk"), "game/prog/x.blk"),
            None,
            "{name}"
        );
    }
    assert_eq!(target(&files, "%scripts/a.blk", "game/prog/x.blk"), None);
    assert_eq!(target(&files, "%scripts/a.blk", "elsewhere/x.blk"), None);
    assert_eq!(
        target(
            &[farther, ("game/scripts/a.blk", "")],
            "%scripts/a.blk",
            "elsewhere/x.blk"
        )
        .as_deref(),
        Some("game/scripts/a.blk")
    );
}

#[test]
fn a_later_entry_replaces_and_a_block_without_a_source_directory_erases() {
    let settings = "mountPoints{\n  \"%a\"{ forSource:t=\"../one\"; }\n  \"%a\"{ forSource:t=\"../two\"; }\n  \
                    \"%b\"{ forSource:t=\"../three\"; }\n  b{ forVromfs:t=\"three\"; }\n  \
                    \"%c\"{ forVromfs:t=\"packed\"; forSource:i=1; }\n}\n";
    let files = [
        ("game/settings.blk", settings),
        ("one/f.blk", ""),
        ("two/f.blk", ""),
        ("three/f.blk", ""),
        ("packed/f.blk", ""),
    ];

    assert_eq!(
        target(&files, "%a/f.blk", "game/x.blk").as_deref(),
        Some("two/f.blk")
    );
    assert_eq!(target(&files, "%b/f.blk", "game/x.blk"), None);
    assert_eq!(target(&files, "%c/f.blk", "game/x.blk"), None);
}

#[test]
fn only_valid_names_map_and_the_lookup_keeps_letter_case() {
    let settings = "mountPoints{ darg{ forSource:t=\"../d\"; } \"%bad-name\"{ forSource:t=\"../x\"; } \
                    \"%Case\"{ forSource:t=\"../c\"; } }\n";
    let files = [
        ("game/settings.blk", settings),
        ("game/c/a.blk", ""),
        ("game/d/a.blk", ""),
        ("game/x/a.blk", ""),
    ];

    assert_eq!(
        target(&files, "%darg/a.blk", "game/x.blk").as_deref(),
        Some("game/d/a.blk")
    );
    assert_eq!(
        target(&files, "%Case/a.blk", "game/x.blk").as_deref(),
        Some("game/c/a.blk")
    );
    assert_eq!(target(&files, "%case/a.blk", "game/x.blk"), None);
    assert_eq!(target(&files, "%bad-name/a.blk", "game/x.blk"), None);
}

#[test]
fn a_path_without_a_slash_or_with_an_unmapped_name_resolves_nothing() {
    let files = [
        (
            "game/settings.blk",
            "mountPoints{ \"%gameBase\"{ forSource:t=\"../gameBase\"; } }\n",
        ),
        ("gameBase/a.blk", ""),
    ];

    assert_eq!(target(&files, "%gameBase", "game/x.blk"), None);
    assert_eq!(target(&files, "%gameBase\\a.blk", "game/x.blk"), None);
    assert_eq!(target(&files, "%other/a.blk", "game/x.blk"), None);
    assert_eq!(
        target(&files, "%gameBase/a.blk", "game/x.blk").as_deref(),
        Some("gameBase/a.blk")
    );
}
