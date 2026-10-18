//! Path-aware fuzzy scorer that ranks repository paths against a query by optimal alignment.
//!
//! For every candidate containing each term of the query as a subsequence, a dynamic program finds the highest-scoring
//! alignment, rewarding matches at path boundaries — segment starts, word separators, camelCase humps, the basename —
//! and charging for the gaps between matched characters. Scores are integers, so ranking is deterministic across
//! platforms and free of NaN-ordering hazards. The weights are fixed, and the tests pin the orderings they produce.
//!
//! A candidate every term matches in full always ranks first. "In full" is per term, not against the whole query:
//! query `a a` ranks `a`, which both terms match in full, above `a a`.
//!
//! One candidate is not aligned: a candidate longer than 1024 positions, which no repository path reaches. Running an
//! `O(n*m)` alignment over a length the caller chose is what that bound exists to prevent, so such a candidate is
//! still reported as a match — dropping it would make the file unfindable — but takes a floor score that ranks it
//! below every candidate that was scored, since a candidate that could not be scored cannot be ordered against ones
//! that were.
//!
//! A full match is the exception to the exception: it is settled by comparing lengths, never by aligning, so the
//! bound does not reach it and an over-long candidate a term matches in full still ranks first.
//!
//! # What makes it path-aware
//!
//! A match inside the **basename** — the last path segment — earns a bonus on every matched character. That is what
//! makes `main` rank `src/main.rs` above `main/lib.rs`, and it is the behaviour general-purpose fuzzy matchers do not
//! have: they award boundary bonuses uniformly per delimiter, which scores a directory hit and a filename hit alike.
//! The bonus lives inside the alignment recurrence, where it costs nothing.
//!
//! `/` ends a segment, so the basename is everything after the last one; a `\` is a character of a name, as it is in a
//! repository path. `-`, `_` and a space separate words.
//!
//! # Ties
//!
//! Equal scores rank the shorter candidate first, then the earlier one. Length is UTF-8 byte length, not a character
//! count: the tiebreak is a proxy for "less path to read", the byte length is already at hand, and a character count
//! would cost a decode per candidate to settle a tie. The two differ only for a non-ASCII candidate, where `ééa` counts
//! as longer than `aaaa`. Lengths compare up to 4 GiB, which keeps every ranked entry two words; longer candidates tie
//! on length and keep their input order.
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
//! A ranking takes candidates one at a time with their positions and answers with positions. A query splits on
//! whitespace into terms and every term must match, so a query narrows as it grows:
//!
//! ```
//! use std::num::NonZeroUsize;
//!
//! use kartta_fuzz::{Matcher, Ranked};
//!
//! let paths = ["src/auth/handler.go", "src/api/handler.go", "src/main.rs"];
//! let mut matcher = Matcher::new();
//! let mut rank = |query: &str, limit: Option<NonZeroUsize>| -> Vec<Ranked> {
//!     let mut ranking = matcher.ranking(query, limit);
//!     for (position, path) in paths.iter().enumerate() {
//!         ranking.offer(position, path);
//!     }
//!     ranking.finish()
//! };
//!
//! assert_eq!(rank("handler", None).len(), 2);
//! assert_eq!(rank("handler auth", None).len(), 1);
//! assert_eq!(rank("handler", NonZeroUsize::new(1)).len(), 1);
//! ```

mod class;
mod matcher;
mod score;

pub use crate::matcher::{Matcher, Ranked, Ranking};
