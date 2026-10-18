use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::num::NonZeroUsize;

use crate::score::{ALIGNED_MIN, FULL_MATCH_SCORE, MATCH_FLOOR, Representation, Scorer, Term};

// constraint: the variants run worst first, so `min` over a candidate's terms gives its band: one floored term floors
// it, and one term short of full makes it aligned
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Band {
    Floored,
    Aligned,
    Full,
}

impl Band {
    const fn of(score: i32) -> Self {
        if score == MATCH_FLOOR {
            Self::Floored
        } else if score == FULL_MATCH_SCORE {
            Self::Full
        } else {
            Self::Aligned
        }
    }

    fn confine(self, total: i64) -> i32 {
        let ceiling = match self {
            Self::Floored => return MATCH_FLOOR,
            Self::Full => i64::from(i32::MAX),
            Self::Aligned => i64::from(i32::MAX) - 1,
        };

        i32::try_from(total.clamp(i64::from(ALIGNED_MIN), ceiling)).unwrap_or(i32::MAX)
    }
}

fn length_key(candidate: &str) -> u32 {
    u32::try_from(candidate.len()).unwrap_or(u32::MAX)
}

/// One ranked candidate, known by the position it was offered at.
///
/// A ranking holds positions rather than borrows, so it allocates one `Vec` and copies no candidate. The score is a
/// ranking key, not a similarity measure.
///
/// Entries order by rank, the better one first: the higher score, then the shorter candidate, then the earlier
/// position. Sorting a list of them ranks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranked {
    /// The position the candidate was offered at.
    pub position: usize,
    /// Its rank key: higher is better.
    pub score: i32,
    tiebreak: u32,
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.tiebreak.cmp(&other.tiebreak))
            .then_with(|| self.position.cmp(&other.position))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A ranking of candidates against one query, fed one candidate at a time, so its candidates need never be alive
/// together — a path rebuilt into one reused buffer, say.
///
/// A query splits on whitespace into terms, and a candidate must match **every** term — the rank is the sum of the
/// per-term scores. An empty or whitespace-only query matches everything, in input order, at score zero.
///
/// The ranking owns the scratch buffers the alignment reuses, so ranking a list allocates once rather than once per
/// candidate. Positions must rise in the order candidates are offered, because they break ties the way input order
/// does.
///
/// # Examples
///
/// ```
/// use kartta_fuzz::Ranking;
///
/// let paths = ["main/lib.rs", "src/main.rs", "src/lib.rs"];
/// let mut ranking = Ranking::new("main", None);
/// for (position, path) in paths.iter().enumerate() {
///     ranking.offer(position, path);
/// }
///
/// // `src/lib.rs` has no `main`; the filename hit outranks the directory hit.
/// let ranked: Vec<&str> = ranking.finish().iter().map(|found| paths[found.position]).collect();
/// assert_eq!(ranked, ["src/main.rs", "main/lib.rs"]);
/// ```
#[derive(Debug)]
pub struct Ranking {
    scorer: Scorer,
    terms: Vec<Term>,
    limit: Option<NonZeroUsize>,
    heap: BinaryHeap<Ranked>,
    all: Vec<Ranked>,
}

impl Ranking {
    /// Starts a ranking against `query`. `limit` caps the ranking; `None` keeps every match.
    #[must_use]
    pub fn new(query: &str, limit: Option<NonZeroUsize>) -> Self {
        let terms: Vec<Term> = query.split_whitespace().map(Term::new).collect();
        let longest = terms.iter().map(|term| term.chars.len()).max();

        Self {
            scorer: Scorer::new(Representation::of(&terms), longest.unwrap_or(0)),
            terms,
            limit,
            heap: BinaryHeap::new(),
            all: Vec::new(),
        }
    }

    /// Offers one candidate, and answers whether a later candidate can still enter the ranking.
    ///
    /// Only an empty query ever answers `false`: everything matches it in input order, so once the limit is reached
    /// the rest is turned away unscored.
    pub fn offer(&mut self, position: usize, candidate: &str) -> bool {
        if self.terms.is_empty() {
            self.all.push(Ranked {
                position,
                score: 0,
                tiebreak: 0,
            });
            return self.limit.is_none_or(|limit| self.all.len() < limit.get());
        }

        let Some(score) = self.score_terms(candidate) else {
            return true;
        };
        let entry = Ranked {
            position,
            score,
            tiebreak: length_key(candidate),
        };

        let Some(limit) = self.limit else {
            self.all.push(entry);
            return true;
        };

        if self.heap.len() < limit.get() {
            self.heap.push(entry);
        } else if self.heap.peek().is_some_and(|worst| entry < *worst) {
            self.heap.pop();
            self.heap.push(entry);
        }
        true
    }

    /// The ranking, best first, keeping only the candidates that match every term.
    #[must_use]
    pub fn finish(self) -> Vec<Ranked> {
        if self.terms.is_empty() {
            return self.all;
        }

        if self.limit.is_some() {
            return self.heap.into_sorted_vec();
        }

        let mut all = self.all;
        all.sort();
        all
    }

    /// Merges the rankings of consecutive slices of one candidate list into the ranking of the whole list.
    ///
    /// `rankings` holds one ranking per slice, in the order the slices occur in the list, each built with this `query`
    /// and this `limit`. Nothing here can check that: a ranking built otherwise merges into an order no single ranking
    /// of the list would give. That is what lets a long list be ranked slice by slice on several threads: ties break
    /// by input order, and the slices arrive in it.
    ///
    /// Merging is associative: a merge of consecutive rankings is itself a ranking of their slices put together, so
    /// rankings can be merged in any grouping that keeps their order — pairwise as parallel tasks finish, say — and
    /// answer as one merge of them all.
    #[must_use]
    pub fn merge(
        rankings: impl IntoIterator<Item = Vec<Ranked>>,
        query: &str,
        limit: Option<NonZeroUsize>,
    ) -> Vec<Ranked> {
        let mut merged: Vec<Ranked> = rankings.into_iter().flatten().collect();

        if query.split_whitespace().next().is_some() {
            merged.sort();
        }

        if let Some(limit) = limit {
            merged.truncate(limit.get());
        }
        merged
    }

    fn score_terms(&mut self, candidate: &str) -> Option<i32> {
        let all_present = match self.terms.as_slice() {
            [only] => only.present_in(candidate),
            terms => terms.iter().all(|term| term.present_in(candidate)),
        };
        if !all_present {
            return None;
        }

        self.scorer.prepare(candidate);

        // constraint: summed exactly in `i64` and banded once after — an `i32` sum saturates on enough terms and then
        // no longer says which band it came from
        let mut total: i64 = 0;
        let mut band = Band::Full;
        for term in &self.terms {
            let score = self.scorer.score_matched(term);
            band = band.min(Band::of(score));
            total += i64::from(score);
        }

        Some(band.confine(total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ranked_entry_stays_a_position_and_two_32_bit_keys() {
        assert_eq!(
            size_of::<Ranked>(),
            size_of::<usize>() + 2 * size_of::<u32>(),
            "an unbounded ranking holds one of these per match, so every byte is paid per match"
        );
    }
}
