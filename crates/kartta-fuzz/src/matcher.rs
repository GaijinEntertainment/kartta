//! The configured ranking engine and the result it produces.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::num::NonZeroUsize;

use crate::class::Classes;
use crate::config::{Config, Scoring};
use crate::score::{ALIGNED_MIN, CaseSensitivity, MATCH_FLOOR, Representation, Scorer, Term};

/// One ranked candidate and the score it earned.
///
/// The candidate is borrowed from the input, so a ranking allocates one `Vec` and copies no path. The score is
/// meaningful only against other scores from the same [`Config`] — it is a ranking key, not a similarity measure, and
/// nothing about its magnitude is stable across configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match<'a> {
    /// The matched candidate, borrowed from the input.
    pub candidate: &'a str,
    /// Its rank key: higher is better.
    pub score: i32,
}

/// What an aggregate was built from, which decides the range it may occupy.
///
/// The three ranges do not overlap, so no arithmetic on the scores inside one can move a candidate into another.
///
/// The variants are ordered worst-first, so folding an aggregate's bands with `min` gives the one it belongs to: any
/// floored term floors the whole aggregate, and one term short of full makes it aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Band {
    /// Some term was too long to align, so nothing about the candidate was actually scored.
    Floored,
    /// At least one term was aligned, and none was floored.
    Aligned,
    /// Every term matched the candidate in full.
    Full,
}

impl Band {
    /// Which band one term's score falls in. Each is reachable only by its own kind of match: an alignment is clamped
    /// clear of the floor below and of the full-match score above.
    const fn of(score: i32, full_match_score: i32) -> Self {
        if score == MATCH_FLOOR {
            Self::Floored
        } else if score == full_match_score {
            Self::Full
        } else {
            Self::Aligned
        }
    }

    /// Maps a summed aggregate into this band's range.
    ///
    /// Clamping here rather than at each addition is what makes the ranges hold for weights that would otherwise
    /// saturate the sum: a saturated total carries no trace of what it was built from, so a floored aggregate and an
    /// aligned one, or a partial and a full one, land on the same value and the order falls to whichever tiebreak is
    /// configured.
    fn apply(self, total: i64) -> i32 {
        let ceiling = match self {
            Self::Floored => return MATCH_FLOOR,
            Self::Full => i64::from(i32::MAX),
            Self::Aligned => i64::from(i32::MAX) - 1,
        };

        i32::try_from(total.clamp(i64::from(ALIGNED_MIN), ceiling)).unwrap_or(i32::MAX)
    }
}

/// A scored candidate inside the bounded heap, ordered so the **worst** compares greatest and pops first.
///
/// `len` is the tiebreak: the candidate's length when [`Config::length_tiebreak`] is on, and zero for every entry when
/// it is off, so one ordering serves both without a branch per comparison. `index` is the input position, which makes
/// the order total and reproduces the stable sort's "equal candidates keep their input order".
#[derive(Debug, PartialEq, Eq)]
struct Worst<'a> {
    candidate: &'a str,
    score: i32,
    len: usize,
    index: usize,
}

impl Ord for Worst<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.len.cmp(&other.len))
            .then_with(|| self.index.cmp(&other.index))
    }
}

impl PartialOrd for Worst<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A configured scorer that ranks candidates against a query.
///
/// The matcher owns the scratch buffers the alignment reuses, so ranking a list allocates once rather than once per
/// candidate. Build one per query and rank a whole list with it; building one per candidate defeats the reuse it
/// exists for.
///
/// A query splits on whitespace into terms, and a candidate must match **every** term — the rank is the sum of the
/// per-term scores. An empty or whitespace-only query matches everything, in input order, at score zero.
///
/// # Examples
///
/// ```
/// use kartta_fuzz::{Config, Matcher};
///
/// let mut matcher = Matcher::new(Config::DEFAULT);
/// let ranked = matcher.rank(["main/lib.rs", "src/main.rs"], "main", None);
///
/// // The filename hit outranks the directory hit.
/// assert_eq!(ranked[0].candidate, "src/main.rs");
/// ```
#[derive(Debug)]
pub struct Matcher {
    config: Config,
    /// The weights the recurrence runs on: the configured ones with the gaps clamped to penalties.
    scoring: Scoring,
    /// The score a full match takes, derived from the configured weights once.
    full_match_score: i32,
    classes: Classes,
    scorer: Scorer,
    terms: Vec<Term>,
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new(Config::DEFAULT)
    }
}

impl Matcher {
    /// Builds a matcher with the given configuration.
    ///
    /// The configuration is resolved into lookup tables here, so this is the one place that pays for it — another
    /// reason to build a matcher per query rather than per candidate.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            classes: Classes::new(&config),
            scoring: config.scoring.as_penalties(),
            full_match_score: config.full_match_score(),
            config,
            scorer: Scorer::default(),
            terms: Vec::new(),
        }
    }

    /// The configuration in force.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Ranks `candidates` against `query`, best first, keeping only those that match every term.
    ///
    /// Equal scores break by shorter candidate when [`Config::length_tiebreak`] is set, and then always by input
    /// order — the sort is stable, so an unchanged input ranks identically every time. `limit` caps the returned
    /// list; `None` returns every match.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fuzz::{Config, Matcher};
    ///
    /// let mut matcher = Matcher::new(Config::DEFAULT);
    /// let paths = ["src/auth/handler.go", "src/api/handler.go"];
    ///
    /// // Both terms must match, so only the auth path survives.
    /// let ranked = matcher.rank(paths, "handler auth", None);
    /// assert_eq!(ranked.len(), 1);
    /// assert_eq!(ranked[0].candidate, "src/auth/handler.go");
    /// ```
    #[must_use]
    pub fn rank<'a>(
        &mut self,
        candidates: impl IntoIterator<Item = &'a str>,
        query: &str,
        limit: Option<NonZeroUsize>,
    ) -> Vec<Match<'a>> {
        self.begin(query);

        // A whitespace-only query matches everything in input order, so a limit is a prefix and nothing is scored.
        if self.terms.is_empty() {
            let keep = limit.map_or(usize::MAX, NonZeroUsize::get);
            return candidates
                .into_iter()
                .take(keep)
                .map(|candidate| Match {
                    candidate,
                    score: 0,
                })
                .collect();
        }

        if let Some(limit) = limit {
            return self.rank_limited(candidates, limit);
        }

        let mut scored: Vec<Match<'a>> = candidates
            .into_iter()
            .filter_map(|candidate| {
                self.score_terms(candidate)
                    .map(|score| Match { candidate, score })
            })
            .collect();
        self.sort(&mut scored);
        scored
    }

    /// Merges the rankings of consecutive slices of one candidate list into the ranking [`Matcher::rank`] gives the
    /// whole list.
    ///
    /// `rankings` holds one [`Matcher::rank`] result per slice, in the order the slices occur in the list, each ranked
    /// with this configuration, this `query` and this `limit`. That is what lets a long list be ranked slice by slice
    /// on several threads: ties break by input order, and the slices arrive in it.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fuzz::{Config, Matcher};
    /// # use std::num::NonZeroUsize;
    ///
    /// let paths = ["main/lib.rs", "src/main.rs", "docs/main.md", "src/lib.rs"];
    /// let limit = NonZeroUsize::new(2);
    /// let mut matcher = Matcher::new(Config::DEFAULT);
    ///
    /// let whole = matcher.rank(paths, "main", limit);
    /// let halves = [
    ///     matcher.rank(paths[..2].iter().copied(), "main", limit),
    ///     matcher.rank(paths[2..].iter().copied(), "main", limit),
    /// ];
    ///
    /// assert_eq!(matcher.merge(halves, "main", limit), whole);
    /// ```
    #[must_use]
    pub fn merge<'a>(
        &self,
        rankings: impl IntoIterator<Item = Vec<Match<'a>>>,
        query: &str,
        limit: Option<NonZeroUsize>,
    ) -> Vec<Match<'a>> {
        let mut merged: Vec<Match<'a>> = rankings.into_iter().flatten().collect();

        let ranked_by_score = query.split_whitespace().next().is_some();
        if ranked_by_score {
            self.sort(&mut merged);
        }

        if let Some(limit) = limit {
            merged.truncate(limit.get());
        }
        merged
    }

    /// The `limit` best matches, kept in a bounded heap rather than collected and sorted.
    ///
    /// Ranking a repository of millions of paths against a permissive query otherwise materializes a `Match` per
    /// survivor purely to throw almost all of them away. The heap holds `limit` entries, so memory is bounded by what
    /// the caller asked for, and once it is full an entry that cannot displace the worst costs a single comparison.
    fn rank_limited<'a>(
        &mut self,
        candidates: impl IntoIterator<Item = &'a str>,
        limit: NonZeroUsize,
    ) -> Vec<Match<'a>> {
        let limit = limit.get();
        let tiebreak = self.config.length_tiebreak;
        let candidates = candidates.into_iter();

        // A limit is a cap on what comes back, not a promise that much exists. Reserving against it alone lets a
        // caller's `usize::MAX` demand an allocation before a single candidate is read, so the reservation is bounded
        // by what the input claims to hold and the heap grows from there.
        let mut heap: BinaryHeap<Worst<'a>> =
            BinaryHeap::with_capacity(limit.min(candidates.size_hint().0));

        for (index, candidate) in candidates.enumerate() {
            let Some(score) = self.score_terms(candidate) else {
                continue;
            };
            let entry = Worst {
                candidate,
                score,
                len: if tiebreak { candidate.len() } else { 0 },
                index,
            };

            if heap.len() < limit {
                heap.push(entry);
            } else if heap.peek().is_some_and(|worst| entry < *worst) {
                heap.pop();
                heap.push(entry);
            }
        }

        // `Worst` orders the worst greatest, so ascending order is best first.
        heap.into_sorted_vec()
            .into_iter()
            .map(|entry| Match {
                candidate: entry.candidate,
                score: entry.score,
            })
            .collect()
    }

    /// The score of one candidate against `query`, or `None` when it does not match every term.
    ///
    /// An empty or whitespace-only query matches everything at score zero. Prefer [`Matcher::rank`] for a list: this
    /// re-splits the query on every call.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fuzz::{Config, Matcher};
    ///
    /// let mut matcher = Matcher::new(Config::DEFAULT);
    /// assert!(matcher.score("src/main.rs", "main").is_some());
    /// assert_eq!(matcher.score("src/lib.rs", "main"), None);
    /// ```
    #[must_use]
    pub fn score(&mut self, candidate: &str, query: &str) -> Option<i32> {
        self.begin(query);

        if self.terms.is_empty() {
            return Some(0);
        }
        self.score_terms(candidate)
    }

    /// Splits `query` into terms and fixes the case policy for the query about to run.
    ///
    /// Both the byte form of each term and the case decision are settled here, once, rather than per candidate.
    fn begin(&mut self, query: &str) {
        let sensitivity = CaseSensitivity::resolve(self.config.case, query);

        self.terms.clear();
        self.terms.extend(
            query
                .split_whitespace()
                .map(|text| Term::new(text, sensitivity)),
        );

        let longest = self.terms.iter().map(|term| term.chars.len()).max();
        self.scorer.begin(
            sensitivity,
            Representation::of(&self.terms),
            longest.unwrap_or(0),
        );
    }

    /// The summed score of every term against `candidate`, or `None` as soon as one term does not match.
    ///
    /// Requires a non-empty term list, which [`Matcher::begin`] has already established.
    fn score_terms(&mut self, candidate: &str) -> Option<i32> {
        if self.terms.is_empty() {
            return None;
        }
        // Cheap reject, streamed over the raw candidate. Every term is checked here rather than only the first,
        // because a candidate that fails any of them is dropped — and proving that before preparation keeps the
        // boundary pass and the alignment off candidates a later term would have rejected anyway.
        let all_present = match self.terms.as_slice() {
            [only] => only.present_in(candidate),
            terms => terms.iter().all(|term| term.present_in(candidate)),
        };
        if !all_present {
            return None;
        }

        self.scorer.prepare(candidate, &self.classes);

        // Every term is a known subsequence; the prefilter just proved it.
        //
        // The sum is taken in `i64` because saturating it in `i32` is what repeatedly cost the ordering: a saturated
        // aggregate carries no information about what it was built from, so a full aggregate and a partial one, or an
        // aligned one and a floored one, collapse onto the same value and the order falls to whichever tiebreak
        // happens to be configured. Summing exactly and banding once afterwards keeps each kind in its own range,
        // whatever the weights.
        let mut total: i64 = 0;
        let mut band = Band::Full;
        for term in &self.terms {
            let score = self
                .scorer
                .score_matched(term, &self.scoring, self.full_match_score);
            band = band.min(Band::of(score, self.full_match_score));
            // Saturating, because the term count is the caller's: enough full terms at ceiling weights overflow even
            // an i64, which panics in debug and wraps in release. Saturating costs the ordering nothing that matters,
            // since the band is the primary key and a total that large is already past any meaningful comparison.
            total = total.saturating_add(i64::from(score));
        }

        Some(band.apply(total))
    }

    /// Orders by descending score, then by the configured tiebreak. `sort_by` is stable, so candidates equal on every
    /// key keep their input position.
    fn sort(&self, matches: &mut [Match<'_>]) {
        if self.config.length_tiebreak {
            matches.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| a.candidate.len().cmp(&b.candidate.len()))
            });
        } else {
            matches.sort_by_key(|m| Reverse(m.score));
        }
    }
}
