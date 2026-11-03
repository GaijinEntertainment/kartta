//! How the symbol tools read a qualified name written with the owner separator of another language: Rust's and C++'s
//! `::` written as `.`, and Go's `.` written as `::`; one whose last part holds a separator, as an outline prints it;
//! and one a typo keeps from every definition.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test), so the client's helpers fail the test by a failed expectation"
)]

#[expect(
    dead_code,
    reason = "the helpers serve every test crate, and this one calls some of them"
)]
mod common;

use serde_json::{Value, json};

use common::{Client, tree};

/// A Rust crate, a C++ library, a Go package, a Python module and a JavaScript class, the last named as the Rust type
/// is, so `Index.refresh` is the JavaScript method as written and the Rust one read with `::`.
fn indexed() -> (tempfile::TempDir, Client) {
    let tree = tree(&[
        (
            "Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub mod render {\n    pub trait Paint {\n        fn paint(&self);\n    }\n}\n\npub struct Index;\n\n\
             impl Index {\n    pub fn refresh(&self) {\n        helper();\n    }\n\n    pub fn rebuild(&self) {\n\
             \x20       self.refresh();\n    }\n}\n\nimpl render::Paint for Index {\n    fn paint(&self) {}\n}\n\n\
             fn helper() {}\n",
        ),
        (
            "ui/widget.h",
            "namespace ui {\n\nclass Base {\npublic:\n    virtual void draw();\n};\n\n\
             class Widget : public Base {\npublic:\n    void draw() override;\n    void paint();\n};\n\n}\n",
        ),
        (
            "ui/widget.cpp",
            "#include \"widget.h\"\n\nnamespace ui {\n\nstatic void helper() {}\n\nvoid Widget::draw() {\n\
             \x20   helper();\n}\n\nvoid Widget::paint() {\n    draw();\n}\n\n}\n",
        ),
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "pkg/service.go",
            "package pkg\n\ntype Service struct{}\n\nfunc New() *Service {\n\treturn &Service{}\n}\n\n\
             func (s *Service) Greet(name string) string {\n\treturn \"hi \" + name\n}\n\n\
             func (s *Service) Welcome() string {\n\treturn s.Greet(\"x\")\n}\n",
        ),
        (
            "shapes.py",
            "class Shape:\n    def area(self):\n        return 0\n\n    def report(self):\n\
             \x20       return self.area()\n",
        ),
        (
            "web/index.js",
            "export class Index {\n  refresh() {\n    return 1;\n  }\n\n  rebuild() {\n    return this.refresh();\n\
             \x20 }\n}\n",
        ),
    ]);
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());
    (tree, client)
}

/// `answer` with its first `key="read"` spelled as the call wrote the name, `written`, and the reading named after it.
fn read_as(answer: &str, key: &str, written: &str, read: &str) -> String {
    let as_read = format!("{key}={read:?}");
    assert!(answer.contains(&as_read), "{answer}");
    let suffix = if key == "of" {
        String::new()
    } else {
        format!("{key}_")
    };
    answer.replacen(
        &as_read,
        &format!("{key}={written:?} {suffix}read_as={read:?}"),
        1,
    )
}

/// The calls of each tool that takes a symbol's name, by `name`, in `languages`.
fn by_name(name: &str, languages: &[&str]) -> [(&'static str, Value); 4] {
    [
        (
            "find_callers",
            json!({"name": name, "languages": languages}),
        ),
        (
            "find_callees",
            json!({"name": name, "languages": languages}),
        ),
        (
            "find_references",
            json!({"name": name, "languages": languages}),
        ),
        (
            "read_definitions",
            json!({"names": [name], "languages": languages}),
        ),
    ]
}

/// Asserts that every symbol tool answers `written` as it answers `read`, the spelling of the language's own
/// separator, and says it read the name so; `read` itself must select a symbol.
fn assert_read_as(client: &mut Client, languages: &[&str], [written, read]: [&str; 2]) {
    for ((tool, written_call), (_, read_call)) in by_name(written, languages)
        .into_iter()
        .zip(by_name(read, languages))
    {
        let expected = client.answer(tool, &read_call);
        assert!(
            !expected.contains(r#"matches="0""#) && !expected.contains(r#"total="0""#),
            "{expected}"
        );
        assert_eq!(
            client.answer(tool, &written_call),
            read_as(&expected, "of", written, read),
            "{tool}"
        );
    }
}

#[test]
fn a_rust_name_written_with_dots_is_read_with_colons_by_every_symbol_tool() {
    let (_tree, mut client) = indexed();

    assert_read_as(&mut client, &["rust"], ["Index.refresh", "Index::refresh"]);
    assert_read_as(
        &mut client,
        &["rust"],
        ["render.Paint.paint", "render::Paint::paint"],
    );
    let path = client.answer(
        "find_call_path",
        &json!({"from": "Index::rebuild", "to": "Index::refresh", "languages": ["rust"]}),
    );
    assert!(path.contains(r#"hops="1""#), "{path}");
    let path = read_as(&path, "from", "Index.rebuild", "Index::rebuild");
    assert_eq!(
        client.answer(
            "find_call_path",
            &json!({"from": "Index.rebuild", "to": "Index.refresh", "languages": ["rust"]}),
        ),
        read_as(&path, "to", "Index.refresh", "Index::refresh")
    );
    let implementations = client.answer(
        "find_implementations",
        &json!({"name": "render::Paint", "languages": ["rust"]}),
    );
    assert!(
        implementations.contains(r#"<type name="Index""#),
        "{implementations}"
    );
    assert_eq!(
        client.answer(
            "find_implementations",
            &json!({"name": "render.Paint", "languages": ["rust"]}),
        ),
        read_as(&implementations, "of", "render.Paint", "render::Paint")
    );
    let symbols = client.answer(
        "find_symbols",
        &json!({"query": "Index.refresh", "languages": ["rust"], "limit": 1}),
    );
    assert!(symbols.contains(r#"qn="Index::refresh""#), "{symbols}");
}

#[test]
fn a_cpp_name_written_with_dots_is_read_with_colons_by_every_symbol_tool() {
    let (_tree, mut client) = indexed();

    assert_read_as(&mut client, &["cpp"], ["Widget.draw", "Widget::draw"]);
    assert_read_as(
        &mut client,
        &["cpp"],
        ["ui.Widget.draw", "ui::Widget::draw"],
    );
    let path = client.answer(
        "find_call_path",
        &json!({"from": "Widget::paint", "to": "Widget::draw", "languages": ["cpp"]}),
    );
    assert!(path.contains(r#"hops="1""#), "{path}");
    assert_eq!(
        client.answer(
            "find_call_path",
            &json!({"from": "Widget.paint", "to": "Widget::draw", "languages": ["cpp"]}),
        ),
        read_as(&path, "from", "Widget.paint", "Widget::paint")
    );
    let implementations = client.answer(
        "find_implementations",
        &json!({"name": "ui::Base", "languages": ["cpp"]}),
    );
    assert!(
        implementations.contains(r#"<type name="Widget""#),
        "{implementations}"
    );
    assert_eq!(
        client.answer(
            "find_implementations",
            &json!({"name": "ui.Base", "languages": ["cpp"]}),
        ),
        read_as(&implementations, "of", "ui.Base", "ui::Base")
    );
    let symbols = client.answer(
        "find_symbols",
        &json!({"query": "Widget.draw", "languages": ["cpp"], "limit": 1}),
    );
    assert!(symbols.contains(r#"qn="Widget::draw""#), "{symbols}");
}

#[test]
fn a_go_name_written_with_colons_is_read_with_dots() {
    let (_tree, mut client) = indexed();

    assert_read_as(&mut client, &["go"], ["Service::Greet", "Service.Greet"]);
    assert_read_as(
        &mut client,
        &["go"],
        ["pkg::Service::Greet", "pkg.Service.Greet"],
    );
}

#[test]
fn a_name_that_selects_a_symbol_as_written_is_not_read_another_way() {
    let (_tree, mut client) = indexed();

    for (name, languages) in [
        ("Index.refresh", None),
        ("Service.Greet", None),
        ("pkg.New", Some(["go"])),
        ("shapes.Shape.area", None),
        ("shapes.Shape.area", Some(["python"])),
        ("Index.refresh", Some(["jsts"])),
    ] {
        for (tool, mut call) in by_name(name, &[]) {
            match languages {
                Some(languages) => call["languages"] = json!(languages),
                None => {
                    call.as_object_mut()
                        .expect("a call is an object")
                        .remove("languages");
                }
            }
            let answer = client.answer(tool, &call);
            assert!(!answer.contains("read_as="), "{tool} {call}: {answer}");
            assert!(
                !answer.contains(r#"matches="0""#) && !answer.contains(r#"total="0""#),
                "{tool} {call}: {answer}"
            );
            assert!(
                !answer.contains(r#"lang="rust""#),
                "{tool} {call}: {answer}"
            );
        }
    }
}

#[test]
fn a_name_is_read_another_way_when_it_selects_nothing_in_the_files_a_path_prefix_keeps() {
    let (_tree, mut client) = indexed();

    for tool in ["find_callers", "find_callees", "find_references"] {
        let under = |prefix: &str| json!({"name": "Index.refresh", "path_prefix": prefix});
        let read = client.answer(tool, &under("src/"));
        assert!(
            read.contains(r#"read_as="Index::refresh""#) && read.contains(r#"lang="rust""#),
            "{tool}: {read}"
        );
        let written = client.answer(tool, &under("web/"));
        assert!(
            !written.contains("read_as=") && written.contains(r#"lang="jsts""#),
            "{tool}: {written}"
        );
        let elsewhere = client.answer(tool, &under("ui/"));
        assert!(
            !elsewhere.contains("read_as=")
                && elsewhere.contains("no definition of this name is under path_prefix")
                && elsewhere.contains(r#"qn="Index.refresh""#),
            "{tool}: {elsewhere}"
        );
    }
}

#[test]
fn a_name_neither_separator_selects_answers_with_close_alternatives() {
    let (_tree, mut client) = indexed();

    for (tool, call) in by_name("Index.refrsh", &["rust"]) {
        let answer = client.answer(tool, &call);
        assert!(!answer.contains("read_as="), "{tool}: {answer}");
        assert!(
            answer.contains("no symbol matches - did you mean:")
                && answer.contains(r#"<suggestion name="refresh" qn="Index::refresh""#),
            "{tool}: {answer}"
        );
    }
}

#[test]
fn a_qualified_name_whose_last_part_holds_a_separator_is_asked_back_as_the_outline_prints_it() {
    let tree = tree(&[(
        "src/kartta.ts",
        "export class Kartta {\n  #handle: number | undefined;\n\n  close(): void {\n    const handle = this.#handle;\n\
         \x20   this.#handle = undefined;\n  }\n}\n",
    )]);
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());

    let outline = client.answer("file_outline", &json!({"file": "src/kartta.ts"}));
    let references = client.answer("find_references", &json!({"name": "Kartta.#handle"}));
    let read = client.answer("read_definitions", &json!({"names": ["Kartta.#handle"]}));
    let misspelled = client.answer("find_references", &json!({"name": "Kartta.#handel"}));

    assert!(
        outline.contains(r##"name="#handle" qn="Kartta.#handle""##),
        "{outline}"
    );
    assert!(
        references.contains(r##"of="#handle" qn="Kartta.#handle""##)
            && references.contains(r#"reads="1" writes="1""#),
        "{references}"
    );
    assert!(
        read.contains(r#"total="1""#) && read.contains("#handle: number | undefined;"),
        "{read}"
    );
    assert!(
        misspelled.contains("no symbol matches - did you mean:")
            && misspelled.contains(r##"<suggestion name="#handle" qn="Kartta.#handle""##),
        "{misspelled}"
    );
}

/// A Go function `handle` and the private fields `#handle`, `#cache` and `#instance` of a TypeScript class.
fn misspellable() -> (tempfile::TempDir, Client) {
    let tree = tree(&[
        ("go.mod", "module example.com/walk\n\ngo 1.22\n"),
        ("walk.go", "package walk\n\nfunc handle() {}\n"),
        (
            "src/kartta.ts",
            "export class Kartta {\n  #handle: number | undefined;\n  #cache = new Map<string, number>();\n\
             \x20 #instance: Kartta | undefined;\n\n  close(): void {\n    this.#handle = undefined;\n\
             \x20   this.#cache.clear();\n    this.#instance = undefined;\n  }\n}\n",
        ),
    ]);
    let mut client = Client::unwatched(&[]);
    client.index(tree.path());
    (tree, client)
}

#[test]
fn a_name_one_character_from_a_definition_suggests_that_definition_first() {
    let (_tree, mut client) = misspellable();

    for (name, meant) in [
        (
            "#handel",
            r##"<suggestion name="#handle" qn="Kartta.#handle""##,
        ),
        (
            "#cahce",
            r##"<suggestion name="#cache" qn="Kartta.#cache""##,
        ),
        (
            "#insatnce",
            r##"<suggestion name="#instance" qn="Kartta.#instance""##,
        ),
    ] {
        let answer = client.answer("find_references", &json!({"name": name}));
        assert!(
            answer.contains(r#"matches="0" one_edit="1">"#)
                && answer.contains(&format!("no symbol matches - did you mean:\n{meant}")),
            "{name}: {answer}"
        );
    }
    let read = client.answer("read_definitions", &json!({"names": ["hadnle"]}));
    assert!(
        read.contains(r#"<name of="hadnle" total="0" one_edit="1">"#)
            && read.contains(r#"<suggestion name="handle" qn="walk.handle""#),
        "{read}"
    );
    let found = client.answer("find_symbols", &json!({"query": "hadnle"}));
    assert!(
        found.contains(r#"unmatched="hadnle" one_edit="1">"#)
            && found.contains(r#"<suggestion name="handle" qn="walk.handle""#),
        "{found}"
    );
    let owned = client.answer("find_symbols", &json!({"query": "Kartta.#handel"}));
    let suggested = owned.find(r##"<suggestion name="#handle" qn="Kartta.#handle""##);
    let hit = owned.find("<hit ");
    assert!(
        owned.contains(r#"unmatched="handel" one_edit="1">"#)
            && suggested
                .zip(hit)
                .is_some_and(|(suggested, hit)| suggested < hit),
        "{owned}"
    );
}

#[test]
fn a_call_path_whose_ends_select_in_different_graphs_names_each_ones_languages() {
    let (_tree, mut client) = indexed();

    assert_eq!(
        client.answer("find_call_path", &json!({"from": "Greet", "to": "paint"})),
        "<call_path from=\"Greet\" to=\"paint\" matches=\"0\">no graph holds both a symbol from selects and one to \
         selects: from selects in go, to in cpp, rust - a call path runs inside one language's graph; find_symbols \
         can pin the exact names</call_path>"
    );
}

#[test]
fn a_blank_name_or_query_is_a_wrong_call() {
    let (_tree, mut client) = indexed();
    let named = "is blank - pass a symbol's name; find_symbols finds one from a word of it";

    for blank in ["", " \t"] {
        for (tool, arguments, refusal) in [
            (
                "find_symbols",
                json!({"query": blank}),
                "query is blank - pass a word of a symbol's name or of its file's path".to_owned(),
            ),
            (
                "find_callers",
                json!({"name": blank}),
                format!("name {named}"),
            ),
            (
                "find_callees",
                json!({"name": blank}),
                format!("name {named}"),
            ),
            (
                "find_references",
                json!({"name": blank}),
                format!("name {named}"),
            ),
            (
                "find_implementations",
                json!({"name": blank}),
                format!("name {named}"),
            ),
            (
                "find_call_path",
                json!({"from": blank, "to": "helper"}),
                format!("from {named}"),
            ),
            (
                "find_call_path",
                json!({"from": "helper", "to": blank}),
                format!("to {named}"),
            ),
            (
                "read_definitions",
                json!({"names": ["helper", blank]}),
                "names must not contain a blank name".to_owned(),
            ),
        ] {
            let answer = client.call(tool, &arguments);
            assert!(answer.is_error, "{tool} {arguments}: {}", answer.text);
            assert_eq!(answer.text, refusal, "{tool} {arguments}");
        }
    }
}
