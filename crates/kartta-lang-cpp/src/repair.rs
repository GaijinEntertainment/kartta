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
const CALL_MODIFIERS: [&[u8]; 6] = [
    b"__cdecl",
    b"__clrcall",
    b"__stdcall",
    b"__fastcall",
    b"__thiscall",
    b"__vectorcall",
];

pub(crate) fn for_parse(source: &[u8]) -> Option<Vec<u8>> {
    let mut repaired = Repaired::of(source);

    let tables = Tables::of(source);
    let mut judged_to = 0;
    let mut macro_run = MacroRun::default();
    let mut modifier_line = ModifierLine::default();

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
            b'_' if is_call_modifier(source, at, &mut modifier_line) => {
                Some((at, at + word_len(&source[at..])))
            }
            b'c' | b's' | b'u' if is_class_key(source, at) => head_macros(source, at)
                .or_else(|| keyed_head(source, at))
                .map(|[before, after]| {
                    repaired.blank(before);
                    (after.start, after.end)
                }),
            _ if at == 0 || source[at - 1] == b'\n' => lone_macro_line(source, at, &mut macro_run)
                .or_else(|| {
                    macro_before_a_constructor(&source[at..])
                        .map(|annotation| (at + annotation.start, at + annotation.end))
                })
                .or_else(|| {
                    split_head(source, at).map(|[opening, later]| {
                        repaired.blank(later);
                        (opening.start, opening.end)
                    })
                }),
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

// constraint: the grammar reads a calling convention the compiler reserves, `__stdcall`, only between a function
// definition's return type and its declarator; in a prototype, `void __cdecl f(int a);`, and in a class body it is a
// syntax error, whose recovery outside a class can take the declaration after it. The word names nothing and is kept
// in the walk's signature, which reads the original. A directive's text is opaque to the grammar, and a `#define`
// whose value is blanked to spaces, `#define XM_CALLCONV __vectorcall`, takes the `#elif` on the next line into it
fn is_call_modifier(source: &[u8], at: usize, line: &mut ModifierLine) -> bool {
    if (at > 0 && is_word_byte(source[at - 1]))
        || !CALL_MODIFIERS.contains(&&source[at..at + word_len(&source[at..])])
    {
        return false;
    }
    if at >= line.end {
        let start = line_start(source, at);
        let continues_a_directive = start.checked_sub(1).is_some_and(|newline| {
            let above = &source[..newline];
            above.strip_suffix(b"\r").unwrap_or(above).ends_with(b"\\")
        });
        *line = ModifierLine {
            end: at + line_len(&source[at..]),
            directive: continues_a_directive
                || source.get(start + indent_len(&source[start..])) == Some(&b'#'),
        };
    }
    !line.directive
}

/// The line that holds the last calling convention [`is_call_modifier`] judged, so a line is scanned for its
/// directive once, however many conventions it holds.
#[derive(Default)]
struct ModifierLine {
    end: usize,
    directive: bool,
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
// attributes the grammar reads, around its one name holds them, and so does a specialization's, `struct
// RTM_DEPRECATED("...") float_traits<float> {`, whose arguments follow the name; while `struct STAT st;` declares a
// variable and `class DECLARE(Foo) {}` names no class. The macros after a name count only where the name shares the
// key's line: a key that ends a comment, `// a list class`, would read the key of the line below as the name and that
// class's name as a macro
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
    let specializes =
        count > 0 && words[count - 1].2 == HeadWord::Name && source.get(at) == Some(&b'<');
    if specializes && let Some(arguments) = group_len(&source[at..], ANGLE_BRACKETS) {
        at += arguments + whitespace_len(&source[at + arguments..]);
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

/// The length of the group `text` opens, up to its closing byte; a parenthesized expression inside an angle group,
/// `Foo<(1 > 2)>`, holds its own `<` and `>`.
fn group_len(text: &[u8], (open, close): (u8, u8)) -> Option<usize> {
    let mut depth = 0usize;
    let mut parentheses = 0usize;
    for (at, &byte) in text.iter().take(HEAD_GROUP_MAX).enumerate() {
        match byte {
            b'{' | b'}' | b';' => return None,
            b'(' if open != b'(' => parentheses += 1,
            b')' if open != b'(' => parentheses = parentheses.checked_sub(1)?,
            _ if parentheses > 0 => {}
            _ if byte == open => depth += 1,
            _ if byte == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
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

// constraint: a macro alone on its line above a template, a class or a namespace head, `JPH_NAMESPACE_BEGIN` or
// `RTM_IMPL_FILE_PRAGMA_PUSH` above `namespace rtm {`, parses as the type of a declaration whose declarator is the key
// or the name below it, with the class body or the namespace read as a function's body, and the errors that leaves
// span too few rows for the spill gate. Above a function the macro reads as its return type and stays; a macro call,
// as the `TABLE_END(name)` that closes a table an earlier line opens, is no such macro, and blanking it unbalances what
// it closes. A macro that opens a namespace of its own inside another, `ABSL_NAMESPACE_BEGIN` right after `namespace
// absl {`, reads as the type of the declaration below it, which loses an enum or a constant and starts a function at
// the macro; the one that closes it, right before the outer `}`, reads as a declaration that takes the brace, after
// which a misread member elsewhere can take the whole namespace. Only a namespace's edges: before the `}` of a table or
// a function the name is a value or a statement, and one with code right below it annotates that code
fn lone_macro_line(source: &[u8], line: usize, run: &mut MacroRun) -> Option<(usize, usize)> {
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
        run.code = code;
        run.blanked = word == b"template"
            || word == b"namespace"
            || CLASS_KEYS.contains(&word)
            || (source[code..].starts_with(b"}") && run.closes_a_namespace(source, code))
            || (is_set_apart(&source[line_end..code]) && opens_a_namespace_body(source, line));
    }
    run.blanked.then_some((start, end))
}

/// The code a run of lone macro lines leads to, which every line of the run shares, so the run is scanned once, and
/// the braces that close a namespace, found once a file holds a run before a `}`.
#[derive(Default)]
struct MacroRun {
    code: usize,
    blanked: bool,
    namespace_closes: Option<Vec<usize>>,
}

impl MacroRun {
    fn closes_a_namespace(&mut self, source: &[u8], brace: usize) -> bool {
        self.namespace_closes
            .get_or_insert_with(|| namespace_closes(source))
            .binary_search(&brace)
            .is_ok()
    }
}

/// The offsets of the braces that close a namespace body, in order, read with each conditional's first branch taken,
/// since branches that each open a brace share the brace that closes them.
fn namespace_closes(source: &[u8]) -> Vec<usize> {
    let mut open: Vec<bool> = Vec::new();
    let mut closes = Vec::new();
    let mut branches = FirstBranches::default();
    let mut at = 0;
    while let Some(&byte) = source.get(at) {
        if (at == 0 || source[at - 1] == b'\n')
            && let Some(directive) = directive_at(source, at)
        {
            branches.read(directive);
            at = directive_end(source, at);
            continue;
        }
        let taken = branches.untaken == 0;
        match byte {
            b'{' if taken => open.push(is_namespace_head(&source[..at])),
            b'}' if taken && open.pop() == Some(true) => closes.push(at),
            _ => {}
        }
        at += opaque_len(source, at);
    }
    closes
}

/// The open conditionals, each marked once a later branch begins, and how many of them are in a later branch.
#[derive(Default)]
struct FirstBranches {
    later: Vec<bool>,
    untaken: usize,
}

impl FirstBranches {
    fn read(&mut self, directive: Directive) {
        match directive {
            Directive::Opens => self.later.push(false),
            Directive::Branches => {
                if let Some(later @ false) = self.later.last_mut() {
                    *later = true;
                    self.untaken += 1;
                }
            }
            Directive::Closes => {
                if self.later.pop() == Some(true) {
                    self.untaken -= 1;
                }
            }
            Directive::Other => {}
        }
    }
}

// constraint: the grammar reads a conditional's branch as a run of whole statements, so a head each branch writes,
// `#ifdef A` `void f(int a)` `#else` `void f(int a, int b)` `#endif` `{`, misses the `;` of the first branch and reads
// the `#endif` as an error inside the definition the last branch opens; the conditional then stays open past that
// definition and can take its class down, or fail the whole tree, which leaves the misread members of the file to
// their macros. The first branch alone is a whole definition of what the others write, while heads of two names are
// two definitions, which the tree already holds, and a macro call may be a class head that goes on past the branch
/// The directive of the conditional at `line` and the lines from its first later branch through its `#endif`, where
/// each branch holds a [`branch_head`] of one name and a `{` follows the conditional.
fn split_head(source: &[u8], line: usize) -> Option<[Range<usize>; 2]> {
    if directive_at(source, line)? != Directive::Opens {
        return None;
    }
    let first_branch = directive_end(source, line);
    let mut branch = branch_head(source, first_branch)?;
    let names = branch.names;
    let later = branch.ended_by.start;
    while branch.directive == Directive::Branches {
        branch = branch_head(source, branch.ended_by.end).filter(|next| next.names == names)?;
    }
    let end = branch.ended_by;
    (later != end.start && source.get(next_code(source, end.end)) == Some(&b'{'))
        .then_some([line..first_branch, later..end.end])
}

/// The head a branch of a conditional holds: the names before its first and its last outermost `(`, since a return type
/// or an attribute can hold the first of them, and the directive that ends the branch, with that directive's line.
struct BranchHead<'s> {
    names: [&'s [u8]; 2],
    directive: Directive,
    ended_by: Range<usize>,
}

/// The [`BranchHead`] of the branch at `from`, where the branch holds no `;`, no brace and no conditional, and its
/// first `(` closes a name no macro spells.
fn branch_head(source: &[u8], from: usize) -> Option<BranchHead<'_>> {
    let mut first: Option<&[u8]> = None;
    let mut last = None;
    let mut depth = 0usize;
    let mut code = from;
    let mut at = from;
    loop {
        let byte = *source.get(at)?;
        let line_start = source[at - 1] == b'\n';
        match line_start.then(|| directive_at(source, at)).flatten() {
            Some(Directive::Opens) => return None,
            Some(Directive::Other) => {
                at = directive_end(source, at);
                code = at;
                continue;
            }
            Some(directive) => {
                return Some(BranchHead {
                    names: [first.filter(|name| !is_macro_name(name))?, last?],
                    directive,
                    ended_by: at..directive_end(source, at),
                });
            }
            None => {}
        }
        match byte {
            b';' | b'{' | b'}' => return None,
            b'(' if depth == 0 => {
                depth = 1;
                last = Some(head_name(&source[code..at]));
                first = first.or(last);
            }
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        at += opaque_len(source, at);
    }
}

/// The name of the head a `(` ends `text` with: the scoped name before it, or the `:` that opens an initializer list.
fn head_name(text: &[u8]) -> &[u8] {
    let text = text.trim_ascii();
    if text.starts_with(b":") {
        return b":";
    }
    let spaced = text.iter().rposition(u8::is_ascii_whitespace);
    let name = &text[spaced.map_or(0, |space| space + 1)..];
    let pointers = name.iter().take_while(|byte| matches!(byte, b'*' | b'&'));
    &name[pointers.count()..]
}

/// Whether the code before a `{` ends in a namespace head, `namespace`, `namespace a` or `namespace a::b`.
fn is_namespace_head(before: &[u8]) -> bool {
    let head = before.trim_ascii_end();
    let name = head
        .iter()
        .rev()
        .take_while(|byte| is_word_byte(**byte) || **byte == b':')
        .count();
    let head = head[..head.len() - name].trim_ascii_end();
    head.strip_suffix(b"namespace")
        .is_some_and(|before| !before.last().copied().is_some_and(is_word_byte))
}

/// Whether a blank or a comment line stands between a run of lone macro lines and the code after it.
fn is_set_apart(gap: &[u8]) -> bool {
    let mut rows = gap.split(|byte| *byte == b'\n').skip(1).peekable();
    while let Some(row) = rows.next() {
        if rows.peek().is_none() {
            return false;
        }
        let row = row.trim_ascii();
        if row.is_empty() || row.starts_with(b"//") || row.starts_with(b"/*") {
            return true;
        }
    }
    false
}

/// Whether the code above a line, past comments and lone macro lines, is the `{` of a namespace head.
fn opens_a_namespace_body(source: &[u8], mut line: usize) -> bool {
    while let Some(above_end) = line.checked_sub(1) {
        line = line_start(source, above_end);
        let mut row = &source[line..above_end];
        let text = row.trim_ascii();
        if text.is_empty() || text.starts_with(b"//") || is_macro_name(text) {
            continue;
        }
        if text.ends_with(b"*/") {
            let Some(comment) = source[..above_end]
                .windows(2)
                .rposition(|pair| pair == b"/*")
            else {
                return false;
            };
            line = line_start(source, comment);
            row = &source[line..comment];
            if row.trim_ascii().is_empty() {
                continue;
            }
        }
        let code_end = line + row.trim_ascii_end().len();
        return source[..code_end]
            .strip_suffix(b"{")
            .is_some_and(is_namespace_head);
    }
    false
}

fn line_start(source: &[u8], at: usize) -> usize {
    source[..at]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |newline| newline + 1)
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
        stray_branch,
        listed_conditional,
        misread_bodies: bodies,
        blanks,
    } = recovery_of(tree, source);
    let mut repaired = Repaired::of(source);
    for blank in blanks {
        repaired.blank(blank);
    }
    let split = if misread_branch {
        Some(BraceSplit::OpensOrCloses)
    } else {
        stray_branch.then_some(BraceSplit::Opens)
    };
    if split.is_some() || listed_conditional {
        for directives in untaken_branches(source, split) {
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
    stray_branch: bool,
    listed_conditional: bool,
    misread_bodies: Vec<MisreadBody>,
    blanks: Vec<Range<usize>>,
}

// constraint: the grammar reads a conditional's branch as a run of whole statements, and a label that ends one, `skip:`
// right before its `#endif`, takes the directive for the statement it labels; the conditional then never closes, and
// the definition around it is lost or runs on far past its brace. A label is no symbol, call or use, so the second
// parse reads the directive without it
/// The label and colon of a labeled statement whose statement is an error the grammar opens at an `#endif`.
fn label_before_an_endif(statement: Node<'_>, source: &[u8]) -> Option<Range<usize>> {
    let colon = child_nodes(statement).find(|child| child.kind() == ":")?;
    let held = colon.next_sibling().filter(Node::is_error)?;
    let text = &source[held.byte_range()];
    (text.starts_with(b"#") && directive_of(text) == Directive::Closes)
        .then(|| statement.start_byte()..colon.end_byte())
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
// the grammar reads the next. Where a larger error holds the directive as a bare token instead, only the conditionals
// whose first branch leaves a brace open are read with that branch taken. A conditional inside a constructor's
// initializer list splits one member's declaration, and the grammar then reads the list's items as members of their own
// or loses the class at them, so where the tree holds a conditional that opens after a list item, each one inside an
// initializer list is read with its first branch taken
fn untaken_branches(source: &[u8], split: Option<BraceSplit>) -> Vec<Range<usize>> {
    let mut scan = ConditionalScan {
        split,
        open: Vec::new(),
        untaken: Vec::new(),
        last_code: b'\n',
        list: None,
    };
    let mut at = 0;
    while let Some(&byte) = source.get(at) {
        if (at == 0 || source[at - 1] == b'\n')
            && let Some(directive) = directive_at(source, at)
        {
            let end = directive_end(source, at);
            scan.directive(directive, at..end);
            at = end;
            continue;
        }
        match byte {
            b'{' => scan.brace(1),
            b'}' => scan.brace(-1),
            _ => {}
        }
        let len = opaque_len(source, at);
        if byte != b'/' || len == 1 {
            scan.code(byte, source.get(at + 1).copied());
        }
        at += len;
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

/// Where the directive at `line` ends: its logical line's end, or, past a block comment that line opens, the end of the
/// line the comment closes on, since the preprocessor reads the comment as one space.
fn directive_end(source: &[u8], line: usize) -> usize {
    let mut end = logical_line_end(source, line);
    let mut at = line;
    while at < end {
        at += opaque_len(source, at);
        if at > end {
            end = logical_line_end(source, at);
        }
    }
    end
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

/// The conditionals whose later branches the second parse skips, by what their first branch does to the braces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BraceSplit {
    Opens,
    OpensOrCloses,
}

struct ConditionalScan {
    split: Option<BraceSplit>,
    open: Vec<OpenConditional>,
    untaken: Vec<Range<usize>>,
    last_code: u8,
    list: Option<InitializerList>,
}

struct OpenConditional {
    head: Range<usize>,
    later: Option<usize>,
    depth: i32,
    closes_unopened: bool,
}

/// A constructor's initializer list the scan is inside, from the `:` after its parameters to its body's `{`, with the
/// directive lines it holds and its conditionals' later branches; a `;`, an unmatched closer before a body, or a branch
/// of a conditional open before it abandons it.
struct InitializerList {
    enclosing: usize,
    nesting: usize,
    untaken: Vec<Range<usize>>,
}

impl ConditionalScan {
    fn directive(&mut self, directive: Directive, line: Range<usize>) {
        if matches!(directive, Directive::Branches | Directive::Closes)
            && self
                .list
                .as_ref()
                .is_some_and(|list| self.open.len() <= list.enclosing)
        {
            self.list = None;
        }
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
                let Some(conditional) = self.open.pop() else {
                    return;
                };
                let untaken = [
                    conditional.head,
                    conditional.later.unwrap_or(line.start)..line.end,
                ];
                if let Some(list) = self.list.as_mut() {
                    list.untaken.extend(untaken);
                } else if conditional.later.is_some()
                    && self.split.is_some_and(|split| match split {
                        BraceSplit::Opens => conditional.depth > 0,
                        BraceSplit::OpensOrCloses => {
                            conditional.depth != 0 || conditional.closes_unopened
                        }
                    })
                {
                    self.untaken.extend(untaken);
                }
            }
            Directive::Other => {
                if let Some(list) = self.list.as_mut() {
                    list.untaken.push(line);
                }
            }
        }
    }

    fn code(&mut self, byte: u8, next: Option<u8>) {
        let after = self.last_code;
        if !byte.is_ascii_whitespace() {
            self.last_code = byte;
        }
        let Some(list) = self.list.as_mut() else {
            if byte == b':' && after == b')' && next != Some(b':') {
                self.list = Some(InitializerList {
                    enclosing: self.open.len(),
                    nesting: 0,
                    untaken: Vec::new(),
                });
            }
            return;
        };
        let initializes = after == b'_' || after == b'>' || after.is_ascii_alphanumeric();
        match byte {
            b'(' | b'{' if list.nesting > 0 => list.nesting += 1,
            b')' | b'}' if list.nesting > 0 => list.nesting -= 1,
            b'(' => list.nesting = 1,
            b'{' if initializes => list.nesting = 1,
            b'{' if matches!(after, b')' | b'}') => {
                let body = std::mem::take(&mut list.untaken);
                self.untaken.extend(body);
                self.list = None;
            }
            b'{' | b'}' | b')' | b';' => self.list = None,
            _ => {}
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

/// Whether an error holds the `#if`, `#ifdef` or `#ifndef` of a conditional between the items of a list as a bare
/// token. One inside an initializer list the grammar parsed costs the list nothing.
fn holds_a_listed_conditional(error: Node<'_>, source: &[u8]) -> bool {
    error.children(&mut error.walk()).any(|token| {
        !token.is_named()
            && directive_of(token.kind().as_bytes()) == Directive::Opens
            && opens_between_list_items(source, token.start_byte())
    }) && error
        .parent()
        .is_none_or(|list| list.kind() != "field_initializer_list")
}

/// Whether the directive at `at` stands between the items of a list, as one inside a constructor's initializer list
/// does: the code before it ends in `,` or `) :`, or the code after its line opens with `,` or `:`.
fn opens_between_list_items(source: &[u8], at: usize) -> bool {
    let before = source[..at].trim_ascii_end();
    let after = source[directive_end(source, line_start(source, at))..].trim_ascii_start();
    before.ends_with(b",")
        || before
            .strip_suffix(b":")
            .is_some_and(|head| head.trim_ascii_end().ends_with(b")"))
        || after.starts_with(b",")
        || (after.starts_with(b":") && !after.starts_with(b"::"))
}

fn recovery_of(tree: &Tree, source: &[u8]) -> Recovery {
    let mut recovery = Recovery {
        spills: Vec::new(),
        misread_branch: false,
        stray_branch: false,
        listed_conditional: false,
        misread_bodies: Vec::new(),
        blanks: Vec::new(),
    };
    let parsed_at_the_top = !tree.root_node().is_error();

    preorder(tree.root_node(), |node| {
        if node.is_error() {
            recovery.stray_branch |= node.children(&mut node.walk()).any(|token| {
                !token.is_named() && directive_of(token.kind().as_bytes()) == Directive::Branches
            });
            recovery.listed_conditional =
                recovery.listed_conditional || holds_a_listed_conditional(node, source);
            let (start, end) = (node.start_position().row, node.end_position().row);
            if end.saturating_sub(start) >= RECOVERY_SPILL_ROWS {
                let class_end = closing_row_of_the_class_it_opens(node, source);
                recovery
                    .spills
                    .push((start, class_end.map_or(end, |row| row.max(end))));
            }
        }
        let kind = node.kind();
        if kind == "labeled_statement"
            && let Some(label) = label_before_an_endif(node, source)
        {
            recovery.blanks.push(label);
        }
        if !recovery.listed_conditional && matches!(kind, "preproc_if" | "preproc_ifdef") {
            recovery.listed_conditional = opens_between_list_items(source, node.start_byte());
        }
        if !recovery.misread_branch && kind == "preproc_call" {
            recovery.misread_branch =
                node.child_by_field_name("directive")
                    .is_some_and(|directive| {
                        directive_of(&source[directive.byte_range()]) == Directive::Branches
                    });
        }
        if kind == "declaration"
            && let Some(annotation) = misread_data_macro(node, source)
        {
            recovery.blanks.push(annotation);
        }
        if parsed_at_the_top
            && kind == "field_declaration_list"
            && holds_a_misread_member(node, source)
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

// constraint: the grammar reads a conditional in a class body as a run of members, so a member it misreads through its
// macro can sit under a conditional's branch, an artifact of a conditional the second parse takes apart included
/// Whether a member of `body`, or of a conditional `body` holds, is one whose macro the grammar reads as its type.
fn holds_a_misread_member(body: Node<'_>, source: &[u8]) -> bool {
    body.children(&mut body.walk()).any(|member| {
        misreads_a_members_macro(member, source)
            || (member.kind().starts_with("preproc_") && holds_a_misread_member(member, source))
    })
}

fn misreads_a_members_macro(member: Node<'_>, source: &[u8]) -> bool {
    matches!(member.kind(), "field_declaration" | "function_definition")
        && member.child_by_field_name("type").is_some_and(|written| {
            written.kind() == "type_identifier" && is_macro_name(&source[written.byte_range()])
        })
        && misreads_its_type(member)
}

// constraint: the grammar reads a macro that opens a namespace-scope variable, `DAS_API uint32_t flags;`, as its type,
// so the type written after it becomes an error, a declarator of its own or a missing `::` of a scope; the walk takes
// no data from a declaration that holds an error, and without the macro it parses as written
/// The macro a namespace-scope data declaration opens with, past its storage class, where the declaration holds an
/// error outside its initializers.
fn misread_data_macro(declaration: Node<'_>, source: &[u8]) -> Option<Range<usize>> {
    let written = declaration
        .child_by_field_name("type")
        .and_then(leading_type_word)
        .filter(|written| {
            let after = &source[written.end_byte()..];
            let gap = after
                .iter()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            written.kind() == "type_identifier"
                && is_macro_name(&source[written.byte_range()])
                && word_len(&after[gap..]) > 0
        })?;
    let at_namespace_scope =
        std::iter::successors(declaration.parent(), Node::parent).all(|ancestor| {
            let kind = ancestor.kind();
            kind.starts_with("preproc_")
                || matches!(
                    kind,
                    "translation_unit"
                        | "namespace_definition"
                        | "declaration_list"
                        | "linkage_specification"
                )
        });
    if !at_namespace_scope {
        return None;
    }
    let mut cursor = declaration.walk();
    let mut misread = Misread::No;
    for declarator in declaration.children_by_field_name("declarator", &mut cursor) {
        if declarator.kind() != "init_declarator" {
            misread = misread.max(Misread::of_part(declarator));
            continue;
        }
        let value = declarator.child_by_field_name("value");
        for part in child_nodes(declarator).filter(|part| Some(*part) != value) {
            misread = misread.max(Misread::of_part(part));
        }
    }
    for child in child_nodes(declaration).filter(|child| child.is_error() || child.is_missing()) {
        misread = misread.max(Misread::of_part(child));
    }
    (misread == Misread::Yes).then(|| written.byte_range())
}

/// The word that opens a declaration's type `written`, which is `written` itself or, where a macro opens it, `MACRO` of
/// `MACRO unsigned`, the word the parser's `sized_type_specifier` holds.
pub(crate) fn leading_type_word(written: Node<'_>) -> Option<Node<'_>> {
    match written.kind() {
        "sized_type_specifier" => written.child_by_field_name("type"),
        _ => Some(written),
    }
}

/// What a part of a declaration a macro opens shows, the worst of its parts deciding: no error, the error of a type
/// the macro displaced, or a function's declarator or an error that holds more than a type's words, where the
/// declaration declares no variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Misread {
    No,
    Yes,
    NotData,
}

impl Misread {
    fn of_part(part: Node<'_>) -> Self {
        let mut not_data = part.is_missing() && part.kind() == ";"
            || part.is_error()
                && child_nodes(part).any(|word| word.is_named() && !is_type_word(word));
        preorder(part, |node| {
            not_data |= matches!(node.kind(), "function_declarator" | "parameter_list");
            !not_data
        });
        if not_data {
            Self::NotData
        } else if part.has_error() {
            Self::Yes
        } else {
            Self::No
        }
    }
}

fn is_type_word(word: Node<'_>) -> bool {
    matches!(
        word.kind(),
        "identifier"
            | "type_identifier"
            | "primitive_type"
            | "sized_type_specifier"
            | "qualified_identifier"
            | "namespace_identifier"
            | "template_type"
            | "template_function"
            | "type_qualifier"
            | "storage_class_specifier"
    )
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

/// The macro that opens the line `text` starts with, past the specifiers before it, and the offset of the code after
/// it on that line.
fn opening_macro(text: &[u8]) -> Option<(Range<usize>, usize)> {
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
    (is_macro_name(&text[annotation.clone()]) && declaration > annotation.end)
        .then_some((annotation, declaration))
}

/// The macro that opens the line `text` starts with, past the specifiers before it, where it annotates the declaration
/// after it.
fn annotation_macro(text: &[u8]) -> Option<Range<usize>> {
    opening_macro(text)
        .filter(|(_, declaration)| annotates(&text[*declaration..]))
        .map(|(annotation, _)| annotation)
}

// constraint: the grammar reads a macro before an out-of-line constructor, `PX_INLINE Sc::Shape::Shape(bool b)\n\t:
// mFlags(0)\n{`, as the constructor's type, and an initializer list takes no type, so the name and the `:` fall into an
// error, the first initializer reads as the function and the body can follow it into the error. A scoped name, its
// parameters and a `:` written after a word hold no other reading
/// The macro that opens the line `text` starts with, past the specifiers before it, where an out-of-line constructor
/// with an initializer list follows it.
fn macro_before_a_constructor(text: &[u8]) -> Option<Range<usize>> {
    let (annotation, name) = opening_macro(text)?;
    let declaration = &text[name..];
    let len = term_len(declaration)?;
    let parameters = &declaration[len + whitespace_len(&declaration[len..])..];
    (bounded_find(&declaration[..len], b"::").is_some()
        && parameters.starts_with(b"(")
        && initializes_members(parameters))
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

// constraint: a macro left before a body, `NO_ANALYSIS {` of `TEST(Suite, Case) NO_ANALYSIS {` once the invocation is
// blanked, reads as the type of a compound literal whose braces hold the body, and the locals past the first statement
// it cannot hold parse at namespace scope
/// The length of the macros `text` starts with, each with or without its arguments, where a body's `{` follows, or 0:
/// ` NO_ANALYSIS` of ` NO_ANALYSIS {`.
fn macros_before_a_body_len(text: &[u8]) -> usize {
    let mut at = 0;
    loop {
        let gap = whitespace_len(&text[at..]);
        let rest = &text[at + gap..];
        let len = word_len(rest);
        if !is_macro_name(&rest[..len]) {
            return if rest.starts_with(b"{") { at } else { 0 };
        }
        at += gap + len;
        let gap = whitespace_len(&text[at..]);
        if text[at + gap..].starts_with(b"(") {
            let Some(group) = group_len(&text[at + gap..], PARENTHESES) else {
                return 0;
            };
            at += gap + group;
        }
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
                    return (next != Some(&b';'))
                        .then(|| end + macros_before_a_body_len(&line[end..]));
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
    fn a_reserved_calling_convention_is_blanked_outside_directives() {
        const SOURCE: &[u8] = b"#define XM_CALLCONV __vectorcall\n#define CC \\\n  __stdcall\n\
                                void __cdecl f(int a);\nint x__cdecl;\nint __cdecl_x;\n";

        let repaired = for_parse(SOURCE).expect("a calling convention in a prototype");

        let changed = changed_offsets(&repaired, SOURCE);
        let at = SOURCE
            .windows(b"__cdecl f".len())
            .position(|window| window == b"__cdecl f")
            .expect("the prototype's convention");
        assert_eq!(changed, (at..at + "__cdecl".len()).collect::<Vec<_>>());

        for modifier in CALL_MODIFIERS {
            let source = [b"void ", modifier, b" f(int a);\n"].concat();
            let repaired = for_parse(&source).expect("a calling convention in a prototype");
            assert_eq!(
                changed_offsets(&repaired, &source),
                (5..5 + modifier.len()).collect::<Vec<_>>(),
                "{}",
                String::from_utf8_lossy(modifier)
            );
        }
    }

    #[test]
    fn a_reserved_calling_convention_keeps_the_function() {
        const SOURCE: &[u8] = b"class IBuild\n{\npublic:\n\tvirtual void __stdcall release() = 0;\n\
                                \tvirtual int __stdcall build(int a);\n\tvirtual HRESULT __stdcall attach(int a) = 0;\n\
                                \tvirtual void ok() = 0;\n};\n\
                                void __cdecl proto_a(int a);\nstatic int __cdecl cmp(const void *a, const void *b);\n\
                                int WINAPI WinMain(int a, int b)\n{\n\treturn 0;\n}\nvoid plain(int a);\n";

        let raw = Cpp::new().extract(SOURCE);

        let symbols: Vec<(&str, Option<&str>)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.signature.as_deref()))
            .collect();
        assert_eq!(
            symbols,
            [
                ("IBuild", None),
                (
                    "IBuild::release",
                    Some("virtual void __stdcall release() = 0")
                ),
                (
                    "IBuild::attach",
                    Some("virtual HRESULT __stdcall attach(int a) = 0")
                ),
                ("IBuild::ok", Some("virtual void ok() = 0")),
                ("WinMain", Some("int WINAPI WinMain(int a, int b)")),
                ("IBuild::build", Some("virtual int __stdcall build(int a)")),
                ("proto_a", Some("void __cdecl proto_a(int a)")),
                (
                    "cmp",
                    Some("static int __cdecl cmp(const void *a, const void *b)")
                ),
                ("plain", Some("void plain(int a)")),
            ]
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
    fn a_class_head_macro_before_a_specialization_is_blanked_up_to_its_arguments() {
        const SOURCE: &[u8] = b"template<> struct RTM_DEPRECATED(\"x\") float_traits<float> {};\n\
                                template <> struct OZZ_BASE_DLL Extern<math::Float2> : Base<1> {};\n\
                                template <> struct MATH_API Less<(1 > 2)> {};\n\
                                struct EXPORT name<int> value;\ntemplate <> struct hash<Key> {};\n";

        let repaired = for_parse(SOURCE).expect("specialization heads with macros");

        assert_eq!(repaired.len(), SOURCE.len());
        let macros: Vec<usize> = ["RTM_DEPRECATED(\"x\")", "OZZ_BASE_DLL", "MATH_API"]
            .iter()
            .flat_map(|word| {
                let start = SOURCE
                    .windows(word.len())
                    .position(|window| window == word.as_bytes())
                    .expect("the macro is in the source");
                start..start + word.len()
            })
            .collect();
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
    fn a_lone_macro_line_is_blanked_at_a_namespace_s_edges_and_nowhere_else() {
        const SOURCE: &[u8] =
            b"namespace a {\nVERSION_BEGIN\n// note\nint f();\n\nVERSION_END\n}\n\
                                namespace b {\nEXPORTED\nint g();\n}\n\
                                struct S {\nBODY_BEGIN\n\nint x;\nBODY_END\n};\n\
                                int t[] = {\n  FIRST,\n  LAST\n};\n\
                                void h() {\n  SCOPE_GUARD\n\n  run();\n  TRAILER\n}\n";

        let repaired = for_parse(SOURCE).expect("lone macros at a namespace's edges");

        let lines: Vec<&[u8]> = repaired.split(|byte| *byte == b'\n').collect();
        let blanked: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !line.is_empty() && line.iter().all(|byte| *byte == b' '))
            .map(|(row, _)| row + 1)
            .collect();
        assert_eq!(blanked, [2, 6]);
        assert_eq!(repaired.len(), SOURCE.len());
    }

    #[test]
    fn a_namespace_closes_at_its_brace_past_branches_that_each_open_one() {
        const SOURCE: &[u8] =
            b"namespace n {\n#ifdef A\nvoid f() {\n#else\nvoid g() {\n#endif\n}\nVERSION_END\n}\n";

        let repaired = for_parse(SOURCE).expect("the closing macro");

        assert_eq!(
            changed_offsets(&repaired, SOURCE),
            (60..71).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_namespace_closes_at_its_brace_past_a_comment_a_directive_opens() {
        const SOURCE: &[u8] =
            b"namespace n {\n#if A /* note\n#else\n*/\nvoid g() {\n#endif\n}\n\nVERSION_END\n}\n";
        let macro_at = SOURCE
            .windows(b"VERSION_END".len())
            .position(|window| window == b"VERSION_END")
            .expect("the closing macro");

        let repaired = for_parse(SOURCE).expect("the closing macro");

        assert_eq!(
            changed_offsets(&repaired, SOURCE),
            (macro_at..macro_at + b"VERSION_END".len()).collect::<Vec<_>>()
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

    #[test]
    fn an_invocation_takes_the_macros_between_it_and_a_body() {
        assert_eq!(
            macro_invocation_len(b"TEST(Suite, Case) NO_ANALYSIS {"),
            Some("TEST(Suite, Case) NO_ANALYSIS".len())
        );
        assert_eq!(
            macro_invocation_len(b"TEST(Suite, Case) LOCKS_EXCLUDED(mu) NO_ANALYSIS { run(); }"),
            Some("TEST(Suite, Case) LOCKS_EXCLUDED(mu) NO_ANALYSIS".len())
        );
        assert_eq!(
            macro_invocation_len(b"TEST(Suite, Case) {"),
            Some("TEST(Suite, Case)".len())
        );
        assert_eq!(
            macro_invocation_len(b"ATTR(malloc) ALLOC_SIZE(1)"),
            Some("ATTR(malloc)".len())
        );
        assert_eq!(
            macro_invocation_len(b"CHECK(ready) NO_ANALYSIS void handler() {"),
            Some("CHECK(ready)".len())
        );
    }

    #[test]
    fn a_test_body_after_a_macro_keeps_its_locals() {
        const SOURCE: &[u8] = br"namespace {
#ifdef SANITIZER
TEST(Mutex, DISABLED_Bug) NO_ANALYSIS {
#else
TEST(Mutex, Bug) NO_ANALYSIS {
#endif
  setup(1);
  Mutex *a = new Mutex;
  Mutex b, c;
}

TEST(Mutex, Plain) {
  Mutex d;
}

int g_count;
}  // namespace
";

        let raw = Cpp::new().extract(SOURCE);

        let symbols: Vec<(&str, bool)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.exported))
            .collect();
        assert_eq!(symbols, [("g_count", false)]);
        let calls: Vec<(&str, u32)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.line))
            .collect();
        assert_eq!(calls, [("setup", 7), ("Mutex", 8)]);
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
    fn a_function_whose_conditional_splits_a_call_and_a_brace_ends_at_its_own_brace() {
        const SOURCE: &[u8] = br#"int open_file(struct file *tif)
{
	if (tif->magic != BIG
	    #if MDI_SUPPORT
	    && tif->magic != MDI) {
		report(tif,
		    "Not a TIFF or MDI file %d",
	    #else
	    ) {
		report(tif,
		    "Not a TIFF file %d",
	    #endif
		    tif->magic);
		goto bad;
	}
	return 1;
bad:
	return 0;
}

int after(int count)
{
	return check(count);
}
"#;

        let raw = Cpp::new().extract(SOURCE);

        let lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(lines, [("open_file", 1, 19), ("after", 21, 24)]);
    }

    #[test]
    fn a_function_whose_label_stands_before_an_endif_ends_at_its_own_brace() {
        const SOURCE: &[u8] = br"void create(int flags)
{
#if defined(PROXY)
	if (flags) {
		goto skip;
	}
	step(flags);
	goto done;

 skip:
#endif  /* PROXY */

	walk(flags);

#if defined(PROXY)
 done:
#endif
	finish(flags);
}

void after(int count)
{
	check(count);
}
";

        let raw = Cpp::new().extract(SOURCE);

        let lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(lines, [("create", 1, 19), ("after", 21, 24)]);
        let calls: Vec<(&str, u32)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.line))
            .collect();
        assert_eq!(
            calls,
            [("step", 7), ("walk", 13), ("finish", 18), ("check", 23)]
        );
    }

    #[test]
    fn only_a_first_branch_that_leaves_a_brace_open_splits_where_an_error_holds_the_directive() {
        let source = b"#if A\n{\n#else\n{\n#endif\n}\n\
                       #if B\n}\nnamespace n {\n#else\n#endif\n\
                       #if C\n}\n#else\n}\n#endif\n";
        let texts = |split| {
            untaken_branches(source, Some(split))
                .into_iter()
                .map(|range| std::str::from_utf8(&source[range]).expect("utf-8"))
                .collect::<Vec<_>>()
        };

        assert_eq!(texts(BraceSplit::Opens), ["#if A", "#else\n{\n#endif"]);
        assert_eq!(
            texts(BraceSplit::OpensOrCloses),
            [
                "#if A",
                "#else\n{\n#endif",
                "#if B",
                "#else\n#endif",
                "#if C",
                "#else\n}\n#endif"
            ]
        );
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
    fn a_comment_a_directive_opens_belongs_to_its_line() {
        let directive = b"#if A /* note\n#else\n*/\n{\n#else\n{\n#endif\n";
        let brace = b"#if A /* note\n{\n*/\n#else\n{\n#endif\n";

        assert_eq!(untaken_texts(brace), Vec::<&str>::new());
        assert_eq!(
            untaken_texts(directive),
            ["#if A /* note\n#else\n*/", "#else\n{\n#endif"]
        );
    }

    #[test]
    fn a_raw_string_holds_no_brace() {
        let source = b"#if A\nconst char *text = R\"x(\n\")\" {\n)x\";\n#else\n{\n#endif\n\
                       #if B\nauto open = R\"({)\"; {\n#else\n{\n#endif\n";

        assert_eq!(untaken_texts(source), ["#if B", "#else\n{\n#endif"]);
    }

    fn untaken_texts(source: &[u8]) -> Vec<&str> {
        untaken_branches(source, Some(BraceSplit::OpensOrCloses))
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

    #[test]
    fn only_a_misread_variables_macro_is_blanked() {
        const SOURCE: &[u8] = br"DAS_API uint32_t flags;
static STACK_OF(BIO) *split(BIO *bio);
IMGUI_API ImGuiPlatformIO& GetPlatformIO();
ABSL_CONST_INIT static std::string* name ABSL_GUARDED_BY(mu) = nullptr;
PX_INLINE Sc::Shape::Shape(bool b) : mFlags(0) {}
ENGINE_END
uint32_t lone;
STDAPI CreateHandle(int a);
MY_API 1 int odd;
FMT_EXPORT template <typename... T>
void print(std::ostream& os, format_string<T...> fmt, T&&... args) {}
";
        let tree = parse(&grammar(), SOURCE).expect("parse");

        let repaired = after_recovery(&tree, SOURCE).expect("a misread variable");

        assert_eq!(
            changed_offsets(&repaired, SOURCE),
            (0.."DAS_API".len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn only_a_macro_before_an_out_of_line_constructors_initializer_list_is_blanked() {
        const SOURCE: &[u8] = br"static PX_INLINE Sc::Shape::Shape(bool b)
    : mFlags(0) {}
HACD_INLINE Plane<T>::Plane(int a) noexcept : Vec(a) {}
HACD_INLINE Plane::Plane(float x) { m_w = x; }
HACD_INLINE Plane::Plane(float x) ;
HACD_INLINE Plane::Plane();
HACD_INLINE plane();
HACD_INLINE Widget(int x) : mX(x) {}
HACD_INLINE void Widget::Widget(int x) {}
inline Widget::Widget(int x) : mX(x) {}
Inline Widget::Widget(int x) : mX(x) {}
";

        let repaired = for_parse(SOURCE).expect("two constructors");

        let first = "static ".len();
        let second = SOURCE
            .windows(11)
            .position(|word| word == b"HACD_INLINE")
            .expect("the macro");
        assert_eq!(
            changed_offsets(&repaired, SOURCE),
            (first..first + "PX_INLINE".len())
                .chain(second..second + "HACD_INLINE".len())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_constructor_whose_initializer_list_holds_a_conditional_keeps_its_class_members() {
        const SOURCE: &[u8] = br"struct SphereAABBTest_SIMD
{
	PX_FORCE_INLINE SphereAABBTest_SIMD(const Sphere& sphere) :
#ifdef PX_WINDOWS
		mCenter	(V4LoadU(&sphere.center.x)),
#else
		mCenter	(V3LoadU(sphere.center)),
#endif
		mRadius2(FLoad(sphere.radius * sphere.radius))
	{}

	PX_FORCE_INLINE int operator()(const BucketBox& box) const
	{
		return 1;
	}

private:
	SphereAABBTest_SIMD& operator=(const SphereAABBTest_SIMD&);
	const Vec4V		mCenter;
	const FloatV	mRadius2;
};

struct Plain
{
	Plain(int a) :
		mA(a),
		mB(a)
	{}

	int operator()() const { return mA; }

	int mA;
	int mB;
};
";

        let raw = Cpp::new().extract(SOURCE);

        let mut lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.line, symbol.end_line))
            .collect();
        lines.sort_by_key(|&(_, line, _)| line);
        assert_eq!(
            lines,
            [
                ("SphereAABBTest_SIMD", 1, 21),
                ("SphereAABBTest_SIMD::SphereAABBTest_SIMD", 3, 10),
                ("SphereAABBTest_SIMD::operator()", 12, 15),
                ("SphereAABBTest_SIMD::operator=", 18, 18),
                ("SphereAABBTest_SIMD::mCenter", 19, 19),
                ("SphereAABBTest_SIMD::mRadius2", 20, 20),
                ("Plain", 23, 34),
                ("Plain::Plain", 25, 28),
                ("Plain::operator()", 30, 30),
                ("Plain::mA", 32, 32),
                ("Plain::mB", 33, 33)
            ]
        );
    }

    #[test]
    fn a_class_whose_only_constructor_lists_a_conditional_ends_at_its_own_brace() {
        const SOURCE: &[u8] = br"struct ComponentList
{
  ComponentList(int type_, int line_) :
    type{type_}
#if DAGOR_DBGLEVEL > 0
    ,
    line(line_)
#endif
  {
    next = type_;
  }
};

class Constraint
{
	explicit Constraint(const Settings &inSettings) :
#ifdef JPH_DEBUG_RENDERER
		mDrawSize(inSettings.mDrawSize),
#endif // JPH_DEBUG_RENDERER
		mPriority(inSettings.mPriority)
	{
	}
};

namespace internal
{
Impl::Impl(Test* parent)
	: parent_(parent),
#ifdef _MSC_VER
#pragma warning(push)
#pragma warning(disable : 4355)  // Temporarily disables warning 4355 \
								 // (using this in initializer).
	  reporter_(this),
#pragma warning(pop)
#else
	  reporter_(this),
#endif  // _MSC_VER
	  seed_(0)
{
}
}

int after(int count) { return count; }
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
                ("ComponentList", 1, 12),
                ("ComponentList::ComponentList", 3, 11),
                ("Constraint", 14, 23),
                ("Constraint::Constraint", 16, 22),
                ("Impl::Impl", 27, 40),
                ("after", 43, 43)
            ]
        );
    }

    #[test]
    fn a_conditional_the_grammar_reads_inside_a_parsed_initializer_list_is_left_as_written() {
        const SOURCE: &[u8] = br"Value::Value( ValueType type )
   : type_( type )
   , comments_( 0 )
# ifdef JSON_VALUE_USE_INTERNAL_MAP
   , itemIsUsed_( 0 )
#endif
{
   step( type );
}
";
        let tree = parse(&grammar(), SOURCE).expect("parse");

        let repaired = after_recovery(&tree, SOURCE);

        assert!(tree.root_node().has_error());
        assert_eq!(repaired, None);
    }

    #[test]
    fn an_initializer_list_inside_one_branch_of_a_conditional_keeps_its_directives() {
        const SOURCE: &[u8] = br"class PxTransform
{
public:
	PxQuat q;

	PX_FORCE_INLINE PxTransform()
#ifdef DEFAULT_IDENTITY
		: q(0, 0, 0, 1)
#elif defined(DEFAULT_NAN)
#define invalid PxSqrt(-1.0f)
		: q(invalid, invalid, invalid, invalid)
#undef invalid
#endif
	{
	}

	PX_FORCE_INLINE explicit PxTransform(const PxQuat& orientation) : q(orientation)
	{
	}
};

int broken(  ;
";
        let tree = parse(&grammar(), SOURCE).expect("parse");

        let repaired = after_recovery(&tree, SOURCE);

        assert_eq!(repaired, None);
    }

    #[test]
    fn a_member_whose_macro_is_misread_inside_a_conditional_keeps_its_class_open() {
        const SOURCE: &[u8] = br"struct SphereAABBTest_SIMD
{
	PX_FORCE_INLINE SphereAABBTest_SIMD(const Gu::Sphere& sphere) :
#ifdef PX_WINDOWS
		mCenter	(V4LoadU(&sphere.center.x)),
#else
		mCenter	(V3LoadU(sphere.center)),
#endif
		mRadius2(FLoad(sphere.radius * sphere.radius))
	{}

	PX_FORCE_INLINE Ps::IntBool operator()(const BucketBox& box) const
	{
	#ifdef PX_WINDOWS
		const Vec4V boxCenter = AlignedLoad(&box.mCenter.x);
		return (Ps::IntBool)BAllTrue3_R(FIsGrtrOrEq(mRadius2, boxCenter));
	#else
		const Vec3V boxCenter = V3LoadU(box.mCenter);
		return (IntBool)BAllEq(FIsGrtrOrEq(mRadius2, V3Dot(boxCenter, boxCenter)), BTTTT());
	#endif
	}

private:
	SphereAABBTest_SIMD& operator=(const SphereAABBTest_SIMD&);
#ifdef PX_WINDOWS
	const Vec4V		mCenter;
#else
	const Vec3V		mCenter;
#endif
	const FloatV	mRadius2;
};
";

        let raw = Cpp::new().extract(SOURCE);

        let mut lines: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.line, symbol.end_line))
            .collect();
        lines.sort_by_key(|&(_, line, _)| line);
        assert_eq!(
            lines,
            [
                ("SphereAABBTest_SIMD", 1, 31),
                ("SphereAABBTest_SIMD::SphereAABBTest_SIMD", 3, 10),
                ("SphereAABBTest_SIMD::operator()", 12, 21),
                ("SphereAABBTest_SIMD::operator=", 24, 24),
                ("SphereAABBTest_SIMD::mCenter", 26, 26),
                ("SphereAABBTest_SIMD::mCenter", 28, 28),
                ("SphereAABBTest_SIMD::mRadius2", 30, 30)
            ]
        );
    }

    fn symbol_lines(source: &[u8]) -> Vec<(String, u32, u32)> {
        let mut lines: Vec<(String, u32, u32)> = Cpp::new()
            .extract(source)
            .symbols
            .into_iter()
            .map(|symbol| (symbol.qualified_name, symbol.line, symbol.end_line))
            .collect();
        lines.sort_by_key(|&(_, line, _)| line);
        lines
    }

    #[test]
    fn a_macro_line_above_a_template_leaves_its_class_a_destructor_and_a_constructor() {
        const SOURCE: &[u8] = br"JPH_NAMESPACE_BEGIN

template <class F>
class ScopeExit : public NonCopyable
{
public:
	JPH_INLINE explicit	ScopeExit(F &&inFunction) : mFunction(std::move(inFunction)) { }

	JPH_INLINE			~ScopeExit() { if (!mInvoked) mFunction(); }

	JPH_INLINE void		Invoke()
	{
		mFunction();
	}

private:
	F					mFunction;
	bool				mInvoked = false;
};

JPH_NAMESPACE_END
";

        let lines = symbol_lines(SOURCE);

        let named = |name: &str| lines.iter().filter(|line| line.0 == name).count();
        assert_eq!(named("ScopeExit::ScopeExit"), 1, "{lines:?}");
        assert_eq!(named("ScopeExit::~ScopeExit"), 1, "{lines:?}");
    }

    #[test]
    fn a_destructor_a_macro_precedes_under_a_conditional_keeps_its_name() {
        const SOURCE: &[u8] = br"class PruningPool
{
public:
	PruningPool();
#ifdef __SPU__
	PX_FORCE_INLINE ~PruningPool() {}
#else
	virtual ~PruningPool();
#endif
};
";

        let lines = symbol_lines(SOURCE);

        assert_eq!(
            lines
                .iter()
                .find(|line| line.1 == 6)
                .map(|line| line.0.as_str()),
            Some("PruningPool::~PruningPool"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_head_each_branch_writes_is_read_with_the_first_branch_taken() {
        const SOURCE: &[u8] = br"#ifndef PX_TRIANGLE
#define PX_TRIANGLE
#ifndef PX_DOXYGEN
namespace physx
{
#endif

class PxTriangle
{
public:
	PX_FORCE_INLINE	PxTriangle() {}
	PX_FORCE_INLINE	~PxTriangle() {}

#ifdef __SPU__
	void normalOf(PxVec3& normal) const
#else
	PX_FORCE_INLINE	void normalOf(PxVec3& normal) const
#endif
	{
		normal = 1;
	}

	PX_FORCE_INLINE	PxReal area() const
	{
		return 1;
	}
};

#ifndef PX_DOXYGEN
}
#endif

#endif
";

        assert_eq!(
            symbol_lines(SOURCE),
            [
                ("physx::PxTriangle".to_owned(), 8, 27),
                ("PxTriangle::PxTriangle".to_owned(), 11, 11),
                ("PxTriangle::~PxTriangle".to_owned(), 12, 12),
                ("PxTriangle::normalOf".to_owned(), 15, 21),
                ("PxTriangle::area".to_owned(), 23, 26),
            ]
        );
    }

    #[test]
    fn a_head_each_branch_writes_leaves_the_file_its_misread_destructor() {
        const SOURCE: &[u8] = br"#ifdef WITH_TOPOLOGY
bool Adjacencies::GetBoundaryVertices(PxU32 count, bool* status) const
#else
bool Adjacencies::GetBoundaryVertices(PxU32 count, bool* status, const Face* faces) const
#endif
{
#ifndef WITH_TOPOLOGY
	if(!faces)
	{
		return false;
	}
#endif
	return true;
}

#ifdef WITH_TOPOLOGY
bool Adjacencies::MakeLastRef(AdjTriangle& triangle, PxU32 vref)
#else
bool Adjacencies::MakeLastRef(AdjTriangle& triangle, PxU32 vref, Face* topology)
#endif
{
#ifndef WITH_TOPOLOGY
	if(!topology)
	{
		return false;
	}
#endif

	PxU32 Id = 0;
#ifdef WITH_TOPOLOGY
	if(triangle.v[0]==vref)
#else
	if(topology->v[0]==vref)
#endif
	{
		return true;
	}
#ifdef WITH_TOPOLOGY
	else if(triangle.v[1]==vref)
#else
	else if(topology->v[1]==vref)
#endif
	{
		return false;
	}
	return false;
}

class AdjEdge : public Gu::EdgeData, public Ps::UserAllocated
{
public:
	PX_INLINE	AdjEdge()	{}
	PX_INLINE	~AdjEdge()	{}

	PxU32		mFaceNb;
};
";

        assert_eq!(
            symbol_lines(SOURCE),
            [
                ("Adjacencies::GetBoundaryVertices".to_owned(), 2, 14),
                ("Adjacencies::MakeLastRef".to_owned(), 17, 47),
                ("AdjEdge".to_owned(), 49, 56),
                ("AdjEdge::AdjEdge".to_owned(), 52, 52),
                ("AdjEdge::~AdjEdge".to_owned(), 53, 53),
                ("AdjEdge::mFaceNb".to_owned(), 55, 55),
            ]
        );
    }

    #[test]
    fn a_pointer_written_against_a_heads_name_is_no_part_of_it() {
        const SOURCE: &[u8] =
            b"#ifdef ONE\nPxU32* Pool::find(int a)\n#else\nPxU32 *Pool::find(int a, int b)\n#endif\n{\n}\n";

        assert!(for_parse(SOURCE).is_some());
    }

    #[test]
    fn every_branch_of_a_split_head_names_it() {
        const ALIKE: &[u8] =
            b"#if ONE\nint run(int a)\n#elif TWO\nint run(long a)\n#else\nint run(char a)\n#endif\n{\n}\n";
        const LAST_APART: &[u8] =
            b"#if ONE\nint run(int a)\n#elif TWO\nint run(long a)\n#else\nint walk(char a)\n#endif\n{\n}\n";

        assert!(for_parse(ALIKE).is_some());
        assert_eq!(for_parse(LAST_APART), None);
    }

    #[test]
    fn the_parentheses_of_a_parameter_name_no_head() {
        const SOURCE: &[u8] =
            b"#ifdef ONE\nint run(void (*callback)(int))\n#else\nint run(void (*hook)(int))\n#endif\n{\n}\n";

        assert!(for_parse(SOURCE).is_some());
    }

    #[test]
    fn a_conditional_of_initializer_lists_is_a_split_head() {
        const SOURCE: &[u8] = br"Context::Context(Context&& rhs) noexcept
#if WITH_LOADER
	: m_loader(rhs.m_loader), m_dispatcher(rhs.m_dispatcher)
#elif WITH_NAN
#define invalid sqrt(-1.0f)
	: m_dispatcher(invalid)
#undef invalid
#endif
{
}
";

        let repaired = for_parse(SOURCE).expect("a split head");

        assert_eq!(
            String::from_utf8_lossy(&repaired)
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>(),
            [
                "Context::Context(Context&& rhs) noexcept",
                "",
                "\t: m_loader(rhs.m_loader), m_dispatcher(rhs.m_dispatcher)",
                "",
                "",
                "",
                "",
                "",
                "{",
                "}",
            ]
        );
    }

    const HEADS_NO_BRANCH_CAN_STAND_FOR: &[u8] = br"#ifdef ONE
int main_of_one()
#else
int main_of_other()
#endif
{
}
#ifdef ONE
void One::run()
#else
void Other::run()
#endif
{
}
#ifdef ONE
Value Value::operator+(int a)
#else
Value Value::operator-(int a)
#endif
{
}
#ifdef ONE
decltype(factory()) One::first()
#else
decltype(factory()) One::second()
#endif
{
}
#ifdef ONE
int guarded_one() LOCKED_BY(mtx)
#else
int guarded_other() LOCKED_BY(mtx)
#endif
{
}
#ifdef ONE
DECLARED_AS(int) f()
#else
DECLARED_AS(int) f(int a)
#endif
{
}
#ifdef ONE
int g()
#endif
{
}
#ifdef ONE
#else
int g()
#endif
{
}
#ifdef ONE
int g()
#else
#endif
{
}
#ifdef ONE
int g();
#else
int g(int a);
#endif
{
}
#ifdef ONE
int g() {
#else
int g(int a) {
#endif
}
#ifdef ONE
int g()
#else
int g(int a)
#endif
const int after = 0;
#ifdef ONE
int g()
#else
#ifdef TWO
int g(int a)
#endif
{
}
#endif
#ifdef ONE
void h()
{
#else
void h(int a)
{
#endif
	{
	}
}
";

    #[test]
    fn only_a_head_each_branch_names_alike_is_read_with_the_first_branch_taken() {
        assert_eq!(for_parse(HEADS_NO_BRANCH_CAN_STAND_FOR), None);
    }
}
