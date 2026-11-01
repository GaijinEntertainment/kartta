//! What `find_unused` lists of a Go field reflection reads: a marshal call reaches a struct's fields through a defined
//! type as it does through an alias, and a struct no such call reaches stays listed.

use std::fs;

use kartta::{BuildOptions, Engine};

const DEFINED: &str = "package b

import \"encoding/json\"

type S struct {
	Name string
}

type T S

func run() ([]byte, error) { return json.Marshal(T{}) }
";

const ALIASED: &str = "package b

import \"encoding/json\"

type C struct {
	Tag string
}

type U = C

func alias() ([]byte, error) { return json.Marshal(U{}) }
";

const UNREACHED: &str = "package b

type Idle struct {
	Unread string
}

type Quiet Idle
";

#[test]
fn a_reflective_call_through_a_defined_type_keeps_its_struct_s_fields_off_the_list() {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (name, source) in [
        ("go.mod", "module x\n\ngo 1.22\n"),
        ("b/a.go", DEFINED),
        ("b/c.go", ALIASED),
        ("b/idle.go", UNREACHED),
    ] {
        let path = tree.path().join(name);
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is made");
        fs::write(path, source).expect("the file is written");
    }
    let languages = kartta_languages::select(["go"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = (outcome.engine)
        .graph_of("b/a.go")
        .expect("Go claims the file");

    let dead = graph.dead_code(&[], Some("b/"), 50);

    let names: Vec<&str> = (dead.found.iter())
        .map(|dead| dead.definition.qualified_name)
        .collect();
    assert!(names.contains(&"b.Idle.Unread"), "{names:?}");
    assert!(!names.contains(&"b.S.Name"), "{names:?}");
    assert!(!names.contains(&"b.C.Tag"), "{names:?}");
}
