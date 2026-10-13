//! Scoring weights and matching policy.

/// Per-character weights of the alignment model.
///
/// Every field is a bonus added to, or a penalty subtracted from, the score of one alignment step. The shipped
/// [`Scoring::DEFAULT`] is the tuned point in this space; the tests pin its behaviour, and any other point is
/// guaranteed only by the invariants that hold for all of them, and only those: a candidate every term matches in full
/// still ranks first, a candidate missing a term still scores nothing, and no weight can overflow the score. A
/// contiguous run beating a gapped one is a property of the shipped weights, not of the model.
///
/// The relative magnitudes carry the model, not the absolute values. Match bonuses are three orders of magnitude
/// larger than the gap penalties, so alignment quality decides the rank and gaps only break near-ties.
///
/// # Examples
///
/// ```
/// use kartta_fuzz::Scoring;
///
/// // Rank a filename hit far above a directory hit.
/// let mut scoring = Scoring::DEFAULT;
/// scoring.basename = 400;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scoring {
    /// Awarded when a character immediately follows the previously matched one. Under [`Scoring::DEFAULT`] it is the
    /// strongest signal, so a contiguous run beats any arrangement of the same characters with gaps; raising a
    /// boundary bonus above it reverses that, which is a choice this field leaves open rather than a guarantee it
    /// makes.
    pub consecutive: i32,
    /// Awarded at the start of a path segment — the first character of the candidate, or one following a member of
    /// [`Config::segment_separators`].
    pub after_segment: i32,
    /// Awarded after a member of [`Config::word_separators`].
    pub after_word: i32,
    /// Awarded on a non-digit to digit transition, which starts a version or an index without a separator.
    pub digit_boundary: i32,
    /// Awarded on a lowercase to uppercase transition — a camelCase hump.
    pub camel_hump: i32,
    /// Awarded after `.`, which starts an extension or a qualifier. Never fires when `.` is itself configured as a
    /// segment or word separator, since those classes are tested first.
    pub after_dot: i32,
    /// Awarded per matched character inside the basename — the last path segment. This is what makes a filename hit
    /// outrank a directory hit, and it is the reason this crate exists rather than a general-purpose matcher.
    pub basename: i32,
    /// Awarded once when the first matched character sits at the very start of the candidate. Zero by default: the
    /// segment bonus already covers a whole-path prefix, so this is an extra weight for callers who want completion-
    /// style ranking, where the query is expected to be a prefix of the answer.
    pub candidate_start: i32,
    /// Charged per candidate character skipped before the first match. A value above zero is treated as zero, since
    /// a gap is a penalty.
    pub gap_leading: i32,
    /// Charged per candidate character skipped between two matches. A value above zero is treated as zero, since a
    /// gap is a penalty.
    pub gap_inner: i32,
    /// Charged per candidate character skipped after the last match. Cheaper than an inner gap by default, so a
    /// trailing extension costs a candidate almost nothing. A value above zero is treated as zero, since a gap is a
    /// penalty.
    pub gap_trailing: i32,
}

impl Scoring {
    /// The tuned default: fzy's weights scaled by 1000, plus the basename bonus.
    ///
    /// Integer weights keep scores deterministic across platforms and free of NaN-ordering hazards. Implemented as an
    /// associated constant because [`Default::default`] cannot be called in a `const` context.
    pub const DEFAULT: Self = Self {
        consecutive: 1_000,
        after_segment: 900,
        after_word: 800,
        digit_boundary: 800,
        camel_hump: 700,
        after_dot: 600,
        basename: 150,
        candidate_start: 0,
        gap_leading: -5,
        gap_inner: -10,
        gap_trailing: -5,
    };

    /// The largest bonus one matched character can earn, used to derive the full-match score.
    pub(crate) const fn max_char_bonus(&self) -> i32 {
        let boundary = max(
            max(self.after_segment, self.after_word),
            max(self.digit_boundary, max(self.camel_hump, self.after_dot)),
        );
        max(self.consecutive, boundary).saturating_add(self.basename)
    }

    /// A copy with every gap weight clamped to at most zero.
    ///
    /// A gap weight is a penalty, so a positive one is a contradiction — and not merely a strange ranking. The
    /// recurrence carries an `i32::MIN` sentinel for "no alignment ends here", and adding a positive gap to it lifts
    /// it off that value: a few unmatched positions then promote the sentinel above every real alignment, and a
    /// candidate scores on cells that matched nothing. Clamping once, here, keeps the arithmetic sound for every
    /// value the public field can hold.
    ///
    /// It is also what lets the recurrence add a gap to a state without first asking whether the state is the
    /// sentinel: a non-positive addend cannot lift `i32::MIN`. A bonus has no such guarantee and is guarded per
    /// addition instead, which is the cost this clamp exists to avoid paying on every skipped position.
    pub(crate) const fn clamp_gaps(&self) -> Self {
        Self {
            gap_leading: min(self.gap_leading, 0),
            gap_inner: min(self.gap_inner, 0),
            gap_trailing: min(self.gap_trailing, 0),
            consecutive: self.consecutive,
            after_segment: self.after_segment,
            after_word: self.after_word,
            digit_boundary: self.digit_boundary,
            camel_hump: self.camel_hump,
            after_dot: self.after_dot,
            basename: self.basename,
            candidate_start: self.candidate_start,
        }
    }
}

impl Default for Scoring {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How a query character compares against a candidate character.
///
/// Case folding is deliberately ASCII-first and not full Unicode folding: a non-ASCII character whose fold maps onto
/// ASCII (U+212A KELVIN SIGN against `k`, U+017F LATIN SMALL LETTER LONG S against `s`) compares unequal to its ASCII
/// counterpart. Repository paths are ASCII-dominant and the comparison sits in the per-candidate hot scan, so the
/// trade is deliberate. A pair of non-ASCII characters does fall back to Unicode lowercase comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseMatching {
    /// Case never matters between two characters this crate considers comparable — which, per the note above, means
    /// ASCII case and a pair of non-ASCII characters, not a fold that crosses between the two. U+212A still compares
    /// unequal to `k` here.
    #[default]
    Ignore,
    /// Case matters only when the query contains an uppercase character — a lowercase query matches anything, and
    /// typing a capital narrows it.
    Smart,
    /// Case always matters. The fastest of the three, since it skips folding entirely.
    Respect,
}

/// Weights plus the policy that decides what a segment, a word and a case difference are.
///
/// Construct from [`Config::DEFAULT`] and adjust the fields you need; the type is `#[non_exhaustive]`, so it gains
/// fields without breaking callers, and it is `Copy`, so passing one costs nothing.
///
/// # Examples
///
/// ```
/// use kartta_fuzz::{CaseMatching, Config};
///
/// let mut config = Config::DEFAULT;
/// config.case = CaseMatching::Smart;
/// config.scoring.candidate_start = 500;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The alignment weights.
    pub scoring: Scoring,
    /// How query and candidate characters compare.
    pub case: CaseMatching,
    /// Characters that end a path segment. They decide two things at once: which positions earn
    /// [`Scoring::after_segment`], and where the basename starts — it is everything after the last one. Replace them
    /// to score something other than paths; `['.', ':']` turns the scorer into a symbol matcher.
    pub segment_separators: &'static [char],
    /// Characters that separate words inside a segment.
    pub word_separators: &'static [char],
    /// Whether a shorter candidate wins a tie on score. With this off, candidates that score equally keep their input
    /// order.
    ///
    /// Length is UTF-8 byte length, not a character count: the tiebreak is a proxy for "less path to read", the byte
    /// length is already at hand, and a character count would cost a decode per candidate to settle a tie. The two
    /// differ only for a non-ASCII candidate, where `ééa` counts as longer than `aaaa`.
    pub length_tiebreak: bool,
}

impl Config {
    /// The default policy: forward and backward slash end a segment, `-`, `_` and space separate words, case is
    /// ignored, and shorter candidates win ties.
    ///
    /// Both slashes are segment separators regardless of platform, so a candidate carrying either separator scores the
    /// same way on every target.
    pub const DEFAULT: Self = Self {
        scoring: Scoring::DEFAULT,
        case: CaseMatching::Ignore,
        segment_separators: &['/', '\\'],
        word_separators: &['-', '_', ' '],
        length_tiebreak: true,
    };

    /// The score a term takes against a candidate it matches in full, derived from the active weights.
    ///
    /// Deriving it rather than declaring a constant is what keeps the "a full match always ranks first" guarantee
    /// true for every [`Scoring`]: no alignment can reach the sum of the per-character maxima over the longest
    /// candidate the alignment ever runs on, so no configuration can invert the two.
    ///
    /// Each component is clamped to zero or above, which is what makes that hold for a configuration whose weights are
    /// negative. Summing a negative per-character maximum would drive the full-match score *below* a short partial
    /// match and invert the guarantee it exists to protect; an alignment under such weights cannot exceed zero, so zero
    /// is the bound to sum from.
    pub(crate) const fn full_match_score(&self) -> i32 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "MAX_ALIGNED_POSITIONS is a 1024 literal, far inside i32"
        )]
        const MAX_POSITIONS: i32 = crate::score::MAX_ALIGNED_POSITIONS as i32;

        MAX_POSITIONS
            .saturating_mul(max(0, self.scoring.max_char_bonus()))
            .saturating_add(max(0, self.scoring.candidate_start))
            .saturating_add(1)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `Ord::max` is not usable in a `const fn`.
const fn max(a: i32, b: i32) -> i32 {
    if a > b { a } else { b }
}

/// `Ord::min` is not usable in a `const fn`.
const fn min(a: i32, b: i32) -> i32 {
    if a < b { a } else { b }
}
