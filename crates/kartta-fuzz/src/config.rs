//! Matching policy.

/// The policy that decides what a segment and a word are.
///
/// Construct from [`Config::DEFAULT`] and adjust the fields you need; the type is `#[non_exhaustive]`, so it gains
/// fields without breaking callers, and it is `Copy`, so passing one costs nothing.
///
/// # Examples
///
/// ```
/// use kartta_fuzz::Config;
///
/// let mut config = Config::DEFAULT;
/// config.segment_separators = &['/'];
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Characters that end a path segment. They decide two things at once: which positions earn the segment bonus,
    /// and where the basename starts — it is everything after the last one. Replace them to score something other
    /// than paths; `['.', ':']` turns the scorer into a symbol matcher.
    pub segment_separators: &'static [char],
    /// Characters that separate words inside a segment.
    pub word_separators: &'static [char],
    /// Whether a shorter candidate wins a tie on score. With this off, candidates that score equally keep their input
    /// order.
    ///
    /// Length is UTF-8 byte length, not a character count: the tiebreak is a proxy for "less path to read", the byte
    /// length is already at hand, and a character count would cost a decode per candidate to settle a tie. The two
    /// differ only for a non-ASCII candidate, where `ééa` counts as longer than `aaaa`.
    ///
    /// Lengths compare up to 4 GiB, which keeps every ranked entry two words. Candidates longer than that tie on
    /// length and keep their input order.
    pub length_tiebreak: bool,
}

impl Config {
    /// The default policy: forward and backward slash end a segment, `-`, `_` and space separate words, and shorter
    /// candidates win ties.
    ///
    /// Both slashes are segment separators regardless of platform, so a candidate carrying either separator scores the
    /// same way on every target.
    pub const DEFAULT: Self = Self {
        segment_separators: &['/', '\\'],
        word_separators: &['-', '_', ' '],
        length_tiebreak: true,
    };
}

impl Default for Config {
    fn default() -> Self {
        Self::DEFAULT
    }
}
