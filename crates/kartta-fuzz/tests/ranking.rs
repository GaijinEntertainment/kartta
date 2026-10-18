//! The ranking specification: what the scorer guarantees under [`Config::DEFAULT`].
//!
//! Every test here states a behaviour a caller can rely on, not an implementation detail. Weight values are never
//! asserted — only the orderings they exist to produce — so retuning the default is free as long as it keeps its
//! promises.

use std::num::NonZeroUsize;

use kartta_fuzz::{Config, Matcher};

/// The ranked candidates for `query`, best first.
fn ranked(candidates: &[&str], query: &str) -> Vec<String> {
    ranked_with(Config::DEFAULT, candidates, query)
}

/// The ranked candidates for `query` under a specific configuration.
fn ranked_with(config: Config, candidates: &[&str], query: &str) -> Vec<String> {
    Matcher::new(config)
        .rank(candidates.iter().copied(), query, None)
        .iter()
        .map(|m| m.candidate.to_owned())
        .collect()
}

/// The score of one candidate under the default configuration.
fn score(candidate: &str, query: &str) -> Option<i32> {
    score_with(Config::DEFAULT, candidate, query)
}

/// The score of one candidate under a specific configuration.
fn score_with(config: Config, candidate: &str, query: &str) -> Option<i32> {
    Matcher::new(config).score(candidate, query)
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
fn shorter_candidate_wins_a_tie_when_configured() {
    // Gaps are charged per character, so two candidates tie only at the same character count: here one of them spends
    // two bytes on its first character.
    let candidates = ["\u{e9}a", "xa"];
    assert_eq!(score(candidates[0], "a"), score(candidates[1], "a"));

    let mut config = Config::DEFAULT;
    assert_eq!(
        ranked_with(config, &candidates, "a")[0],
        "xa",
        "the length tiebreak reorders"
    );

    config.length_tiebreak = false;
    assert_eq!(
        ranked_with(config, &candidates, "a")[0],
        "\u{e9}a",
        "without it, input order stands"
    );
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
    let limited = Matcher::new(Config::DEFAULT).rank(candidates, "main", NonZeroUsize::new(1));

    assert_eq!(limited.len(), 1);
    assert_eq!(
        limited[0].candidate, "src/main.rs",
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
fn segment_separators_decide_the_basename() {
    // With `.` as the separator, the basename of `pkg.mod.Symbol` is `Symbol`.
    let mut config = Config::DEFAULT;
    config.segment_separators = &['.'];

    let ranked = ranked_with(config, &["sym.other.Thing", "pkg.mod.Symbol"], "sym");
    assert_eq!(
        ranked[0], "pkg.mod.Symbol",
        "the last dot-segment is the basename"
    );
}

#[test]
fn backslash_separates_segments_like_a_slash() {
    assert_eq!(
        ranked(&[r"main\lib.rs", r"src\main.rs"], "main"),
        [r"src\main.rs", r"main\lib.rs"]
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

    for query in ["r", "render", "pkg mod", "pass rs"] {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let unlimited = matcher.rank(borrowed.iter().copied(), query, None);

        for limit in [1usize, 2, 7, 50, 499, 500, 501] {
            let limited = Matcher::new(Config::DEFAULT).rank(
                borrowed.iter().copied(),
                query,
                NonZeroUsize::new(limit),
            );
            let expected = &unlimited[..limit.min(unlimited.len())];
            assert_eq!(limited, expected, "query {query:?} at limit {limit}");
        }
    }
}

#[test]
fn a_limit_agrees_without_the_length_tiebreak_too() {
    let mut config = Config::DEFAULT;
    config.length_tiebreak = false;

    let candidates = ["aa/x", "bb/x", "c/xxxxxx", "dd/x", "e/x"];
    let unlimited = ranked_with(config, &candidates, "x");

    for limit in [1usize, 3, 5] {
        let limited: Vec<String> = Matcher::new(config)
            .rank(candidates, "x", NonZeroUsize::new(limit))
            .iter()
            .map(|m| m.candidate.to_owned())
            .collect();
        assert_eq!(
            limited,
            unlimited[..limit.min(unlimited.len())],
            "limit {limit}"
        );
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

    let mut matcher = Matcher::new(Config::DEFAULT);
    assert!(
        matcher.score(&huge, "z").is_some(),
        "an over-long candidate still matches"
    );

    let after: Vec<&str> = matcher
        .rank(normal, "main", None)
        .iter()
        .map(|m| m.candidate)
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
    let ranked = Matcher::new(Config::DEFAULT).rank(
        ["src/main.rs", "main/lib.rs"],
        "main",
        NonZeroUsize::new(usize::MAX),
    );

    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].candidate, "src/main.rs");
}

#[test]
fn a_candidate_with_no_segments_is_all_basename() {
    // An empty separator set is reachable through the public field. Nothing then ends a segment, so the candidate is
    // one segment and every matched character sits inside the basename -- the coherent reading, and the one the
    // recurrence already produces, but nothing pinned it.
    let mut config = Config::DEFAULT;
    config.segment_separators = &[];

    let ranked = ranked_with(config, &["zzz/main.rs", "main.rs"], "main");
    assert_eq!(
        ranked[0], "main.rs",
        "the earlier match leads once nothing ends a segment"
    );

    // With `/` no longer a separator it is an ordinary character, so it neither starts a segment nor moves the
    // basename, and a query may match straight through it.
    assert!(
        score_with(config, "z/main.rs", "zm").is_some(),
        "a separator that is not configured is just a character"
    );
}

#[test]
fn an_empty_word_separator_set_only_drops_the_word_bonus() {
    // Asserting that the candidate still matches would prove nothing: it matches whether or not `_` separates words.
    // The observable difference is the boundary bonus at `p`, so the two settings are scored and compared.
    let mut config = Config::DEFAULT;
    let with_separators = score_with(config, "render_pass.rs", "rp").expect("matches");

    config.word_separators = &[];
    let without = score_with(config, "render_pass.rs", "rp").expect("still matches");

    assert!(
        with_separators > without,
        "emptying the set drops the word bonus at `p`"
    );
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

    let mut matcher = Matcher::new(Config::DEFAULT);
    assert!(
        matcher.score(&huge, "é").is_some(),
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
    let mut config = Config::DEFAULT;
    config.length_tiebreak = false;

    let over_long = "a".repeat(2000);
    let aligned = score_with(config, "xaxa", "a a").expect("an aligned aggregate");
    let floored = score_with(config, &over_long, "a a").expect("a floored aggregate");
    assert!(
        floored < aligned,
        "a floored aggregate must stay below an aligned one, got {floored} against {aligned}"
    );

    // Both orders of the same pair, since a tie would otherwise be settled by input order and hide the defect.
    assert_eq!(
        ranked_with(config, &[over_long.as_str(), "xaxa"], "a a")[0],
        "xaxa"
    );
    assert_eq!(
        ranked_with(config, &["xaxa", over_long.as_str()], "a a")[0],
        "xaxa"
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
fn merging_the_rankings_of_slices_reproduces_the_ranking_of_the_whole_list() {
    // The four `?/main.rs` paths tie on score and on length, so the merge has to keep input order among them to agree
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
    let mut matcher = Matcher::new(Config::DEFAULT);

    for query in ["main", "", "rs"] {
        for limit in [None, NonZeroUsize::new(1), NonZeroUsize::new(3)] {
            let whole = matcher.rank(paths, query, limit);

            for split in 0..=paths.len() {
                let (left, right) = paths.split_at(split);
                let halves = [
                    matcher.rank(left.iter().copied(), query, limit),
                    matcher.rank(right.iter().copied(), query, limit),
                ];

                assert_eq!(
                    matcher.merge(halves, query, limit),
                    whole,
                    "query {query:?}, limit {limit:?}, split at {split}"
                );
            }
        }
    }
}

#[test]
fn a_ranking_fed_by_position_agrees_with_rank_whole_and_merged() {
    // Ties on score and on length among the `?/main.rs` paths are what a position-keyed order most easily breaks.
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
    let mut without_length_tiebreak = Config::DEFAULT;
    without_length_tiebreak.length_tiebreak = false;
    let queries = ["main", "", "rs", "zz"];
    let limits = [None, NonZeroUsize::new(1), NonZeroUsize::new(3)];

    for config in [Config::DEFAULT, without_length_tiebreak] {
        let mut matcher = Matcher::new(config);
        for (query, limit) in queries
            .iter()
            .flat_map(|&query| limits.map(|limit| (query, limit)))
        {
            let expected: Vec<(&str, i32)> = matcher
                .rank(paths, query, limit)
                .iter()
                .map(|found| (found.candidate, found.score))
                .collect();
            let as_pairs = |ranked: Vec<kartta_fuzz::Ranked>| -> Vec<(&str, i32)> {
                ranked
                    .iter()
                    .map(|found| (paths[found.position], found.score))
                    .collect()
            };

            // The ranking of `paths[range]`, built one offer at a time.
            let mut offered = |range: std::ops::Range<usize>| {
                let mut ranking = matcher.ranking(query, limit);
                // `all` stops at the first offer the ranking turns away, as a caller feeding it would.
                let _ = range
                    .into_iter()
                    .all(|position| ranking.offer(position, paths[position]));
                ranking.finish()
            };

            let tiebreak = config.length_tiebreak;
            assert_eq!(
                as_pairs(offered(0..paths.len())),
                expected,
                "query {query:?}, limit {limit:?}, length tiebreak {tiebreak}"
            );

            for split in 0..=paths.len() {
                let halves = [offered(0..split), offered(split..paths.len())];
                assert_eq!(
                    as_pairs(Matcher::merge_ranked(halves, query, limit)),
                    expected,
                    "query {query:?}, limit {limit:?}, length tiebreak {tiebreak}, split at {split}"
                );
            }
        }
    }
}

#[test]
fn rankings_merge_in_any_grouping_that_keeps_their_order() {
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
    let mut without_length_tiebreak = Config::DEFAULT;
    without_length_tiebreak.length_tiebreak = false;
    let queries = ["main", "", "rs"];
    let limits = [None, NonZeroUsize::new(1), NonZeroUsize::new(3)];
    let splits: Vec<(usize, usize)> = (0..=paths.len())
        .flat_map(|first| (first..=paths.len()).map(move |second| (first, second)))
        .collect();

    for config in [Config::DEFAULT, without_length_tiebreak] {
        let mut matcher = Matcher::new(config);
        for (query, limit) in queries
            .iter()
            .flat_map(|&query| limits.map(|limit| (query, limit)))
        {
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
}
