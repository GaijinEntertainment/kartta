//! Path-aware fuzzy scorer that ranks repository paths against a query by optimal alignment.
//!
//! For every candidate containing each term of the query as a subsequence, a dynamic program finds the highest-scoring
//! alignment, rewarding matches at path boundaries — segment starts, word separators, camelCase humps, the basename —
//! and charging for the gaps between matched characters. Scores are integers, so ranking is deterministic across
//! platforms and free of NaN-ordering hazards, and all score arithmetic saturates, so no configuration can make one
//! overflow.
//!
//! One candidate is not aligned: a candidate longer than 1024 positions, which no repository path reaches. Running an
//! `O(n*m)` alignment over a length the caller chose is what that bound exists to prevent, so such a candidate is
//! still reported as a match — dropping it would make the file unfindable — but takes a floor score that ranks it
//! below every candidate that was scored, since a candidate that could not be scored cannot be ordered against ones
//! that were.
//!
//! A full match is the exception to the exception: it is settled by comparing lengths, never by aligning, so the
//! bound does not reach it and an over-long candidate a term matches in full still ranks first. That ordering is one
//! of the guarantees below, and it outranks the floor rather than yielding to it.
//!
//! # What makes it path-aware
//!
//! A match inside the **basename** — the last path segment — earns a bonus on every matched character. That is what
//! makes `main` rank `src/main.rs` above `main/lib.rs`, and it is the behaviour general-purpose fuzzy matchers do not
//! have: they award boundary bonuses uniformly per delimiter, which scores a directory hit and a filename hit alike.
//! The bonus lives inside the alignment recurrence, where it costs nothing.
//!
//! # Configuration
//!
//! Every weight and every policy is a field of [`Config`]. The shipped [`Config::DEFAULT`] is the tuned point in that
//! space and the one the behaviour tests pin; other points are guaranteed only by the invariants that hold across all
//! of them, and only those — a candidate every term matches in full always ranks first ("in full" is per term, not
//! against the whole query), a candidate missing a term never matches, and no weight overflows a score. Replacing
//! [`Config::segment_separators`] is what turns the scorer into something other than a path matcher.
//!
//! # Case
//!
//! Matching ignores case, and so does a full match. Folding is ASCII-first, not full Unicode folding: a non-ASCII
//! character whose fold lands on ASCII (U+212A KELVIN SIGN against `k`, U+017F LATIN SMALL LETTER LONG S against `s`)
//! compares unequal to its ASCII counterpart, while a pair of non-ASCII characters compares by Unicode lowercase.
//! Repository paths are ASCII-dominant and the comparison sits in the per-candidate scan, so the trade is deliberate.
//!
//! # Examples
//!
//! ```
//! use kartta_fuzz::{Config, Matcher};
//!
//! let paths = ["src/lib.rs", "src/main.rs", "main/mod.rs"];
//! let mut matcher = Matcher::new(Config::DEFAULT);
//!
//! let ranked = matcher.rank(paths, "main", None);
//! let names: Vec<&str> = ranked.iter().map(|m| m.candidate).collect();
//!
//! // `src/lib.rs` has no `main`; the filename hit outranks the directory hit.
//! assert_eq!(names, ["src/main.rs", "main/mod.rs"]);
//! ```
//!
//! A query splits on whitespace into terms and every term must match, so a query narrows as it grows:
//!
//! ```
//! use kartta_fuzz::{Config, Matcher};
//! # use std::num::NonZeroUsize;
//!
//! let paths = ["src/auth/handler.go", "src/api/handler.go"];
//! let mut matcher = Matcher::new(Config::DEFAULT);
//!
//! assert_eq!(matcher.rank(paths, "handler", None).len(), 2);
//! assert_eq!(matcher.rank(paths, "handler auth", None).len(), 1);
//! assert_eq!(matcher.rank(paths, "handler", NonZeroUsize::new(1)).len(), 1);
//! ```

mod class;
mod config;
mod matcher;
mod score;

pub use crate::config::{Config, Scoring};
pub use crate::matcher::{Match, Matcher, Ranked, Ranking};
