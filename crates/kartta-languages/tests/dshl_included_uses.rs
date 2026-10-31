//! What a DSHL use binds past its own file: a shader file and its includes are one script, so a read of a global and a
//! `supports` of a block bind what an included file declares, and a declaration of the file itself of any kind stays
//! bound, a shader that a `shader ==` condition names included. An included file compiles inside its includers, so a
//! use all that leaves open binds the one file of every script above it that declares the name, and of several the one
//! in its own game. A macro call reads the same closure, and a macro no include reaches stays the one callable of its
//! name.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits, Selector, SiteQuery, SymbolKind};
use tempfile::TempDir;

const GLOBALS: &str = "texture frame_tex;\nblock(scene) global_frame {}\n";

const WATER: &str = r#"include "globals.dshl"

shader water, foam
{
  supports global_frame;
  (ps) { frame@smp2d = frame_tex; }
  if (shader == foam) { dont_render; }
}
"#;

const DEEP: &str = "float deep_value = 1;\nblock(scene) deep_frame {}\n";

const MIDDLE: &str = r#"include "deep.dshl"

float middle_value = 2;
"#;

const TOP: &str = r#"include "middle.dshl"

shader lit
{
  (ps) { direct@f1 = middle_value; }
  (ps) { past@f1 = deep_value; }
  supports deep_frame;
}

block(scene) top_frame {}

shader unlit
{
  supports top_frame;
}
"#;

const LEFT: &str = "float shared_value = 1;\n";

const RIGHT: &str = "float shared_value = 2;\n";

const BETWEEN: &str = r#"include "left.dshl"
include "right.dshl"
"#;

const TWICE: &str = r#"include "between.dshl"

shader twice
{
  (ps) { read@f1 = shared_value; }
}
"#;

const HOLDER: &str = r"shader holder
{
  (ps) { held@f1 = 1; }
}
";

const THROUGH: &str = r#"include "holder.dshl"
"#;

const READER: &str = r#"include "through.dshl"

shader reader
{
  (ps) { read@f1 = held; }
}
"#;

const OWN_VALUE: &str = r#"include "deep.dshl"

float deep_value = 3;

shader own
{
  (ps) { read@f1 = deep_value; }
}
"#;

const VARS: &str = "int special_vision = 0;\n";

const RANGES: &str = "interval special_vision: off<1, on;\n";

const SEES_BOTH: &str = r#"include "vars.dshl"
include "iv.dshl"

shader lit
{
  if (special_vision == on) { dont_render; }
}
"#;

const OWN_RANGES: &str = r#"include "vars.dshl"

interval special_vision: off<1, on;

shader lit
{
  if (special_vision == on) { dont_render; }
}
"#;

const RANGES_OVER_VARS: &str = r#"include "vars.dshl"

interval special_vision: off<1, on;
"#;

const THROUGH_RANGES: &str = r#"include "ranges.dshl"

shader lit
{
  if (special_vision == on) { dont_render; }
}
"#;

const ONLY_RANGES: &str = r"interval quality: low<1, high;

shader lit
{
  if (quality == high) { dont_render; }
}
";

const SEES_TWO_VARS: &str = r#"include "left.dshl"
include "right.dshl"

interval shared_value: low<1, high;

shader lit
{
  if (shared_value == high) { dont_render; }
}
"#;

const SHADER_OF_RANGES_NAME: &str = "shader special_vision\n{\n}\n";

const SEES_SHADER: &str = r#"include "named.dshl"
include "iv.dshl"

shader lit
{
  if (special_vision == on) { dont_render; }
}
"#;

const OWN_SHADER_OF_VARIABLE_NAME: &str = r#"include "vars.dshl"

shader special_vision
{
  if (special_vision == 1) { dont_render; }
}
"#;

const INCLUDER: &str = r#"float up_value = 1;
float side_value = 2;
include "lib.dshl"

shader own
{
  (ps) { own@f1 = side_value; }
}
"#;

const LIB: &str = r"shader lit
{
  (ps) { past@f1 = up_value; }
}
";

const OTHER_INCLUDER: &str = r#"float up_value = 3;
include "lib.dshl"
"#;

const INCLUDER_WITHOUT_IT: &str = r#"include "lib.dshl"
"#;

const INCLUDER_OF_MIDDLE: &str = r#"float up_value = 1;
include "pass.dshl"
"#;

const PASS: &str = r#"include "lib.dshl"
"#;

const GLOBAL: &str = "float up_value = 1;\n";

const GLOBAL_THEN_LIB: &str = r#"include "global.dshl"
include "lib.dshl"
"#;

const GLOBAL_INCLUDER: &str = r#"include "global.dshl"
"#;

const SHADER_WITH_THE_FIELD: &str = r#"include "lib.dshl"

shader holder
{
  (ps) { up_value@f1 = 1; }
}
"#;

const TWO_GLOBALS_THEN_LIB: &str = r#"include "left.dshl"
include "right.dshl"
include "shared_lib.dshl"
"#;

const SHARED_LIB: &str = r"shader lit
{
  (ps) { read@f1 = shared_value; }
}
";

const OTHER_GAME_ROOT: &str = r#"include "global.dshl"
include "../a/lib.dshl"
"#;

const BOTH_COPIES_THEN_LIB: &str = r#"include "global.dshl"
include "../../b/shaders/global.dshl"
include "lib.dshl"
"#;

const TWO_COPIES_THEN_LIB: &str = r#"include "copies/left.dshl"
include "lib.dshl"
"#;

const OTHER_COPY_THEN_LIB: &str = r#"include "copies/right.dshl"
include "lib.dshl"
"#;

const INCLUDER_OF_VARIABLE: &str = r#"int special_vision = 0;
include "ranges_lib.dshl"
"#;

const RANGES_LIB: &str = r"interval special_vision: off<1, on;

shader lit
{
  if (special_vision == on) { dont_render; }
}
";

const DEEP_MACRO: &str = "macro DEEP_M()\n  dont_render;\nendmacro\n";

const MIDDLE_MACROS: &str = r#"include "deep.dshl"

macro MID_M()
  dont_render;
endmacro
"#;

const OTHER_MACROS: &str = r"macro DEEP_M()
  dont_render;
endmacro

macro MID_M()
  dont_render;
endmacro
";

const CALLS_PAST_THE_FIRST_INCLUDE: &str = r#"include "middle.dshl"

shader lit
{
  DEEP_M()
  MID_M()
}
"#;

const UNFOLLOWED_MACRO: &str = "macro UNFOLLOWED_M()\n  dont_render;\nendmacro\n";

const CALLS_THROUGH_AN_UNFOLLOWED_INCLUDE: &str = r#"include "lost/unfollowed.dshl"

shader lit
{
  UNFOLLOWED_M()
}
"#;

const INCLUDES_TWO_THAT_INCLUDE_THE_MACRO: &str = r#"include "left.dshl"
include "right.dshl"

shader lit
{
  DEEP_M()
}
"#;

const CALLS_THROUGH_A_SHARED_FILE: &str = r#"include "shared.dshl"

shader lit
{
  DEEP_M()
}
"#;

const SHARED_THAT_INCLUDES_THE_MACRO: &str = "include \"deep.dshl\"\n";

fn build() -> (TempDir, Engine) {
    build_tree(&[
        ("globals.dshl", GLOBALS),
        ("water.dshl", WATER),
        ("deep.dshl", DEEP),
        ("middle.dshl", MIDDLE),
        ("top.dshl", TOP),
    ])
}

fn build_tree(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (name, text) in files {
        let path = tree.path().join(name);
        let parent = path.parent().expect("a file sits in a directory");
        fs::create_dir_all(parent).expect("the directory is made");
        fs::write(path, text).expect("the file is written");
    }
    let languages = kartta_languages::select(["dshl"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// Each use site's holder, line and confidence.
fn uses<'e, 's>(engine: &'e Engine, name: impl Into<Selector<'s>>) -> Vec<(&'e str, u32, u8)> {
    let (_, graph) = engine.graph_of("water.dshl").expect("DSHL claims the file");
    graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name is defined")
        .sites
        .iter()
        .filter(|site| !site.call)
        .map(|site| (site.site_of.qualified_name, site.line, site.confidence))
        .collect()
}

/// Each caller of the definition `name` selects under `path`, with its confidence, and the calls left ambiguous.
fn callers<'e>(engine: &'e Engine, name: &str, path: &str) -> (Vec<(&'e str, u8)>, usize) {
    let (_, graph) = engine.graph_of("water.dshl").expect("DSHL claims the file");
    let walk = graph
        .callers(Selector::new(name, Some(path)), Limits::new(1))
        .expect("the macro is defined");
    let reached = (walk.reached.iter())
        .map(|reached| (reached.symbol.qualified_name, reached.confidence))
        .collect();
    (reached, walk.ambiguous)
}

/// The file and the kind of the definition `name` selects.
fn selected<'e>(engine: &'e Engine, name: &str) -> (&'e str, SymbolKind) {
    let (_, graph) = engine.graph_of("water.dshl").expect("DSHL claims the file");
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name is defined");
    (references.symbol.file, references.symbol.kind)
}

#[test]
fn a_read_past_an_interval_in_another_file_selects_and_binds_the_variable() {
    let (_tree, engine) = build_tree(&[
        ("vars.dshl", VARS),
        ("iv.dshl", RANGES),
        ("top.dshl", SEES_BOTH),
    ]);

    assert_eq!(uses(&engine, "special_vision"), [("lit", 6, 70)]);
    assert_eq!(
        selected(&engine, "special_vision"),
        ("vars.dshl", SymbolKind::Variable)
    );
}

#[test]
fn a_read_of_an_interval_its_own_file_writes_binds_the_variable() {
    let (_tree, engine) = build_tree(&[("vars.dshl", VARS), ("top.dshl", OWN_RANGES)]);

    assert_eq!(uses(&engine, "special_vision"), [("lit", 7, 70)]);
    assert_eq!(
        selected(&engine, "special_vision"),
        ("vars.dshl", SymbolKind::Variable)
    );
}

#[test]
fn a_read_of_an_interval_a_direct_include_writes_binds_the_variable_it_includes() {
    let (_tree, engine) = build_tree(&[
        ("vars.dshl", VARS),
        ("ranges.dshl", RANGES_OVER_VARS),
        ("top.dshl", THROUGH_RANGES),
    ]);

    assert_eq!(uses(&engine, "special_vision"), [("lit", 5, 70)]);
    assert_eq!(
        selected(&engine, "special_vision"),
        ("vars.dshl", SymbolKind::Variable)
    );
}

#[test]
fn an_interval_with_no_variable_its_includes_reach_stays_bound() {
    let (_tree, engine) = build_tree(&[("top.dshl", ONLY_RANGES)]);

    assert_eq!(uses(&engine, "quality"), [("lit", 5, 80)]);
}

#[test]
fn an_interval_over_two_variables_its_includes_reach_stays_bound() {
    let (_tree, engine) = build_tree(&[
        ("left.dshl", LEFT),
        ("right.dshl", RIGHT),
        ("top.dshl", SEES_TWO_VARS),
    ]);

    assert_eq!(selected(&engine, "shared_value").1, SymbolKind::Enum);
    assert_eq!(uses(&engine, "shared_value"), [("lit", 8, 80)]);
}

#[test]
fn a_name_an_interval_and_a_shader_share_stays_unbound() {
    let (_tree, engine) = build_tree(&[
        ("named.dshl", SHADER_OF_RANGES_NAME),
        ("iv.dshl", RANGES),
        ("top.dshl", SEES_SHADER),
    ]);

    assert_eq!(uses(&engine, "special_vision"), []);
}

#[test]
fn a_shader_its_own_file_writes_keeps_a_read_a_variable_past_it_could_take() {
    let (_tree, engine) = build_tree(&[
        ("vars.dshl", VARS),
        ("top.dshl", OWN_SHADER_OF_VARIABLE_NAME),
    ]);

    assert_eq!(
        selected(&engine, "special_vision"),
        ("top.dshl", SymbolKind::Class)
    );
    assert_eq!(uses(&engine, "special_vision"), [("special_vision", 5, 80)]);
}

#[test]
fn a_read_of_a_global_an_included_file_declares_binds_it() {
    let (_tree, engine) = build();

    assert_eq!(uses(&engine, "frame_tex"), [("water", 6, 70)]);
}

#[test]
fn a_supports_of_a_block_an_included_file_declares_binds_it() {
    let (_tree, engine) = build();

    assert_eq!(uses(&engine, "global_frame"), [("water", 5, 70)]);
}

#[test]
fn a_shader_the_file_declares_stays_bound_by_a_condition_that_names_it() {
    let (_tree, engine) = build();

    assert_eq!(uses(&engine, "foam"), [("water", 7, 80)]);
}

#[test]
fn a_read_of_a_global_past_the_first_include_binds_it() {
    let (_tree, engine) = build();

    assert_eq!(uses(&engine, "middle_value"), [("lit", 5, 70)]);
    assert_eq!(uses(&engine, "deep_value"), [("lit", 6, 70)]);
}

#[test]
fn a_supports_of_a_block_past_the_first_include_binds_it() {
    let (_tree, engine) = build();

    assert_eq!(uses(&engine, "deep_frame"), [("lit", 7, 70)]);
}

#[test]
fn a_name_two_files_past_the_first_include_declare_binds_neither() {
    let (_tree, engine) = build_tree(&[
        ("left.dshl", LEFT),
        ("right.dshl", RIGHT),
        ("between.dshl", BETWEEN),
        ("twice.dshl", TWICE),
    ]);

    assert_eq!(uses(&engine, "shared_value"), []);
}

#[test]
fn a_supports_of_a_block_the_file_declares_stays_bound() {
    let (_tree, engine) = build();

    assert_eq!(uses(&engine, "top_frame"), [("unlit", 14, 80)]);
}

#[test]
fn a_member_past_the_first_include_binds_no_read_of_its_name() {
    let (_tree, engine) = build_tree(&[
        ("holder.dshl", HOLDER),
        ("through.dshl", THROUGH),
        ("reader.dshl", READER),
    ]);

    assert_eq!(uses(&engine, "held"), []);
}

#[test]
fn a_value_the_file_declares_binds_before_one_its_includes_reach() {
    let (_tree, engine) = build_tree(&[("deep.dshl", DEEP), ("own.dshl", OWN_VALUE)]);

    assert_eq!(uses(&engine, "deep_value"), [("own", 7, 80)]);
}

#[test]
fn a_read_of_a_global_its_includer_declares_binds_it() {
    let (_tree, engine) = build_tree(&[("a.dshl", INCLUDER), ("lib.dshl", LIB)]);

    assert_eq!(uses(&engine, "up_value"), [("lit", 3, 60)]);
    assert_eq!(uses(&engine, "side_value"), [("own", 7, 80)]);
}

#[test]
fn a_name_two_includers_declare_binds_neither() {
    let (_tree, engine) = build_tree(&[
        ("a.dshl", INCLUDER),
        ("b.dshl", OTHER_INCLUDER),
        ("lib.dshl", LIB),
    ]);

    assert_eq!(uses(&engine, "up_value"), []);
}

#[test]
fn an_includer_that_declares_nothing_of_the_name_leaves_the_one_that_does() {
    let (_tree, engine) = build_tree(&[
        ("a.dshl", INCLUDER),
        ("b.dshl", INCLUDER_WITHOUT_IT),
        ("lib.dshl", LIB),
    ]);

    assert_eq!(uses(&engine, "up_value"), [("lit", 3, 60)]);
}

#[test]
fn a_file_no_script_of_the_read_holds_binds_nothing() {
    let (_tree, engine) = build_tree(&[
        ("a.dshl", INCLUDER_WITHOUT_IT),
        ("lib.dshl", LIB),
        ("apart.dshl", GLOBAL),
    ]);

    assert_eq!(uses(&engine, "up_value"), []);
}

#[test]
fn a_read_binds_a_global_many_scripts_hold_when_it_shares_one_of_them() {
    for shared in ["a.dshl", "b.dshl", "c.dshl", "d.dshl", "e.dshl"] {
        let includer = |name: &str| {
            if name == shared {
                GLOBAL_THEN_LIB
            } else {
                GLOBAL_INCLUDER
            }
        };
        let (_tree, engine) = build_tree(&[
            ("global.dshl", GLOBAL),
            ("lib.dshl", LIB),
            ("a.dshl", includer("a.dshl")),
            ("b.dshl", includer("b.dshl")),
            ("c.dshl", includer("c.dshl")),
            ("d.dshl", includer("d.dshl")),
            ("e.dshl", includer("e.dshl")),
        ]);

        assert_eq!(uses(&engine, "up_value"), [("lit", 3, 60)], "{shared}");
    }
}

#[test]
fn a_read_binds_a_global_an_includer_of_its_includer_declares() {
    let (_tree, engine) = build_tree(&[
        ("a.dshl", INCLUDER_OF_MIDDLE),
        ("pass.dshl", PASS),
        ("lib.dshl", LIB),
    ]);

    assert_eq!(uses(&engine, "up_value"), [("lit", 3, 60)]);
}

#[test]
fn a_read_binds_a_global_another_include_of_its_includer_declares() {
    let (_tree, engine) = build_tree(&[
        ("global.dshl", GLOBAL),
        ("a.dshl", GLOBAL_THEN_LIB),
        ("lib.dshl", LIB),
    ]);

    assert_eq!(uses(&engine, "up_value"), [("lit", 3, 60)]);
}

#[test]
fn a_member_of_an_includer_binds_no_read_of_its_name() {
    let (_tree, engine) = build_tree(&[("a.dshl", SHADER_WITH_THE_FIELD), ("lib.dshl", LIB)]);

    assert_eq!(uses(&engine, "up_value"), []);
}

#[test]
fn a_name_two_files_of_one_script_declare_binds_neither() {
    let (_tree, engine) = build_tree(&[
        ("left.dshl", LEFT),
        ("right.dshl", RIGHT),
        ("a.dshl", TWO_GLOBALS_THEN_LIB),
        ("shared_lib.dshl", SHARED_LIB),
    ]);

    assert_eq!(uses(&engine, "shared_value"), []);
}

#[test]
fn a_read_in_a_game_binds_the_copy_of_its_own_game() {
    let (_tree, engine) = build_tree(&[
        ("games/a/global.dshl", GLOBAL),
        ("games/a/lib.dshl", LIB),
        ("games/a/root.dshl", GLOBAL_THEN_LIB),
        ("games/b/global.dshl", GLOBAL),
        ("games/b/root.dshl", OTHER_GAME_ROOT),
    ]);

    let own = Selector::new("up_value", Some("games/a/global.dshl"));
    let other = Selector::new("up_value", Some("games/b/global.dshl"));
    assert_eq!(uses(&engine, own), [("lit", 3, 60)]);
    assert_eq!(uses(&engine, other), []);
}

#[test]
fn a_read_whose_own_includes_reach_two_copies_binds_the_copy_of_its_own_game() {
    let (_tree, engine) = build_tree(&[
        ("games/a/shaders/global.dshl", GLOBAL),
        ("games/a/shaders/lib.dshl", LIB),
        ("games/a/shaders/root.dshl", BOTH_COPIES_THEN_LIB),
        ("games/b/shaders/global.dshl", GLOBAL),
    ]);

    let own = Selector::new("up_value", Some("games/a/shaders/global.dshl"));
    assert_eq!(uses(&engine, own), [("lit", 3, 60)]);
}

#[test]
fn a_library_two_games_include_binds_neither_copy() {
    let (_tree, engine) = build_tree(&[
        ("lib/lib.dshl", LIB),
        ("games/a/global.dshl", GLOBAL),
        ("games/a/root.dshl", GLOBAL_THEN_LIB),
        ("games/b/global.dshl", GLOBAL),
        ("games/b/root.dshl", GLOBAL_THEN_LIB),
    ]);

    assert_eq!(uses(&engine, "up_value"), []);
}

#[test]
fn a_library_beside_one_game_under_one_top_directory_binds_neither_copy() {
    let (_tree, engine) = build_tree(&[
        ("prog/lib/lib.dshl", LIB),
        ("prog/game/global.dshl", GLOBAL),
        ("prog/game/root.dshl", GLOBAL_THEN_LIB),
        ("other/game/global.dshl", GLOBAL),
        ("other/game/root.dshl", GLOBAL_THEN_LIB),
    ]);

    assert_eq!(uses(&engine, "up_value"), []);
}

#[test]
fn two_copies_equally_near_the_read_bind_neither() {
    let (_tree, engine) = build_tree(&[
        ("games/a/shaders/lib.dshl", LIB),
        ("games/a/shaders/copies/left.dshl", GLOBAL),
        ("games/a/shaders/copies/right.dshl", GLOBAL),
        ("games/a/shaders/left_root.dshl", TWO_COPIES_THEN_LIB),
        ("games/a/shaders/right_root.dshl", OTHER_COPY_THEN_LIB),
    ]);

    assert_eq!(uses(&engine, "up_value"), []);
}

#[test]
fn a_read_of_an_interval_its_file_writes_stays_bound_past_a_variable_its_includer_declares() {
    let (_tree, engine) = build_tree(&[
        ("a.dshl", INCLUDER_OF_VARIABLE),
        ("ranges_lib.dshl", RANGES_LIB),
    ]);

    let (_, graph) = engine.graph_of("a.dshl").expect("DSHL claims the file");
    let interval = Selector::new("special_vision", Some("ranges_lib.dshl"));
    let references = graph
        .references(interval, SiteQuery::new(usize::MAX))
        .expect("the interval is defined");
    assert_eq!(references.symbol.kind, SymbolKind::Enum);
    let lines: Vec<u32> = references.sites.iter().map(|site| site.line).collect();
    assert_eq!(lines, [5]);
}

#[test]
fn a_macro_call_binds_the_macro_past_the_first_include() {
    let (_tree, engine) = build_tree(&[
        ("deep.dshl", DEEP_MACRO),
        ("middle.dshl", MIDDLE_MACROS),
        ("other.dshl", OTHER_MACROS),
        ("top.dshl", CALLS_PAST_THE_FIRST_INCLUDE),
    ]);

    assert_eq!(
        callers(&engine, "DEEP_M", "deep.dshl"),
        (vec![("lit", 70)], 0)
    );
    assert_eq!(callers(&engine, "DEEP_M", "other.dshl"), (vec![], 0));
    assert_eq!(
        callers(&engine, "MID_M", "middle.dshl"),
        (vec![("lit", 70)], 0)
    );
}

#[test]
fn a_macro_call_keeps_the_one_callable_no_include_of_its_file_reaches() {
    let (_tree, engine) = build_tree(&[
        ("unfollowed.dshl", UNFOLLOWED_MACRO),
        ("top.dshl", CALLS_THROUGH_AN_UNFOLLOWED_INCLUDE),
    ]);

    assert_eq!(
        callers(&engine, "UNFOLLOWED_M", "unfollowed.dshl"),
        (vec![("lit", 60)], 0)
    );
}

#[test]
fn a_macro_two_files_past_the_first_include_declare_binds_neither() {
    let (_tree, engine) = build_tree(&[
        ("lib/a/deep.dshl", DEEP_MACRO),
        ("lib/b/deep.dshl", DEEP_MACRO),
        ("left.dshl", "include \"lib/a/deep.dshl\"\n"),
        ("right.dshl", "include \"lib/b/deep.dshl\"\n"),
        ("top.dshl", INCLUDES_TWO_THAT_INCLUDE_THE_MACRO),
    ]);

    assert_eq!(callers(&engine, "DEEP_M", "lib/a/deep.dshl"), (vec![], 1));
}

#[test]
fn a_macro_call_keeps_its_game_s_copy_past_a_shared_file_s_include() {
    let (_tree, engine) = build_tree(&[
        ("games/a/shaders/top.dshl", CALLS_THROUGH_A_SHARED_FILE),
        ("games/a/shaders/own.dshl", DEEP_MACRO),
        ("libs/shaders/shared.dshl", SHARED_THAT_INCLUDES_THE_MACRO),
        ("libs/shaders/deep.dshl", DEEP_MACRO),
    ]);

    assert_eq!(
        callers(&engine, "DEEP_M", "games/a/shaders/own.dshl"),
        (vec![("lit", 50)], 0)
    );
    assert_eq!(
        callers(&engine, "DEEP_M", "libs/shaders/deep.dshl"),
        (vec![], 0)
    );
}

#[test]
fn a_macro_call_binds_the_copy_its_includes_reach_where_that_copy_is_the_nearest() {
    let (_tree, engine) = build_tree(&[
        ("games/a/shaders/top.dshl", CALLS_THROUGH_A_SHARED_FILE),
        (
            "games/a/shaders/shared.dshl",
            SHARED_THAT_INCLUDES_THE_MACRO,
        ),
        ("games/a/shaders/deep.dshl", DEEP_MACRO),
        ("games/b/shaders/deep.dshl", DEEP_MACRO),
    ]);

    assert_eq!(
        callers(&engine, "DEEP_M", "games/a/shaders/deep.dshl"),
        (vec![("lit", 70)], 0)
    );
}

#[test]
fn a_macro_call_binds_the_copy_its_includes_reach_past_two_copies_equally_near() {
    let (_tree, engine) = build_tree(&[
        ("games/a/shaders/top.dshl", CALLS_THROUGH_A_SHARED_FILE),
        ("games/a/shaders/left/own.dshl", DEEP_MACRO),
        ("games/a/shaders/right/own.dshl", DEEP_MACRO),
        ("libs/shaders/shared.dshl", SHARED_THAT_INCLUDES_THE_MACRO),
        ("libs/shaders/deep.dshl", DEEP_MACRO),
    ]);

    assert_eq!(
        callers(&engine, "DEEP_M", "libs/shaders/deep.dshl"),
        (vec![("lit", 70)], 0)
    );
}
