//! The ranking specification: what the scorer guarantees under [`Config::DEFAULT`].
//!
//! Every test here states a behaviour a caller can rely on, not an implementation detail. Weight values are never
//! asserted — only the orderings they exist to produce — so retuning the default is free as long as it keeps its
//! promises.

use std::num::NonZeroUsize;

use kartta_fuzz::{CaseMatching, Config, Matcher};

/// The ranked candidates for `pattern`, best first.
fn ranked(pattern: &str, candidates: &[&str]) -> Vec<String> {
    ranked_with(Config::DEFAULT, pattern, candidates)
}

/// The ranked candidates for `pattern` under a specific configuration.
fn ranked_with(config: Config, pattern: &str, candidates: &[&str]) -> Vec<String> {
    Matcher::new(config)
        .match_list(candidates.iter().copied(), pattern, None)
        .iter()
        .map(|m| m.candidate.to_owned())
        .collect()
}

/// The score of one candidate under the default configuration.
fn score(pattern: &str, candidate: &str) -> Option<i32> {
    score_with(Config::DEFAULT, pattern, candidate)
}

/// The score of one candidate under a specific configuration.
fn score_with(config: Config, pattern: &str, candidate: &str) -> Option<i32> {
    Matcher::new(config).score(candidate, pattern)
}

#[test]
fn basename_hit_outranks_directory_hit() {
    // The reason this crate exists. Input order is reversed to prove a reorder rather than a passthrough.
    assert_eq!(
        ranked("main", &["main/lib.rs", "src/main.rs"]),
        ["src/main.rs", "main/lib.rs"]
    );
}

#[test]
fn exact_basename_ranks_first() {
    let ranked = ranked("cargo", &["src/cargo/mod.rs", "Cargo.toml"]);
    assert_eq!(
        ranked[0], "Cargo.toml",
        "an exact, case-insensitive basename wins"
    );
}

#[test]
fn exact_match_outranks_every_partial_match() {
    let exact = score("ab", "ab").expect("an exact match");
    let cased = score("ab", "AB").expect("an exact match up to case");
    let partial = score("ab", "abc").expect("a subsequence match");

    assert_eq!(exact, cased, "case never changes an exact match");
    assert!(
        partial < exact,
        "a partial match ranks below an exact one, got {partial} against {exact}"
    );
}

#[test]
fn contiguous_run_outranks_gapped_match() {
    let ranked = ranked("abc", &["a/b/c.rs", "axbxc.rs", "abc.rs"]);
    assert_eq!(
        ranked[0], "abc.rs",
        "a contiguous substring is the strongest match"
    );
}

#[test]
fn camel_hump_outranks_flat_match() {
    let ranked = ranked("fb", &["foobar.rs", "fooBar.rs"]);
    assert_eq!(
        ranked[0], "fooBar.rs",
        "a camelCase hump is a word boundary"
    );
}

#[test]
fn matching_requires_order() {
    assert_eq!(score("ba", "ab"), None, "a subsequence is ordered");
    assert_eq!(score("xyz", "abc"), None);
}

#[test]
fn every_term_must_match() {
    let paths = ["src/auth/handler.go", "src/api/handler.go"];
    assert_eq!(ranked("handler auth", &paths), ["src/auth/handler.go"]);
    assert_eq!(ranked("handler zz", &paths), [] as [&str; 0]);
}

#[test]
fn multi_term_score_sums_the_terms() {
    let both = score("ab cd", "ab/cd").expect("both terms match");
    let ab = score("ab", "ab/cd").expect("the first term matches");
    let cd = score("cd", "ab/cd").expect("the second term matches");

    assert_eq!(both, ab + cd, "the rank is the sum of the per-term scores");
}

#[test]
fn equal_scores_keep_input_order() {
    // "x" matches the basename of both identically, and both are the same length, so nothing breaks the tie.
    assert_eq!(ranked("x", &["aa/x", "bb/x"]), ["aa/x", "bb/x"]);
    assert_eq!(ranked("x", &["bb/x", "aa/x"]), ["bb/x", "aa/x"]);
}

#[test]
fn shorter_candidate_wins_a_tie_when_configured() {
    // Gap penalties are what usually separate two candidates matching at the same position, so zeroing them is what
    // makes a genuine score tie between different lengths reachable at all.
    let mut config = Config::DEFAULT;
    config.scoring.gap_leading = 0;
    config.scoring.gap_trailing = 0;
    let candidates = ["a/xyz", "a/x"];

    assert_eq!(
        ranked_with(config, "x", &candidates)[0],
        "a/x",
        "the length tiebreak reorders"
    );

    config.length_tiebreak = false;
    assert_eq!(
        ranked_with(config, "x", &candidates)[0],
        "a/xyz",
        "without it, input order stands"
    );
}

#[test]
fn empty_pattern_returns_every_candidate_in_input_order() {
    let candidates = ["b.rs", "a.rs"];
    assert_eq!(ranked("", &candidates), ["b.rs", "a.rs"]);
    assert_eq!(
        ranked("   ", &candidates),
        ["b.rs", "a.rs"],
        "whitespace only is an empty pattern"
    );
    assert_eq!(score("   ", "anything"), Some(0));
}

#[test]
fn limit_truncates_after_ranking() {
    let candidates = ["main/lib.rs", "src/main.rs"];
    let limited =
        Matcher::new(Config::DEFAULT).match_list(candidates, "main", NonZeroUsize::new(1));

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
        score("cx", "café/x.rs").expect("a subsequence match"),
        score("cx", "cafe/x.rs").expect("a subsequence match"),
        "a two-symbol term scores the same either side of a multi-byte character"
    );
    assert_eq!(
        score("é", "café/x.rs").expect("a subsequence match"),
        score("e", "cafe/x.rs").expect("a subsequence match"),
        "and so does a non-ASCII term"
    );

    // The ordering the crate exists for survives the character path too.
    assert_eq!(
        ranked("cx", &["cx/é.rs", "é/cx.rs"]),
        ["é/cx.rs", "cx/é.rs"],
        "the basename hit still wins"
    );
}

#[test]
fn long_contiguous_match_saturates_positive() {
    // Overflow-checked debug builds must not panic and release builds must not wrap negative.
    let pattern: String = std::iter::repeat_n('a', 50).collect();
    let candidate: String = std::iter::repeat_n('a', 60).collect();

    let score = score(&pattern, &candidate).expect("every pattern character matches");
    assert!(score > 0, "a saturated score stays positive, got {score}");
}

#[test]
fn candidate_beyond_the_alignment_limit_matches_but_ranks_last() {
    // Too long to align, so it cannot be ordered against candidates that were scored. It still matches — dropping it
    // would make the file unfindable — but it ranks below every scored candidate rather than above the ones whose
    // leading gaps outweighed their bonuses.
    let long = "z".repeat(2000);
    let deep = format!("{}z", "x".repeat(1000));

    assert!(score("z", &long).is_some(), "it is still a match");
    let deep_score = score("z", &deep).expect("a scored match, however poorly");
    let long_score = score("z", &long).expect("a match beyond the alignment limit");
    assert!(
        long_score < deep_score,
        "the unscoreable candidate must not outrank a scored one, got {long_score} against {deep_score}"
    );

    assert_eq!(
        ranked("z", &[long.as_str(), deep.as_str()]),
        [deep, long],
        "and the ranking follows"
    );
}

#[test]
fn smart_case_narrows_only_when_the_pattern_has_uppercase() {
    let mut config = Config::DEFAULT;
    config.case = CaseMatching::Smart;
    let candidates = ["readme.md", "README.md"];

    assert_eq!(
        ranked_with(config, "readme", &candidates).len(),
        2,
        "a lowercase pattern matches both"
    );
    assert_eq!(
        ranked_with(config, "README", &candidates),
        ["README.md"],
        "an uppercase one narrows"
    );

    // The uppercase test is char::is_uppercase, not an ASCII range, so a non-ASCII capital narrows the same way.
    assert_eq!(
        score_with(config, "\u{c9}", "\u{e9}"),
        None,
        "capital E-acute is uppercase"
    );
    assert!(
        score_with(config, "\u{e9}", "\u{c9}").is_some(),
        "and a lowercase pattern still folds onto it"
    );
}

#[test]
fn respect_case_never_folds() {
    let mut config = Config::DEFAULT;
    config.case = CaseMatching::Respect;
    assert_eq!(
        ranked_with(config, "readme", &["README.md"]),
        [] as [&str; 0]
    );
}

#[test]
fn candidate_start_bonus_lifts_a_whole_path_prefix() {
    // The match in the first candidate lands in the directory, so it earns no basename bonus and loses by default.
    let candidates = ["api/zzzzzzzzzz.rs", "z/api.rs"];

    let mut config = Config::DEFAULT;
    assert_eq!(
        ranked_with(config, "api", &candidates)[0],
        "z/api.rs",
        "the basename hit leads by default"
    );

    config.scoring.candidate_start = 10_000;
    assert_eq!(
        ranked_with(config, "api", &candidates)[0],
        "api/zzzzzzzzzz.rs",
        "the prefix bonus lifts a match starting at position zero above it"
    );
}

#[test]
fn segment_separators_decide_the_basename() {
    // With `.` as the separator, the basename of `pkg.mod.Symbol` is `Symbol`.
    let mut config = Config::DEFAULT;
    config.segment_separators = &['.'];

    let ranked = ranked_with(config, "sym", &["sym.other.Thing", "pkg.mod.Symbol"]);
    assert_eq!(
        ranked[0], "pkg.mod.Symbol",
        "the last dot-segment is the basename"
    );
}

#[test]
fn backslash_separates_segments_like_a_slash() {
    assert_eq!(
        ranked("main", &[r"main\lib.rs", r"src\main.rs"]),
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

    for pattern in ["r", "render", "pkg mod", "pass rs"] {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let full = matcher.match_list(borrowed.iter().copied(), pattern, None);

        for limit in [1usize, 2, 7, 50, 499, 500, 501] {
            let capped = Matcher::new(Config::DEFAULT).match_list(
                borrowed.iter().copied(),
                pattern,
                NonZeroUsize::new(limit),
            );
            let expected = &full[..limit.min(full.len())];
            assert_eq!(capped, expected, "pattern {pattern:?} at limit {limit}");
        }
    }
}

#[test]
fn a_limit_agrees_without_the_length_tiebreak_too() {
    let mut config = Config::DEFAULT;
    config.length_tiebreak = false;

    let candidates = ["aa/x", "bb/x", "c/xxxxxx", "dd/x", "e/x"];
    let full = ranked_with(config, "x", &candidates);

    for limit in [1usize, 3, 5] {
        let capped: Vec<String> = Matcher::new(config)
            .match_list(candidates, "x", NonZeroUsize::new(limit))
            .iter()
            .map(|m| m.candidate.to_owned())
            .collect();
        assert_eq!(capped, full[..limit.min(full.len())], "limit {limit}");
    }
}

/// Configurations spanning the corners of the weight space, including ones no sane caller would set.
///
/// Every field is a public `i32`, so every `i32` is reachable. The guarantees below are the ones stated for *any*
/// configuration, so they are tested against the values that break a naive derivation rather than only the tuned
/// default: negative weights, zero weights, and gaps pointing the wrong way.
fn hostile_configs() -> Vec<(&'static str, Config)> {
    /// Sets every weight to `weight` and every gap to `gap`.
    const fn uniform(weight: i32, gap: i32) -> Config {
        let mut config = Config::DEFAULT;
        let s = &mut config.scoring;
        s.consecutive = weight;
        s.after_segment = weight;
        s.after_word = weight;
        s.digit_boundary = weight;
        s.camel_hump = weight;
        s.after_dot = weight;
        s.basename = weight;
        s.candidate_start = weight;
        s.gap_leading = gap;
        s.gap_inner = gap;
        s.gap_trailing = gap;
        config
    }

    let mut ceiling = Config::DEFAULT;
    ceiling.scoring.consecutive = i32::MAX;
    ceiling.scoring.basename = i32::MAX;
    ceiling.scoring.candidate_start = i32::MAX;

    vec![
        ("default", Config::DEFAULT),
        ("all weights negative", uniform(-1, -1)),
        ("all weights zero", uniform(0, 0)),
        ("gaps that reward skipping", uniform(0, i32::MAX)),
        ("weights at the ceiling", ceiling),
    ]
}

#[test]
fn an_exact_match_ranks_first_under_any_configuration() {
    // The one ordering the crate promises for every point in the weight space. A derivation that sums a negative
    // per-character maximum, or a sentinel a positive gap lifted off the floor, breaks it here and nowhere else.
    let candidates = ["ab", "xaxbx", "a/b", "abc", "zzab", "ab/ab"];

    for (name, config) in hostile_configs() {
        let ranked = ranked_with(config, "ab", &candidates);
        assert_eq!(
            ranked[0], "ab",
            "exact match must lead under {name}, got {ranked:?}"
        );
    }
}

#[test]
fn no_configuration_lets_an_unmatched_candidate_score() {
    // A candidate missing the pattern never matches, whatever the weights. A gap weight that promotes the "no
    // alignment here" sentinel would otherwise let cells that matched nothing carry a score.
    for (name, config) in hostile_configs() {
        assert_eq!(
            ranked_with(config, "ab", &["ba", "b", "xxxx", ""]),
            [] as [&str; 0],
            "nothing matches ab under {name}"
        );
    }
}

#[test]
fn a_gap_weight_above_zero_ranks_exactly_as_zero_does() {
    // Gaps are penalties by contract, so a positive one is clamped rather than honoured. Without the clamp it does not
    // merely rank oddly: added to the "no alignment here" sentinel it lifts it off the floor, and cells that matched
    // nothing outrank every real alignment.
    let candidates = ["axxxxxxxxb", "ab.rs", "a/b", "zzz", "ab"];

    let mut clamped = Config::DEFAULT;
    clamped.scoring.gap_leading = 0;
    clamped.scoring.gap_inner = 0;
    clamped.scoring.gap_trailing = 0;

    let mut rewarding = Config::DEFAULT;
    rewarding.scoring.gap_leading = i32::MAX;
    rewarding.scoring.gap_inner = i32::MAX;
    rewarding.scoring.gap_trailing = 7;

    assert_eq!(
        ranked_with(rewarding, "ab", &candidates),
        ranked_with(clamped, "ab", &candidates),
        "a gap above zero must rank as zero does"
    );
}

#[test]
fn an_over_limit_candidate_is_scored_without_being_prepared() {
    // Sizing the per-candidate buffers to a candidate that is never aligned would let one caller-supplied length hold
    // an allocation for the rest of the query, for work immediately thrown away. Reusing one matcher across a huge
    // candidate and then a normal one proves the huge one left nothing behind that changes the normal one's answer.
    let huge = "z".repeat(4 * 1024 * 1024);
    let normal = ["src/main.rs", "main/lib.rs"];

    let mut matcher = Matcher::new(Config::DEFAULT);
    assert!(
        matcher.score(&huge, "z").is_some(),
        "an over-limit candidate still matches"
    );

    let after: Vec<&str> = matcher
        .match_list(normal, "main", None)
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
    let ranked = Matcher::new(Config::DEFAULT).match_list(
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

    let ranked = ranked_with(config, "main", &["zzz/main.rs", "main.rs"]);
    assert_eq!(
        ranked[0], "main.rs",
        "the earlier match leads once nothing ends a segment"
    );

    // With `/` no longer a separator it is an ordinary character, so it neither starts a segment nor moves the
    // basename, and a query may match straight through it.
    assert!(
        score_with(config, "zm", "z/main.rs").is_some(),
        "a separator that is not configured is just a character"
    );
}

#[test]
fn an_empty_word_separator_set_only_drops_the_word_bonus() {
    // Asserting that the candidate still matches would prove nothing: it matches whether or not `_` separates words.
    // The observable difference is the boundary bonus at `p`, so the two settings are scored and compared.
    let mut config = Config::DEFAULT;
    let with_separators = score_with(config, "rp", "render_pass.rs").expect("matches");

    config.word_separators = &[];
    let without = score_with(config, "rp", "render_pass.rs").expect("still matches");

    assert_eq!(
        with_separators - without,
        Config::DEFAULT.scoring.after_word,
        "emptying the set costs exactly the word bonus at `p`, and nothing else"
    );
}

#[test]
fn a_full_match_is_per_term_not_per_pattern() {
    // With several terms the rank is the sum of per-term scores, so "matched in full" is a property each term has
    // against the candidate -- not a property of the candidate against the pattern string. Pattern `a a` therefore
    // ranks `a`, which both terms match in full, above `a a`, which neither does.
    //
    // The whitespace in a pattern separates terms; it is not content the candidate has to contain. Privileging a
    // candidate that happens to reproduce the pattern's own spacing would rank `a ab` above `ab` for pattern `a ab`,
    // which is the wrong answer for a query asking only that both terms be present.
    assert_eq!(ranked("a a", &["a a", "a"]), ["a", "a a"]);
    assert_eq!(ranked("a ab", &["a ab", "ab"]), ["ab", "a ab"]);
}

#[test]
fn an_over_limit_non_ascii_candidate_matches_and_ranks_last() {
    // The non-ASCII path establishes a candidate's length by decoding it, so an over-limit candidate must settle that
    // from a bounded prefix rather than a full pass -- a one-character term matching the first character otherwise
    // leaves the whole remaining suffix to scan for a length already known to be too large.
    let huge: String = std::iter::repeat_n('é', 2 * 1024 * 1024).collect();
    let scored = "é/main.rs";

    let mut matcher = Matcher::new(Config::DEFAULT);
    assert!(
        matcher.score(&huge, "é").is_some(),
        "an over-limit non-ASCII candidate still matches"
    );

    assert_eq!(
        ranked("é", &[huge.as_str(), scored]),
        [scored.to_owned(), huge.clone()],
        "and ranks below the one that was scored"
    );
}

#[test]
fn full_term_precedence_survives_aggregation() {
    // Summing per-term scores is where the full-match rule is most easily lost: weights large enough to saturate
    // drive a full aggregate and a partial one to the same ceiling, and the ordering then falls to whichever tiebreak
    // happens to be configured rather than to the guarantee.
    let mut config = Config::DEFAULT;
    config.scoring.consecutive = i32::MAX;
    config.scoring.basename = i32::MAX;
    config.scoring.candidate_start = i32::MAX;
    config.length_tiebreak = false;

    assert_eq!(
        ranked_with(config, "a a", &["aa", "a"]),
        ["a", "aa"],
        "the candidate both terms match in full must lead"
    );
}

#[test]
fn the_length_tiebreak_orders_by_utf8_byte_length() {
    // The unit is stated in the contract because these two candidates order one way by bytes and the other by
    // characters: with acutes, "eea" is 5 bytes and 3 characters, "aaaa" is 4 of each.
    //
    // Reaching the tiebreak at all takes work. A match at position zero collects the segment bonus, which alone
    // separates the two on score and leaves the tiebreak untouched -- so the segment bonus goes to zero here, and the
    // scores are asserted equal before the order is, or this test would pass whichever unit the sort used.
    let mut config = Config::DEFAULT;
    config.scoring.gap_leading = 0;
    config.scoring.gap_trailing = 0;
    config.scoring.after_segment = 0;

    let fewer_bytes = score_with(config, "a", "aaaa").expect("matches");
    let fewer_chars = score_with(config, "a", "\u{e9}\u{e9}a").expect("matches");
    assert_eq!(
        fewer_bytes, fewer_chars,
        "the candidates must tie on score for the tiebreak to decide anything"
    );

    let ranked = ranked_with(config, "a", &["\u{e9}\u{e9}a", "aaaa"]);
    assert_eq!(
        ranked[0], "aaaa",
        "fewer bytes wins, though it has more characters"
    );
}

#[test]
fn an_alignment_always_outranks_the_over_limit_floor() {
    // The floor is a constant and an alignment is not, so no constant can sit under every alignment for every set of
    // weights: `basename = i32::MIN` saturates an aligned score onto the sentinel, beneath a floor one above it, and
    // a candidate that was never aligned would outrank one that was. The alignment is lifted off the floor instead.
    let mut config = Config::DEFAULT;
    config.scoring.basename = i32::MIN;

    let aligned = score_with(config, "a", "xa").expect("an aligned match, however saturated");
    let floored = score_with(config, "a", &"a".repeat(2000)).expect("an over-limit match");
    assert!(
        floored < aligned,
        "the unaligned candidate must stay below the aligned one, got {floored} against {aligned}"
    );
}

#[test]
fn an_over_limit_candidate_matched_in_full_still_ranks_first() {
    // The alignment limit bounds the recurrence, and a full match never runs it: term and candidate lengths settle it
    // outright. So the bound does not reach a full match, and "a candidate every term matches in full ranks first"
    // holds past 1024 positions as well.
    let long: String = "q".repeat(1025);
    let other = format!("{long}x");

    let ranked = ranked(&long, &[other.as_str(), long.as_str()]);
    assert_eq!(
        ranked[0], long,
        "the full match leads even beyond the alignment limit"
    );
}

#[test]
fn the_score_bands_survive_aggregation() {
    // Summing per-term scores is where the ordering has been lost repeatedly: a saturated aggregate says nothing
    // about what it was built from, so a floored aggregate and an aligned one, or a partial and a full one, collapse
    // onto the same value and the order falls to whichever tiebreak is configured. The sum is exact and the bands are
    // applied once afterwards, so each kind keeps its own range whatever the weights.
    let mut config = Config::DEFAULT;
    config.scoring.basename = i32::MIN;
    config.length_tiebreak = false;

    let over_limit = "a".repeat(2000);
    let aligned = score_with(config, "a a", "xaxa").expect("an aligned aggregate");
    let floored = score_with(config, "a a", &over_limit).expect("a floored aggregate");
    assert!(
        floored < aligned,
        "a floored aggregate must stay below an aligned one, got {floored} against {aligned}"
    );

    // Both orders of the same pair, since a tie would otherwise be settled by input order and hide the defect.
    assert_eq!(
        ranked_with(config, "a a", &[over_limit.as_str(), "xaxa"])[0],
        "xaxa"
    );
    assert_eq!(
        ranked_with(config, "a a", &["xaxa", over_limit.as_str()])[0],
        "xaxa"
    );
}

#[test]
fn ignore_case_does_not_fold_across_the_ascii_boundary() {
    // The variant is documented as ASCII-first rather than as full Unicode folding, and this is the case that parts
    // the two: U+212A KELVIN SIGN folds to `k` under Unicode rules and does not here.
    assert_eq!(
        score("k", "\u{212a}"),
        None,
        "the Kelvin sign is not an ASCII k"
    );
    assert!(score("k", "Kk").is_some(), "an ASCII K still folds");

    // The other side of the same rule: neither character is ASCII, so the Unicode lowercase comparison decides, and
    // it folds. Without this the variant would read as "ASCII only" rather than "not across the boundary".
    assert!(
        score("\u{e9}", "\u{c9}").is_some(),
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

    for pattern in ["main", "", "rs"] {
        for limit in [None, NonZeroUsize::new(1), NonZeroUsize::new(3)] {
            let whole = matcher.match_list(paths, pattern, limit);

            for split in 0..=paths.len() {
                let (left, right) = paths.split_at(split);
                let halves = [
                    matcher.match_list(left.iter().copied(), pattern, limit),
                    matcher.match_list(right.iter().copied(), pattern, limit),
                ];

                assert_eq!(
                    matcher.merge(halves, pattern, limit),
                    whole,
                    "pattern {pattern:?}, limit {limit:?}, split at {split}"
                );
            }
        }
    }
}
