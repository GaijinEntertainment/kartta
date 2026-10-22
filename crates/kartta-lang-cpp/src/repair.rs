use std::ops::Range;

use kartta_lang::tree::preorder;
use tree_sitter::Tree;

const RECOVERY_SPILL_ROWS: usize = 3;
const CLASS_KEYS: [&[u8]; 3] = [b"class", b"struct", b"union"];
const HEAD_WORDS_MAX: usize = 4;
const HEAD_GROUP_MAX: usize = 256;
const HEAD_ATTRIBUTES: [&[u8]; 4] = [b"alignas", b"_Alignas", b"__declspec", b"__attribute__"];
const DATA_GROUP_MIN: usize = 4096;
const GENERATED_PARSER_MARKS: [&[u8]; 2] = [b"tree_sitter/parser.h", b"#define LANGUAGE_VERSION "];
const RAW_DELIMITER_MAX: usize = 16;

pub(crate) fn for_parse(source: &[u8]) -> Option<Vec<u8>> {
    let mut repaired: Option<Vec<u8>> = None;

    let tables = Tables::of(source);
    let mut judged_to = 0;

    let mut at = 0;
    while at < source.len() {
        if source[at] == b'{' && at >= judged_to {
            let scan = scan_data(source, at, tables);
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
// call or use is written among the literals, so the parse is spared them. The tables tree-sitter generates for a
// grammar's parser call the parser's macros and name the grammar's enumerators, neither a definition a graph holds; the
// one name they spell that a graph defines is the parser's state type, which older generators cast to in lexer modes
struct DataScan {
    tables: Vec<(usize, usize)>,
    end: usize,
    closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tables {
    Numeric,
    GeneratedParser,
}

impl Tables {
    fn of(source: &[u8]) -> Self {
        let head = source
            .split(|byte| *byte == b'{')
            .next()
            .unwrap_or_default();
        let generated = GENERATED_PARSER_MARKS
            .iter()
            .all(|mark| head.windows(mark.len()).any(|window| window == *mark));
        if generated {
            Self::GeneratedParser
        } else {
            Self::Numeric
        }
    }
}

fn scan_data(source: &[u8], open: usize, tables: Tables) -> DataScan {
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
            _ => match data_token_len(&source[at..], tables) {
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

fn data_token_len(text: &[u8], tables: Tables) -> Option<usize> {
    match text[0] {
        b'/' => comment_len(text),
        byte if byte.is_ascii_digit()
            || (byte == b'.' && text.get(1).is_some_and(u8::is_ascii_digit)) =>
        {
            Some(number_len(text))
        }
        byte if byte.is_ascii_whitespace() || matches!(byte, b',' | b'-' | b'+') => Some(1),
        _ if tables == Tables::Numeric => None,
        quote @ (b'"' | b'\'') => quoted_len(text, quote),
        byte if is_word_byte(byte) => Some(word_len(text)),
        b'(' | b')' | b'[' | b']' | b'=' | b'.' => Some(1),
        _ => None,
    }
}

fn quoted_len(text: &[u8], quote: u8) -> Option<usize> {
    let mut at = 1;
    while let Some(&byte) = text.get(at) {
        match byte {
            b'\\' => at += 2,
            b'\n' => return None,
            byte if byte == quote => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeadWord {
    Name,
    Macro,
    MacroCall,
    Attribute,
}

// constraint: an export, visibility or alignment macro between the key and the class name, `class TORRENT_EXPORT
// torrent : base` or `struct DECLSPEC_ALIGN(16) N {`, parses as the class name and takes the head and the body down
// with it; a head that goes on to a body or a base list with such macros, and the attributes the grammar reads, before
// its last name holds them, while `struct STAT st;` declares a variable and `class DECLARE(Foo) {}` names no class
fn head_macros(source: &[u8], key: usize) -> Option<(usize, usize)> {
    let mut words = [(0, 0, HeadWord::Name); HEAD_WORDS_MAX];
    let mut count = 0;
    let mut at = key + word_len(&source[key..]);
    while count < HEAD_WORDS_MAX {
        at += whitespace_len(&source[at..]);
        let Some((end, kind)) = head_word(source, at) else {
            break;
        };
        words[count] = (at, end, kind);
        count += 1;
        at = end;
    }

    let next = source.get(at).copied();
    let base_list = next == Some(b':') && source.get(at + 1) != Some(&b':');
    let (names, opens_a_body) = match &words[..count] {
        [head @ .., (start, end, HeadWord::Name)] if &source[*start..*end] == b"final" => {
            (head, true)
        }
        all => (all, next == Some(b'{') || base_list),
    };
    let [prefix @ .., (_, _, HeadWord::Name | HeadWord::Macro)] = names else {
        return None;
    };
    let mut macros = prefix
        .iter()
        .filter(|(_, _, kind)| matches!(kind, HeadWord::Macro | HeadWord::MacroCall));
    let prefix_reads = prefix.iter().all(|(_, _, kind)| *kind != HeadWord::Name);

    if !(opens_a_body && prefix_reads) {
        return None;
    }
    let first = macros.next()?;
    Some((first.0, macros.next_back().unwrap_or(first).1))
}

fn head_word(source: &[u8], at: usize) -> Option<(usize, HeadWord)> {
    if source[at..].starts_with(b"[[") {
        let close = bounded_find(&source[at..], b"]]")?;
        return Some((at + close + 2, HeadWord::Attribute));
    }
    let len = word_len(&source[at..]);
    if len == 0 {
        return None;
    }
    let word = &source[at..at + len];
    let open = at + len + whitespace_len(&source[at + len..]);
    if source.get(open) != Some(&b'(') {
        let kind = if is_macro_name(word) {
            HeadWord::Macro
        } else {
            HeadWord::Name
        };
        return Some((at + len, kind));
    }
    let end = open + group_len(&source[open..])?;
    let kind = if is_macro_name(word) {
        HeadWord::MacroCall
    } else if HEAD_ATTRIBUTES.contains(&word) {
        HeadWord::Attribute
    } else {
        return None;
    };
    Some((end, kind))
}

fn whitespace_len(text: &[u8]) -> usize {
    text.iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count()
}

fn group_len(text: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    for (at, byte) in text.iter().take(HEAD_GROUP_MAX).enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            b'{' | b'}' | b';' => return None,
            _ => {}
        }
    }
    None
}

fn bounded_find(text: &[u8], needle: &[u8]) -> Option<usize> {
    text[..text.len().min(HEAD_GROUP_MAX)]
        .windows(needle.len())
        .position(|window| window == needle)
}

pub(crate) fn after_recovery(tree: &Tree, source: &[u8]) -> Option<Vec<u8>> {
    if !tree.root_node().has_error() {
        return None;
    }

    let Recovery {
        mut spills,
        misread_branch,
    } = recovery_of(tree, source);
    let mut repaired: Option<Vec<u8>> = None;
    if misread_branch {
        for directives in untaken_branches(source) {
            blank(&mut repaired.get_or_insert_with(|| source.to_vec())[directives]);
        }
    }
    if spills.is_empty() {
        return repaired;
    }
    let lines: Vec<&[u8]> = source.split(|byte| *byte == b'\n').collect();
    for spill in &mut spills {
        spill.0 = lead_in(&lines, spill.0);
    }

    let mut offset = 0;
    let mut continuation = false;

    for (row, line) in lines.into_iter().enumerate() {
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

struct Recovery {
    spills: Vec<(usize, usize)>,
    misread_branch: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Directive {
    Opens,
    Branches,
    Closes,
    Other,
}

fn directive_of(text: &[u8]) -> Directive {
    let text = text.strip_prefix(b"#").unwrap_or(text);
    let text = &text[indent_len(text)..];
    match &text[..word_len(text)] {
        b"if" | b"ifdef" | b"ifndef" => Directive::Opens,
        b"elif" | b"else" | b"elifdef" | b"elifndef" => Directive::Branches,
        b"endif" => Directive::Closes,
        _ => Directive::Other,
    }
}

fn indent_len(text: &[u8]) -> usize {
    text.iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

// constraint: the grammar reads a conditional's branches as runs of whole declarations or statements, and an `#else`
// or `#elif` that follows an unclosed brace of the first branch, `#ifdef _XBOX ... {` `#else ... if (x) {` `#endif`,
// parses as an unknown directive inside that brace's block; the later branch's code then joins the first's, its brace
// opens a second block that the code after `#endif` never closes, and the definition runs on to the end of the file.
// Where the first tree shows that misreading, every conditional whose first branch leaves a brace open, or closes one
// it did not open, is read as the preprocessor reads it with the first branch taken: its directives and its later
// branches are blanked. Every such conditional of the file, not only the misread one, because blanking one changes how
// the grammar reads the next
fn untaken_branches(source: &[u8]) -> Vec<Range<usize>> {
    let mut scan = ConditionalScan::default();
    let mut at = 0;
    while let Some(&byte) = source.get(at) {
        if (at == 0 || source[at - 1] == b'\n')
            && let Some(directive) = directive_at(source, at)
        {
            let end = logical_line_end(source, at);
            scan.directive(directive, at..end);
            at = end;
            continue;
        }
        at += match byte {
            b'/' => match source.get(at + 1) {
                Some(b'/') => logical_line_end(source, at) - at,
                Some(b'*') => comment_len(&source[at..]).unwrap_or(source.len() - at),
                _ => 1,
            },
            b'"' if source[..at].ends_with(b"R") => raw_string_len(&source[at..])
                .or_else(|| quoted_len(&source[at..], byte))
                .unwrap_or(1),
            b'"' | b'\'' => quoted_len(&source[at..], byte).unwrap_or(1),
            b'{' | b'}' => {
                scan.brace(if byte == b'{' { 1 } else { -1 });
                1
            }
            _ => 1,
        };
    }
    scan.untaken
}

fn raw_string_len(text: &[u8]) -> Option<usize> {
    let open = text
        .iter()
        .take(RAW_DELIMITER_MAX + 2)
        .position(|byte| *byte == b'(')?;
    let delimiter = &text[1..open];
    let close = text[open..]
        .windows(delimiter.len() + 2)
        .position(|window| {
            window[0] == b')' && window[1..].starts_with(delimiter) && window.ends_with(b"\"")
        });
    Some(close.map_or(text.len(), |close| open + close + delimiter.len() + 2))
}

fn directive_at(source: &[u8], line: usize) -> Option<Directive> {
    let text = &source[line..];
    text[indent_len(text)..]
        .strip_prefix(b"#")
        .map(directive_of)
}

fn logical_line_end(source: &[u8], from: usize) -> usize {
    let mut end = from;
    loop {
        end += source[end..]
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(source.len() - end);
        let line = source[..end].strip_suffix(b"\r").unwrap_or(&source[..end]);
        if end == source.len() || !line.ends_with(b"\\") {
            return end;
        }
        end += 1;
    }
}

#[derive(Default)]
struct ConditionalScan {
    open: Vec<OpenConditional>,
    untaken: Vec<Range<usize>>,
}

struct OpenConditional {
    head: Range<usize>,
    later: Option<usize>,
    depth: i32,
    closes_unopened: bool,
}

impl ConditionalScan {
    fn directive(&mut self, directive: Directive, line: Range<usize>) {
        match directive {
            Directive::Opens => self.open.push(OpenConditional {
                head: line,
                later: None,
                depth: 0,
                closes_unopened: false,
            }),
            Directive::Branches => {
                if let Some(conditional) = self.open.last_mut() {
                    conditional.later.get_or_insert(line.start);
                }
            }
            Directive::Closes => {
                if let Some(OpenConditional {
                    head,
                    later: Some(later),
                    depth,
                    closes_unopened,
                }) = self.open.pop()
                    && (depth != 0 || closes_unopened)
                {
                    self.untaken.extend([head, later..line.end]);
                }
            }
            Directive::Other => {}
        }
    }

    fn brace(&mut self, step: i32) {
        let in_first_branch = self
            .open
            .iter_mut()
            .rev()
            .take_while(|conditional| conditional.later.is_none());
        for conditional in in_first_branch {
            conditional.depth += step;
            conditional.closes_unopened |= conditional.depth < 0;
        }
    }
}

// constraint: a bare macro on a line of its own that opens a scope, `JPH_NAMESPACE_BEGIN` above a class, joins the
// declaration after it, and the error tree-sitter recovers into starts at that declaration, past blank lines and
// comments, rather than at the macro. Only a template or a class head takes no type before it: above a function, the
// macro reads as its return type, as `NAMESPACE_END` above `STDAPI_(bool) f()` does once the macro call is blanked.
// A macro call above an error, as the `TABLE_END(name)` that closes a table an earlier line opens, is no such macro,
// and blanking it unbalances what it closes
fn lead_in(lines: &[&[u8]], start: usize) -> usize {
    let opens_a_type = lines.get(start).is_some_and(|line| {
        let text = line.trim_ascii_start();
        let word = &text[..word_len(text)];
        word == b"template" || CLASS_KEYS.contains(&word)
    });
    if !opens_a_type {
        return start;
    }
    let mut row = start;
    while let Some(line) = row.checked_sub(1).and_then(|above| lines.get(above)) {
        let text = line.trim_ascii();
        if !(text.is_empty() || text.starts_with(b"//") || is_macro_name(text)) {
            break;
        }
        row -= 1;
    }
    row
}

fn recovery_of(tree: &Tree, source: &[u8]) -> Recovery {
    let mut recovery = Recovery {
        spills: Vec::new(),
        misread_branch: false,
    };

    preorder(tree.root_node(), |node| {
        if node.is_error() {
            let (start, end) = (node.start_position().row, node.end_position().row);
            if end.saturating_sub(start) >= RECOVERY_SPILL_ROWS {
                recovery.spills.push((start, end));
            }
        }
        if !recovery.misread_branch && node.kind() == "preproc_call" {
            recovery.misread_branch =
                node.child_by_field_name("directive")
                    .is_some_and(|directive| {
                        directive_of(&source[directive.byte_range()]) == Directive::Branches
                    });
        }
        true
    });

    recovery
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

    let name_len = word_len(head);
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

    use kartta_lang::tree::parse;
    use kartta_lang::{FILE_SCOPE, Language, RawExtraction};

    use super::*;
    use crate::{Cpp, grammar};

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
        let tree = parse(&grammar(), SOURCE).expect("parse");

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
        let tree = parse(&grammar(), SOURCE).expect("parse");

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

    const PARSER_HEAD: &str = "/* Automatically @generated by tree-sitter */\n\n#include \"tree_sitter/parser.h\"\n\n\
                               #define LANGUAGE_VERSION 15\n#define STATE_COUNT 200\n";
    const SCANNER_HEAD: &str = "/* Automatically @generated by tree-sitter */\n\n#include \"tree_sitter/parser.h\"\n\n\
                                #define SCANNER_REVISION 15\n#define STATE_COUNT 200\n";
    const PARSER_CODE: &str = "static bool ts_lex(TSLexer *lexer, TSStateId state) {\n  START_LEXER();\n  \
                               eof = lexer->eof(lexer);\n  switch (state) {\n    case 0:\n      \
                               if (lookahead == '{') ADVANCE(1);\n      \
                               if (set_contains(sym_rule_character_set_1, 400, lookahead)) ADVANCE(2);\n      \
                               END_STATE();\n    default:\n      return false;\n  }\n}\n\n\
                               void *tree_sitter_rules_external_scanner_create(void);\n\n\
                               TS_PUBLIC const TSLanguage *tree_sitter_rules(void) {\n  \
                               static const TSLanguage language = {\n    .parse_table = &ts_parse_table[0][0],\n    \
                               .lex_modes = (const void*)ts_lex_modes,\n    .lex_fn = ts_lex,\n    \
                               .external_scanner = {\n      tree_sitter_rules_external_scanner_create,\n    },\n  \
                               };\n  return &language;\n}\n";
    const PARSER_TABLES: [(&str, &str); 5] = [
        ("enum ts_symbol_identifiers {", "  sym_rule_# = #,"),
        (
            "static const char * const ts_symbol_names[] = {",
            "  [sym_rule_#] = \"{\\\"#\",",
        ),
        (
            "static const TSLexerMode ts_lex_modes[STATE_COUNT] = {",
            "  [#] = {.lex_state = #, .external_lex_state = 1}, {(TSStateId)(-1),},",
        ),
        (
            "static const uint16_t ts_parse_table[LARGE_STATE_COUNT][SYMBOL_COUNT] = {",
            "  [STATE(#)] = {\n    [sym_rule_#] = ACTIONS(#), REDUCE(sym_rule, 2, 0, 0),\n  },",
        ),
        (
            "static const TSCharacterRange sym_rule_character_set_1[] = {",
            "  {'a', 'z'}, {'\\\\', '\\''},",
        ),
    ];

    fn parser_source(head: &str) -> String {
        let mut text = String::from(head);
        for (open, row) in PARSER_TABLES {
            writeln!(text, "\n{open}").expect("a String takes any write");
            for at in 0..200 {
                writeln!(text, "{}", row.replace('#', &at.to_string()))
                    .expect("a String takes any write");
            }
            text.push_str("};\n");
        }
        text.push('\n');
        text.push_str(PARSER_CODE);
        text
    }

    #[test]
    fn a_generated_parsers_tables_are_blanked_and_its_code_parsed_as_written() {
        let source = parser_source(PARSER_HEAD);

        let repaired = for_parse(source.as_bytes()).expect("the parser's tables to blank");

        assert_eq!(repaired.len(), source.len());
        for (table, _) in PARSER_TABLES {
            let open = source.find(table).expect("the table opens") + table.len();
            let close = open + source[open..].find("\n};").expect("the table closes");
            let blanked = repaired[open..close]
                .iter()
                .zip(&source.as_bytes()[open..close])
                .all(|(after, before)| *after == if *before == b'\n' { b'\n' } else { b' ' });
            assert!(blanked, "{table}");
        }
        let code = source.len() - PARSER_CODE.len();
        assert_eq!(repaired[code..], *PARSER_CODE.as_bytes());
        assert_eq!(repaired[..PARSER_HEAD.len()], *PARSER_HEAD.as_bytes());
    }

    #[test]
    fn the_same_tables_are_parsed_as_written_in_a_file_that_is_no_generated_parser() {
        assert_eq!(for_parse(parser_source(SCANNER_HEAD).as_bytes()), None);

        let marks_past_a_brace = format!("struct Head {{ int a; }};\n{PARSER_HEAD}");
        assert_eq!(
            for_parse(parser_source(&marks_past_a_brace).as_bytes()),
            None
        );
    }

    #[test]
    fn a_generated_parser_keeps_its_definitions_and_what_its_code_calls_and_uses() {
        let generated = Cpp::new().extract(parser_source(PARSER_HEAD).as_bytes());
        let written = Cpp::new().extract(parser_source(SCANNER_HEAD).as_bytes());

        assert_eq!(generated.symbols, written.symbols);
        assert_eq!(generated.callables, written.callables);
        let in_definitions = |raw: &RawExtraction| {
            let calls = raw.calls.iter().filter(|call| call.site_of != FILE_SCOPE);
            let uses = raw.uses.iter().filter(|site| site.site_of != FILE_SCOPE);
            (
                calls.cloned().collect::<Vec<_>>(),
                uses.cloned().collect::<Vec<_>>(),
            )
        };
        let (calls, uses) = in_definitions(&generated);
        assert_eq!((calls.clone(), uses.clone()), in_definitions(&written));
        assert!(
            calls.iter().any(|call| call.callee == "set_contains"),
            "{calls:?}"
        );
        let scanner = "tree_sitter_rules_external_scanner_create";
        assert!(uses.iter().any(|site| site.name == scanner), "{uses:?}");
        assert!(written.calls.iter().any(|call| call.site_of == FILE_SCOPE));
        assert!(
            generated
                .calls
                .iter()
                .all(|call| call.site_of != FILE_SCOPE)
        );
    }

    #[test]
    fn a_class_head_macro_with_arguments_is_blanked_beside_the_attributes_the_grammar_reads() {
        const SOURCE: &[u8] = b"struct DECLSPEC_ALIGN(16) N { void i(); };\n\
                                class alignas(32) DLL_API E final { void l(); };\n\
                                class [[nodiscard]] ALIGN_A(8) F : Base {};\n\
                                struct __declspec(align(16)) M {};\nstruct ALIGNED(4) STAT st;\n\
                                class DECLARE_CLASS(Foo) {};\n";

        let repaired = for_parse(SOURCE).expect("class heads with macros");

        let text = String::from_utf8_lossy(&repaired);
        for kept in [
            "struct                    N { void i(); };",
            "class alignas(32)         E final",
            "class [[nodiscard]]            F : Base",
            "struct __declspec(align(16)) M {};",
            "struct ALIGNED(4) STAT st;",
            "class DECLARE_CLASS(Foo) {};",
        ] {
            assert!(text.contains(kept), "{kept:?} in {text}");
        }
        assert_eq!(repaired.len(), SOURCE.len());
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

    #[test]
    fn a_function_whose_conditional_branches_each_open_a_brace_ends_at_its_own_brace() {
        const SOURCE: &[u8] = br"static bool search(int count)
{
  for (int i = 0; i < count; i++)
  {
#ifdef _XBOX
    int axis = wide(i);
    {
#else
    int axis = narrow(i);
    if (axis)
    {
#endif
      test(axis);
    }
  }
  return true;
}

static bool after(int count)
{
  return check(count);
}
";

        let raw = Cpp::new().extract(SOURCE);

        let lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(lines, [("search", 1, 17), ("after", 19, 22)]);
        let calls: Vec<(&str, u32)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.line))
            .collect();
        assert_eq!(calls, [("wide", 6), ("test", 13), ("check", 21)]);
    }

    #[test]
    fn a_conditional_whose_first_branch_unbalances_its_braces_is_read_with_that_branch_taken() {
        const SOURCE: &[u8] = br"void scan(int count)
{
#ifdef WIDE
  for (int i = 0; i < count; i++) {
/* #else
#endif */
#else
  for (int i = count; i > 0; i--) {
#endif
    step(i);
  }
#if defined(FAST)
  if (count) {
#elif defined(SLOW)
  if (count > 1) {
#endif
    finish();
  }
}

void check(int value)
{
  if (value == 0) {
    zero();
#ifdef STRICT
  } else if (value < 0) {
#else
  } else if (value < -1) {
#endif
    negative();
  }
#ifdef TRACE
  trace(value);
#else
  skip(value);
#endif
}
";
        let untaken = [3, 7, 8, 9, 12, 14, 15, 16, 25, 27, 28, 29];
        let tree = parse(&grammar(), SOURCE).expect("parse");

        let repaired = after_recovery(&tree, SOURCE).expect("a misread branch");

        let expected: Vec<Vec<u8>> = SOURCE
            .split(|byte| *byte == b'\n')
            .zip(1..)
            .map(|(line, number)| {
                if untaken.contains(&number) {
                    vec![b' '; line.len()]
                } else {
                    line.to_vec()
                }
            })
            .collect();
        assert_eq!(
            String::from_utf8_lossy(&repaired),
            String::from_utf8_lossy(&expected.join(&b'\n'))
        );
        let raw = Cpp::new().extract(SOURCE);
        let lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(lines, [("scan", 1, 19), ("check", 21, 37)]);
    }

    #[test]
    fn a_comment_holds_no_directive() {
        let closed = b"#if A\n{\n#else\n{\n#endif\n/* open\n";
        let open = b"#if A\n{\n#else\n{\n/* open\n#endif\n";
        let continued = b"#if A\n{\n// spliced \\\n#else\n{\n#endif\n";

        assert_eq!(untaken_texts(closed), ["#if A", "#else\n{\n#endif"]);
        assert_eq!(untaken_texts(open), Vec::<&str>::new());
        assert_eq!(untaken_texts(continued), Vec::<&str>::new());
    }

    #[test]
    fn a_raw_string_holds_no_brace() {
        let source = b"#if A\nconst char *text = R\"x(\n\")\" {\n)x\";\n#else\n{\n#endif\n\
                       #if B\nauto open = R\"({)\"; {\n#else\n{\n#endif\n";

        assert_eq!(untaken_texts(source), ["#if B", "#else\n{\n#endif"]);
    }

    fn untaken_texts(source: &[u8]) -> Vec<&str> {
        untaken_branches(source)
            .into_iter()
            .map(|range| std::str::from_utf8(&source[range]).expect("utf-8"))
            .collect()
    }
}
