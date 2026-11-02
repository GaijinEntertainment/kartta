//! The scorer's contract: the orderings a caller can rely on, never the weight values that produce them.

use std::num::NonZeroUsize;

use kartta_fuzz::{Ranked, Ranking};

fn rank(candidates: &[&str], query: &str, limit: Option<NonZeroUsize>) -> Vec<Ranked> {
    let mut ranking = Ranking::new(query, limit);
    for (position, candidate) in candidates.iter().enumerate() {
        if !ranking.offer(position, candidate) {
            break;
        }
    }
    ranking.finish()
}

fn ranked(candidates: &[&str], query: &str) -> Vec<String> {
    rank(candidates, query, None)
        .iter()
        .map(|found| candidates[found.position].to_owned())
        .collect()
}

fn score(candidate: &str, query: &str) -> Option<i32> {
    rank(&[candidate], query, None)
        .first()
        .map(|found| found.score)
}

#[test]
fn basename_hit_outranks_directory_hit() {
    assert_eq!(
        ranked(&["main/lib.rs", "src/main.rs"], "main"),
        ["src/main.rs", "main/lib.rs"],
        "the directory hit, offered first, ranks second"
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
    assert_eq!(
        score("aa/x", "x"),
        score("bb/x", "x"),
        "same length, same score: nothing but input order breaks the tie"
    );
    assert_eq!(ranked(&["aa/x", "bb/x"], "x"), ["aa/x", "bb/x"]);
    assert_eq!(ranked(&["bb/x", "aa/x"], "x"), ["bb/x", "aa/x"]);
}

#[test]
fn shorter_candidate_wins_a_tie() {
    let [two_byte_lead, one_byte_lead] = ["\u{e9}a", "xa"];
    assert_eq!(
        score(two_byte_lead, "a"),
        score(one_byte_lead, "a"),
        "gaps are charged per character, so the same character count ties"
    );
    assert_eq!(
        ranked(&[two_byte_lead, one_byte_lead], "a"),
        [one_byte_lead, two_byte_lead]
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
fn an_empty_query_keeps_its_limit_for_a_caller_that_keeps_offering() {
    let mut ranking = Ranking::new("", NonZeroUsize::new(2));
    let answers: Vec<bool> = (0..4)
        .map(|position| ranking.offer(position, "x.rs"))
        .collect();

    assert_eq!(
        answers,
        [true, true, false, false],
        "the offer past the limit is the first that answers false, so a caller that stops there saw one was turned away"
    );
    assert_eq!(
        ranking.matched(),
        4,
        "every offered candidate matches an empty query, those past its limit included"
    );
    let positions: Vec<usize> = ranking
        .finish()
        .iter()
        .map(|found| found.position)
        .collect();
    assert_eq!(
        positions,
        [0, 1],
        "the first candidates in input order, never more than the limit"
    );
}

#[test]
fn limit_truncates_after_ranking() {
    let candidates = ["main/lib.rs", "src/main.rs"];
    let limited = rank(&candidates, "main", NonZeroUsize::new(1));

    assert_eq!(limited.len(), 1);
    assert_eq!(
        candidates[limited[0].position], "src/main.rs",
        "the cap keeps the best, not the first"
    );
}

#[test]
fn a_ranking_counts_every_match_its_limit_keeps_out() {
    let candidates = ["src/main.rs", "main/lib.rs", "src/lib.rs", "docs/main.md"];
    let counted = |query: &str| {
        let mut ranking = Ranking::new(query, NonZeroUsize::new(1));
        for (position, candidate) in candidates.iter().enumerate() {
            ranking.offer(position, candidate);
        }
        (ranking.matched(), ranking.finish().len())
    };

    assert_eq!(counted("main"), (3, 1), "src/lib.rs holds no main");
    assert_eq!(
        counted("main rs"),
        (2, 1),
        "a match holds every term, and docs/main.md holds no rs"
    );
    assert_eq!(counted("zz"), (0, 0));
}

#[test]
fn slices_offered_in_full_count_what_the_whole_list_counts() {
    let paths = [
        "a/main.rs",
        "src/lib.rs",
        "main/lib.rs",
        "docs/main.md",
        "lib.rs",
    ];
    let limits = [None, NonZeroUsize::new(1)];

    for (query, limit) in ["main", "", "rs", "zz"]
        .iter()
        .flat_map(|&query| limits.map(|limit| (query, limit)))
    {
        let counted = |range: std::ops::Range<usize>| {
            let mut ranking = Ranking::new(query, limit);
            for position in range {
                ranking.offer(position, paths[position]);
            }
            ranking.matched()
        };

        for split in 0..=paths.len() {
            assert_eq!(
                counted(0..split) + counted(split..paths.len()),
                counted(0..paths.len()),
                "query {query:?}, limit {limit:?}, split at {split}"
            );
        }
    }
}

#[test]
fn a_multi_byte_character_never_shifts_the_alignment() {
    assert_eq!(
        score("café/x.rs", "cx").expect("a subsequence match"),
        score("cafe/x.rs", "cx").expect("a subsequence match"),
        "scored as characters and as bytes, the same classes at the same positions score alike; a byte offset taken \
         for a character offset moves the basename and boundary bonuses"
    );
    assert_eq!(
        score("café/x.rs", "é").expect("a subsequence match"),
        score("cafe/x.rs", "e").expect("a subsequence match"),
        "and so does a non-ASCII term"
    );
    assert_eq!(
        ranked(&["cx/é.rs", "é/cx.rs"], "cx"),
        ["é/cx.rs", "cx/é.rs"],
        "the basename hit still wins when scored as characters"
    );
}

#[test]
fn long_contiguous_match_saturates_positive() {
    let query: String = std::iter::repeat_n('a', 50).collect();
    let candidate: String = std::iter::repeat_n('a', 60).collect();

    let score = score(&candidate, &query)
        .expect("every query character matches, without an overflow panic");
    assert!(
        score > 0,
        "a saturated score never wraps negative, got {score}"
    );
}

#[test]
fn an_over_long_candidate_matches_but_ranks_last() {
    let long = "z".repeat(2000);
    let heavily_gapped = format!("{}z", "x".repeat(1000));

    assert!(score(&long, "z").is_some(), "it is still a match");
    let gapped_score = score(&heavily_gapped, "z").expect("a scored match, however poorly");
    let long_score = score(&long, "z").expect("an over-long match");
    assert!(
        long_score < gapped_score,
        "the unscoreable candidate must not outrank even a heavily gapped scored one, got {long_score} against \
         {gapped_score}"
    );

    assert_eq!(
        ranked(&[long.as_str(), heavily_gapped.as_str()], "z"),
        [heavily_gapped, long],
        "and the ranking follows"
    );
}

#[test]
fn a_backslash_is_part_of_a_name() {
    assert_eq!(
        ranked(&[r"src\main.rs", r"main\lib.rs"], "main"),
        [r"main\lib.rs", r"src\main.rs"],
        "without a `/` each candidate is one segment, and the match at its start leads"
    );
}

#[test]
fn a_limit_returns_exactly_the_prefix_of_the_unlimited_ranking() {
    let candidates: Vec<String> = (0..500)
        .map(|i| format!("pkg{}/mod{}/render_pass{}.rs", i % 7, i % 13, i % 11))
        .collect();
    let borrowed: Vec<&str> = candidates.iter().map(String::as_str).collect();

    for query in ["r", "render", "pkg mod", "pass rs"] {
        let unlimited = rank(&borrowed, query, None);

        for limit in [1usize, 2, 7, 50, 499, 500, 501] {
            let limited = rank(&borrowed, query, NonZeroUsize::new(limit));
            let expected = &unlimited[..limit.min(unlimited.len())];
            assert_eq!(
                limited, expected,
                "the bounded heap must agree with the sorted ranking, ties included: query {query:?} at limit {limit}"
            );
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
    let huge = "z".repeat(4 * 1024 * 1024);

    assert_eq!(
        ranked(&[huge.as_str(), "src/z.rs", "z/lib.rs"], "z"),
        ["src/z.rs", "z/lib.rs", huge.as_str()],
        "an over-long candidate still matches, ranks last and leaves nothing behind that changes a later candidate"
    );
}

#[test]
fn a_limit_far_larger_than_the_input_costs_nothing_up_front() {
    let candidates = ["src/main.rs", "main/lib.rs"];
    let ranked = rank(&candidates, "main", NonZeroUsize::new(usize::MAX));

    assert_eq!(
        ranked.len(),
        2,
        "a limit caps the answer and reserves nothing before a candidate arrives"
    );
    assert_eq!(candidates[ranked[0].position], "src/main.rs");
}

#[test]
fn a_full_match_is_per_term_not_per_query() {
    assert_eq!(
        ranked(&["a a", "a"], "a a"),
        ["a", "a a"],
        "both terms match `a` in full, and neither matches `a a` in full"
    );
    assert_eq!(
        ranked(&["a ab", "ab"], "a ab"),
        ["ab", "a ab"],
        "a query's whitespace separates terms; a candidate need not reproduce it"
    );
}

#[test]
fn an_over_long_non_ascii_candidate_matches_and_ranks_last() {
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
    let fewer_bytes = format!("{}-a", "z".repeat(100));
    let fewer_chars = format!("{}.a", "\u{e9}".repeat(60));
    assert_eq!(
        score(&fewer_bytes, "a").expect("matches"),
        score(&fewer_chars, "a").expect("matches"),
        "the candidates must tie on score for the tiebreak to decide anything: the one with more characters pays more \
         gaps and earns them back with the larger boundary bonus after `-` than after `.`"
    );

    let ranked = ranked(&[fewer_chars.as_str(), fewer_bytes.as_str()], "a");
    assert_eq!(
        ranked[0], fewer_bytes,
        "fewer bytes wins, though it has more characters"
    );
}

#[test]
fn an_over_long_candidate_matched_in_full_still_ranks_first() {
    let long: String = "q".repeat(1025);
    let other = format!("{long}x");

    let ranked = ranked(&[other.as_str(), long.as_str()], &long);
    assert_eq!(ranked[0], long, "the full match leads even when over-long");
}

#[test]
fn the_score_bands_survive_aggregation() {
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
    assert_eq!(
        score("\u{212a}", "k"),
        None,
        "the Kelvin sign, `k` under Unicode folding, is not an ASCII k"
    );
    assert!(score("Kk", "k").is_some(), "an ASCII K still folds");
    assert!(
        score("\u{c9}", "\u{e9}").is_some(),
        "a pair of non-ASCII characters folds by Unicode lowercase"
    );
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
    let queries = ["main", "", "rs", "zz"];
    let limits = [None, NonZeroUsize::new(1), NonZeroUsize::new(3)];
    let splits: Vec<(usize, usize)> = (0..=paths.len())
        .flat_map(|first| (first..=paths.len()).map(move |second| (first, second)))
        .collect();

    for (query, limit) in queries
        .iter()
        .flat_map(|&query| limits.map(|limit| (query, limit)))
    {
        let offered = |range: std::ops::Range<usize>| {
            let mut ranking = Ranking::new(query, limit);
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
            let merge = |left, right| Ranking::merge([left, right], query, limit);
            let context = format!(
                "query {query:?}, limit {limit:?}, slices at {first} and {second}; the `?/main.rs` paths tie on score \
                 and length, so only input order places them"
            );

            assert_eq!(
                merge(merge(a.clone(), b.clone()), c.clone()),
                whole,
                "left first, {context}"
            );
            assert_eq!(merge(a, merge(b, c)), whole, "right first, {context}");
        }
    }
}
