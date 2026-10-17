use kartta_lang::tree::preorder;
use tree_sitter::Tree;

const RECOVERY_SPILL_ROWS: usize = 3;
const CLASS_KEYS: [&[u8]; 3] = [b"class", b"struct", b"union"];
const HEAD_WORDS_MAX: usize = 4;

pub(crate) fn for_parse(source: &[u8]) -> Option<Vec<u8>> {
    let mut repaired: Option<Vec<u8>> = None;

    for at in 0..source.len() {
        let blanked = match source[at] {
            b',' if opens_an_empty_slot(source, at) => Some((at, at + 1)),
            b'c' | b's' | b'u' if is_class_key(source, at) => head_macros(source, at),
            _ => None,
        };
        if let Some((start, end)) = blanked {
            repaired.get_or_insert_with(|| source.to_vec())[start..end].fill(b' ');
        }
    }

    repaired
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
