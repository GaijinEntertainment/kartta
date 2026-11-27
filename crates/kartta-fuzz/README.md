# kartta-fuzz

Path-aware fuzzy scorer that ranks repository paths against a query by optimal alignment.

For every candidate containing each term of the query as a subsequence, a dynamic program finds the highest-scoring
alignment. It rewards matches at path boundaries — segment starts, word separators, camelCase humps, the basename — and
charges for the gaps between matched characters. Scores are integers, so ranking is deterministic across platforms and
free of NaN-ordering hazards.

A candidate longer than 1024 positions is the one exception, and no repository path reaches it: aligning it would mean
running an `O(n*m)` recurrence over a length the caller chose. It still counts as a match, so the file stays findable,
but takes a floor score that ranks it below every candidate that was actually scored.

A full match escapes that bound, because it is settled by comparing lengths rather than by aligning: an over-long
candidate a term matches in full still ranks first.

## What makes it path-aware

A match inside the **basename** — the last path segment — earns a bonus on every matched character. That is what makes
`main` rank `src/main.rs` above `main/lib.rs`.

General-purpose fuzzy matchers do not do this. They award boundary bonuses uniformly per delimiter, which scores a
directory hit and a filename hit alike, and neither of the two mature Rust matchers exposes a per-position hook to add
one. Since the ranking decides which files a language model reads first, a directory hit arriving before a filename hit
is the failure this crate exists to prevent. `design-docs/adr/1.2-path-scorer.md` records what was measured and
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
U+212A KELVIN SIGN does not match `k`, while a pair of non-ASCII characters compares by Unicode lowercase.

`/` ends a segment, so the basename is everything after the last one; a `\` is a character of a name, as it is in a
repository path. `-`, `_` and a space separate words.

The weights are fixed: fzy's, scaled by 1000, plus a basename bonus per matched character. Match bonuses dwarf the gap
penalties, so alignment quality decides the rank and gaps only break near-ties. A skipped character costs less before
the first match or after the last than between two, so a file extension costs a candidate little. The tests pin the
orderings they produce — a contiguous run beating a gapped one, a filename hit beating a directory hit — not the values.
Nothing is configurable: no caller needed another point in that space, and ADR 1.51 records what keeping it open cost.

A ranking takes candidates one at a time with their positions, so they need never be alive together, and answers with
positions:

```rust
use kartta_fuzz::Ranking;

let paths = ["main/lib.rs", "src/main.rs"];
let mut ranking = Ranking::new("main", None);
for (position, path) in paths.iter().enumerate() {
    ranking.offer(position, path);
}
assert_eq!(paths[ranking.finish()[0].position], "src/main.rs");
```

Rankings of consecutive slices of one list, each built with the same query and limit, merge into the ranking of the
whole list with `Ranking::merge`, which is how a long list is ranked on several threads.

`Ranking::matched` counts the offered candidates that matched every term, those the limit kept out included, so a caller
can say how many its page was cut from. An empty query counts only what was offered, which stops short of the list where
a caller stops at `offer`'s `false`; where every slice was offered in full, the slices' counts sum to the list's.

## Cost

Cost is at query time: the alignment runs per surviving candidate on every search, and nothing is precomputed between
queries. Every term is proved present before anything is prepared or aligned, so that work only reaches candidates that
can actually match. An all-ASCII term — which a repository query almost always is — is proved with a vectorized scan
over the candidate's raw bytes, and the crate's one dependency, `memchr`, is there for that; a term carrying a
non-ASCII character has no byte form and takes a scalar scan over decoded characters instead.

Repository paths are ASCII-dominant, so a candidate is likewise prepared as normalized bytes whenever it and the query
both are, and as characters otherwise; the alignment is written once, generic over the two. A candidate is prepared once
and every term of the query scores against that preparation. A one-symbol term — the most permissive non-empty query —
is scored by a running maximum without the rows, and a longer term only over the span from the first match of its first
symbol to the last match of its last, charging the positions outside it as gaps. A candidate past the 1024-position
bound is never prepared, and its length is counted only as far as the bound needs, so neither the buffers nor the scan
grow with a length the caller chose.

A `Ranking` owns the buffers the alignment reuses, so ranking a list allocates once rather than once per candidate.
Results are positions, so a ranking allocates one `Vec` and copies no path. Passing a `limit` also bounds memory: the
best matches are kept in a heap of that size rather than collected and sorted, which matters when a permissive query
over millions of paths would otherwise materialize a result per survivor.

`benches/rank.rs` tracks regressions against a generated corpus of repository-shaped paths, ranking both without and
with a `limit`; a caller answering a language model asks for a page, so the limited ranking is the one in production.
The corpus is deterministic, so criterion's run-to-run comparison holds, and generated, because one large enough to
profile like a repository is too large to commit; it is sized so per-candidate cost dominates setup while the suite
still runs in well under a minute. Every offered candidate goes through the subsequence prefilter, and only its
survivors are aligned, so the queries span selectivity: from one character nearly every path matches, where the
alignment dominates, through a two-term query, to one nothing matches, which times the prefilter alone. It is also
the profiling target: `cargo bench --bench rank -- --profile-time 10` under a sampling profiler reports where the time
goes.
