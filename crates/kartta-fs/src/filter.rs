//! [`Filter`] — the compiled globs a path search selects with.

use std::fmt;

use globset::{Candidate, Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// The globs a path search selects with, compiled once.
///
/// Every pattern compiles when the filter is built, so holding a `Filter` is proof that its patterns parsed and a query
/// that takes one cannot fail. A path is selected when **any** pattern matches it, which makes a filter with no
/// patterns match nothing rather than everything — it is the empty disjunction, and "select everything" is a different
/// question spelled differently, as [`FuzzyOptions::filter`](crate::FuzzyOptions::filter) left `None`.
///
/// A filter only ever selects, and what happens to the selection belongs to the query that takes it: a path search
/// keeps what a filter selects, a listing's [`collapse`](crate::ListOptions::collapse) stops at the directories one
/// selects, and its [`exclude`](crate::ListOptions::exclude) leaves out what one selects. What git's rules hide is
/// [`IgnoreSources`](crate::IgnoreSources)'s business.
///
/// # The dialect
///
/// [`globset`](https://crates.io/crates/globset), the one ripgrep and `fd` match with, so a pattern a caller already
/// knows works unchanged. `*` and `?` match inside a segment, `**` crosses segments, `[a-z]` is a character class and
/// `{a,b}` alternates. Three rules settle the rest:
///
/// - **A pattern is anchored at the index root**, which is what an indexed path is relative to. `src/*.rs` finds
///   `src/main.rs` and never `vendor/src/main.rs`; `**/src/*.rs` finds both.
/// - **A pattern containing `/` stops `*` at a segment boundary; one without `/` does not.** So `*.rs` finds
///   `src/main.rs` — a pattern that names a file rather than a place matches it wherever it sits — while `src/*.rs`
///   does not reach `src/deep/main.rs`. A `**/` segment matches zero segments as readily as many, so `src/**/*.rs`
///   finds `src/main.rs` as well.
/// - **`\` escapes the character after it, on every platform.** An indexed path is forward-slash form everywhere, so
///   `\` is never a separator here and is free to be the escape. Pinning it is what keeps one pattern meaning one
///   thing on unix and on Windows, where the dialect's own default splits.
///
/// Matching is case-sensitive, where [`Index::fuzzy`](crate::Index::fuzzy) folds case. A glob is a
/// specification the caller wrote out and a fuzzy query is a guess at one, so the two want opposite answers about a
/// spelling that does not quite match.
///
/// # Examples
///
/// ```
/// use kartta_fs::Filter;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let filter = Filter::new(["*.{rs,toml}"])?;
///
/// assert!(filter.matches("src/main.rs"));
/// assert!(filter.matches("Cargo.toml"));
/// assert!(!filter.matches("README.md"));
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Filter {
    patterns: Box<[Box<str>]>,
    globs: GlobSet,
}

impl Filter {
    /// Compiles `patterns` into a filter.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError::Pattern`] for the first pattern that does not compile, naming that pattern rather than
    /// the set, so the caller is handed the one string to fix. A set that compiles pattern by pattern but not as one
    /// matcher returns [`FilterError::Oversized`].
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{Filter, FilterError};
    ///
    /// assert!(matches!(Filter::new(["src/["]), Err(FilterError::Pattern { .. })));
    /// ```
    pub fn new<I, P>(patterns: I) -> Result<Self, FilterError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<str>,
    {
        let mut given: Vec<Box<str>> = Vec::new();
        let mut globs = GlobSetBuilder::new();

        for pattern in patterns {
            let pattern = pattern.as_ref();
            globs.add(compile(pattern)?);
            given.push(pattern.into());
        }

        let globs = globs.build().map_err(|source| FilterError::Oversized {
            count: given.len(),
            reason: source.kind().to_string(),
        })?;

        Ok(Self {
            patterns: given.into_boxed_slice(),
            globs,
        })
    }

    /// The patterns the filter was compiled from, in the order they were given.
    pub fn patterns(&self) -> impl ExactSizeIterator<Item = &str> {
        self.patterns.iter().map(AsRef::as_ref)
    }

    /// Whether any pattern matches `path`, which is a root-relative forward-slash path as the index spells one.
    ///
    /// A `\` in `path` is an ordinary character of a name on unix, where a filename may hold one, and cannot occur at
    /// all on Windows.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        self.globs.is_match_candidate(&Candidate::from_bytes(path))
    }
}

/// Shows the patterns, which is the whole of what a filter says.
///
/// The compiled matcher derives a [`fmt::Debug`] that prints its automaton, so a derive here would bury the two
/// strings a reader wants under several screens of transition table.
impl fmt::Debug for Filter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Filter")
            .field(&self.patterns)
            .finish()
    }
}

/// Compiles one pattern, giving `*` its reach from whether the pattern names a place or a file.
fn compile(pattern: &str) -> Result<Glob, FilterError> {
    GlobBuilder::new(pattern)
        .literal_separator(pattern.contains('/'))
        .backslash_escape(true)
        .build()
        .map_err(|source| FilterError::Pattern {
            pattern: pattern.to_owned(),
            reason: source.kind().to_string(),
        })
}

/// Why a [`Filter`] could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FilterError {
    /// One pattern is not a glob.
    #[error("invalid glob {pattern:?}: {reason}")]
    Pattern {
        /// The pattern that did not compile.
        pattern: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The patterns are globs one at a time but will not compile into one matcher.
    ///
    /// A limit on the matcher's size rather than a mistake in any single pattern, so no one pattern can be named as
    /// the one to fix. It takes thousands of patterns to reach, which a filter built from a caller's own input does
    /// not and one built from somebody else's can.
    #[error("cannot compile {count} globs into one matcher: {reason}")]
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

    fn filter(patterns: &[&str]) -> Filter {
        Filter::new(patterns).expect("compile the patterns")
    }

    #[test]
    fn a_pattern_without_a_slash_matches_a_file_wherever_it_sits() {
        let filter = filter(&["*.rs"]);

        assert!(filter.matches("main.rs"));
        assert!(filter.matches("src/deep/main.rs"));
    }

    #[test]
    fn a_pattern_with_a_slash_stops_a_star_at_the_segment_boundary() {
        let filter = filter(&["src/*.rs"]);

        assert!(filter.matches("src/main.rs"));
        assert!(!filter.matches("src/deep/main.rs"));
    }

    #[test]
    fn a_double_star_segment_matches_zero_segments_as_well_as_many() {
        let filter = filter(&["src/**/*.rs"]);

        assert!(filter.matches("src/main.rs"));
        assert!(filter.matches("src/deep/main.rs"));
    }

    #[test]
    fn a_pattern_is_anchored_at_the_index_root() {
        let filter = filter(&["src/*.rs"]);

        assert!(filter.matches("src/main.rs"));
        assert!(!filter.matches("vendor/src/main.rs"));
    }

    #[test]
    fn a_path_is_selected_when_any_one_pattern_matches_it() {
        let filter = filter(&["*.rs", "*.toml"]);

        assert!(filter.matches("main.rs"));
        assert!(filter.matches("Cargo.toml"));
        assert!(!filter.matches("README.md"));
    }

    #[test]
    fn a_filter_with_no_patterns_matches_nothing() {
        assert!(
            !Filter::new(Vec::<&str>::new())
                .expect("compile nothing")
                .matches("main.rs")
        );
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert!(!filter(&["*.rs"]).matches("MAIN.RS"));
    }

    #[test]
    fn a_backslash_escapes_the_character_after_it() {
        let filter = filter(&["a\\*b.rs"]);

        assert!(filter.matches("a*b.rs"));
        assert!(!filter.matches("axb.rs"));
    }

    /// Windows cannot spell such a name, so the case exists only where a filename may hold a backslash.
    #[cfg(unix)]
    #[test]
    fn a_backslash_in_a_name_is_an_ordinary_character_and_not_a_separator() {
        assert!(!filter(&["a/*.rs"]).matches("a\\b.rs"));
        assert!(filter(&["*.rs"]).matches("a\\b.rs"));
    }

    #[test]
    fn the_patterns_read_back_in_the_order_they_were_given() {
        let filter = filter(&["*.toml", "*.rs"]);

        assert_eq!(filter.patterns().collect::<Vec<_>>(), ["*.toml", "*.rs"]);
    }

    #[test]
    fn an_invalid_pattern_is_named_rather_than_the_set() {
        let failed = Filter::new(["*.rs", "src/["]).expect_err("reject the unclosed class");

        assert!(
            matches!(&failed, FilterError::Pattern { pattern, .. } if pattern == "src/["),
            "expected the offending pattern, got {failed:?}"
        );
    }
}
