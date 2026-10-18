use crate::class::{Class, MAX_BOUNDARY, boundary_bonus, class_of};

const CONSECUTIVE: i32 = 1_000;
const BASENAME: i32 = 150;
// constraint: every gap is negative, so a gap added to NO_ALIGNMENT saturates back onto it and only a bonus needs
// add_bonus's guard
const GAP_LEADING: i32 = -5;
const GAP_INNER: i32 = -10;
const GAP_TRAILING: i32 = -5;

pub(crate) const MAX_ALIGNED_POSITIONS: usize = 1024;

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

pub(crate) const NO_ALIGNMENT: i32 = i32::MIN;
pub(crate) const ALIGNED_MIN: i32 = NO_ALIGNMENT + 2;
pub(crate) const MATCH_FLOOR: i32 = NO_ALIGNMENT + 1;

fn fold_eq(a: char, b: char) -> bool {
    if a == b {
        true
    } else if a.is_ascii() || b.is_ascii() {
        a.eq_ignore_ascii_case(&b)
    } else {
        a.to_lowercase().eq(b.to_lowercase())
    }
}

#[derive(Debug)]
pub(crate) struct Term {
    pub(crate) chars: Vec<char>,
    pub(crate) lowercase_ascii: Option<Vec<u8>>,
}

impl Term {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            lowercase_ascii: text
                .is_ascii()
                .then(|| text.bytes().map(|b| b.to_ascii_lowercase()).collect()),
        }
    }

    pub(crate) fn present_in(&self, candidate: &str) -> bool {
        // constraint: no byte of a multi-byte UTF-8 character is ASCII, so scanning the bytes of an ASCII term finds
        // exactly what scanning characters would
        self.lowercase_ascii.as_ref().map_or_else(
            || is_subsequence_chars(&self.chars, candidate),
            |bytes| is_subsequence_bytes(bytes, candidate.as_bytes()),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Representation {
    #[default]
    Bytes,
    Chars,
}

impl Representation {
    pub(crate) fn of(terms: &[Term]) -> Self {
        if terms.iter().all(|term| term.lowercase_ascii.is_some()) {
            Self::Bytes
        } else {
            Self::Chars
        }
    }
}

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

fn is_subsequence_bytes(term: &[u8], candidate: &[u8]) -> bool {
    let mut at = 0;
    for &wanted in term {
        let found = memchr::memchr2(wanted, wanted.to_ascii_uppercase(), &candidate[at..]);
        match found {
            Some(offset) => at += offset + 1,
            None => return false,
        }
    }
    true
}

#[derive(Debug, Default)]
struct Rows {
    prev_ending: Vec<i32>,
    prev_best: Vec<i32>,
    curr_ending: Vec<i32>,
    curr_best: Vec<i32>,
    len: usize,
}

impl Rows {
    fn ensure(&mut self, len: usize) {
        self.len = len;
        // constraint: cells keep what the last candidate wrote, because every cell is written before it is read: the
        // first row reads nothing of the previous one, and a later row reads only cells the row before it filled
        for row in [
            &mut self.prev_ending,
            &mut self.prev_best,
            &mut self.curr_ending,
            &mut self.curr_best,
        ] {
            if row.len() < len {
                row.resize(len, NO_ALIGNMENT);
            }
        }
    }

    const fn swap(&mut self) {
        std::mem::swap(&mut self.prev_ending, &mut self.curr_ending);
        std::mem::swap(&mut self.prev_best, &mut self.curr_best);
    }

    fn best(&self) -> i32 {
        self.len
            .checked_sub(1)
            .and_then(|last| self.prev_best.get(last))
            .copied()
            .unwrap_or(NO_ALIGNMENT)
    }
}

struct Alignment<'a, T, E> {
    window: &'a [T],
    bonus: &'a [i32],
    window_start: usize,
    after_window: usize,
    basename_start: usize,
    eq: E,
}

impl<T: Copy, E: Fn(T, T) -> bool> Alignment<'_, T, E> {
    fn narrow(self, term: &[T]) -> Self {
        let &[first, .., last] = term else {
            return self;
        };
        let start = self.window.iter().position(|&c| (self.eq)(first, c));
        let end = self.window.iter().rposition(|&c| (self.eq)(last, c));
        let (Some(start), Some(end)) = (start, end) else {
            return self;
        };
        if end <= start {
            return self;
        }

        Self {
            window: &self.window[start..=end],
            bonus: &self.bonus[start..=end],
            window_start: start,
            after_window: self.window.len() - 1 - end,
            ..self
        }
    }

    fn run(&self, term: &[T], rows: &mut Rows) -> i32 {
        let best = match term {
            [only] => self.run_single(*only),
            _ => self.run_rows(term, rows),
        };
        best.saturating_add(to_score(self.after_window).saturating_mul(GAP_TRAILING))
    }

    fn run_single(&self, symbol: T) -> i32 {
        let mut leading = to_score(self.window_start).saturating_mul(GAP_LEADING);
        let mut best = NO_ALIGNMENT;

        for (j, (&c, &bonus)) in self.window.iter().zip(self.bonus.iter()).enumerate() {
            let ending = if (self.eq)(symbol, c) {
                leading
                    .saturating_add(bonus)
                    .saturating_add(self.basename_bonus(j))
            } else {
                NO_ALIGNMENT
            };

            best = ending.max(best.saturating_add(GAP_TRAILING));
            leading = leading.saturating_add(GAP_LEADING);
        }

        best
    }

    fn run_rows(&self, term: &[T], rows: &mut Rows) -> i32 {
        rows.ensure(self.window.len());

        let last = term.len() - 1;
        let gap_of = |i: usize| if i == last { GAP_TRAILING } else { GAP_INNER };

        self.fill_first_row(term[0], rows, gap_of(0));
        for (i, &symbol) in term.iter().enumerate().skip(1) {
            self.fill_row(symbol, rows, gap_of(i));
        }

        rows.best()
    }

    fn fill_first_row(&self, symbol: T, rows: &mut Rows, gap: i32) {
        let mut leading = to_score(self.window_start).saturating_mul(GAP_LEADING);
        let mut left_best = NO_ALIGNMENT;

        // constraint: the rows are zipped rather than indexed, because the compiler cannot see that they share the
        // window's length and would check bounds on every access of the hot loop
        let len = rows.len;
        let cells = rows.curr_ending[..len]
            .iter_mut()
            .zip(rows.curr_best[..len].iter_mut());
        let inputs = self.window.iter().zip(self.bonus.iter());

        for (j, ((cell_ending, cell_best), (&c, &bonus))) in cells.zip(inputs).enumerate() {
            let ending = if (self.eq)(symbol, c) {
                leading
                    .saturating_add(bonus)
                    .saturating_add(self.basename_bonus(j))
            } else {
                NO_ALIGNMENT
            };

            let best = ending.max(left_best.saturating_add(gap));
            *cell_ending = ending;
            *cell_best = best;
            left_best = best;
            leading = leading.saturating_add(GAP_LEADING);
        }

        rows.swap();
    }

    fn fill_row(&self, symbol: T, rows: &mut Rows, gap: i32) {
        let last = self.window.len() - 1;

        rows.curr_ending[0] = NO_ALIGNMENT;
        rows.curr_best[0] = NO_ALIGNMENT;
        let mut left_best = NO_ALIGNMENT;

        let len = rows.len;
        let cells = rows.curr_ending[1..len]
            .iter_mut()
            .zip(rows.curr_best[1..len].iter_mut());
        let inputs = self.window[1..].iter().zip(self.bonus[1..].iter());
        let carried = rows.prev_ending[..last]
            .iter()
            .zip(rows.prev_best[..last].iter());

        for (j, (((cell_ending, cell_best), (&c, &bonus)), (&prev_ending, &prev_best))) in
            cells.zip(inputs).zip(carried).enumerate()
        {
            let ending = if (self.eq)(symbol, c) {
                let consecutive = add_bonus(prev_ending, CONSECUTIVE);
                let after_gap = add_bonus(prev_best, bonus);
                add_bonus(consecutive.max(after_gap), self.basename_bonus(j + 1))
            } else {
                NO_ALIGNMENT
            };

            let best = ending.max(left_best.saturating_add(gap));
            *cell_ending = ending;
            *cell_best = best;
            left_best = best;
        }

        rows.swap();
    }

    const fn basename_bonus(&self, j: usize) -> i32 {
        if j + self.window_start >= self.basename_start {
            BASENAME
        } else {
            0
        }
    }
}

const fn add_bonus(state: i32, bonus: i32) -> i32 {
    if state == NO_ALIGNMENT {
        NO_ALIGNMENT
    } else {
        state.saturating_add(bonus)
    }
}

fn to_score(positions: usize) -> i32 {
    i32::try_from(positions).unwrap_or(i32::MAX)
}

#[derive(Debug, Default)]
pub(crate) struct Scorer {
    bytes: Vec<u8>,
    chars: Vec<char>,
    bonus: Vec<i32>,
    basename_start: usize,
    // constraint: a lower bound for an over-long candidate, counted only as far as scan_bound
    positions: usize,
    // constraint: past every term and past MAX_ALIGNED_POSITIONS, because all an over-long candidate's length still
    // decides is whether some term equals it
    scan_bound: usize,
    prepared_as_bytes: bool,
    representation: Representation,
    rows: Rows,
}

impl Scorer {
    pub(crate) fn new(representation: Representation, longest_term: usize) -> Self {
        Self {
            representation,
            scan_bound: longest_term.max(MAX_ALIGNED_POSITIONS).saturating_add(1),
            ..Self::default()
        }
    }

    pub(crate) fn prepare(&mut self, candidate: &str) {
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

    pub(crate) fn score_matched(&mut self, term: &Term) -> i32 {
        if term.chars.len() == self.positions {
            return FULL_MATCH_SCORE;
        }
        if self.positions > MAX_ALIGNED_POSITIONS {
            return MATCH_FLOOR;
        }
        self.align(term)
    }

    fn align(&mut self, term: &Term) -> i32 {
        let basename_start = self.basename_start;
        debug_assert!(
            !self.prepared_as_bytes || term.lowercase_ascii.is_some(),
            "a byte-prepared candidate implies every term is ASCII, since Representation::of decided both"
        );

        match (self.prepared_as_bytes, &term.lowercase_ascii) {
            (true, Some(bytes)) => Alignment {
                window: &self.bytes,
                bonus: &self.bonus,
                window_start: 0,
                after_window: 0,
                basename_start,
                eq: |a: u8, b: u8| a == b,
            }
            .narrow(bytes)
            .run(bytes, &mut self.rows),
            _ => Alignment {
                window: &self.chars,
                bonus: &self.bonus,
                window_start: 0,
                after_window: 0,
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

    fn narrowed_and_unnarrowed(candidate: &str, term: &str) -> (i32, i32, bool) {
        let term = Term::new(term);
        let bytes = term.lowercase_ascii.as_deref().unwrap_or_default();

        let mut scorer = Scorer::new(Representation::Bytes, term.chars.len());
        scorer.prepare(candidate);

        let build = || Alignment {
            window: &scorer.bytes,
            bonus: &scorer.bonus,
            window_start: 0,
            after_window: 0,
            basename_start: scorer.basename_start,
            eq: |a: u8, b: u8| a == b,
        };

        let mut rows = Rows::default();
        let unnarrowed = build().run(bytes, &mut rows);
        let window = build().narrow(bytes);
        let engaged = window.window.len() < scorer.bytes.len();
        let narrowed = window.run(bytes, &mut rows);
        (narrowed, unnarrowed, engaged)
    }

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

    fn draw_subsequence(candidate: &str, next: &mut impl FnMut(usize) -> usize) -> String {
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
                let term = draw_subsequence(&candidate, &mut next);
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

        let total = cases.len();
        assert!(
            engaged * 4 > total,
            "narrowing engaged on only {engaged} of {total} cases, so the differential proves little"
        );
    }

    /// `reachable[i][j]` holds when `term[..=i]` matches `candidate` as a subsequence with its last symbol at `j`.
    fn reachable(term: &[u8], candidate: &[u8]) -> Vec<Vec<bool>> {
        let Some((&head, tail)) = term.split_first() else {
            return Vec::new();
        };

        let head_row: Vec<bool> = candidate.iter().map(|&c| c == head).collect();
        let mut rows = vec![head_row];

        for &symbol in tail {
            let mut reached_to_the_left = false;
            let above = rows.last().expect("the head row is pushed before the loop");
            let row = candidate
                .iter()
                .zip(above)
                .map(|(&c, &reached_above)| {
                    let here = reached_to_the_left && symbol == c;
                    reached_to_the_left |= reached_above;
                    here
                })
                .collect();
            rows.push(row);
        }

        rows
    }

    fn assert_only_reachable_cells_carry_a_score(
        candidate: &str,
        term: &str,
        rows: &mut Rows,
    ) -> usize {
        let term = Term::new(term);
        let Some(symbols) = term.lowercase_ascii.as_deref().filter(|b| b.len() >= 2) else {
            return 0;
        };

        let mut scorer = Scorer::new(Representation::Bytes, symbols.len());
        scorer.prepare(candidate);
        if scorer.bytes.len() <= symbols.len() {
            return 0;
        }

        let alignment = Alignment {
            window: &scorer.bytes,
            bonus: &scorer.bonus,
            window_start: 0,
            after_window: 0,
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

            let just_filled = rows.prev_ending[..rows.len].iter().zip(&expected[i]);
            for (j, (&ending, &reached)) in just_filled.enumerate() {
                assert_eq!(
                    ending != NO_ALIGNMENT,
                    reached,
                    "row {i} cell {j} of {term:?} against {candidate:?} holds {ending}"
                );
            }
        }

        symbols.len() * rows.len
    }

    #[test]
    fn no_cell_ever_holds_a_state_no_alignment_reaches() {
        // constraint: a moved sentinel stays far below every real score, so only a cell-by-cell check sees one
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
        let mut rows = Rows::default();

        for (candidate, term) in generated_cases() {
            let Some(symbol) = term.bytes().next() else {
                continue;
            };
            let term = [symbol.to_ascii_lowercase()];

            let mut scorer = Scorer::new(Representation::Bytes, term.len());
            scorer.prepare(&candidate);

            let build = || Alignment {
                window: &scorer.bytes,
                bonus: &scorer.bonus,
                window_start: 0,
                after_window: 0,
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
