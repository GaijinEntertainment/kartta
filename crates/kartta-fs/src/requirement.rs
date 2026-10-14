use regex_syntax::ast::parse::ParserBuilder;
use regex_syntax::hir::translate::TranslatorBuilder;
use regex_syntax::hir::{Class, Hir, HirKind};

use crate::patterns::Case;
use crate::trigram::{self, KELVIN_SIGN, LONG_S, Trigram};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Requirement {
    Anything,
    Trigram(Trigram),
    All(Vec<Self>),
    Any(Vec<Self>),
}

impl Requirement {
    pub(crate) fn of(patterns: &[Box<str>], case: Case) -> Self {
        Self::any(
            patterns
                .iter()
                .map(|pattern| parse(pattern, case).map_or(Self::Anything, |hir| of_hir(&hir))),
        )
    }

    fn all(parts: impl IntoIterator<Item = Self>) -> Self {
        let mut flat = Vec::new();
        for part in parts {
            match part {
                Self::Anything => {}
                Self::All(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }

        match flat.len() {
            0 => Self::Anything,
            1 => flat.swap_remove(0),
            _ => Self::All(flat),
        }
    }

    fn any(parts: impl IntoIterator<Item = Self>) -> Self {
        let mut flat = Vec::new();
        for part in parts {
            match part {
                Self::Anything => return Self::Anything,
                Self::Any(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }

        match flat.len() {
            0 => Self::Anything,
            1 => flat.swap_remove(0),
            _ => Self::Any(flat),
        }
    }
}

// constraint: these are the settings `grep-regex` translates a pattern under for `Patterns`, so the expression read
// here is the one the matcher runs; a pattern this parse rejects requires nothing, which is never wrong
fn parse(pattern: &str, case: Case) -> Option<Hir> {
    let ast = ParserBuilder::new().build().parse(pattern).ok()?;

    TranslatorBuilder::new()
        .utf8(false)
        .unicode(true)
        .crlf(true)
        .case_insensitive(case == Case::Insensitive)
        .build()
        .translate(pattern, &ast)
        .ok()
}

fn of_hir(hir: &Hir) -> Requirement {
    match hir.kind() {
        HirKind::Concat(pieces) => of_concat(pieces),
        HirKind::Alternation(branches) => Requirement::any(branches.iter().map(of_hir)),
        HirKind::Repetition(repetition) if repetition.min >= 1 => of_hir(&repetition.sub),
        HirKind::Capture(capture) => of_hir(&capture.sub),
        HirKind::Literal(_) | HirKind::Class(_) => of_concat(std::slice::from_ref(hir)),
        HirKind::Empty | HirKind::Look(_) | HirKind::Repetition(_) => Requirement::Anything,
    }
}

// constraint: adjacent literal pieces spell one folded run, so a trigram straddling two of them — a case-folded letter
// class beside a literal digit — is required too; any other piece ends the run and requires what it requires alone
fn of_concat(pieces: &[Hir]) -> Requirement {
    let mut parts = Vec::new();
    let mut run = Run::default();

    for piece in pieces {
        match piece.kind() {
            HirKind::Literal(literal) => run.literal(&literal.0, &mut parts),
            HirKind::Class(class) => match folded_class(class) {
                Some(byte) => run.bytes.push(byte),
                None => parts.push(run.end()),
            },
            _ => {
                parts.push(run.end());
                parts.push(of_hir(piece));
            }
        }
    }
    parts.push(run.end());

    Requirement::all(parts)
}

#[derive(Default)]
struct Run {
    bytes: Vec<u8>,
}

impl Run {
    fn literal(&mut self, literal: &[u8], parts: &mut Vec<Requirement>) {
        for chunk in literal.utf8_chunks() {
            let mut valid = chunk.valid().as_bytes();
            while !valid.is_empty() {
                let (folded, width) = trigram::fold(valid);
                self.bytes.push(folded);
                valid = &valid[width..];
            }
            for &byte in chunk.invalid() {
                self.stray_byte(byte, parts);
            }
        }
    }

    // constraint: a byte of a sequence the file side folds, standing outside a whole character, can match text that
    // folding turned into a letter, so no trigram may be required across it
    fn stray_byte(&mut self, byte: u8, parts: &mut Vec<Requirement>) {
        if KELVIN_SIGN.contains(&byte) || LONG_S.contains(&byte) {
            parts.push(self.end());
        } else {
            self.bytes.push(byte);
        }
    }

    fn end(&mut self) -> Requirement {
        let mut trigrams: Vec<Trigram> = self
            .bytes
            .windows(3)
            .map(|window| trigram::pack(window[0], window[1], window[2]))
            .collect();
        self.bytes.clear();
        trigrams.sort_unstable();
        trigrams.dedup();

        Requirement::all(trigrams.into_iter().map(Requirement::Trigram))
    }
}

// constraint: case-insensitive translation turns a letter into the class of its spellings — `[Aa]`, `[Kk\u{212A}]`,
// `[Ss\u{17F}]` — every one of which the file side folds to the same byte; any other class could match bytes the run
// does not name
fn folded_class(class: &Class) -> Option<u8> {
    const MOST_SPELLINGS: u32 = 3;

    let ranges: Vec<(u32, u32)> = match class {
        Class::Unicode(class) => class
            .ranges()
            .iter()
            .map(|range| (u32::from(range.start()), u32::from(range.end())))
            .collect(),
        Class::Bytes(class) => class
            .ranges()
            .iter()
            .map(|range| (u32::from(range.start()), u32::from(range.end())))
            .collect(),
    };
    let members: u32 = ranges.iter().map(|&(start, end)| end - start + 1).sum();
    if members == 0 || members > MOST_SPELLINGS {
        return None;
    }

    let unicode = matches!(class, Class::Unicode(_));
    let mut folded = None;
    for member in ranges.iter().flat_map(|&(start, end)| start..=end) {
        let byte = fold_member(member, unicode)?;
        if folded.is_some_and(|folded| folded != byte) {
            return None;
        }
        folded = Some(byte);
    }

    folded
}

// constraint: a member folds only if `trigram::fold` folds its whole spelling to one byte, as the file side reads it; a
// byte class member past ASCII is a raw byte that can sit inside a folded sequence, so it folds to nothing
fn fold_member(member: u32, unicode: bool) -> Option<u8> {
    let mut encoded = [0; 4];
    let spelling = if unicode {
        char::from_u32(member)?.encode_utf8(&mut encoded).as_bytes()
    } else {
        encoded[0] = u8::try_from(member).ok().filter(u8::is_ascii)?;
        &encoded[..1]
    };
    let (folded, width) = trigram::fold(spelling);

    (width == spelling.len()).then_some(folded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigram(text: &str) -> Requirement {
        let [a, b, c] = text.as_bytes() else {
            panic!("{text:?} is not three bytes");
        };
        Requirement::Trigram(trigram::pack(*a, *b, *c))
    }

    fn all(texts: &[&str]) -> Requirement {
        let mut trigrams: Vec<Trigram> = texts
            .iter()
            .map(|text| match trigram(text) {
                Requirement::Trigram(trigram) => trigram,
                _ => unreachable!(),
            })
            .collect();
        trigrams.sort_unstable();
        Requirement::All(trigrams.into_iter().map(Requirement::Trigram).collect())
    }

    fn of(patterns: &[&str], case: Case) -> Requirement {
        let patterns: Vec<Box<str>> = patterns.iter().map(|&pattern| pattern.into()).collect();
        Requirement::of(&patterns, case)
    }

    fn sensitive(pattern: &str) -> Requirement {
        of(&[pattern], Case::Sensitive)
    }

    #[test]
    fn a_literal_requires_each_of_its_trigrams_once_in_the_index_casing() {
        assert_eq!(sensitive("abcd"), all(&["abc", "bcd"]));
        assert_eq!(
            sensitive("ABcD"),
            all(&["abc", "bcd"]),
            "the index folds case"
        );
        assert_eq!(sensitive("aaaa"), trigram("aaa"));
    }

    #[test]
    fn a_literal_shorter_than_a_trigram_requires_nothing() {
        assert_eq!(sensitive("ab"), Requirement::Anything);
    }

    #[test]
    fn patterns_are_a_union_and_one_requiring_nothing_makes_it_require_nothing() {
        assert_eq!(
            of(&["abc", "xyz"], Case::Sensitive),
            Requirement::Any(vec![trigram("abc"), trigram("xyz")])
        );
        assert_eq!(of(&["abc", r"\w+"], Case::Sensitive), Requirement::Anything);
    }

    #[test]
    fn a_look_around_ends_the_run_without_widening_it() {
        assert_eq!(sensitive(r"\babc\b"), trigram("abc"));
        assert_eq!(sensitive(r"ab\bcd"), Requirement::Anything);
    }

    #[test]
    fn a_wide_class_ends_the_run() {
        assert_eq!(sensitive("abc[0-9]def"), all(&["abc", "def"]));
    }

    #[test]
    fn an_optional_piece_requires_nothing_and_a_repeated_one_requires_itself() {
        assert_eq!(sensitive("(abc)?"), Requirement::Anything);
        assert_eq!(sensitive("(abc)*"), Requirement::Anything);
        assert_eq!(sensitive("(abc)+"), trigram("abc"));
    }

    #[test]
    fn an_alternation_inside_a_concatenation_requires_one_of_its_branches() {
        assert_eq!(
            sensitive("xyz(abc|def)"),
            Requirement::All(vec![
                trigram("xyz"),
                Requirement::Any(vec![trigram("abc"), trigram("def")])
            ])
        );
    }

    #[test]
    fn case_insensitive_letters_join_one_run_with_what_surrounds_them() {
        assert_eq!(of(&["ab1d"], Case::Insensitive), all(&["ab1", "b1d"]));
        assert_eq!(
            sensitive("(?i)ab1d"),
            all(&["ab1", "b1d"]),
            "an inline flag folds as the option does"
        );
        assert_eq!(sensitive("[Aa]bc"), trigram("abc"));
        assert_eq!(
            sensitive("[Ab]bc"),
            Requirement::Anything,
            "two letters are no one spelling"
        );
    }

    #[test]
    fn letters_whose_case_folding_reaches_past_ascii_still_narrow() {
        assert_eq!(
            of(&["abkcd"], Case::Insensitive),
            all(&["abk", "bkc", "kcd"])
        );
        assert_eq!(of(&["task"], Case::Insensitive), all(&["ask", "tas"]));
        assert_eq!(sensitive("a\u{212A}c"), trigram("akc"));
    }

    #[test]
    fn a_letter_whose_case_folding_reaches_past_ascii_ends_the_run() {
        assert_eq!(of(&["abcédef"], Case::Insensitive), all(&["abc", "def"]));
    }

    #[test]
    fn other_characters_past_ascii_stay_in_the_run_as_their_bytes() {
        let bytes = "a€".as_bytes();
        let expected =
            Requirement::all(bytes.windows(3).map(|window| {
                Requirement::Trigram(trigram::pack(window[0], window[1], window[2]))
            }));

        assert_eq!(
            sensitive("a€"),
            expected,
            "€ begins with a byte KELVIN SIGN begins with too"
        );
    }

    #[test]
    fn a_lone_byte_of_a_folded_sequence_ends_the_run() {
        assert_eq!(sensitive(r"(?-u)abc\xBFdef"), all(&["abc", "def"]));
        assert_eq!(sensitive(r"(?-u)abc\xE2\x84def"), all(&["abc", "def"]));
    }

    #[test]
    fn a_byte_class_folds_only_its_ascii_letters() {
        assert_eq!(sensitive(r"(?-u)[Aa]bc"), trigram("abc"));
        assert_eq!(
            sensitive(r"(?-u)ab[\xAA]cd"),
            Requirement::Anything,
            "a raw byte past ASCII can sit inside a sequence the file side folds"
        );
    }

    #[test]
    fn a_pattern_the_parse_rejects_requires_nothing() {
        assert_eq!(sensitive("abc("), Requirement::Anything);
    }
}
