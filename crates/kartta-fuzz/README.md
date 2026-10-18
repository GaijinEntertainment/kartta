# kartta-fuzz

Path-aware fuzzy scorer that ranks repository paths against a query by optimal alignment.

For every candidate containing each term of the query as a subsequence, a dynamic program finds the highest-scoring
alignment. It rewards matches at path boundaries — segment starts, word separators, camelCase humps, the basename — and
charges for the gaps between matched characters. Scores are integers, so ranking is deterministic across platforms and
free of NaN-ordering hazards, and every arithmetic operation on a score saturates, so no configuration makes one
overflow.

A candidate longer than 1024 positions is the one exception, and no repository path reaches it: aligning it would mean
running an `O(n*m)` recurrence over a length the caller chose. It still counts as a match, so the file stays findable,
but takes a floor score that ranks it below every candidate that was actually scored.

A full match escapes that bound, because it is settled by comparing lengths rather than by aligning: an over-long
candidate a term matches in full still ranks first, since that ordering is one of the guarantees below and the floor
is not.

## What makes it path-aware

A match inside the **basename** — the last path segment — earns a bonus on every matched character. That is what makes
`main` rank `src/main.rs` above `main/lib.rs`.

General-purpose fuzzy matchers do not do this. They award boundary bonuses uniformly per delimiter, which scores a
directory hit and a filename hit alike, and neither of the two mature Rust matchers exposes a per-position hook to add
one. Since the ranking decides which files a language model reads first, a directory hit arriving before a filename hit
is the failure this crate exists to prevent. `design-docs/adr/1.2-own-the-path-scorer.md` records what was measured and
why the alternatives were rejected.

## The model

A query splits on whitespace into terms; a candidate must match **every** term, and its rank is the sum of the
per-term scores. Ranking is stable — equal scores break by shorter candidate, then by input order — so an unchanged
input ranks identically every time. "Shorter" is UTF-8 byte length, which differs from a character count only for a
non-ASCII candidate. An empty or whitespace-only query is the exception: everything matches at score zero and the
input order stands, since nothing was scored for a tiebreak to break.

A candidate a term matches in full takes the highest score that term can award, so for a single-term query — the
ordinary case — the candidate equal to the query ranks first. With several terms the rank is their sum, and "in full"
stays a property of each term against the candidate rather than of the candidate against the whole query: a query's
whitespace separates its terms and is not content a candidate has to contain. Query `a a` therefore ranks `a`, which
both terms match in full, above `a a`.

Matching ignores case, so a full match is equality up to case. Folding is ASCII-first rather than full Unicode folding:
U+212A KELVIN SIGN does not match `k`, while a pair of non-ASCII characters compares by Unicode lowercase. The full-match
guarantee survives reconfiguration because the score it takes is derived from the active weights rather than declared as
a constant, and each component of that derivation is clamped to zero or above, so no set of weights — including one that
is entirely negative — inverts the two.

Three guarantees hold for every configuration, and only these three: a candidate every term matches in full ranks first,
a candidate missing a term never matches, and no weight makes a score overflow. Everything else the model produces — a
contiguous run beating a gapped one, a filename hit beating a directory hit — is a property of the shipped weights,
which the tests pin, and not of any point in the space.

## Configuration

Every weight and every policy is a field of `Config`. Start from `Config::DEFAULT` and adjust what you need:

```rust
use kartta_fuzz::{Config, Matcher};

let mut config = Config::DEFAULT;
config.scoring.candidate_start = 500;     // reward matches at the very start

let mut matcher = Matcher::new(config);
let ranked = matcher.rank(["src/main.rs", "main/lib.rs"], "main", None);
```

`Config::DEFAULT` is the tuned point in that space and the one the behaviour tests pin. Every other point is guaranteed
only by the invariants that hold across all of them — the full-match rule above, the requirement that a candidate
missing a term never matches, and freedom from overflow.

`segment_separators` is the field that decides what the scorer scores. It fixes both which positions earn the segment
bonus and where the basename starts, so replacing it with `['.', ':']` makes the same model rank symbols instead of
paths.

## Cost

Cost is at query time: the alignment runs per surviving candidate on every search, and nothing is precomputed between
queries. Every term is proved present before anything is prepared or aligned, so that work only reaches candidates that
can actually match. An all-ASCII term — which a repository query almost always is — is proved with a vectorized scan
over the candidate's raw bytes, and the crate's one dependency, `memchr`, is there for that; a term carrying a
non-ASCII character has no byte form and takes a scalar scan over decoded characters instead.

Repository paths are ASCII-dominant, so a candidate is likewise prepared as normalized bytes whenever it and the
query both are, and as characters otherwise; the alignment is written once, generic over the two.

A `Matcher` owns the buffers the alignment reuses. Build one per query and rank a whole list with it; building one per
candidate defeats the reuse it exists for. Results borrow from the input, so a ranking allocates one `Vec` and
copies no path. Passing a `limit` also bounds memory: the best matches are kept in a heap of that size rather than
collected and sorted, which matters when a permissive query over millions of paths would otherwise materialize a
result per survivor.

`benches/rank.rs` tracks regressions against a generated corpus, ranking both without and with a `limit`, the way a
caller does. It is also the profiling target: `cargo bench --bench rank -- --profile-time 10` under a sampling profiler
reports where the time goes.

Status: the API is not published yet.
