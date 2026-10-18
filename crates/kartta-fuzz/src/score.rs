//! The alignment dynamic program and the scratch it reuses.
//!
//! Repository paths are ASCII-dominant, so a candidate is prepared as folded bytes whenever it and the query are
//! both ASCII, and as characters otherwise. The alignment itself is written once, generic over the symbol type and
//! the comparison, so the two representations cannot drift apart.

use crate::class::{Class, MAX_BOUNDARY, boundary_bonus, class_of};

const CONSECUTIVE: i32 = 1_000;
const BASENAME: i32 = 150;
const GAP_LEADING: i32 = -5;
const GAP_INNER: i32 = -10;
const GAP_TRAILING: i32 = -5;

/// A candidate with more positions than this is over-long: it skips the `O(n*m)` alignment and takes [`MATCH_FLOOR`].
/// Real paths are far shorter; the bound exists so a pathological input cannot make one query quadratic in a value the
/// caller controls.
pub(crate) const MAX_ALIGNED_POSITIONS: usize = 1024;

/// The score a term takes against a candidate it matches in full: one more than the sum of the largest bonus each
/// position can earn over the longest candidate the alignment runs on, so no alignment reaches it.
pub(crate) const FULL_MATCH_SCORE: i32 = {
    let largest_step = if CONSECUTIVE > MAX_BOUNDARY {
        CONSECUTIVE
    } else {
        MAX_BOUNDARY
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "MAX_ALIGNED_POSITIONS is a 1024 literal, far inside i32"
    )]
    let positions = MAX_ALIGNED_POSITIONS as i32;
    positions * (largest_step + BASENAME) + 1
};

/// "No alignment ends here" sentinel. Nothing the recurrence adds can lift it: every gap is a penalty, and a bonus
/// goes through [`add_bonus`].
pub(crate) const NO_ALIGNMENT: i32 = i32::MIN;

/// The lowest score an aggregate of aligned terms may take, one clear of [`MATCH_FLOOR`], so a sum of many poor
/// alignments never reads as a floored candidate.
pub(crate) const ALIGNED_MIN: i32 = NO_ALIGNMENT + 2;

/// Score for a match on a candidate too long to align.
///
/// One above the sentinel, so such a candidate still ranks as a match rather than disappearing, but below every
/// candidate the alignment actually scored. A candidate that could not be scored cannot be ordered against ones that
/// were, and ranking it last is the only choice that never inverts a real comparison — a positive floor would put it
/// above any match whose leading gaps outweighed its bonuses.
pub(crate) const MATCH_FLOOR: i32 = NO_ALIGNMENT + 1;

/// Whether two characters are equal up to case.
///
/// Takes an ASCII fast path whenever either side is ASCII and falls back to Unicode lowercase comparison only for a
/// pair of non-ASCII characters. The crate documentation says why that is not full Unicode folding.
fn fold_eq(a: char, b: char) -> bool {
    if a == b {
        true
    } else if a.is_ascii() || b.is_ascii() {
        a.eq_ignore_ascii_case(&b)
    } else {
        a.to_lowercase().eq(b.to_lowercase())
    }
}

/// One whitespace-separated term of a query, in the forms the scorer needs.
#[derive(Debug)]
pub(crate) struct Term {
    /// The term as characters.
    pub(crate) chars: Vec<char>,
    /// The term as lowercased bytes when it is all-ASCII, so both the prefilter and the alignment can work on bytes.
    /// `None` for a term holding a non-ASCII character.
    pub(crate) bytes: Option<Vec<u8>>,
}

impl Term {
    /// Prepares `text`, lowercasing its bytes once here rather than once per candidate.
    pub(crate) fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            bytes: text
                .is_ascii()
                .then(|| text.bytes().map(|b| b.to_ascii_lowercase()).collect()),
        }
    }

    /// Whether `candidate` contains this term as an ordered subsequence.
    ///
    /// Scanning bytes is exactly equivalent to scanning characters for an ASCII term: every byte of a multi-byte
    /// character is `>= 0x80`, so it can never equal an ASCII term byte, which is the answer the character comparison
    /// reaches through its own ASCII fast path.
    pub(crate) fn present_in(&self, candidate: &str) -> bool {
        self.bytes.as_ref().map_or_else(
            || is_subsequence_chars(&self.chars, candidate),
            |bytes| is_subsequence_bytes(bytes, candidate.as_bytes()),
        )
    }
}

/// Which representation a whole query can use, decided from its terms once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Representation {
    /// Every term is ASCII, so an ASCII candidate is scored as bytes.
    #[default]
    Bytes,
    /// Some term holds a non-ASCII character, so every candidate is scored as characters.
    Chars,
}

impl Representation {
    /// The representation `terms` allow.
    pub(crate) fn of(terms: &[Term]) -> Self {
        if terms.iter().all(|term| term.bytes.is_some()) {
            Self::Bytes
        } else {
            Self::Chars
        }
    }
}

/// Whether `term` occurs in `candidate` as an ordered subsequence, up to case.
///
/// Allocates nothing. Used when either side holds a non-ASCII character.
fn is_subsequence_chars(term: &[char], candidate: &str) -> bool {
    let mut next = 0;
    for c in candidate.chars() {
        if next < term.len() && fold_eq(c, term[next]) {
            next += 1;
            if next == term.len() {
                return true;
            }
        }
    }
    next == term.len()
}

/// Whether an already-lowercased ASCII `term` occurs in raw `candidate` bytes as an ordered subsequence, up to case.
fn is_subsequence_bytes(term: &[u8], candidate: &[u8]) -> bool {
    let mut at = 0;
    for &wanted in term {
        // Searching for each term byte in turn is the same greedy leftmost match the scalar scan performs, but the
        // search itself is vectorized — which is what the overwhelmingly common non-matching candidate pays for.
        let found = memchr::memchr2(wanted, wanted.to_ascii_uppercase(), &candidate[at..]);
        match found {
            Some(offset) => at += offset + 1,
            None => return false,
        }
    }
    true
}

/// The two dynamic-program rows, reused across candidates.
///
/// `D[j]` is the best score of an alignment whose last matched symbol sits at position `j`; `M[j]` is the best over
/// all alignments using positions `0..=j`.
#[derive(Debug, Default)]
struct Rows {
    prev_d: Vec<i32>,
    prev_m: Vec<i32>,
    curr_d: Vec<i32>,
    curr_m: Vec<i32>,
    /// How many leading cells of each row the current alignment uses. The buffers themselves stay at the high-water
    /// mark of every candidate seen.
    len: usize,
}

impl Rows {
    /// Makes room for `len` cells per row, growing only when a candidate is longer than any before it.
    ///
    /// The cells are deliberately left holding whatever the last candidate wrote. Every cell of a row is written
    /// before anything reads it — the first row reads nothing from the previous one, and later rows read only cells
    /// the row before them filled — so initializing them was pure waste, and at millions of candidates it was the
    /// largest single waste in the alignment.
    fn ensure(&mut self, len: usize) {
        self.len = len;
        for row in [
            &mut self.prev_d,
            &mut self.prev_m,
            &mut self.curr_d,
            &mut self.curr_m,
        ] {
            if row.len() < len {
                row.resize(len, NO_ALIGNMENT);
            }
        }
    }

    /// Makes the row just filled the previous row.
    const fn swap(&mut self) {
        std::mem::swap(&mut self.prev_d, &mut self.curr_d);
        std::mem::swap(&mut self.prev_m, &mut self.curr_m);
    }

    /// The whole-candidate best, which after the final swap is the last used column of `prev_m`.
    fn best(&self) -> i32 {
        self.len
            .checked_sub(1)
            .and_then(|last| self.prev_m.get(last))
            .copied()
            .unwrap_or(NO_ALIGNMENT)
    }
}

/// Everything the alignment reads about one prepared candidate, plus how two symbols compare.
///
/// Written once and instantiated for `u8` and for `char`, so the ASCII fast path and the Unicode path cannot diverge.
struct Alignment<'a, T, E> {
    /// The window of the candidate the alignment runs over.
    candidate: &'a [T],
    /// Boundary bonuses for that same window.
    bonus: &'a [i32],
    /// Where the window starts in the whole candidate, so absolute positions stay recoverable.
    offset: usize,
    /// How many candidate positions follow the window, each of which costs one trailing gap.
    trailing: usize,
    /// First position of the basename, in whole-candidate terms.
    basename_start: usize,
    eq: E,
}

impl<T: Copy, E: Fn(T, T) -> bool> Alignment<'_, T, E> {
    /// Builds an alignment over the narrowest window that can hold a match of `term`.
    ///
    /// No alignment can start before the first position matching the term's first symbol, or end after the last
    /// position matching its last, so the rows outside that span carry nothing. Narrowing is exact rather than
    /// approximate: the skipped prefix costs `offset` leading gaps, which the first row starts from, and the skipped
    /// suffix costs `trailing` trailing gaps, which [`Alignment::run`] adds back.
    ///
    /// The span is only worth locating for a term of two or more symbols — for a single symbol the two scans cost
    /// more than the one row they would shorten.
    fn narrow(self, term: &[T]) -> Self {
        let &[first, .., last] = term else {
            return self;
        };
        let start = self.candidate.iter().position(|&c| (self.eq)(first, c));
        let end = self.candidate.iter().rposition(|&c| (self.eq)(last, c));
        let (Some(start), Some(end)) = (start, end) else {
            return self;
        };
        if end <= start {
            return self;
        }

        Self {
            candidate: &self.candidate[start..=end],
            bonus: &self.bonus[start..=end],
            offset: start,
            trailing: self.candidate.len() - 1 - end,
            ..self
        }
    }

    /// Runs the whole alignment of `term` and returns the score of its best arrangement. Requires
    /// `0 < term.len() < candidate.len()`.
    fn run(&self, term: &[T], rows: &mut Rows) -> i32 {
        let best = match term {
            [only] => self.run_single(*only),
            _ => self.run_rows(term, rows),
        };

        // Every alignment ends inside the window, so the gaps over the skipped suffix are the same constant for all of
        // them and are charged once here rather than walked one position at a time.
        best.saturating_add(to_score(self.trailing).saturating_mul(GAP_TRAILING))
    }

    /// Scores a single-symbol term, which needs no rows at all.
    ///
    /// With one symbol there is no previous row to carry, so the recurrence collapses to a running maximum: the best
    /// alignment is the best position to match at, less the trailing gaps from there to the end. That makes this the
    /// same arithmetic as [`Alignment::fill_first_row`] with every array write removed — and a one-character query is
    /// both the most permissive case and the first keystroke of any interactive one.
    fn run_single(&self, symbol: T) -> i32 {
        let mut leading = to_score(self.offset).saturating_mul(GAP_LEADING);
        let mut best = NO_ALIGNMENT;

        for (j, (&c, &bonus)) in self.candidate.iter().zip(self.bonus.iter()).enumerate() {
            let d = if (self.eq)(symbol, c) {
                leading
                    .saturating_add(bonus)
                    .saturating_add(self.basename_bonus(j))
            } else {
                NO_ALIGNMENT
            };

            best = d.max(best.saturating_add(GAP_TRAILING));
            leading = leading.saturating_add(GAP_LEADING);
        }

        best
    }

    /// Scores a term of two or more symbols by filling one row per symbol.
    fn run_rows(&self, term: &[T], rows: &mut Rows) -> i32 {
        rows.ensure(self.candidate.len());

        let last = term.len() - 1;
        // The trailing gap is cheaper than an inner gap, so the last term symbol tolerates a long unmatched suffix —
        // a file extension — without paying inner-gap rates for it.
        let gap_of = |i: usize| if i == last { GAP_TRAILING } else { GAP_INNER };

        self.fill_first_row(term[0], rows, gap_of(0));
        for (i, &symbol) in term.iter().enumerate().skip(1) {
            self.fill_row(symbol, rows, gap_of(i));
        }

        rows.best()
    }

    /// Fills the row for the term's first symbol, which pays a leading gap for every symbol it skips and is the only
    /// row that can align at position 0.
    ///
    /// The row is walked by zipping the buffers rather than indexing them: the compiler cannot see that they are all
    /// the length of the candidate when it indexes, so it emits a bounds check per access, and zipped iterators carry
    /// that proof instead.
    fn fill_first_row(&self, symbol: T, rows: &mut Rows, gap: i32) {
        // The window skipped `offset` positions, each of which the first symbol pays a leading gap for.
        let mut leading = to_score(self.offset).saturating_mul(GAP_LEADING);
        let mut left_m = NO_ALIGNMENT;

        let len = rows.len;
        let cells = rows.curr_d[..len]
            .iter_mut()
            .zip(rows.curr_m[..len].iter_mut());
        let inputs = self.candidate.iter().zip(self.bonus.iter());

        for (j, ((cell_d, cell_m), (&c, &bonus))) in cells.zip(inputs).enumerate() {
            let d = if (self.eq)(symbol, c) {
                leading
                    .saturating_add(bonus)
                    .saturating_add(self.basename_bonus(j))
            } else {
                NO_ALIGNMENT
            };

            let m_here = d.max(left_m.saturating_add(gap));
            *cell_d = d;
            *cell_m = m_here;
            left_m = m_here;
            leading = leading.saturating_add(GAP_LEADING);
        }

        rows.swap();
    }

    /// Fills the row for any term symbol after the first: it either continues the previous match consecutively or
    /// lands after a gap, whichever scores higher.
    ///
    /// Position 0 is settled before the loop, so the body reads the previous row at `j - 1` by zipping slices that are
    /// already offset by one.
    fn fill_row(&self, symbol: T, rows: &mut Rows, gap: i32) {
        let last = self.candidate.len() - 1;

        // A later term symbol cannot align at position 0 — there is no room for what must precede it.
        rows.curr_d[0] = NO_ALIGNMENT;
        rows.curr_m[0] = NO_ALIGNMENT;
        let mut left_m = NO_ALIGNMENT;

        let len = rows.len;
        let cells = rows.curr_d[1..len]
            .iter_mut()
            .zip(rows.curr_m[1..len].iter_mut());
        let inputs = self.candidate[1..].iter().zip(self.bonus[1..].iter());
        let carried = rows.prev_d[..last].iter().zip(rows.prev_m[..last].iter());

        for (j, (((cell_d, cell_m), (&c, &bonus)), (&prev_d, &prev_m))) in
            cells.zip(inputs).zip(carried).enumerate()
        {
            let d = if (self.eq)(symbol, c) {
                let consecutive = add_bonus(prev_d, CONSECUTIVE);
                let after_gap = add_bonus(prev_m, bonus);
                add_bonus(consecutive.max(after_gap), self.basename_bonus(j + 1))
            } else {
                NO_ALIGNMENT
            };

            let m_here = d.max(left_m.saturating_add(gap));
            *cell_d = d;
            *cell_m = m_here;
            left_m = m_here;
        }

        rows.swap();
    }

    /// The basename bonus for a match at position `j`: awarded only inside the last segment, so a query hitting the
    /// filename outranks one hitting a directory.
    const fn basename_bonus(&self, j: usize) -> i32 {
        if j + self.offset >= self.basename_start {
            BASENAME
        } else {
            0
        }
    }
}

/// Adds a bonus to an alignment state, leaving the "no alignment" sentinel where it is.
///
/// A bonus is the only weight that is positive, and adding one to [`NO_ALIGNMENT`] would turn "nothing ends here" into
/// a state that competes with real alignments — leaving the recurrence free to select an alignment that does not
/// exist. A gap needs no such guard, and pays for none: every gap weight is a penalty, so a gap added to the sentinel
/// saturates back onto it.
const fn add_bonus(state: i32, bonus: i32) -> i32 {
    if state == NO_ALIGNMENT {
        NO_ALIGNMENT
    } else {
        state.saturating_add(bonus)
    }
}

/// A position count as a score, for multiplying by a gap weight.
///
/// A count above [`MAX_ALIGNED_POSITIONS`] cannot occur, since the alignment never runs on a longer candidate, but a
/// saturating conversion keeps that an assumption the arithmetic does not depend on.
fn to_score(positions: usize) -> i32 {
    i32::try_from(positions).unwrap_or(i32::MAX)
}

/// Scores one term against one candidate, reusing its buffers across every candidate in a query.
///
/// The candidate is prepared once by [`Scorer::prepare`]; every term of a multi-term query then scores against that
/// same preparation.
#[derive(Debug, Default)]
pub(crate) struct Scorer {
    /// The prepared candidate as normalized bytes, when [`Scorer::prepared_as_bytes`] holds.
    bytes: Vec<u8>,
    /// The prepared candidate as characters, when it does not.
    chars: Vec<char>,
    /// Per-position boundary bonus for a match landing there.
    bonus: Vec<i32>,
    /// First position of the candidate's basename — just past the last segment separator.
    basename_start: usize,
    /// How many positions the prepared candidate occupies. For an over-long candidate this is a lower bound: the scan
    /// stops at [`Scorer::scan_bound`], which is far enough to decide every question still open. Recorded rather than
    /// read off a buffer, because an over-long candidate never fills one.
    positions: usize,
    /// How far a candidate is ever counted.
    scan_bound: usize,
    /// Whether the prepared candidate is in `bytes` rather than `chars`.
    prepared_as_bytes: bool,
    /// The representation the query's terms allow.
    representation: Representation,
    rows: Rows,
}

impl Scorer {
    /// A scorer for a query whose terms allow `representation` and whose longest term has `longest_term` symbols.
    ///
    /// `longest_term` bounds how far a candidate is ever counted: for an over-long candidate the only thing its length
    /// still decides is whether some term equals it, and a candidate longer than every term cannot.
    pub(crate) fn new(representation: Representation, longest_term: usize) -> Self {
        Self {
            representation,
            scan_bound: longest_term.max(MAX_ALIGNED_POSITIONS).saturating_add(1),
            ..Self::default()
        }
    }

    /// Records each position's boundary bonus and the basename start in one pass, reusing the buffers across
    /// candidates.
    ///
    /// Position 0 always earns the segment bonus: the start of a candidate is the start of a segment, which
    /// is why the virtual predecessor of position 0 is [`Class::SegmentSeparator`].
    pub(crate) fn prepare(&mut self, candidate: &str) {
        // An over-long candidate is scored from its length alone, so nothing here is sized to it and
        // nothing reads more of it than that answer needs. Byte length bounds character length for free, so only a
        // candidate long in bytes is scanned at all, and that scan stops once the answer is fixed — otherwise one
        // over-long candidate would cost a full pass to establish a length that is already known to be too large.
        if candidate.len() > MAX_ALIGNED_POSITIONS {
            let counted = candidate.chars().take(self.scan_bound).count();
            if counted > MAX_ALIGNED_POSITIONS {
                self.positions = counted;
                self.prepared_as_bytes = false;
                self.basename_start = 0;
                return;
            }
        }

        self.prepared_as_bytes =
            self.representation == Representation::Bytes && candidate.is_ascii();
        self.positions = if self.prepared_as_bytes {
            candidate.len()
        } else {
            candidate.chars().count()
        };

        self.basename_start = if self.prepared_as_bytes {
            self.prepare_bytes(candidate)
        } else {
            self.prepare_chars(candidate)
        };
    }

    /// Prepares an all-ASCII candidate as lowercased bytes, returning where its basename starts.
    ///
    /// The buffers are sized once and then written through zipped iterators. Pushing instead would re-check capacity
    /// on every byte, and the loop carries a dependency on the previous class, so nothing else recovers that.
    fn prepare_bytes(&mut self, candidate: &str) -> usize {
        let source = candidate.as_bytes();
        self.bonus.resize(source.len(), 0);
        self.bytes.resize(source.len(), 0);

        let mut prev = Class::SegmentSeparator;
        let mut basename_start = 0;

        let cells = self.bonus.iter_mut().zip(self.bytes.iter_mut());
        for (j, ((bonus, out), &byte)) in cells.zip(source.iter()).enumerate() {
            let class = class_of(char::from(byte));
            *bonus = boundary_bonus(prev, class);
            *out = byte.to_ascii_lowercase();
            if class == Class::SegmentSeparator {
                basename_start = j + 1;
            }
            prev = class;
        }

        basename_start
    }

    /// Prepares a candidate as characters, returning where its basename starts.
    ///
    /// The character count is not known without a pass of its own, so this path pushes. It is the rare one — a
    /// candidate or query holding a non-ASCII character.
    fn prepare_chars(&mut self, candidate: &str) -> usize {
        self.bonus.clear();
        self.chars.clear();

        let mut prev = Class::SegmentSeparator;
        let mut basename_start = 0;

        for (j, c) in candidate.chars().enumerate() {
            let class = class_of(c);
            self.bonus.push(boundary_bonus(prev, class));
            self.chars.push(c);
            if class == Class::SegmentSeparator {
                basename_start = j + 1;
            }
            prev = class;
        }

        basename_start
    }

    /// The score of a `term` already known to be a non-empty subsequence of the prepared candidate.
    pub(crate) fn score_matched(&mut self, term: &Term) -> i32 {
        // Equal length plus subsequence means every symbol matched in order: a full match, up to case.
        if term.chars.len() == self.positions {
            return FULL_MATCH_SCORE;
        }
        if self.positions > MAX_ALIGNED_POSITIONS {
            return MATCH_FLOOR;
        }
        self.align(term)
    }

    /// Dispatches to the alignment in whichever representation the candidate was prepared in.
    fn align(&mut self, term: &Term) -> i32 {
        let basename_start = self.basename_start;
        debug_assert!(
            !self.prepared_as_bytes || term.bytes.is_some(),
            "a byte-prepared candidate implies every term is ASCII, since Representation::of decided both"
        );

        match (self.prepared_as_bytes, &term.bytes) {
            (true, Some(bytes)) => Alignment {
                candidate: &self.bytes,
                bonus: &self.bonus,
                offset: 0,
                trailing: 0,
                basename_start,
                eq: |a: u8, b: u8| a == b,
            }
            .narrow(bytes)
            .run(bytes, &mut self.rows),
            _ => Alignment {
                candidate: &self.chars,
                bonus: &self.bonus,
                offset: 0,
                trailing: 0,
                basename_start,
                eq: fold_eq,
            }
            .narrow(&term.chars)
            .run(&term.chars, &mut self.rows),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scores `term` against `candidate` twice — once as bytes, once as characters.
    ///
    /// The ASCII fast path exists only to be faster, so the two representations must return the same score for input
    /// both can handle. Nothing else in the suite would notice them drifting apart.
    fn both_representations(candidate: &str, term: &str) -> (i32, i32) {
        let term = Term::new(term);

        let mut as_bytes = Scorer::new(Representation::Bytes, term.chars.len());
        as_bytes.prepare(candidate);

        let mut as_chars = Scorer::new(Representation::Chars, term.chars.len());
        as_chars.prepare(candidate);

        (as_bytes.score_matched(&term), as_chars.score_matched(&term))
    }

    #[test]
    fn the_byte_path_scores_ascii_exactly_like_the_character_path() {
        let cases = [
            ("src/main.rs", "main"),
            ("main/lib.rs", "main"),
            ("Cargo.toml", "cargo"),
            ("src/cargo/mod.rs", "cargo"),
            ("a/b/c.rs", "abc"),
            ("axbxc.rs", "abc"),
            ("abc.rs", "abc"),
            ("fooBar.rs", "fb"),
            ("foobar.rs", "fb"),
            ("internal/telemetry/renderer/commandBuffer_12.hpp", "render"),
            (
                "crates/engine/shader_cache/descriptor_set_7.cpp",
                "shaderset",
            ),
            ("v2/state_machine.go", "v2"),
            ("x", "x"),
            ("ab", "ab"),
            ("no-match-here.rs", "zzz"),
        ];

        for (candidate, term) in cases {
            let (bytes, chars) = both_representations(candidate, term);
            assert_eq!(bytes, chars, "{term:?} against {candidate:?}");
        }
    }

    /// Scores `term` against a prepared candidate with and without the window.
    fn narrowed_and_unnarrowed(candidate: &str, term: &str) -> (i32, i32, bool) {
        let term = Term::new(term);
        let bytes = term.bytes.as_deref().unwrap_or_default();

        let mut scorer = Scorer::new(Representation::Bytes, term.chars.len());
        scorer.prepare(candidate);

        let build = || Alignment {
            candidate: &scorer.bytes,
            bonus: &scorer.bonus,
            offset: 0,
            trailing: 0,
            basename_start: scorer.basename_start,
            eq: |a: u8, b: u8| a == b,
        };

        let mut rows = Rows::default();
        let unnarrowed = build().run(bytes, &mut rows);
        let window = build().narrow(bytes);
        let engaged = window.candidate.len() < scorer.bytes.len();
        let narrowed = window.run(bytes, &mut rows);
        (narrowed, unnarrowed, engaged)
    }

    /// A path built from `PARTS`, two to five segments deep.
    fn build_candidate(next: &mut impl FnMut(usize) -> usize) -> String {
        const PARTS: &[&str] = &[
            "src",
            "engine",
            "render_pass",
            "fooBar",
            "a",
            "zz",
            "v2",
            "core.impl",
            "x-y",
            "SHOUT",
            "mod.rs",
        ];

        let segments = 1 + next(4);
        let mut candidate = String::new();
        for k in 0..segments {
            if k > 0 {
                candidate.push('/');
            }
            candidate.push_str(PARTS[next(PARTS.len())]);
        }
        candidate
    }

    /// A term drawn as a subsequence of `candidate`, so the alignment actually runs rather than rejecting.
    fn draw_term(candidate: &str, next: &mut impl FnMut(usize) -> usize) -> String {
        let chars: Vec<char> = candidate.chars().collect();
        let take = 1 + next(4);
        let mut term = String::new();
        let mut at = 0;

        for _ in 0..take {
            let remaining = chars.len().saturating_sub(at);
            if remaining == 0 {
                break;
            }
            at += next(remaining);
            let picked = chars.get(at).copied().filter(|c| !c.is_whitespace());
            term.extend(picked);
            at += 1;
        }

        term
    }

    /// Deterministic pseudo-random candidate and term pairs, covering far more shapes than a hand-written table.
    fn generated_cases() -> Vec<(String, String)> {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).unwrap_or(0) % bound.max(1)
        };

        (0..4000)
            .filter_map(|_| {
                let candidate = build_candidate(&mut next);
                let term = draw_term(&candidate, &mut next);
                (!term.is_empty() && term.len() < candidate.len()).then_some((candidate, term))
            })
            .collect()
    }

    #[test]
    fn narrowing_the_window_never_changes_a_score() {
        let cases = generated_cases();
        assert!(
            cases.len() > 1000,
            "the generator produced too few cases: {}",
            cases.len()
        );

        let mut engaged = 0;
        for (candidate, term) in &cases {
            let (narrowed, unnarrowed, narrowing_applied) =
                narrowed_and_unnarrowed(candidate, term);
            assert_eq!(narrowed, unnarrowed, "{term:?} against {candidate:?}");
            engaged += usize::from(narrowing_applied);
        }

        // Without this the differential could pass by never narrowing anything.
        let total = cases.len();
        assert!(
            engaged * 4 > total,
            "narrowing engaged on only {engaged} of {total} cases, so the differential proves little"
        );
    }

    /// Which cells of each row a real alignment can end in: `reachable[i][j]` holds when `term[..=i]` matches as a
    /// subsequence with its last symbol at `j`.
    fn reachable(term: &[u8], candidate: &[u8]) -> Vec<Vec<bool>> {
        let Some((&head, tail)) = term.split_first() else {
            return Vec::new();
        };

        let head_row: Vec<bool> = candidate.iter().map(|&c| c == head).collect();
        let mut rows = vec![head_row];

        for &symbol in tail {
            // Whether the row above is reachable strictly left of the cell being decided.
            let mut left = false;
            let above = rows.last().expect("the head row is pushed before the loop");
            let row = candidate
                .iter()
                .zip(above)
                .map(|(&c, &reached_above)| {
                    let here = left && symbol == c;
                    left |= reached_above;
                    here
                })
                .collect();
            rows.push(row);
        }

        rows
    }

    /// Fills every row for `term` against `candidate`, asserting that a cell carries a score exactly where an
    /// alignment can end and the sentinel everywhere else. Returns how many cells it checked.
    fn assert_only_reachable_cells_carry_a_score(
        candidate: &str,
        term: &str,
        rows: &mut Rows,
    ) -> usize {
        let term = Term::new(term);
        let Some(symbols) = term.bytes.as_deref().filter(|b| b.len() >= 2) else {
            return 0;
        };

        let mut scorer = Scorer::new(Representation::Bytes, symbols.len());
        scorer.prepare(candidate);
        if scorer.bytes.len() <= symbols.len() {
            return 0;
        }

        let alignment = Alignment {
            candidate: &scorer.bytes,
            bonus: &scorer.bonus,
            offset: 0,
            trailing: 0,
            basename_start: scorer.basename_start,
            eq: |a: u8, b: u8| a == b,
        };
        let expected = reachable(symbols, &scorer.bytes);
        let last = symbols.len() - 1;
        rows.ensure(scorer.bytes.len());

        for (i, &symbol) in symbols.iter().enumerate() {
            let gap = if i == last { GAP_TRAILING } else { GAP_INNER };
            if i == 0 {
                alignment.fill_first_row(symbol, rows, gap);
            } else {
                alignment.fill_row(symbol, rows, gap);
            }

            // The row just filled became the previous one on the swap that ended the fill.
            let filled = rows.prev_d[..rows.len].iter().zip(&expected[i]);
            for (j, (&d, &reached)) in filled.enumerate() {
                assert_eq!(
                    d != NO_ALIGNMENT,
                    reached,
                    "row {i} cell {j} of {term:?} against {candidate:?} holds {d}"
                );
            }
        }

        symbols.len() * rows.len
    }

    #[test]
    fn no_cell_ever_holds_a_state_no_alignment_reaches() {
        // Every bonus is added to whatever the row above left behind, so a sentinel that moves becomes a state no
        // alignment produced, competing on the same scale as the ones that did. A moved sentinel stays far below every
        // real score, which is why the invariant is checked cell by cell rather than through a score.
        let mut rows = Rows::default();
        let mut checked = 0;
        for (candidate, term) in &generated_cases() {
            checked += assert_only_reachable_cells_carry_a_score(candidate, term, &mut rows);
        }

        assert!(
            checked > 100_000,
            "only {checked} cells were checked, so the invariant proves little"
        );
    }

    #[test]
    fn the_single_symbol_path_agrees_with_the_row_machinery() {
        // `run_rows` handles one symbol perfectly well; `run_single` only exists to do it without the arrays, so the
        // two must not disagree.
        let mut rows = Rows::default();

        for (candidate, term) in generated_cases() {
            let Some(symbol) = term.bytes().next() else {
                continue;
            };
            let term = [symbol.to_ascii_lowercase()];

            let mut scorer = Scorer::new(Representation::Bytes, term.len());
            scorer.prepare(&candidate);

            let build = || Alignment {
                candidate: &scorer.bytes,
                bonus: &scorer.bonus,
                offset: 0,
                trailing: 0,
                basename_start: scorer.basename_start,
                eq: |a: u8, b: u8| a == b,
            };

            assert_eq!(
                build().run_single(term[0]),
                build().run_rows(&term, &mut rows),
                "{:?} against {candidate:?}",
                char::from(symbol)
            );
        }
    }

    #[test]
    fn a_non_ascii_term_forces_the_character_representation() {
        let ascii = [Term::new("main")];
        let mixed = [Term::new("main"), Term::new("café")];

        assert_eq!(Representation::of(&ascii), Representation::Bytes);
        assert_eq!(Representation::of(&mixed), Representation::Chars);
    }
}
