//! What a Go anonymous struct's fields are owned by and what binds them: the field or the variable the struct types
//! owns them under its own name, and a member taken on a value of the struct binds the field that owner declares, or
//! one a type the struct embeds declares.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, SiteQuery};
use tempfile::TempDir;

const SERVER: &str = "package app

type Server struct {
	Limits struct {
		MaxConns int
	}
}

type Limits struct {
	MaxConns int
}

func Run(s *Server, l Limits) int {
	return s.Limits.MaxConns + l.MaxConns
}
";

const TABLES: &str = "package app

type Alias struct {
	Name string
}

func TestRun() {
	tests := []struct {
		in string
	}{{in: \"a\"}}
	for _, tt := range tests {
		_ = tt.in
	}
	{
		tests := []struct {
			want string
		}{{want: \"b\"}}
		for _, tt := range tests {
			_ = tt.want
		}
	}
	aux := &struct {
		*Alias
		Extra int
	}{}
	_ = aux.Name
}
";

fn build() -> (TempDir, Engine) {
    let tree = tempfile::tempdir().expect("a temporary directory");
    fs::write(tree.path().join("server.go"), SERVER).expect("the file is written");
    fs::write(tree.path().join("tables_test.go"), TABLES).expect("the file is written");
    let languages = kartta_languages::select(["go"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    (tree, outcome.engine)
}

fn owners<'e>(engine: &'e Engine, path: &str) -> Vec<(&'e str, Option<&'e str>)> {
    let (_, graph) = engine.graph_of(path).expect("Go claims the file");
    graph
        .outline(path)
        .expect("the file is outlined")
        .into_iter()
        .map(|definition| (definition.name, definition.owner))
        .collect()
}

fn site_lines(engine: &Engine, name: &str) -> Vec<u32> {
    let (_, graph) = engine.graph_of("server.go").expect("Go claims the file");
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    references.sites.iter().map(|site| site.line).collect()
}

#[test]
fn a_field_owns_the_fields_of_its_anonymous_struct_under_its_own_name() {
    let (_tree, engine) = build();

    assert_eq!(
        owners(&engine, "server.go"),
        [
            ("Server", None),
            ("Limits", Some("Server")),
            ("MaxConns", Some("Limits")),
            ("Limits", None),
            ("MaxConns", Some("Limits")),
            ("Run", None),
        ],
        "no symbol names the struct"
    );
    assert_eq!(site_lines(&engine, "app.Server.Limits.MaxConns"), [14]);
    assert_eq!(
        site_lines(&engine, "app.Limits.MaxConns"),
        [14],
        "the namesake type's field is not the anonymous struct's"
    );
}

#[test]
fn a_value_a_body_declares_owns_its_literal_s_struct_and_the_types_it_embeds() {
    let (_tree, engine) = build();

    assert_eq!(
        owners(&engine, "tables_test.go"),
        [
            ("Alias", None),
            ("Name", Some("Alias")),
            ("TestRun", None),
            ("tests", None),
            ("in", Some("tests")),
            ("tests", None),
            ("want", Some("tests")),
            ("aux", None),
            ("Extra", Some("aux")),
        ]
    );
    assert_eq!(site_lines(&engine, "tests.in"), [10, 12]);
    assert_eq!(
        site_lines(&engine, "tests.want"),
        [17, 19],
        "each table's elements read its own struct"
    );
    assert_eq!(
        site_lines(&engine, "app.Alias.Name"),
        [26],
        "a member the struct's embedded type declares binds through the value"
    );
}
