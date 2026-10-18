//! The ranking specification: what the scorer guarantees.
//!
//! Every test here states a behaviour a caller can rely on, not an implementation detail. Weight values are never
//! asserted — only the orderings they exist to produce — so retuning the weights is free as long as they keep their
//! promises.

use std::num::NonZeroUsize;

use kartta_fuzz::{Matcher, Ranked};

/// The ranking of `candidates` for `query`, offered in order, stopping where the ranking turns candidates away.
fn rank(
    matcher: &mut Matcher,
    candidates: &[&str],
    query: &str,
    limit: Option<NonZeroUsize>,
) -> Vec<Ranked> {
    let mut ranking = matcher.ranking(query, limit);
    for (position, candidate) in candidates.iter().enumerate() {
        if !ranking.offer(position, candidate) {
            break;
        }
    }
    ranking.finish()
}

/// The ranked candidates for `query`, best first.
fn ranked(candidates: &[&str], query: &str) -> Vec<String> {
    rank(&mut Matcher::new(), candidates, query, None)
        .iter()
        .map(|found| candidates[found.position].to_owned())
        .collect()
}

/// The score of one candidate, or `None` when it does not match.
fn score(candidate: &str, query: &str) -> Option<i32> {
    rank(&mut Matcher::new(), &[candidate], query, None)
        .first()
        .map(|found| found.score)
}

#[test]
fn basename_hit_outranks_directory_hit() {
    // The reason this crate exists. Input order is reversed to prove a reorder rather than a passthrough.
    assert_eq!(
        ranked(&["main/lib.rs", "src/main.rs"], "main"),
        ["src/main.rs", "main/lib.rs"]
    );
}

#[test]
fn a_basename_hit_outranks_a_directory_hit_across_case() {
    let ranked = ranked(&["src/cargo/mod.rs", "Cargo.toml"], "cargo");
    assert_eq!(
        ranked[0], "Cargo.toml",
        "a case-insensitive basename hit wins"
    );
}

#[test]
fn a_full_match_outranks_every_partial_match() {
    let full = score("ab", "ab").expect("a full match");
    let cased = score("AB", "ab").expect("a full match up to case");
    let partial = score("abc", "ab").expect("a subsequence match");

    assert_eq!(full, cased, "case never changes a full match");
    assert!(
        partial < full,
        "a partial match ranks below a full one, got {partial} against {full}"
    );
}

#[test]
fn contiguous_run_outranks_gapped_match() {
    let ranked = ranked(&["a/b/c.rs", "axbxc.rs", "abc.rs"], "abc");
    assert_eq!(
        ranked[0], "abc.rs",
        "a contiguous substring is the strongest match"
    );
}

#[test]
fn camel_hump_outranks_flat_match() {
    let ranked = ranked(&["foobar.rs", "fooBar.rs"], "fb");
    assert_eq!(
        ranked[0], "fooBar.rs",
        "a camelCase hump is a word boundary"
    );
}

#[test]
fn matching_requires_order() {
    assert_eq!(score("ab", "ba"), None, "a subsequence is ordered");
    assert_eq!(score("abc", "xyz"), None);
}

#[test]
fn every_term_must_match() {
    let paths = ["src/auth/handler.go", "src/api/handler.go"];
    assert_eq!(ranked(&paths, "handler auth"), ["src/auth/handler.go"]);
    assert_eq!(ranked(&paths, "handler zz"), [] as [&str; 0]);
}

#[test]
fn multi_term_score_sums_the_terms() {
    let both = score("ab/cd", "ab cd").expect("both terms match");
    let ab = score("ab/cd", "ab").expect("the first term matches");
    let cd = score("ab/cd", "cd").expect("the second term matches");

    assert_eq!(both, ab + cd, "the rank is the sum of the per-term scores");
}

#[test]
fn equal_scores_keep_input_order() {
    // "x" matches the basename of both identically, and both are the same length, so nothing breaks the tie.
    assert_eq!(ranked(&["aa/x", "bb/x"], "x"), ["aa/x", "bb/x"]);
    assert_eq!(ranked(&["bb/x", "aa/x"], "x"), ["bb/x", "aa/x"]);
}

#[test]
fn shorter_candidate_wins_a_tie() {
    // Gaps are charged per character, so two candidates tie only at the same character count: here one of them spends
    // two bytes on its first character.
    let candidates = ["\u{e9}a", "xa"];
    assert_eq!(score(candidates[0], "a"), score(candidates[1], "a"));
    assert_eq!(ranked(&candidates, "a"), ["xa", "\u{e9}a"]);
}

#[test]
fn empty_query_returns_every_candidate_in_input_order() {
    let candidates = ["b.rs", "a.rs"];
    assert_eq!(ranked(&candidates, ""), ["b.rs", "a.rs"]);
    assert_eq!(
        ranked(&candidates, "   "),
        ["b.rs", "a.rs"],
        "whitespace only is an empty query"
    );
    assert_eq!(score("anything", "   "), Some(0));
}

#[test]
fn limit_truncates_after_ranking() {
    let candidates = ["main/lib.rs", "src/main.rs"];
    let limited = rank(
        &mut Matcher::new(),
        &candidates,
        "main",
        NonZeroUsize::new(1),
    );

    assert_eq!(limited.len(), 1);
    assert_eq!(
        candidates[limited[0].position], "src/main.rs",
        "the cap keeps the best, not the first"
    );
}

#[test]
fn a_multi_byte_character_never_shifts_the_alignment() {
    // "café/x.rs" and "cafe/x.rs" carry the same classes at the same character positions and differ only in which
    // representation they are scored in. Arithmetic that counted bytes where it meant characters would move the
    // basename and the boundary bonuses on the first of them alone, so equal scores are what pins the character path.
    assert_eq!(
        score("café/x.rs", "cx").expect("a subsequence match"),
        score("cafe/x.rs", "cx").expect("a subsequence match"),
        "a two-symbol term scores the same either side of a multi-byte character"
    );
    assert_eq!(
        score("café/x.rs", "é").expect("a subsequence match"),
        score("cafe/x.rs", "e").expect("a subsequence match"),
        "and so does a non-ASCII term"
    );

    // The ordering the crate exists for survives the character path too.
    assert_eq!(
        ranked(&["cx/é.rs", "é/cx.rs"], "cx"),
        ["é/cx.rs", "cx/é.rs"],
        "the basename hit still wins"
    );
}

#[test]
fn long_contiguous_match_saturates_positive() {
    // Overflow-checked debug builds must not panic and release builds must not wrap negative.
    let query: String = std::iter::repeat_n('a', 50).collect();
    let candidate: String = std::iter::repeat_n('a', 60).collect();

    let score = score(&candidate, &query).expect("every query character matches");
    assert!(score > 0, "a saturated score stays positive, got {score}");
}

#[test]
fn an_over_long_candidate_matches_but_ranks_last() {
    // Too long to align, so it cannot be ordered against candidates that were scored. It still matches — dropping it
    // would make the file unfindable — but it ranks below every scored candidate rather than above the ones whose
    // leading gaps outweighed their bonuses.
    let long = "z".repeat(2000);
    let deep = format!("{}z", "x".repeat(1000));

    assert!(score(&long, "z").is_some(), "it is still a match");
    let deep_score = score(&deep, "z").expect("a scored match, however poorly");
    let long_score = score(&long, "z").expect("an over-long match");
    assert!(
        long_score < deep_score,
        "the unscoreable candidate must not outrank a scored one, got {long_score} against {deep_score}"
    );

    assert_eq!(
        ranked(&[long.as_str(), deep.as_str()], "z"),
        [deep, long],
        "and the ranking follows"
    );
}

#[test]
fn a_backslash_is_part_of_a_name() {
    // A repository path separates segments with `/` alone, so `\` neither starts a segment nor moves the basename: each
    // candidate is one segment, and the match at its start leads.
    assert_eq!(
        ranked(&[r"src\main.rs", r"main\lib.rs"], "main"),
        [r"main\lib.rs", r"src\main.rs"]
    );
}

#[test]
fn a_limit_returns_exactly_the_prefix_of_the_unlimited_ranking() {
    // The bounded heap is a different algorithm from ranking everything and truncating, so it has to agree with it —
    // including on the tiebreaks, which is where a heap ordering most easily diverges from a stable sort.
    let candidates: Vec<String> = (0..500)
        .map(|i| format!("pkg{}/mod{}/render_pass{}.rs", i % 7, i % 13, i % 11))
        .collect();
    let borrowed: Vec<&str> = candidates.iter().map(String::as_str).collect();

    let mut matcher = Matcher::new();
    for query in ["r", "render", "pkg mod", "pass rs"] {
        let unlimited = rank(&mut matcher, &borrowed, query, None);

        for limit in [1usize, 2, 7, 50, 499, 500, 501] {
            let limited = rank(&mut matcher, &borrowed, query, NonZeroUsize::new(limit));
            let expected = &unlimited[..limit.min(unlimited.len())];
            assert_eq!(limited, expected, "query {query:?} at limit {limit}");
        }
    }
}

#[test]
fn a_full_match_ranks_first() {
    let ranked = ranked(&["xaxbx", "a/b", "abc", "zzab", "ab/ab", "ab"], "ab");
    assert_eq!(ranked[0], "ab", "the full match must lead, got {ranked:?}");
}

#[test]
fn an_over_long_candidate_is_scored_without_being_prepared() {
    // Sizing the per-candidate buffers to a candidate that is never aligned would let one caller-supplied length hold
    // an allocation for the rest of the query, for work immediately thrown away. Reusing one matcher across a huge
    // candidate and then a normal one proves the huge one left nothing behind that changes the normal one's answer.
    let huge = "z".repeat(4 * 1024 * 1024);
    let normal = ["src/main.rs", "main/lib.rs"];

    let mut matcher = Matcher::new();
    assert_eq!(
        rank(&mut matcher, &[huge.as_str()], "z", None).len(),
        1,
        "an over-long candidate still matches"
    );

    let after: Vec<&str> = rank(&mut matcher, &normal, "main", None)
        .iter()
        .map(|found| normal[found.position])
        .collect();
    assert_eq!(
        after,
        ["src/main.rs", "main/lib.rs"],
        "and leaves no residue"
    );
}

#[test]
fn a_limit_far_larger_than_the_input_costs_nothing_up_front() {
    // A limit caps the result; it is not a claim that many candidates exist. Reserving against it alone would let
    // `NonZeroUsize::MAX` demand an allocation before a candidate is read.
    let candidates = ["src/main.rs", "main/lib.rs"];
    let ranked = rank(
        &mut Matcher::new(),
        &candidates,
        "main",
        NonZeroUsize::new(usize::MAX),
    );

    assert_eq!(ranked.len(), 2);
    assert_eq!(candidates[ranked[0].position], "src/main.rs");
}

#[test]
fn a_full_match_is_per_term_not_per_query() {
    // With several terms the rank is the sum of per-term scores, so "matched in full" is a property each term has
    // against the candidate -- not a property of the candidate against the query string. Query `a a` therefore
    // ranks `a`, which both terms match in full, above `a a`, which neither does.
    //
    // The whitespace in a query separates terms; it is not content the candidate has to contain. Privileging a
    // candidate that happens to reproduce the query's own spacing would rank `a ab` above `ab` for query `a ab`,
    // which is the wrong answer for a query asking only that both terms be present.
    assert_eq!(ranked(&["a a", "a"], "a a"), ["a", "a a"]);
    assert_eq!(ranked(&["a ab", "ab"], "a ab"), ["ab", "a ab"]);
}

#[test]
fn an_over_long_non_ascii_candidate_matches_and_ranks_last() {
    // The non-ASCII path establishes a candidate's length by decoding it, so an over-long candidate must settle that
    // from a bounded prefix rather than a full pass -- a one-character term matching the first character otherwise
    // leaves the whole remaining suffix to scan for a length already known to be too large.
    let huge: String = std::iter::repeat_n('é', 2 * 1024 * 1024).collect();
    let scored = "é/main.rs";

    assert!(
        score(&huge, "é").is_some(),
        "an over-long non-ASCII candidate still matches"
    );

    assert_eq!(
        ranked(&[huge.as_str(), scored], "é"),
        [scored.to_owned(), huge.clone()],
        "and ranks below the one that was scored"
    );
}

#[test]
fn the_length_tiebreak_orders_by_utf8_byte_length() {
    // The unit is stated in the contract because these two candidates order one way by bytes and the other by
    // characters: the first is 102 bytes and 102 characters, the second 122 bytes and 62 characters.
    //
    // Reaching the tiebreak at all takes work. Every character costs a gap, so the candidate with more characters
    // earns them back with a larger boundary bonus at its match, after `-` rather than `.`. The scores are asserted
    // equal before the order is, or this test would pass whichever unit the sort used.
    let fewer_bytes = format!("{}-a", "z".repeat(100));
    let fewer_chars = format!("{}.a", "\u{e9}".repeat(60));
    assert_eq!(
        score(&fewer_bytes, "a").expect("matches"),
        score(&fewer_chars, "a").expect("matches"),
        "the candidates must tie on score for the tiebreak to decide anything"
    );

    let ranked = ranked(&[fewer_chars.as_str(), fewer_bytes.as_str()], "a");
    assert_eq!(
        ranked[0], fewer_bytes,
        "fewer bytes wins, though it has more characters"
    );
}

#[test]
fn an_over_long_candidate_matched_in_full_still_ranks_first() {
    // The over-long bound applies to the recurrence, and a full match never runs it: term and candidate lengths settle
    // it outright. So the bound does not reach a full match, and "a candidate every term matches in full ranks first"
    // holds past 1024 positions as well.
    let long: String = "q".repeat(1025);
    let other = format!("{long}x");

    let ranked = ranked(&[other.as_str(), long.as_str()], &long);
    assert_eq!(ranked[0], long, "the full match leads even when over-long");
}

#[test]
fn the_score_bands_survive_aggregation() {
    // A floored term's score sits just above `i32::MIN` and the terms beside it add to it. The bands are applied once
    // after the sum, so a floored aggregate stays below every aligned one whatever the other terms add.
    let over_long = "a".repeat(2000);
    let aligned = score("xaxa", "a a").expect("an aligned aggregate");
    let floored = score(&over_long, "a a").expect("a floored aggregate");
    assert!(
        floored < aligned,
        "a floored aggregate must stay below an aligned one, got {floored} against {aligned}"
    );
}

#[test]
fn case_folding_does_not_cross_the_ascii_boundary() {
    // Folding is documented as ASCII-first rather than as full Unicode folding, and this is the case that parts the
    // two: U+212A KELVIN SIGN folds to `k` under Unicode rules and does not here.
    assert_eq!(
        score("\u{212a}", "k"),
        None,
        "the Kelvin sign is not an ASCII k"
    );
    assert!(score("Kk", "k").is_some(), "an ASCII K still folds");

    // The other side of the same rule: neither character is ASCII, so the Unicode lowercase comparison decides, and
    // it folds. Without this the folding would read as "ASCII only" rather than "not across the boundary".
    assert!(
        score("\u{c9}", "\u{e9}").is_some(),
        "a pair of non-ASCII characters folds"
    );
}

#[test]
fn rankings_merge_in_any_grouping_that_keeps_their_order() {
    // The four `?/main.rs` paths tie on score and on length, so a merge has to keep input order among them to agree
    // with the whole ranking.
    let paths = [
        "a/main.rs",
        "b/main.rs",
        "src/main.rs",
        "main/lib.rs",
        "c/main.rs",
        "docs/main.md",
        "lib.rs",
        "d/main.rs",
    ];
    let queries = ["main", "", "rs", "zz"];
    let limits = [None, NonZeroUsize::new(1), NonZeroUsize::new(3)];
    let splits: Vec<(usize, usize)> = (0..=paths.len())
        .flat_map(|first| (first..=paths.len()).map(move |second| (first, second)))
        .collect();

    let mut matcher = Matcher::new();
    for (query, limit) in queries
        .iter()
        .flat_map(|&query| limits.map(|limit| (query, limit)))
    {
        // The ranking of `paths[range]`, offered one at a time and stopped at the first offer turned away.
        let mut offered = |range: std::ops::Range<usize>| {
            let mut ranking = matcher.ranking(query, limit);
            let _ = range
                .into_iter()
                .all(|position| ranking.offer(position, paths[position]));
            ranking.finish()
        };
        let whole = offered(0..paths.len());

        for &(first, second) in &splits {
            let [a, b, c] = [
                offered(0..first),
                offered(first..second),
                offered(second..paths.len()),
            ];
            let merge = |left, right| Matcher::merge_ranked([left, right], query, limit);
            let context =
                format!("query {query:?}, limit {limit:?}, slices at {first} and {second}");

            assert_eq!(
                merge(merge(a.clone(), b.clone()), c.clone()),
                whole,
                "left first, {context}"
            );
            assert_eq!(merge(a, merge(b, c)), whole, "right first, {context}");
        }
    }
}
