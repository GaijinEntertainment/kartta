//! Where the doc comment of a TypeScript or JavaScript definition starts, above the line its extraction records.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helper of an integration test crate is neither; a failed lookup is the assertion"
)]

use kartta_lang::Language;
use kartta_lang_jsts::{JavaScript, TypeScript};

fn doc_line(language: &dyn Language, source: &str, name: &str) -> u32 {
    let raw = language.extract(source.as_bytes());
    let symbol = raw
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("the name is extracted");
    let lines: Vec<&[u8]> = source.lines().map(str::as_bytes).collect();
    language.doc_comments().first_line(&lines, symbol.line)
}

const SOURCE: &str = "let x = 1;
/** Parses. */
export function parse() {}

class Widget {
  @Input() first: string;
  @Input() second: string;
}
";

#[test]
fn a_comment_directly_above_a_definition_documents_it() {
    assert_eq!(doc_line(&TypeScript::new(), SOURCE, "parse"), 2);
}

#[test]
fn a_decorated_field_above_documents_nothing_of_the_field_below_it() {
    assert_eq!(doc_line(&TypeScript::new(), SOURCE, "second"), 7);
}

const DECORATED: &str = "/** Doc of Panel. */
@Component({
  selector: 'panel',
})
export class Panel {
  /** Doc of run. */
  @HostListener('click')
  @Throttle(50)
  run() {}

  /** Doc of size. */
  @Input()
  size: number;
}

/** Doc of Store. */
@Injectable()
class Store {}
";

#[test]
fn a_declaration_starts_at_its_first_decorator_where_its_doc_comment_ends() {
    let typescript = TypeScript::new();
    let raw = typescript.extract(DECORATED.as_bytes());
    let starts: Vec<(&str, u32)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.line))
        .collect();
    assert_eq!(
        starts,
        [("Panel", 2), ("run", 7), ("size", 12), ("Store", 17)],
        "decorators written before `export` and a member's decorators start the declaration"
    );
    for (name, doc) in [("Panel", 1), ("run", 6), ("size", 11), ("Store", 16)] {
        assert_eq!(doc_line(&typescript, DECORATED, name), doc, "{name}");
    }
}

#[test]
fn a_javascript_declaration_starts_at_its_first_decorator_too() {
    let source = "/** Doc of Panel. */\n@register\nexport class Panel {\n\
                  /** Doc of run. */\n  @bound\n  run() {}\n}\n";
    let javascript = JavaScript::new();
    assert_eq!(doc_line(&javascript, source, "Panel"), 1);
    assert_eq!(doc_line(&javascript, source, "run"), 4);
}

#[test]
fn javascript_reads_the_same_comments() {
    let source = "/**\n * Parses.\n */\nfunction parse() {}\n";
    assert_eq!(doc_line(&JavaScript::new(), source, "parse"), 1);
}
