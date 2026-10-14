use std::fmt;

use grep_regex::{RegexMatcher, RegexMatcherBuilder};

/// Whether a content search tells letters of different case apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Case {
    /// `Main` finds `Main` and not `main`.
    #[default]
    Sensitive,
    /// `main` finds `main`, `Main` and `MAIN`, by Unicode's simple case folding.
    Insensitive,
}

/// The regular expressions a content search looks for, compiled once.
///
/// Every pattern compiles when the set is built, so holding a `Patterns` is proof that each one parsed and a search that
/// takes it cannot fail on a pattern. A line matches when **any** pattern matches it, and a line several patterns match
/// is reported once.
///
/// # The dialect
///
/// The [`regex`](https://docs.rs/regex) crate's syntax, which is what ripgrep matches with, so a pattern an agent
/// already writes for `rg` works unchanged. Three rules come from searching line by line:
///
/// - **A match never crosses a line.** A pattern that can only match by spanning a line break, such as one holding a
///   literal `\n`, is rejected. A class that merely could, such as `\s`, is narrowed to the characters inside a line.
/// - **`^` and `$` anchor at a line's start and end**, and `$` matches before a `\r\n` as well as before a `\n`, so a
///   file with Windows line endings answers like one without.
/// - **A pattern is unanchored**: `main` finds every line containing it.
///
/// # Examples
///
/// ```
/// use kartta_fs::{Case, PatternError, Patterns};
///
/// let patterns = Patterns::new(["fn main", r"impl\s+Drop"], Case::Sensitive)?;
/// assert_eq!(patterns.patterns().collect::<Vec<_>>(), ["fn main", r"impl\s+Drop"]);
///
/// assert!(matches!(Patterns::new(["main("], Case::Sensitive), Err(PatternError::Pattern { .. })));
/// # Ok::<(), PatternError>(())
/// ```
#[derive(Clone)]
pub struct Patterns {
    given: Box<[Box<str>]>,
    case: Case,
    matcher: RegexMatcher,
}

impl Patterns {
    /// Compiles `patterns` into one matcher.
    ///
    /// # Errors
    ///
    /// Returns [`PatternError::Empty`] when there is no pattern at all, and [`PatternError::Pattern`] for the first
    /// pattern that is empty or will not compile, naming that pattern rather than the set so the caller is handed the
    /// one string to fix. An empty pattern is refused rather than dropped: it matches every line, which is never what a
    /// stray `""` means, and dropping it would answer a different question than the one asked. A set that compiles
    /// pattern by pattern but not as one matcher returns [`PatternError::Oversized`].
    pub fn new<I, P>(patterns: I, case: Case) -> Result<Self, PatternError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<str>,
    {
        let given: Box<[Box<str>]> = patterns
            .into_iter()
            .map(|pattern| pattern.as_ref().into())
            .collect();

        if given.is_empty() {
            return Err(PatternError::Empty);
        }

        let builder = builder(case);
        if given.iter().any(|pattern| pattern.is_empty()) {
            return Err(first_fault(&builder, &given).unwrap_or(PatternError::Empty));
        }
        let matcher = builder.build_many(&given).map_err(|whole| {
            first_fault(&builder, &given).unwrap_or_else(|| PatternError::Oversized {
                count: given.len(),
                reason: whole.to_string(),
            })
        })?;

        Ok(Self {
            given,
            case,
            matcher,
        })
    }

    /// The patterns the set was compiled from, in the order they were given.
    pub fn patterns(&self) -> impl ExactSizeIterator<Item = &str> {
        self.given.iter().map(AsRef::as_ref)
    }

    /// Whether the set tells letters of different case apart.
    #[must_use]
    pub const fn case(&self) -> Case {
        self.case
    }

    pub(crate) const fn matcher(&self) -> &RegexMatcher {
        &self.matcher
    }
}

// constraint: the compiled matcher's own `Debug` prints its automaton, which buries the patterns
impl fmt::Debug for Patterns {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Patterns")
            .field("patterns", &self.given)
            .field("case", &self.case)
            .finish_non_exhaustive()
    }
}

fn builder(case: Case) -> RegexMatcherBuilder {
    let mut builder = RegexMatcherBuilder::new();
    builder
        .case_insensitive(case == Case::Insensitive)
        .crlf(true);

    builder
}

fn first_fault(builder: &RegexMatcherBuilder, patterns: &[Box<str>]) -> Option<PatternError> {
    patterns.iter().find_map(|pattern| {
        if pattern.is_empty() {
            return Some(PatternError::Pattern {
                pattern: String::new(),
                reason: "an empty pattern matches every line".to_owned(),
            });
        }

        builder
            .build(pattern)
            .err()
            .map(|error| PatternError::Pattern {
                pattern: pattern.to_string(),
                reason: error.to_string(),
            })
    })
}

/// Why a [`Patterns`] could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PatternError {
    /// There is nothing to search for.
    #[error("no pattern to search for")]
    Empty,
    /// One pattern is empty or is not a regular expression.
    #[error("invalid pattern {pattern:?}: {reason}")]
    Pattern {
        /// The pattern that did not compile.
        pattern: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The patterns compile one at a time but will not compile into one matcher.
    ///
    /// A limit on the matcher's size rather than a mistake in any single pattern, so no one pattern can be named as the
    /// one to fix.
    #[error("cannot compile {count} patterns into one matcher: {reason}")]
    Oversized {
        /// How many patterns were being compiled together.
        count: usize,
        /// What the matcher reported.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_pattern_at_all_is_refused() {
        assert_eq!(
            Patterns::new(Vec::<&str>::new(), Case::Sensitive).unwrap_err(),
            PatternError::Empty
        );
    }

    #[test]
    fn an_empty_pattern_is_refused_rather_than_dropped() {
        assert!(matches!(
            Patterns::new(["main", ""], Case::Sensitive),
            Err(PatternError::Pattern { pattern, .. }) if pattern.is_empty()
        ));
    }

    #[test]
    fn the_first_fault_in_the_order_given_is_the_one_named() {
        assert!(matches!(
            Patterns::new(["broken(", ""], Case::Sensitive),
            Err(PatternError::Pattern { pattern, .. }) if pattern == "broken("
        ));
        assert!(matches!(
            Patterns::new(["", "broken("], Case::Sensitive),
            Err(PatternError::Pattern { pattern, .. }) if pattern.is_empty()
        ));
    }

    #[test]
    fn the_failing_pattern_is_named_rather_than_the_set() {
        assert!(matches!(
            Patterns::new(["fine", "broken("], Case::Sensitive),
            Err(PatternError::Pattern { pattern, .. }) if pattern == "broken("
        ));
    }

    #[test]
    fn a_pattern_that_can_only_match_across_lines_is_refused() {
        assert!(matches!(
            Patterns::new([r"a\nb"], Case::Sensitive),
            Err(PatternError::Pattern { .. })
        ));
    }

    #[test]
    fn the_patterns_and_the_case_read_back_as_given() {
        let patterns = Patterns::new(["b", "a"], Case::Insensitive).expect("compile");

        assert_eq!(patterns.patterns().collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(patterns.case(), Case::Insensitive);
    }
}
