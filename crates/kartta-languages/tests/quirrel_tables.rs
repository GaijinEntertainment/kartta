//! What a Quirrel member call binds of a method a module table declares: the table is one value, which a call reaches
//! through the table's name or from the table's own members, so a call on another value binds the namesake method of
//! the class it holds.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};
use tempfile::TempDir;

const MANAGER: &str = "let _leaderboards = {
  function getMainLbRequest(event) {
    return event
  }
  function refresh(event) {
    return this.getMainLbRequest(event)
  }
}

let Events = class {
  function getMainLbRequest(event) {
    return _leaderboards.getMainLbRequest(event)
  }
}

let events = Events()

return {
  events
  _leaderboards
}
";

fn build(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["quirrel"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn callees<'e>(engine: &'e Engine, path: &str, name: &str) -> Vec<&'e str> {
    let (_, graph) = engine.graph_of(path).expect("a language claims the file");
    graph
        .callees(name, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| reached.symbol.qualified_name)
        .collect()
}

#[test]
fn a_member_call_on_a_value_binds_the_class_method_past_a_module_table_s_namesake() {
    let (_tree, engine) = build(&[
        ("events/eventsManager.nut", MANAGER),
        (
            "events/eSport.nut",
            "from \"eventsManager.nut\" import events\n\
             function fetchLbData(event) {\n  return events.getMainLbRequest(event)\n}\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "events/eSport.nut", "fetchLbData"),
        ["Events.getMainLbRequest"]
    );
}

#[test]
fn a_member_call_reaches_a_module_table_s_method_through_the_table_s_name_or_from_its_members() {
    let (_tree, engine) = build(&[
        ("events/eventsManager.nut", MANAGER),
        (
            "events/eSport.nut",
            "from \"eventsManager.nut\" import _leaderboards\n\
             function fetchLbData(event) {\n  return _leaderboards.getMainLbRequest(event)\n}\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "events/eSport.nut", "fetchLbData"),
        ["_leaderboards.getMainLbRequest"],
        "an importer writes the table's name"
    );
    assert_eq!(
        callees(
            &engine,
            "events/eventsManager.nut",
            "Events.getMainLbRequest"
        ),
        ["_leaderboards.getMainLbRequest"],
        "so does the table's own file"
    );
    assert_eq!(
        callees(&engine, "events/eventsManager.nut", "_leaderboards.refresh"),
        ["_leaderboards.getMainLbRequest"],
        "and a member of the table reaches its sibling through `this`"
    );
}

const POST_FX: &str = "let PostFxSettings = class {
  function setValue(name, value) {
    return value
  }
}

let g_hud_live_stats = {
  function show(doShow) {
    return doShow
  }
}

function guiStartPostfxSettings() {
  return PostFxSettings()
}

return {
  guiStartPostfxSettings
  g_hud_live_stats
}
";

#[test]
fn a_runtime_method_s_call_on_a_value_binds_no_member_of_an_imported_file() {
    let (_tree, engine) = build(&[
        ("options/postFxSettings.nut", POST_FX),
        (
            "options/optionsWnd.nut",
            "from \"postFxSettings.nut\" import guiStartPostfxSettings, g_hud_live_stats\n\
             function onGroupSelect(obj) {\n  obj.setValue(1)\n  obj.show(true)\n  \
             guiStartPostfxSettings()\n}\n\
             function onPostFx(postFxSettings) {\n  postFxSettings.setValue(1)\n  \
             g_hud_live_stats.show(true)\n}\n",
        ),
    ]);

    assert_eq!(
        callees(&engine, "options/optionsWnd.nut", "onGroupSelect"),
        ["guiStartPostfxSettings"]
    );
    assert_eq!(
        callees(&engine, "options/optionsWnd.nut", "onPostFx"),
        ["PostFxSettings.setValue", "g_hud_live_stats.show"],
        "a receiver that spells the class or the table still reaches its member"
    );
}
