//! Character classification, resolved from a [`Config`] once per matcher.
//!
//! Deciding a boundary bonus from the configuration directly means scanning the separator slices and running Unicode
//! case predicates for every character of every candidate. Both answers depend only on the configuration, so they are
//! computed once here: an ASCII lookup table for the class of a character, and a class-pair table for the bonus. What
//! remains in the hot loop is one indexed load per character.

use crate::config::Config;

const AFTER_SEGMENT: i32 = 900;
const AFTER_WORD: i32 = 800;
const DIGIT_BOUNDARY: i32 = 800;
const CAMEL_HUMP: i32 = 700;
const AFTER_DOT: i32 = 600;

/// The largest bonus a boundary earns.
pub(crate) const MAX_BOUNDARY: i32 = AFTER_SEGMENT;

/// What a character is, for the purpose of deciding whether the character *after* it starts something worth a bonus.
///
/// The order of the variants is the precedence in which a character is tested, so a `.` configured as a separator is a
/// separator rather than a dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// Ends a path segment, and therefore also decides where the basename starts.
    SegmentSeparator,
    /// Separates words inside a segment.
    WordSeparator,
    /// A literal `.`, which starts an extension or a qualifier.
    Dot,
    /// An ASCII digit.
    Digit,
    /// An uppercase letter — the top of a camelCase hump.
    Upper,
    /// A lowercase letter.
    Lower,
    /// Everything else.
    Other,
}

/// How many variants [`Class`] has, and therefore the side length of the bonus table.
const CLASS_COUNT: usize = 7;

/// Characters covered by the lookup table. Everything above falls back to [`classify`].
const ASCII_COUNT: usize = 128;

/// The lookup tables a [`Config`] resolves to.
#[derive(Debug)]
pub(crate) struct Classes {
    /// Class of every ASCII character.
    ascii: [Class; ASCII_COUNT],
    /// Bonus for a character of class `curr` preceded by one of class `prev`, indexed `[prev][curr]`.
    bonus: [[i32; CLASS_COUNT]; CLASS_COUNT],
    /// Kept for classifying the non-ASCII characters the table does not cover.
    segment_separators: &'static [char],
    word_separators: &'static [char],
}

impl Classes {
    /// Resolves `config` into lookup tables.
    pub(crate) fn new(config: &Config) -> Self {
        let mut ascii = [Class::Other; ASCII_COUNT];
        for (code, slot) in (0u8..).zip(ascii.iter_mut()) {
            *slot = classify(
                char::from(code),
                config.segment_separators,
                config.word_separators,
            );
        }

        let mut bonus = [[0; CLASS_COUNT]; CLASS_COUNT];
        for (prev, row) in bonus.iter_mut().enumerate() {
            for (curr, slot) in row.iter_mut().enumerate() {
                *slot = boundary_bonus(CLASSES[prev], CLASSES[curr]);
            }
        }

        Self {
            ascii,
            bonus,
            segment_separators: config.segment_separators,
            word_separators: config.word_separators,
        }
    }

    /// The class of `c`: one indexed load for ASCII, the full test for anything above it.
    pub(crate) fn class_of(&self, c: char) -> Class {
        let code = u32::from(c) as usize;
        match self.ascii.get(code) {
            Some(&class) => class,
            None => classify(c, self.segment_separators, self.word_separators),
        }
    }

    /// The bonus for a character of class `curr` whose predecessor was of class `prev`.
    pub(crate) const fn bonus(&self, prev: Class, curr: Class) -> i32 {
        self.bonus[prev as usize][curr as usize]
    }
}

/// Every [`Class`] in variant order, so the bonus table can be built by index.
const CLASSES: [Class; CLASS_COUNT] = [
    Class::SegmentSeparator,
    Class::WordSeparator,
    Class::Dot,
    Class::Digit,
    Class::Upper,
    Class::Lower,
    Class::Other,
];

/// Classifies one character. The single definition both the ASCII table and the non-ASCII fallback are built from, so
/// the two cannot disagree.
fn classify(c: char, segment_separators: &[char], word_separators: &[char]) -> Class {
    if segment_separators.contains(&c) {
        Class::SegmentSeparator
    } else if word_separators.contains(&c) {
        Class::WordSeparator
    } else if c == '.' {
        Class::Dot
    } else if c.is_ascii_digit() {
        Class::Digit
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_lowercase() {
        Class::Lower
    } else {
        Class::Other
    }
}

/// The boundary bonus for a class pair. Separator classes decide it outright; otherwise a camelCase hump ranks above a
/// digit boundary, and a match mid-token earns nothing.
fn boundary_bonus(prev: Class, curr: Class) -> i32 {
    match prev {
        Class::SegmentSeparator => AFTER_SEGMENT,
        Class::WordSeparator => AFTER_WORD,
        Class::Dot => AFTER_DOT,
        Class::Lower if curr == Class::Upper => CAMEL_HUMP,
        Class::Digit => 0,
        _ if curr == Class::Digit => DIGIT_BOUNDARY,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boundary rules, stated over characters rather than classes, so a table built wrong is visible here rather
    /// than only as a ranking change.
    #[test]
    fn boundary_bonuses_match_the_documented_precedence() {
        let classes = Classes::new(&Config::DEFAULT);
        let bonus =
            |prev: char, curr: char| classes.bonus(classes.class_of(prev), classes.class_of(curr));

        assert_eq!(bonus('/', 'a'), AFTER_SEGMENT);
        assert_eq!(bonus('\\', 'a'), AFTER_SEGMENT);
        for separator in ['-', '_', ' '] {
            assert_eq!(bonus(separator, 'a'), AFTER_WORD, "separator {separator:?}");
        }
        assert_eq!(bonus('.', 'a'), AFTER_DOT);
        assert_eq!(bonus('o', 'B'), CAMEL_HUMP, "camelCase hump");
        assert_eq!(bonus('v', '2'), DIGIT_BOUNDARY, "digit boundary");
        assert_eq!(bonus('2', '3'), 0, "a digit does not start a digit run");
        assert_eq!(bonus('a', 'b'), 0, "mid-token earns nothing");
        assert_eq!(bonus('A', 'B'), 0, "an uppercase run is not a hump");
    }

    #[test]
    fn no_boundary_earns_more_than_the_largest_boundary_bonus() {
        let classes = Classes::new(&Config::DEFAULT);
        for prev in CLASSES {
            for curr in CLASSES {
                assert!(
                    classes.bonus(prev, curr) <= MAX_BOUNDARY,
                    "{prev:?} then {curr:?}"
                );
            }
        }
    }

    #[test]
    fn a_configured_separator_outranks_the_dot_class() {
        let mut config = Config::DEFAULT;
        config.segment_separators = &['.'];
        let classes = Classes::new(&config);

        assert_eq!(classes.class_of('.'), Class::SegmentSeparator);
        assert_eq!(
            classes.bonus(Class::SegmentSeparator, Class::Lower),
            AFTER_SEGMENT
        );
    }

    #[test]
    fn non_ascii_classifies_like_the_table() {
        let classes = Classes::new(&Config::DEFAULT);

        assert_eq!(classes.class_of('é'), Class::Lower);
        assert_eq!(classes.class_of('É'), Class::Upper);
        assert_eq!(classes.class_of('中'), Class::Other);
        assert_eq!(
            classes.bonus(classes.class_of('é'), classes.class_of('É')),
            CAMEL_HUMP,
            "a hump is a hump outside ASCII too"
        );
    }

    #[test]
    fn a_non_ascii_separator_is_honoured() {
        let mut config = Config::DEFAULT;
        config.segment_separators = &['·'];
        let classes = Classes::new(&config);

        assert_eq!(classes.class_of('·'), Class::SegmentSeparator);
        assert_eq!(
            classes.class_of('/'),
            Class::Other,
            "the default separators are replaced, not extended"
        );
    }
}
