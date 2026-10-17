use kartta_lang::tree::preorder;
use tree_sitter::Tree;

const RECOVERY_SPILL_ROWS: usize = 3;
const CLASS_KEYS: [&[u8]; 3] = [b"class", b"struct", b"union"];
const HEAD_WORDS_MAX: usize = 4;
const DATA_GROUP_MIN: usize = 4096;

pub(crate) fn for_parse(source: &[u8]) -> Option<Vec<u8>> {
    let mut repaired: Option<Vec<u8>> = None;

    let mut judged_to = 0;

    let mut at = 0;
    while at < source.len() {
        if source[at] == b'{' && at >= judged_to {
            let scan = scan_data(source, at);
            for &(start, end) in &scan.tables {
                blank(&mut repaired.get_or_insert_with(|| source.to_vec())[start..end]);
            }
            judged_to = scan.end;
            if scan.closed {
                at = scan.end;
                continue;
            }
        }
        let blanked = match source[at] {
            b',' if opens_an_empty_slot(source, at) => Some((at, at + 1)),
            b'c' | b's' | b'u' if is_class_key(source, at) => head_macros(source, at),
            _ => None,
        };
        at = match blanked {
            Some((start, end)) => {
                blank(&mut repaired.get_or_insert_with(|| source.to_vec())[start..end]);
                end
            }
            None => at + 1,
        };
    }

    repaired
}

// constraint: tree-sitter keeps a node for each literal of an initializer, 20 to 60 bytes of tree for each byte of a
// numeric table, and a large table after a syntax error defeats its error recovery for the whole file; no definition,
// call or use is written among the literals, so the parse is spared them
struct DataScan {
    tables: Vec<(usize, usize)>,
    end: usize,
    closed: bool,
}

fn scan_data(source: &[u8], open: usize) -> DataScan {
    let mut opens = Vec::new();
    let mut closed: Vec<(usize, usize)> = Vec::new();
    let mut at = open + 1;
    while let Some(&byte) = source.get(at) {
        match byte {
            b'{' => opens.push(at),
            b'}' => {
                let Some(start) = opens.pop() else {
                    return DataScan {
                        tables: table_interiors(vec![(open, at)]),
                        end: at + 1,
                        closed: true,
                    };
                };
                while closed.last().is_some_and(|&(inner, _)| inner > start) {
                    closed.pop();
                }
                closed.push((start, at));
            }
            _ => match data_token_len(&source[at..]) {
                Some(len) => {
                    at += len;
                    continue;
                }
                None => break,
            },
        }
        at += 1;
    }
    DataScan {
        tables: table_interiors(closed),
        end: at,
        closed: false,
    }
}

fn table_interiors(mut groups: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    groups.retain(|&(open, close)| close - open >= DATA_GROUP_MIN);
    for group in &mut groups {
        group.0 += 1;
    }
    groups
}

fn data_token_len(text: &[u8]) -> Option<usize> {
    match text[0] {
        b'/' => comment_len(text),
        byte if byte.is_ascii_digit()
            || (byte == b'.' && text.get(1).is_some_and(u8::is_ascii_digit)) =>
        {
            Some(number_len(text))
        }
        byte if byte.is_ascii_whitespace() || matches!(byte, b',' | b'-' | b'+') => Some(1),
        _ => None,
    }
}

fn number_len(text: &[u8]) -> usize {
    let mut len = 0;
    while let Some(&byte) = text.get(len) {
        let separator = byte == b'\'' && text.get(len + 1).is_some_and(u8::is_ascii_alphanumeric);
        if !(is_word_byte(byte) || byte == b'.' || separator) {
            break;
        }
        len += 1;
    }
    len
}

fn comment_len(text: &[u8]) -> Option<usize> {
    match text.get(1) {
        Some(b'/') => Some(text.iter().take_while(|byte| **byte != b'\n').count()),
        Some(b'*') => Some(text[2..].windows(2).position(|pair| pair == b"*/")? + 4),
        _ => None,
    }
}

fn blank(text: &mut [u8]) {
    for byte in text.iter_mut().filter(|byte| **byte != b'\n') {
        *byte = b' ';
    }
}

fn opens_an_empty_slot(source: &[u8], comma: usize) -> bool {
    source[comma + 1..]
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b')')
}

const fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn word_len(text: &[u8]) -> usize {
    text.iter().take_while(|byte| is_word_byte(**byte)).count()
}

fn is_class_key(source: &[u8], at: usize) -> bool {
    (at == 0 || !is_word_byte(source[at - 1]))
        && CLASS_KEYS.iter().any(|key| {
            source[at..].starts_with(key)
                && source
                    .get(at + key.len())
                    .is_some_and(|next| !is_word_byte(*next))
        })
}

// constraint: an export or visibility macro between the key and the class name, `class TORRENT_EXPORT torrent : base`,
// parses as the class name and takes the head and the body down with it; a head that goes on to a body or a base list
// with upper-case words before its last name holds such macros, while `struct STAT st;` declares a variable
fn head_macros(source: &[u8], key: usize) -> Option<(usize, usize)> {
    let mut words = [(0, 0); HEAD_WORDS_MAX];
    let mut count = 0;
    let mut at = key + word_len(&source[key..]);
    loop {
        at += source[at..]
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
        let len = word_len(&source[at..]);
        if len == 0 || count == HEAD_WORDS_MAX {
            break;
        }
        words[count] = (at, at + len);
        count += 1;
        at += len;
    }

    let next = source.get(at).copied();
    let base_list = next == Some(b':') && source.get(at + 1) != Some(&b':');
    let (names, opens_a_body) = match &words[..count] {
        [head @ .., (start, end)] if &source[*start..*end] == b"final" => (head, true),
        all => (all, next == Some(b'{') || base_list),
    };
    let [macros @ .., _] = names else {
        return None;
    };
    let all_macros = macros
        .iter()
        .all(|(start, end)| is_macro_name(&source[*start..*end]));

    if !(opens_a_body && all_macros) {
        return None;
    }
    Some((macros.first()?.0, macros.last()?.1))
}

pub(crate) fn after_recovery(tree: &Tree, source: &[u8]) -> Option<Vec<u8>> {
    if !tree.root_node().has_error() {
        return None;
    }

    let spills = spilled_error_rows(tree);
    if spills.is_empty() {
        return None;
    }

    let mut repaired: Option<Vec<u8>> = None;
    let mut offset = 0;
    let mut continuation = false;

    for (row, line) in source.split(|byte| *byte == b'\n').enumerate() {
        let in_define = continuation;
        continuation = line.strip_suffix(b"\r").unwrap_or(line).ends_with(b"\\");

        if !in_define
            && !continuation
            && spills
                .iter()
                .any(|(start, end)| row >= *start && row <= *end)
            && let Some(len) = macro_invocation_len(line)
        {
            repaired.get_or_insert_with(|| source.to_vec())[offset..offset + len].fill(b' ');
        }

        offset += line.len() + 1;
    }

    repaired
}

fn spilled_error_rows(tree: &Tree) -> Vec<(usize, usize)> {
    let mut spills = Vec::new();

    preorder(tree, |node| {
        if node.is_error() {
            let (start, end) = (node.start_position().row, node.end_position().row);
            if end.saturating_sub(start) >= RECOVERY_SPILL_ROWS {
                spills.push((start, end));
            }
        }
        true
    });

    spills
}

/// Whether a name is spelled as a macro: three or more upper-case letters, digits and underscores.
pub(crate) fn is_macro_name(name: &[u8]) -> bool {
    name.len() >= 3
        && name
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn macro_invocation_len(line: &[u8]) -> Option<usize> {
    let indent = line.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let head = &line[indent..];
    if head.starts_with(b"#") || head.starts_with(b"//") {
        return None;
    }

    let name_len = head
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
        .count();
    if !is_macro_name(&head[..name_len]) {
        return None;
    }

    let rest = &head[name_len..];
    if !rest.starts_with(b"(") {
        return rest
            .iter()
            .all(u8::is_ascii_whitespace)
            .then_some(indent + name_len);
    }

    let mut depth = 0usize;
    for (at, byte) in rest.iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let end = indent + name_len + at + 1;
                    let next = line[end..].iter().find(|byte| !byte.is_ascii_whitespace());
                    return (next != Some(&b';')).then_some(end);
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use kartta_lang::Language;

    use super::*;
    use crate::{Cpp, cpp_parser};

    const EMPTY_ARGUMENT_SRC: &[u8] = br"
void run(int idx)
{
  CHECK(idx < items.size(), );
  CHECK(support != nullptr, );
}

void after_it()
{
  more();
}
";

    fn changed_offsets(repaired: &[u8], source: &[u8]) -> Vec<usize> {
        let pairs = repaired.iter().zip(source).enumerate();
        pairs
            .filter(|(_, (after, before))| after != before)
            .map(|(at, _)| at)
            .collect()
    }

    #[test]
    fn the_repair_is_length_preserving_and_lazy() {
        let clean = b"namespace ns {\nint area(int w, int h) { return w * h; }\n}\n";
        assert_eq!(for_parse(clean), None);

        let repaired = for_parse(EMPTY_ARGUMENT_SRC).expect("a repair site");
        assert_eq!(repaired.len(), EMPTY_ARGUMENT_SRC.len());

        let changed = changed_offsets(&repaired, EMPTY_ARGUMENT_SRC);
        assert_eq!(changed.len(), 2);
        assert!(
            changed.iter().all(|&at| EMPTY_ARGUMENT_SRC[at] == b','),
            "{changed:?}"
        );
    }

    #[test]
    fn the_repair_never_reaches_extracted_text() {
        const SOURCE: &[u8] = br#"
void tail(const char *s = ", )")
{
  use_it(s);
}
"#;
        assert!(for_parse(SOURCE).is_some());

        let raw = Cpp::new().extract(SOURCE);

        let signatures: Vec<Option<&str>> = raw
            .symbols
            .iter()
            .map(|symbol| symbol.signature.as_deref())
            .collect();
        assert_eq!(signatures, [Some("void tail(const char *s = \", )\")")]);
    }

    #[test]
    fn macro_repair_declines_a_local_error() {
        const SOURCE: &[u8] = br"
class Wrap
{
public:
  TONEMAP_FIELDS

  void update() { step(); }
  void close() { done(); }
};
";
        let tree = cpp_parser().parse(SOURCE, None).expect("parse");

        assert!(tree.root_node().has_error());
        assert_eq!(after_recovery(&tree, SOURCE), None);
    }

    #[test]
    fn macro_repair_spares_define_bodies_and_preserves_length() {
        const SOURCE: &[u8] = br"
class Wrap final : public Base
{
public:
#define WRAP_PARAMS(PARAM) \
  PARAM(float, speed, 0.f) \
  PARAM(float, alt, 0.f)
  void render(ContextId inst_ctx, uint32_t mask, const Point3 &view_pos, bool cuttings = true,
    bool disable_cockpit = false)
#ifndef PROJECT_X
    DAG_TS_REQUIRES(render_lock)
#endif
      ;
  void render(ContextId inst_ctx, ContextId pilots_ctx, uint32_t mask, const Point3 &view_pos,
    bool cuttings = true, const Frustum *cull_frustum = nullptr)
#ifndef PROJECT_X
    DAG_TS_REQUIRES(render_lock)
#endif
      ;
  void renderTrans(const TMatrix &vtm);
};

void free_fn() {}
";
        let tree = cpp_parser().parse(SOURCE, None).expect("parse");

        let repaired = after_recovery(&tree, SOURCE).expect("a spilled error holding a macro");

        assert_eq!(repaired.len(), SOURCE.len());
        let text = String::from_utf8_lossy(&repaired);
        let continued_body_line = "PARAM(float, speed, 0.f) \\";
        let last_body_line = "PARAM(float, alt, 0.f)";
        assert!(text.contains(continued_body_line), "{text}");
        assert!(text.contains(last_body_line), "{text}");
        let blanked = changed_offsets(&repaired, SOURCE);
        assert_eq!(blanked.len(), 2 * "DAG_TS_REQUIRES(render_lock)".len());
    }

    #[test]
    fn a_class_head_macro_is_blanked_and_nothing_else() {
        const SOURCE: &[u8] = b"class TORRENT_EXTRA_EXPORT torrent : public base {};\n\
                                struct DLL_API Foo { void run(); };\nclass JPH_EXPORT\nBar final {};\n\
                                class API_A API_B Two : x {};\nstruct STAT st;\nstruct STAT st = {};\n\
                                enum class MODE : int { A };\nclass Plain : public Base {};\n\
                                class Mixed API Name {};\n";

        let repaired = for_parse(SOURCE).expect("class heads with macros");

        assert_eq!(repaired.len(), SOURCE.len());
        let mut macros: Vec<usize> = [
            "TORRENT_EXTRA_EXPORT",
            "DLL_API",
            "JPH_EXPORT",
            "API_A",
            "API_B",
        ]
        .iter()
        .flat_map(|word| {
            let start = SOURCE
                .windows(word.len())
                .position(|window| window == word.as_bytes())
                .expect("the macro is in the source");
            start..start + word.len()
        })
        .collect();
        macros.sort_unstable();
        assert_eq!(changed_offsets(&repaired, SOURCE), macros);
        let text = String::from_utf8_lossy(&repaired);
        for kept in [
            "class                      torrent : public base",
            "struct         Foo",
            "class           \nBar final",
            "class             Two : x",
            "struct STAT st;",
            "struct STAT st = {};",
            "enum class MODE : int",
            "class Mixed API Name",
        ] {
            assert!(text.contains(kept), "{kept:?} in {text}");
        }
        assert_eq!(
            for_parse(b"class Plain : public Base {};\nstruct STAT st;\n"),
            None
        );
    }

    fn numeric_rows(rows: usize) -> String {
        let mut text = String::new();
        for row in 0..rows {
            writeln!(
                text,
                "  {{ {row}, -0.5f, 0x1F, .25e-3, +7, 1'000 }}, /* {row} */ // row"
            )
            .expect("a String takes any write");
        }
        text
    }

    #[test]
    fn a_large_numeric_table_is_blanked_inside_its_braces_and_keeps_its_lines() {
        let source = format!(
            "static const float weights[200][5] = {{\n{}}};\nint after() {{ return 1; }}\n",
            numeric_rows(200)
        );

        let repaired = for_parse(source.as_bytes()).expect("a table to blank");

        assert_eq!(repaired.len(), source.len());
        let open = source.find('{').expect("the table opens");
        let close = source.find("};").expect("the table closes");
        assert!(
            repaired[open + 1..close]
                .iter()
                .all(u8::is_ascii_whitespace)
        );
        assert_eq!(repaired[..=open], source.as_bytes()[..=open]);
        assert_eq!(repaired[close..], source.as_bytes()[close..]);
        let newlines = |text: &[u8]| -> Vec<usize> {
            text.iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .map(|(at, _)| at)
                .collect()
        };
        assert_eq!(newlines(&repaired), newlines(source.as_bytes()));
    }

    #[test]
    fn a_small_table_or_one_that_names_anything_is_parsed_as_written() {
        assert_eq!(for_parse(b"int pair[2] = { 1, 2 };\n"), None);

        let named = format!(
            "int rows[201][6] = {{\n{}  {{ LIMIT }},\n}};\n",
            numeric_rows(200)
        );
        assert_eq!(for_parse(named.as_bytes()), None);
    }

    #[test]
    fn a_large_table_inside_a_group_that_names_something_is_blanked_alone() {
        let source = format!(
            "struct Weights weights = {{ 2, {{\n{}}}, LIMIT }};\n",
            numeric_rows(200)
        );

        let repaired = for_parse(source.as_bytes()).expect("the inner table to blank");

        let open = source.find("{\n").expect("the table opens");
        let close = source.find("}, LIMIT").expect("the table closes");
        assert!(
            repaired[open + 1..close]
                .iter()
                .all(u8::is_ascii_whitespace)
        );
        assert_eq!(repaired[..=open], source.as_bytes()[..=open]);
        assert_eq!(repaired[close..], source.as_bytes()[close..]);
    }

    #[test]
    fn the_definitions_around_a_blanked_table_keep_their_lines() {
        let rows = numeric_rows(200);
        let source = format!(
            "const float filters[200][6] = {{\n{rows}}};\n\nconst int8_t ALIGN(taps[200][6], 8) = {{\n{rows}}};\n\n\
             void after(void) {{}}\n"
        );

        let raw = Cpp::new().extract(source.as_bytes());

        let lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(lines[0], ("filters", 1, 202));
        assert_eq!(lines.last(), Some(&("after", 407, 407)));
    }

    #[test]
    fn class_head_macros_on_lines_of_their_own_keep_every_line_after_them() {
        const SOURCE: &[u8] =
            b"class EXPORT_A\nEXPORT_B Widget {\n  void draw();\n};\n\nvoid after() {}\n";

        let raw = Cpp::new().extract(SOURCE);

        let lines: Vec<(&str, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line))
            .collect();
        assert_eq!(lines, [("Widget", 1), ("after", 6), ("draw", 3)]);
    }

    #[test]
    fn a_terminated_macro_call_is_an_ordinary_statement() {
        assert_eq!(macro_invocation_len(b"  CHECK(ready);"), None);
        assert_eq!(
            macro_invocation_len(b"  CHECK(ready)"),
            Some("  CHECK(ready)".len())
        );
        assert_eq!(
            macro_invocation_len(b"  CHECK(ready) static void handler()"),
            Some("  CHECK(ready)".len())
        );
    }

    #[test]
    fn only_an_upper_case_name_of_three_or_more_opens_a_macro_invocation() {
        assert_eq!(macro_invocation_len(b"  check(ready)"), None);
        assert_eq!(macro_invocation_len(b"  OK(ready)"), None);
        assert_eq!(macro_invocation_len(b"  // NOTE(me)"), None);
        assert_eq!(macro_invocation_len(b"#if DEFINED(X)"), None);
        assert_eq!(macro_invocation_len(b"  FIELDS"), Some("  FIELDS".len()));
        assert_eq!(macro_invocation_len(b"  FIELDS int x;"), None);
        assert_eq!(macro_invocation_len(b"  CHECK(open("), None);
    }
}
