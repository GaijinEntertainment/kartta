//! Character classification and the boundary bonuses it decides.
//!
//! Both tables are built at compile time, so what remains in the hot loop is one indexed load per character: the
//! class of an ASCII character, and the bonus of a class pair.

const AFTER_SEGMENT: i32 = 900;
const AFTER_WORD: i32 = 800;
const DIGIT_BOUNDARY: i32 = 800;
const CAMEL_HUMP: i32 = 700;
const AFTER_DOT: i32 = 600;

/// The largest bonus a boundary earns.
pub(crate) const MAX_BOUNDARY: i32 = AFTER_SEGMENT;

/// What a character is, for the purpose of deciding whether the character *after* it starts something worth a bonus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// `/`, which ends a path segment and therefore also decides where the basename starts. A `\` is a character of
    /// a name.
    SegmentSeparator,
    /// `-`, `_` or a space, which separate words inside a segment.
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

/// The class of every ASCII character.
const ASCII: [Class; 128] = {
    let mut table = [Class::Other; 128];
    let mut code: u8 = 0;
    while code < 128 {
        table[code as usize] = match code {
            b'/' => Class::SegmentSeparator,
            b'-' | b'_' | b' ' => Class::WordSeparator,
            b'.' => Class::Dot,
            b'0'..=b'9' => Class::Digit,
            b'A'..=b'Z' => Class::Upper,
            b'a'..=b'z' => Class::Lower,
            _ => Class::Other,
        };
        code += 1;
    }
    table
};

/// Bonus for a character of class `curr` preceded by one of class `prev`, indexed `[prev][curr]`.
const BONUS: [[i32; CLASS_COUNT]; CLASS_COUNT] = {
    let mut table = [[0; CLASS_COUNT]; CLASS_COUNT];
    let mut prev = 0;
    while prev < CLASS_COUNT {
        let mut curr = 0;
        while curr < CLASS_COUNT {
            table[prev][curr] = bonus_rule(CLASSES[prev], CLASSES[curr]);
            curr += 1;
        }
        prev += 1;
    }
    table
};

/// The class of `c`: one indexed load for ASCII. No separator lies outside ASCII, so a non-ASCII character is a
/// letter or nothing.
pub(crate) fn class_of(c: char) -> Class {
    match ASCII.get(u32::from(c) as usize) {
        Some(&class) => class,
        None if c.is_uppercase() => Class::Upper,
        None if c.is_lowercase() => Class::Lower,
        None => Class::Other,
    }
}

/// The bonus for a character of class `curr` whose predecessor was of class `prev`.
pub(crate) const fn boundary_bonus(prev: Class, curr: Class) -> i32 {
    BONUS[prev as usize][curr as usize]
}

/// The boundary bonus for a class pair. Separator classes decide it outright; otherwise a camelCase hump ranks above a
/// digit boundary, and a match mid-token earns nothing.
const fn bonus_rule(prev: Class, curr: Class) -> i32 {
    match prev {
        Class::SegmentSeparator => AFTER_SEGMENT,
        Class::WordSeparator => AFTER_WORD,
        Class::Dot => AFTER_DOT,
        Class::Lower if matches!(curr, Class::Upper) => CAMEL_HUMP,
        Class::Digit => 0,
        _ if matches!(curr, Class::Digit) => DIGIT_BOUNDARY,
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
        let between = |prev: char, curr: char| boundary_bonus(class_of(prev), class_of(curr));

        assert_eq!(between('/', 'a'), AFTER_SEGMENT);
        assert_eq!(
            between('\\', 'a'),
            0,
            "a backslash is a character of a name"
        );
        for separator in ['-', '_', ' '] {
            assert_eq!(
                between(separator, 'a'),
                AFTER_WORD,
                "separator {separator:?}"
            );
        }
        assert_eq!(between('.', 'a'), AFTER_DOT);
        assert_eq!(between('o', 'B'), CAMEL_HUMP, "camelCase hump");
        assert_eq!(between('v', '2'), DIGIT_BOUNDARY, "digit boundary");
        assert_eq!(between('2', '3'), 0, "a digit does not start a digit run");
        assert_eq!(between('a', 'b'), 0, "mid-token earns nothing");
        assert_eq!(between('A', 'B'), 0, "an uppercase run is not a hump");
    }

    #[test]
    fn no_boundary_earns_more_than_the_largest_boundary_bonus() {
        for prev in CLASSES {
            for curr in CLASSES {
                assert!(
                    boundary_bonus(prev, curr) <= MAX_BOUNDARY,
                    "{prev:?} then {curr:?}"
                );
            }
        }
    }

    #[test]
    fn non_ascii_classifies_like_the_table() {
        assert_eq!(class_of('é'), Class::Lower);
        assert_eq!(class_of('É'), Class::Upper);
        assert_eq!(class_of('中'), Class::Other);
        assert_eq!(
            boundary_bonus(class_of('é'), class_of('É')),
            CAMEL_HUMP,
            "a hump is a hump outside ASCII too"
        );
    }
}
