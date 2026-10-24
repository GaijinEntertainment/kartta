//! What a call through the receiver that names the caller's base class binds: Quirrel's `base`, daslang's `super` and
//! PHP's `parent` reach the member of the class the caller's class declares as its base, never the caller's own
//! override, with the base in the caller's file or in another, while a call on the caller's own receiver still binds
//! the override.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

/// The callees of the child's `doQuit`, which calls through the base receiver, and of its `doStay`, which calls on its
/// own receiver.
fn callees(language: &str, files: &[(&str, &str)], path: &str) -> [Vec<String>; 2] {
    reached(language, files, path).map(|reached| {
        reached
            .into_iter()
            .map(|(_, qualified_name)| qualified_name)
            .collect()
    })
}

/// [`callees`], each with the file that defines it.
fn reached(language: &str, files: &[(&str, &str)], path: &str) -> [Vec<(String, String)>; 2] {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        let path = tree.path().join(path);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select([language]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of(path)
        .expect("a language claims the file");
    ["doQuit", "doStay"].map(|name| {
        graph
            .callees(name, Limits::new(1))
            .expect("the name is defined")
            .reached
            .iter()
            .map(|reached| {
                let symbol = &reached.symbol;
                (symbol.file.to_owned(), symbol.qualified_name.to_owned())
            })
            .collect()
    })
}

const QUIRREL_BASE: &str = "class BaseMap {\n  function goBack() {}\n}\n";

const QUIRREL_CHILD: &str = "class ArtilleryMap (BaseMap) {
  function goBack() {}
  function doQuit() {
    base.goBack()
  }
  function doStay() {
    this.goBack()
  }
}
";

#[test]
fn quirrel_base_binds_the_base_class_s_member() {
    let one_file = format!("{QUIRREL_BASE}{QUIRREL_CHILD}");
    assert_eq!(
        callees(
            "quirrel",
            &[("artillery_map.nut", &one_file)],
            "artillery_map.nut"
        ),
        [["BaseMap.goBack"], ["ArtilleryMap.goBack"]]
    );

    let base = format!("{QUIRREL_BASE}return BaseMap\n");
    let child = format!("let BaseMap = require(\"base_map.nut\")\n{QUIRREL_CHILD}");
    assert_eq!(
        callees(
            "quirrel",
            &[("base_map.nut", &base), ("artillery_map.nut", &child)],
            "artillery_map.nut"
        ),
        [["BaseMap.goBack"], ["ArtilleryMap.goBack"]]
    );
}

#[test]
fn quirrel_base_binds_a_member_the_base_inherits() {
    let source = format!(
        "class Root {{\n  function goBack() {{}}\n}}\nclass BaseMap (Root) {{}}\n{QUIRREL_CHILD}"
    );
    assert_eq!(
        callees(
            "quirrel",
            &[("artillery_map.nut", &source)],
            "artillery_map.nut"
        ),
        [["Root.goBack"], ["ArtilleryMap.goBack"]]
    );
}

#[test]
fn quirrel_base_never_binds_the_caller_s_override() {
    let no_member = format!("class BaseMap {{\n  function other() {{}}\n}}\n{QUIRREL_CHILD}");
    let no_base = QUIRREL_CHILD.replace(" (BaseMap)", "");
    for (source, case) in [
        (
            no_member.as_str(),
            "a base that holds no member of the name",
        ),
        (no_base.as_str(), "a class that declares no base"),
        (QUIRREL_CHILD, "a base the graph does not hold"),
    ] {
        assert_eq!(
            callees(
                "quirrel",
                &[("artillery_map.nut", source)],
                "artillery_map.nut"
            ),
            [Vec::<String>::new(), vec!["ArtilleryMap.goBack".to_owned()]],
            "{case}"
        );
    }
}

#[test]
fn quirrel_base_of_the_class_s_own_name_is_the_other_class() {
    let base = "local handlerClass = class {\n  function goBack() {}\n}\nreturn { handlerClass }\n";
    let child = format!(
        "let vehiclesModal = require(\"vehicles.nut\")\n{}",
        QUIRREL_CHILD.replace(
            "class ArtilleryMap (BaseMap)",
            "local handlerClass = class (vehiclesModal.handlerClass)"
        )
    );
    let go_back_in = |file: &str| vec![(file.to_owned(), "handlerClass.goBack".to_owned())];
    assert_eq!(
        reached(
            "quirrel",
            &[("vehicles.nut", base), ("select.nut", &child)],
            "select.nut",
        ),
        [go_back_in("vehicles.nut"), go_back_in("select.nut")]
    );
}

#[test]
fn quirrel_base_in_a_class_without_a_name_binds_as_before() {
    let source = format!(
        "{QUIRREL_BASE}register(class (BaseMap) {{
  function doQuit() {{
    base.goBack()
  }}
  function doStay() {{}}
}})
"
    );
    assert_eq!(
        callees("quirrel", &[("map.nut", &source)], "map.nut"),
        [vec!["BaseMap.goBack".to_owned()], Vec::new()]
    );
}

const DASLANG_BASE: &str = "class BaseMap {\n  def goBack() {\n    pass\n  }\n}\n";

const DASLANG_CHILD: &str = "class ArtilleryMap : BaseMap {
  def override goBack() {
    pass
  }
  def doQuit() {
    super.goBack()
  }
  def doStay() {
    self.goBack()
  }
}
";

#[test]
fn daslang_super_binds_the_base_class_s_member() {
    let one_file = format!("options gen2\n{DASLANG_BASE}{DASLANG_CHILD}");
    assert_eq!(
        callees(
            "daslang",
            &[("artillery_map.das", &one_file)],
            "artillery_map.das"
        ),
        [["BaseMap.goBack"], ["ArtilleryMap.goBack"]]
    );

    let base = format!("options gen2\n{DASLANG_BASE}");
    let child = format!("options gen2\nrequire base_map\n{DASLANG_CHILD}");
    assert_eq!(
        callees(
            "daslang",
            &[("base_map.das", &base), ("artillery_map.das", &child)],
            "artillery_map.das"
        ),
        [["BaseMap.goBack"], ["ArtilleryMap.goBack"]]
    );
}

#[test]
fn daslang_super_binds_a_member_the_base_inherits() {
    let source = format!(
        "options gen2\nclass Root {{\n  def goBack() {{\n    pass\n  }}\n}}\nclass BaseMap : Root {{}}\n{DASLANG_CHILD}"
    );
    assert_eq!(
        callees(
            "daslang",
            &[("artillery_map.das", &source)],
            "artillery_map.das"
        ),
        [["Root.goBack"], ["ArtilleryMap.goBack"]]
    );
}

#[test]
fn daslang_super_never_binds_the_caller_s_override() {
    let source = format!("options gen2\n{DASLANG_CHILD}");
    assert_eq!(
        callees(
            "daslang",
            &[("artillery_map.das", &source)],
            "artillery_map.das"
        ),
        [Vec::<String>::new(), vec!["ArtilleryMap.goBack".to_owned()]]
    );
}

const PHP_BASE: &str = "class BaseMap {\n  public function goBack() {}\n}\n";

const PHP_CHILD: &str = "class ArtilleryMap extends BaseMap {
  public function goBack() {}
  public function doQuit() {
    parent::goBack();
  }
  public function doStay() {
    $this->goBack();
  }
}
";

#[test]
fn php_parent_binds_the_base_class_s_member() {
    let one_file = format!("<?php\n{PHP_BASE}{PHP_CHILD}");
    assert_eq!(
        callees(
            "php",
            &[("ArtilleryMap.php", &one_file)],
            "ArtilleryMap.php"
        ),
        [["BaseMap::goBack"], ["ArtilleryMap::goBack"]]
    );

    let base = format!("<?php\n{PHP_BASE}");
    let child = format!("<?php\n{PHP_CHILD}");
    assert_eq!(
        callees(
            "php",
            &[("BaseMap.php", &base), ("ArtilleryMap.php", &child)],
            "ArtilleryMap.php"
        ),
        [["BaseMap::goBack"], ["ArtilleryMap::goBack"]]
    );
}

#[test]
fn php_parent_never_binds_the_caller_s_override() {
    let source = format!("<?php\n{PHP_CHILD}");
    assert_eq!(
        callees("php", &[("ArtilleryMap.php", &source)], "ArtilleryMap.php"),
        [
            Vec::<String>::new(),
            vec!["ArtilleryMap::goBack".to_owned()]
        ]
    );
}

#[test]
fn php_parent_binds_a_trait_of_the_base_before_its_base_and_never_an_interface() {
    let source = format!(
        "<?php
interface Navigates {{ public function goBack(); }}
trait GoesBack {{ public function goBack() {{}} }}
class Root {{ public function goBack() {{}} }}
class BaseMap extends Root implements Navigates {{ use GoesBack; }}
{PHP_CHILD}"
    );
    assert_eq!(
        callees("php", &[("ArtilleryMap.php", &source)], "ArtilleryMap.php"),
        [["GoesBack::goBack"], ["ArtilleryMap::goBack"]]
    );

    let source = format!(
        "<?php\ninterface BaseMap {{ public function goBack(); }}\n{}",
        PHP_CHILD.replace("extends", "implements")
    );
    assert_eq!(
        callees("php", &[("ArtilleryMap.php", &source)], "ArtilleryMap.php"),
        [
            Vec::<String>::new(),
            vec!["ArtilleryMap::goBack".to_owned()]
        ]
    );
}

#[test]
fn php_parent_imported_from_outside_binds_no_namesake() {
    let child = format!("<?php\nnamespace Maps;\n\nuse BaseMap;\n\n{PHP_CHILD}");
    let namesake = format!("<?php\nnamespace Testing;\n\n{PHP_BASE}");
    assert_eq!(
        callees(
            "php",
            &[
                ("src/Maps/ArtilleryMap.php", &child),
                ("src/Testing/BaseMap.php", &namesake)
            ],
            "src/Maps/ArtilleryMap.php"
        ),
        [
            Vec::<String>::new(),
            vec!["Maps\\ArtilleryMap::goBack".to_owned()]
        ]
    );
}
