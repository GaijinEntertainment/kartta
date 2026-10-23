use std::ops::Range;

use kartta_lang::tree::{child_nodes, preorder};
use tree_sitter::{Node, Tree};

const RECOVERY_SPILL_ROWS: usize = 3;
const CLASS_KEYS: [&[u8]; 3] = [b"class", b"struct", b"union"];
const HEAD_WORDS_MAX: usize = 4;
const HEAD_GROUP_MAX: usize = 256;
const HEAD_ATTRIBUTES: [&[u8]; 4] = [b"alignas", b"_Alignas", b"__declspec", b"__attribute__"];
const DATA_GROUP_MIN: usize = 4096;
const GENERATED_PARSER_MARKS: [&[u8]; 2] = [b"tree_sitter/parser.h", b"#define LANGUAGE_VERSION "];
const RAW_DELIMITER_MAX: usize = 16;
const PARENTHESES: (u8, u8) = (b'(', b')');
const ANGLE_BRACKETS: (u8, u8) = (b'<', b'>');
const LEADING_SPECIFIERS: [&[u8]; 7] = [
    b"static",
    b"inline",
    b"virtual",
    b"friend",
    b"constexpr",
    b"explicit",
    b"extern",
];
const CONVERSIONLESS: [&[u8]; 3] = [b"new", b"delete", b"co_await"];

pub(crate) fn for_parse(source: &[u8]) -> Option<Vec<u8>> {
    let mut repaired = Repaired::of(source);

    let tables = Tables::of(source);
    let mut judged_to = 0;
    let mut macro_run = MacroRun::default();

    let mut at = 0;
    while at < source.len() {
        if source[at] == b'{' && at >= judged_to {
            let scan = scan_data(source, at, tables);
            for &(start, end) in &scan.tables {
                repaired.blank(start..end);
            }
            judged_to = scan.end;
            if scan.closed {
                at = scan.end;
                continue;
            }
        }
        let blanked = match source[at] {
            b',' if opens_an_empty_slot(source, at) => Some((at, at + 1)),
            b'c' | b's' | b'u' if is_class_key(source, at) => head_macros(source, at)
                .or_else(|| keyed_head(source, at))
                .map(|[before, after]| {
                    repaired.blank(before);
                    (after.start, after.end)
                }),
            _ if at == 0 || source[at - 1] == b'\n' => {
                lone_macro_above_a_head(source, at, &mut macro_run)
            }
            _ => None,
        };
        at = match blanked {
            Some((start, end)) => {
                repaired.blank(start..end);
                end
            }
            None => at + 1,
        };
    }

    repaired.copy
}

/// The copy of a source that a repair blanks, made at its first blank, so a source with nothing to repair costs no
/// allocation.
struct Repaired<'s> {
    source: &'s [u8],
    copy: Option<Vec<u8>>,
}

impl<'s> Repaired<'s> {
    const fn of(source: &'s [u8]) -> Self {
        Self { source, copy: None }
    }

    fn blank(&mut self, range: Range<usize>) {
        let source = self.source;
        blank(&mut self.copy.get_or_insert_with(|| source.to_vec())[range]);
    }
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
// torrent : base` or `struct DECLSPEC_ALIGN(16) N {`, parses as the class name, and one between the name and the body,
// `class Builder FLATBUFFERS_FINAL_CLASS {`, as the declarator of a function whose body the class body becomes; either
// takes the head and the body down with it. A head that goes on to a body or a base list with such macros, and the
// attributes the grammar reads, around its one name holds them, while `struct STAT st;` declares a variable and
// `class DECLARE(Foo) {}` names no class. The macros after a name count only where the name shares the key's line: a
// key that ends a comment, `// a list class`, would read the key of the line below as the name and that class's name as
// a macro
/// The macros of a class head before its name and after it, either one empty where the head holds none there.
fn head_macros(source: &[u8], key: usize) -> Option<[Range<usize>; 2]> {
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
    if !opens_a_body {
        return None;
    }
    let trailing = names
        .iter()
        .rev()
        .take_while(|(_, _, kind)| *kind == HeadWord::Macro)
        .count();
    let (names, after) = match names.split_at(names.len() - trailing) {
        (up_to_the_name @ [.., (name, _, HeadWord::Name)], [first, .., last] | [first @ last])
            if !source[key..*name].contains(&b'\n') =>
        {
            (up_to_the_name, Some(first.0..last.1))
        }
        _ => (names, None),
    };
    let [prefix @ .., (_, _, HeadWord::Name | HeadWord::Macro)] = names else {
        return None;
    };
    if prefix.iter().any(|(_, _, kind)| *kind == HeadWord::Name) {
        return None;
    }
    let mut macros = prefix
        .iter()
        .filter(|(_, _, kind)| matches!(kind, HeadWord::Macro | HeadWord::MacroCall));
    let before = macros
        .next()
        .map(|first| first.0..macros.next_back().unwrap_or(first).1);
    match (before, after) {
        (None, None) => None,
        (Some(before), None) => Some([before.clone(), before.end..before.end]),
        (None, Some(after)) => Some([after.start..after.start, after]),
        (Some(before), Some(after)) => Some([before, after]),
    }
}

// constraint: an alignment macro that takes the class key as its argument, Bullet's `ATTRIBUTE_ALIGNED16(class)
// btCollisionObject {`, parses as a call before a name in an error, and the class body as a block of statements
/// The macro call around a class key, its name and its parenthesis before the key and its parenthesis after it, where
/// the head goes on to one name and a body or a base list.
fn keyed_head(source: &[u8], key: usize) -> Option<[Range<usize>; 2]> {
    let open = source[..key]
        .trim_ascii_end()
        .len()
        .checked_sub(1)
        .filter(|open| source[*open] == b'(')?;
    let name_end = source[..open].trim_ascii_end().len();
    let name_start = name_end
        - source[..name_end]
            .iter()
            .rev()
            .take_while(|byte| is_word_byte(**byte))
            .count();
    let close = key + word_len(&source[key..]);
    let close = close + whitespace_len(&source[close..]);
    if !is_macro_name(&source[name_start..name_end]) || source.get(close) != Some(&b')') {
        return None;
    }
    let mut at = close + 1 + whitespace_len(&source[close + 1..]);
    let name = word_len(&source[at..]);
    at += name + whitespace_len(&source[at + name..]);
    if source[at..].starts_with(b"final")
        && source.get(at + 5).is_some_and(|byte| !is_word_byte(*byte))
    {
        at += 5 + whitespace_len(&source[at + 5..]);
    }
    let opens = source.get(at) == Some(&b'{')
        || (source.get(at) == Some(&b':') && source.get(at + 1) != Some(&b':'));
    (name > 0 && opens).then_some([name_start..open + 1, close..close + 1])
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
    let end = open + group_len(&source[open..], PARENTHESES)?;
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

fn group_len(text: &[u8], (open, close): (u8, u8)) -> Option<usize> {
    let mut depth = 0usize;
    for (at, &byte) in text.iter().take(HEAD_GROUP_MAX).enumerate() {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(at + 1);
            }
        } else if matches!(byte, b'{' | b'}' | b';') {
            return None;
        }
    }
    None
}

fn bounded_find(text: &[u8], needle: &[u8]) -> Option<usize> {
    text[..text.len().min(HEAD_GROUP_MAX)]
        .windows(needle.len())
        .position(|window| window == needle)
}

// constraint: a macro alone on its line above a template, a class or a namespace head, `JPH_NAMESPACE_BEGIN` or
// `RTM_IMPL_FILE_PRAGMA_PUSH` above `namespace rtm {`, parses as the type of a declaration whose declarator is the key
// or the name below it, with the class body or the namespace read as a function's body, and the errors that leaves
// span too few rows for the spill gate. Above a function the macro reads as its return type and stays; a macro call,
// as the `TABLE_END(name)` that closes a table an earlier line opens, is no such macro, and blanking it unbalances what
// it closes
fn lone_macro_above_a_head(
    source: &[u8],
    line: usize,
    run: &mut MacroRun,
) -> Option<(usize, usize)> {
    let start = line + indent_len(&source[line..]);
    let opens_a_macro_name = source
        .get(start)
        .is_some_and(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_');
    if !opens_a_macro_name {
        return None;
    }
    let end = start + word_len(&source[start..]);
    if !is_macro_name(&source[start..end]) {
        return None;
    }
    let line_end = end
        + source[end..]
            .iter()
            .take_while(|byte| **byte != b'\n' && byte.is_ascii_whitespace())
            .count();
    let continues_a_define = line.checked_sub(1).is_some_and(|newline| {
        let above = &source[..newline];
        above.strip_suffix(b"\r").unwrap_or(above).ends_with(b"\\")
    });
    if continues_a_define || source.get(line_end).is_some_and(|byte| *byte != b'\n') {
        return None;
    }
    if line >= run.code {
        let code = next_code(source, line_end);
        let word = &source[code..code + word_len(&source[code..])];
        *run = MacroRun {
            code,
            opens_a_head: word == b"template" || word == b"namespace" || CLASS_KEYS.contains(&word),
        };
    }
    run.opens_a_head.then_some((start, end))
}

/// The code a run of lone macro lines leads to, which every line of the run shares, so the run is scanned once.
#[derive(Default)]
struct MacroRun {
    code: usize,
    opens_a_head: bool,
}

/// The start of the first code at or past `at` that is no comment and no line a macro name fills alone.
fn next_code(source: &[u8], mut at: usize) -> usize {
    loop {
        at += whitespace_len(&source[at..]);
        let text = &source[at..];
        let row = &text[..line_len(text)];
        at += if text.starts_with(b"/*") {
            comment_len(text).unwrap_or(text.len())
        } else if row.starts_with(b"//") || is_macro_name(row.trim_ascii_end()) {
            row.len()
        } else {
            return at;
        };
    }
}

fn line_len(text: &[u8]) -> usize {
    text.iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(text.len())
}

pub(crate) fn after_recovery(tree: &Tree, source: &[u8]) -> Option<Vec<u8>> {
    if !tree.root_node().has_error() {
        return None;
    }

    let Recovery {
        spills,
        misread_branch,
        misread_bodies: bodies,
    } = recovery_of(tree, source);
    let mut repaired = Repaired::of(source);
    if misread_branch {
        for directives in untaken_branches(source) {
            repaired.blank(directives);
        }
    }
    if spills.is_empty() && bodies.is_empty() {
        return repaired.copy;
    }

    let mut offset = 0;
    let mut continuation = false;

    for (row, line) in source.split(|byte| *byte == b'\n').enumerate() {
        let in_define = continuation;
        continuation = line.strip_suffix(b"\r").unwrap_or(line).ends_with(b"\\");

        let blanked = if in_define || continuation {
            None
        } else if spills
            .iter()
            .any(|(start, end)| row >= *start && row <= *end)
            && let Some(len) = macro_invocation_len(line)
        {
            Some(0..len)
        } else if let Some(body) = bodies.iter().find(|body| body.braced.contains(&offset)) {
            annotation_macro(&source[offset..]).filter(|annotation| {
                offset >= body.parsed_end || !reads_as_written(tree, offset + annotation.start)
            })
        } else {
            None
        };
        if let Some(blanked) = blanked {
            repaired.blank(offset + blanked.start..offset + blanked.end);
        }

        offset += line.len() + 1;
    }

    repaired.copy
}

struct Recovery {
    spills: Vec<(usize, usize)>,
    misread_branch: bool,
    misread_bodies: Vec<MisreadBody>,
}

// constraint: a member the grammar misreads through its macro can close the class body early, `}` of a member's body
// read as the class's, and the members after it then parse outside the class, where the same macro reads as written;
// the body the source braces, not the body the tree holds, bounds the members to repair
struct MisreadBody {
    braced: Range<usize>,
    parsed_end: usize,
}

fn closing_brace(source: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut at = open;
    while let Some(&byte) = source.get(at) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
        at += opaque_len(source, at);
    }
    source.len()
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
        match byte {
            b'{' => scan.brace(1),
            b'}' => scan.brace(-1),
            _ => {}
        }
        at += opaque_len(source, at);
    }
    scan.untaken
}

/// The bytes from `at` that hold no brace of the code: a comment or a literal, else the one byte at `at`.
fn opaque_len(source: &[u8], at: usize) -> usize {
    match source[at] {
        b'/' => match source.get(at + 1) {
            Some(b'/') => logical_line_end(source, at) - at,
            Some(b'*') => comment_len(&source[at..]).unwrap_or(source.len() - at),
            _ => 1,
        },
        quote @ b'"' if source[..at].ends_with(b"R") => raw_string_len(&source[at..])
            .or_else(|| quoted_len(&source[at..], quote))
            .unwrap_or(1),
        quote @ (b'"' | b'\'') => quoted_len(&source[at..], quote).unwrap_or(1),
        _ => 1,
    }
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

fn recovery_of(tree: &Tree, source: &[u8]) -> Recovery {
    let mut recovery = Recovery {
        spills: Vec::new(),
        misread_branch: false,
        misread_bodies: Vec::new(),
    };
    let parsed_at_the_top = !tree.root_node().is_error();

    preorder(tree.root_node(), |node| {
        if node.is_error() {
            let (start, end) = (node.start_position().row, node.end_position().row);
            if end.saturating_sub(start) >= RECOVERY_SPILL_ROWS {
                let class_end = closing_row_of_the_class_it_opens(node, source);
                recovery
                    .spills
                    .push((start, class_end.map_or(end, |row| row.max(end))));
            }
        }
        let kind = node.kind();
        if !recovery.misread_branch && kind == "preproc_call" {
            recovery.misread_branch =
                node.child_by_field_name("directive")
                    .is_some_and(|directive| {
                        directive_of(&source[directive.byte_range()]) == Directive::Branches
                    });
        }
        if parsed_at_the_top
            && kind == "field_declaration_list"
            && node
                .children(&mut node.walk())
                .any(|member| misreads_a_members_macro(member, source))
        {
            recovery.misread_bodies.push(MisreadBody {
                braced: node.start_byte()..closing_brace(source, node.start_byte()),
                parsed_end: node.end_byte(),
            });
        }
        true
    });

    recovery
}

// constraint: an error that opens at a class head ends at the first member the grammar cannot read past, as
// `OPENMAYA_DEPRECATED(2019, "...")` alone on its line, and the members' macros after it, which the second parse meets
// as written, close the class there again; the brace that closes the class in the source bounds the spill instead
#[expect(
    clippy::naive_bytecount,
    reason = "one class body is counted for each spill a class head opens, too rare to take a dependency for"
)]
fn closing_row_of_the_class_it_opens(error: Node<'_>, source: &[u8]) -> Option<usize> {
    let head = error.start_byte();
    if !is_class_key(source, head) {
        return None;
    }
    let open = head
        + source[head..]
            .iter()
            .position(|byte| matches!(byte, b'{' | b';'))?;
    (source[open] == b'{').then(|| {
        let close = closing_brace(source, open);
        error.start_position().row
            + source[head..close]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
    })
}

fn misreads_a_members_macro(member: Node<'_>, source: &[u8]) -> bool {
    matches!(member.kind(), "field_declaration" | "function_definition")
        && member.child_by_field_name("type").is_some_and(|written| {
            written.kind() == "type_identifier" && is_macro_name(&source[written.byte_range()])
        })
        && misreads_its_type(member)
}

fn reads_as_written(tree: &Tree, annotation: usize) -> bool {
    tree.root_node()
        .descendant_for_byte_range(annotation, annotation + 1)
        .filter(|written| written.kind() == "type_identifier")
        .and_then(|written| written.parent())
        .is_some_and(|declaration| {
            matches!(
                declaration.kind(),
                "field_declaration" | "function_definition" | "declaration"
            ) && !misreads_its_type(declaration)
        })
}

// constraint: the grammar reads a macro that opens a member as the member's type, so the type written after it fails
// to reduce; a one-word type before a function's declarator fails alone in an error the walk reads past, while a
// scoped type, a destructor, a conversion operator or a constructor's initializer list fails beyond it or reads as a
// bit width, and the recovery loses the member, misnames it or closes the class at it
fn misreads_its_type(declaration: Node<'_>) -> bool {
    let body = declaration.child_by_field_name("body");
    let declarator = declaration.child_by_field_name("declarator");
    let mut errors = child_nodes(declaration)
        .filter(|child| Some(*child) != body && (child.is_missing() || child.has_error()));
    let Some(error) = errors.next() else {
        return declarator.is_some_and(|declarator| {
            declarator.kind() == "function_declarator"
                && declarator
                    .next_sibling()
                    .is_some_and(|next| next.kind() == "bitfield_clause")
        });
    };
    let one_word_before_a_function = error.is_error()
        && errors.next().is_none()
        && declarator.is_some_and(|declarator| {
            error.end_byte() <= declarator.start_byte() && declares_a_function(declarator)
        })
        && child_nodes(error).all(|word| {
            matches!(
                word.kind(),
                "identifier" | "type_identifier" | "primitive_type" | "sized_type_specifier"
            )
        });
    !one_word_before_a_function
}

fn declares_a_function(mut declarator: Node<'_>) -> bool {
    while matches!(
        declarator.kind(),
        "pointer_declarator" | "reference_declarator"
    ) {
        let Some(inner) = child_nodes(declarator).find(Node::is_named) else {
            return false;
        };
        declarator = inner;
    }
    declarator.kind() == "function_declarator"
}

/// The macro that opens the line `text` starts with, past the specifiers before it, where it annotates the declaration
/// after it.
fn annotation_macro(text: &[u8]) -> Option<Range<usize>> {
    let mut at = indent_len(text);
    loop {
        let len = word_len(&text[at..]);
        let gap = whitespace_len(&text[at + len..]);
        if gap == 0 || !LEADING_SPECIFIERS.contains(&&text[at..at + len]) {
            break;
        }
        at += len + gap;
    }
    let annotation = at..at + word_len(&text[at..]);
    let declaration = annotation.end + indent_len(&text[annotation.end..]);
    (is_macro_name(&text[annotation.clone()])
        && declaration > annotation.end
        && annotates(&text[declaration..]))
    .then_some(annotation)
}

// constraint: a macro before a type and a declarator, before a destructor, a conversion operator or a constructor with
// an initializer list is no type, since each of them takes a type of its own or none; a macro before one name and its
// parameters may be the type, `HRESULT Query()`, and one beside another macro or a compiler's own `__` word, which the
// walk reads as written only beside a macro, stays
fn annotates(declaration: &[u8]) -> bool {
    let mut at = 0;
    let mut terms = 0;
    loop {
        let Some(len) = term_len(&declaration[at..]) else {
            return false;
        };
        let term = &declaration[at..at + len];
        if is_macro_name(term) || term.starts_with(b"__") {
            return false;
        }
        terms += 1;
        at += term.len();
        let next = at + whitespace_len(&declaration[at..]);
        if term == b"operator" || term.ends_with(b"::operator") {
            let target = &declaration[next..next + word_len(&declaration[next..])];
            return terms > 1 || !(target.is_empty() || CONVERSIONLESS.contains(&target));
        }
        if declaration.get(next) == Some(&b'(') {
            return terms > 1
                || term.starts_with(b"~")
                || initializes_members(&declaration[next..]);
        }
        let gap = declaration[at..]
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace() || matches!(byte, b'*' | b'&'))
            .count();
        if gap == 0 {
            return false;
        }
        at += gap;
    }
}

fn term_len(text: &[u8]) -> Option<usize> {
    let mut at = 0;
    loop {
        at += usize::from(text.get(at) == Some(&b'~'));
        let len = word_len(&text[at..]);
        if len == 0 {
            return None;
        }
        at += len;
        if text.get(at) == Some(&b'<') {
            at += group_len(&text[at..], ANGLE_BRACKETS)?;
        }
        if !text[at..].starts_with(b"::") {
            return Some(at);
        }
        at += 2;
    }
}

fn initializes_members(parameters: &[u8]) -> bool {
    let Some(close) = group_len(parameters, PARENTHESES) else {
        return false;
    };
    let rest = &parameters[close + whitespace_len(&parameters[close..])..];
    let rest = rest
        .strip_prefix(b"noexcept")
        .map_or(rest, |rest| &rest[whitespace_len(rest)..]);
    rest.starts_with(b":") && !rest.starts_with(b"::")
}

/// The length of the macros `text` starts with, each with or without its arguments, where the `;` that closes them
/// follows: `PRINTF_ATTR(2, 3)` of `PRINTF_ATTR(2, 3);`.
pub(crate) fn macro_annotation_len(text: &[u8]) -> Option<usize> {
    let mut at = 0;
    loop {
        let len = word_len(&text[at..]);
        if len == 0 {
            return (at > 0 && text.get(at) == Some(&b';')).then_some(at);
        }
        if !is_macro_name(&text[at..at + len]) {
            return None;
        }
        at += len;
        let gap = whitespace_len(&text[at..]);
        if text[at + gap..].starts_with(b"(") {
            at += gap + group_len(&text[at + gap..], PARENTHESES)?;
        }
        at += whitespace_len(&text[at..]);
    }
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
    fn a_class_head_macro_after_the_name_is_blanked_beside_one_before_it() {
        const SOURCE: &[u8] = b"class Builder FLATBUFFERS_FINAL_CLASS {};\n\
                                struct TORRENT_EXPORT alert TORRENT_FINAL : base {};\n\
                                class Two FIRST_MACRO SECOND_MACRO final {};\nstruct STAT st;\n\
                                class Mixed API Name {};\nclass ALL_CAPS {};\n\
                                // the element of a list class\nstruct FSA_ELEMENT {};\n";

        let repaired = for_parse(SOURCE).expect("class heads with macros");

        assert_eq!(repaired.len(), SOURCE.len());
        let mut macros: Vec<usize> = [
            "FLATBUFFERS_FINAL_CLASS",
            "TORRENT_EXPORT",
            "TORRENT_FINAL",
            "FIRST_MACRO",
            "SECOND_MACRO",
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
    }

    #[test]
    fn a_macro_that_takes_the_class_key_is_blanked_to_the_key() {
        const SOURCE: &[u8] = b"ATTRIBUTE_ALIGNED16(class)\nbtCollisionObject\n{};\n\
                                B3_ATTRIBUTE_ALIGNED16( struct ) b3Contact4 : public b3Base {};\n\
                                ATTRIBUTE_ALIGNED16(class) btFinal final {};\nDECLARE(class) handle;\n\
                                align(class) lower {};\nALIGNED(class Named) {};\n";

        let repaired = for_parse(SOURCE).expect("keyed class heads");

        let text = String::from_utf8_lossy(&repaired);
        for kept in [
            "                    class \nbtCollisionObject\n{};",
            "                        struct   b3Contact4 : public b3Base {};",
            "                    class  btFinal final {};",
            "DECLARE(class) handle;",
            "align(class) lower {};",
            "ALIGNED(class Named) {};",
        ] {
            assert!(text.contains(kept), "{kept:?} in {text}");
        }
        assert_eq!(repaired.len(), SOURCE.len());
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
    fn a_lone_macro_line_is_blanked_above_a_head_and_nowhere_else() {
        const SOURCE: &[u8] = b"PUSH_PACK\n// a note\n\nnamespace a {\nEXPORT_BEGIN\n/* doc */\n  INNER_OPEN\n\
                                  template <class T> class B {};\nNAMESPACE_END\nSTDAPI_(bool) f();\nTABLE_END(x)\n\
                                class C {};\n#define X \\\nFOO\nstruct D {};\nOK\nunion E {};\nPACK_UNION\n\
                                union G {};\nPACK_STRUCT\nstruct H {};\nNOTED // why\nclass F {};\n}\n";

        let repaired = for_parse(SOURCE).expect("lone macros above heads");

        let lines: Vec<&[u8]> = repaired.split(|byte| *byte == b'\n').collect();
        let blanked: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !line.is_empty() && line.iter().all(|byte| *byte == b' '))
            .map(|(row, _)| row + 1)
            .collect();
        assert_eq!(blanked, [1, 5, 7, 18, 20]);
        assert_eq!(repaired.len(), SOURCE.len());
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

    #[test]
    fn a_macro_before_a_members_scoped_return_type_keeps_the_class_open() {
        const SOURCE: &[u8] = br"namespace physx
{
class NpShape
{
public:
	PX_FORCE_INLINE	PxGeometryType::Enum		getGeometryTypeFast()	const	{ return mShape.getGeometryType();	}
	PX_FORCE_INLINE	const Gu::GeometryUnion&	getGeometryFast()		const	{ return mShape.getGeometryUnion();		}

	PX_INLINE NpShape(PxU32 flags) : mFlags(flags) {}
	PX_INLINE ~NpShape() {}
	PX_FORCE_INLINE operator PxU32() const { return mFlags; }

	void after() { step(); }
	PxU32 mFlags;
};
}
";

        let raw = Cpp::new().extract(SOURCE);

        let lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(
            lines,
            [
                ("physx::NpShape", 3, 15),
                ("NpShape::getGeometryTypeFast", 6, 6),
                ("NpShape::getGeometryFast", 7, 7),
                ("NpShape::NpShape", 9, 9),
                ("NpShape::~NpShape", 10, 10),
                ("NpShape::operator PxU32", 11, 11),
                ("NpShape::after", 13, 13),
                ("NpShape::mFlags", 14, 14)
            ]
        );
        let signatures: Vec<&str> = [2, 3, 5]
            .into_iter()
            .filter_map(|index| raw.symbols[index].signature.as_deref())
            .collect();
        assert_eq!(
            signatures,
            [
                "const Gu::GeometryUnion&\tgetGeometryFast()\t\tconst",
                "NpShape(PxU32 flags) : mFlags(flags)",
                "operator PxU32() const"
            ]
        );
    }

    #[test]
    fn a_class_whose_head_opens_an_error_keeps_every_member_past_its_macros() {
        const SOURCE: &[u8] = br#"class OPENMAYA_EXPORT MFnDependencyNode : public MFnBase
{
	declareMFn(MFnDependencyNode, MFnBase);

public:
	OPENMAYA_ENUM(MAttrClass,
		kLocalDynamicAttr = 1,
		kNormalAttr,
		kInvalidAttr
	);

	MTypeId typeId( MStatus* ReturnStatus = NULL ) const;
	OPENMAYA_AVAILABLE(2023)
	MString uniqueName( MStatus * ReturnStatus = NULL ) const;
	MString pluginName( MStatus * ReturnStatus = NULL ) const;

	OPENMAYA_DEPRECATED(2019, "Use the other findPlug instead.")
	MPlug findPlug( const MObject & attr, MStatus* ReturnStatus=NULL ) const;

	MStatus addAttribute( const MObject & attr );
	bool isLocked( MStatus* ReturnStatus=NULL ) const;

	OPENMAYA_DEPRECATED(2019, "Use the other addAttribute instead.")
	MStatus addAttribute( const MObject & attr, MAttrClass type );
	MStatus setLocked( bool locked );
};
"#;

        let raw = Cpp::new().extract(SOURCE);

        let lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(
            lines,
            [
                ("MFnDependencyNode", 1, 26),
                ("MFnDependencyNode::typeId", 12, 12),
                ("MFnDependencyNode::uniqueName", 14, 14),
                ("MFnDependencyNode::pluginName", 15, 15),
                ("MFnDependencyNode::findPlug", 18, 18),
                ("MFnDependencyNode::addAttribute", 20, 20),
                ("MFnDependencyNode::isLocked", 21, 21),
                ("MFnDependencyNode::addAttribute", 24, 24),
                ("MFnDependencyNode::setLocked", 25, 25)
            ]
        );
    }

    #[test]
    fn only_a_misread_classes_annotations_are_blanked() {
        const SOURCE: &[u8] = br"struct Shape
{
	PX_FORCE_INLINE const Gu::GeometryUnion& geometry() const { return mGeometry; }
	PX_FORCE_INLINE bool isValid() const { return true; }
	HRESULT Query(int id);
	PX_INLINE __device__ void kernel(int index);
	PX_INLINE PX_CUDA_CALLABLE PxU32 count() const;
};
struct Other
{
	PX_FORCE_INLINE bool isValid() const { return true; }
};
PX_FORCE_INLINE ns::Result outside() { return {}; }
";
        const ANNOTATION: &[u8] = b"PX_FORCE_INLINE const Gu";
        let tree = parse(&grammar(), SOURCE).expect("parse");
        let start = SOURCE
            .windows(ANNOTATION.len())
            .position(|window| window == ANNOTATION)
            .expect("the misread member");

        let repaired = after_recovery(&tree, SOURCE).expect("a misread member");

        assert_eq!(repaired.len(), SOURCE.len());
        assert_eq!(
            changed_offsets(&repaired, SOURCE),
            (start..start + "PX_FORCE_INLINE".len()).collect::<Vec<_>>()
        );
    }
}
