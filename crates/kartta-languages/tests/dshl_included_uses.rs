//! What a DSHL use binds past its own file: a shader file and its includes are one script, so a read of a global and a
//! `supports` of a block bind what an included file declares, and a declaration of the file itself of any kind stays
//! bound, a shader that a `shader ==` condition names included.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
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

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("globals.dshl"), GLOBALS).expect("the file is written");
    fs::write(tree.path().join("water.dshl"), WATER).expect("the file is written");
    let languages = kartta_languages::select(["dshl"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

/// Each use site's holder, line and confidence.
fn uses<'e>(engine: &'e Engine, name: &str) -> Vec<(&'e str, u32, u8)> {
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
