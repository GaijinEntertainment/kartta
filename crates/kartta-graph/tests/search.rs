//! How a search finds definitions by the words of their names and paths, ranks them, and says what it left out.

use kartta_graph::{Graph, Search};
use kartta_lang::{Conventions, Extraction, Symbol, SymbolFlags, SymbolKind};

fn symbol(name: &str, qualified: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Function, line, line);
    qualified.clone_into(&mut symbol.qualified_name);
    symbol
}

fn flagged(name: &str, flags: SymbolFlags) -> Symbol {
    let mut symbol = symbol(name, name, 1);
    symbol.flags = flags;
    symbol
}

fn file(path: &str, symbols: Vec<Symbol>) -> Extraction {
    let mut extraction = Extraction::default();
    path.clone_into(&mut extraction.file);
    extraction.symbols = symbols;
    extraction
}

fn graph(files: &[Extraction]) -> Graph {
    let mut graph = Graph::new(Conventions::default());
    for extraction in files {
        graph.insert(extraction);
    }
    graph
}

fn places(search: &Search<'_>) -> Vec<String> {
    search
        .hits
        .iter()
        .map(|hit| format!("{}:{}", hit.definition.file, hit.definition.line))
        .collect()
}

fn names<'g>(search: &Search<'g>) -> Vec<&'g str> {
    search
        .hits
        .iter()
        .map(|hit| hit.definition.qualified_name)
        .collect()
}

#[test]
fn a_word_of_a_name_finds_its_definition() {
    let graph = graph(&[file(
        "src/app.go",
        vec![
            symbol("parseConfigFile", "app.parseConfigFile", 1),
            symbol("render", "app.render", 2),
        ],
    )]);

    let found = graph.search("config", 10);

    assert_eq!(names(&found), ["app.parseConfigFile"]);
    assert_eq!((found.total, found.named), (1, 1));
}

#[test]
fn an_acronym_is_one_word_a_query_finds() {
    let graph = graph(&[file(
        "src/net.go",
        vec![
            symbol("HTTPServer", "net.HTTPServer", 1),
            symbol("parseURL", "net.parseURL", 2),
        ],
    )]);

    assert_eq!(names(&graph.search("http", 10)), ["net.HTTPServer"]);
    assert_eq!(names(&graph.search("url", 10)), ["net.parseURL"]);
    assert_eq!(names(&graph.search("server", 10)), ["net.HTTPServer"]);
}

#[test]
fn a_name_that_is_the_query_leads_every_other_hit() {
    let graph = graph(&[
        file(
            "a/parse.go",
            vec![symbol("parse_parse_parse", "parse_parse_parse", 1)],
        ),
        file("b/other.go", vec![symbol("Parse", "util.Parse", 4)]),
    ]);

    let found = graph.search("Parse", 10);

    assert_eq!(names(&found), ["util.Parse", "parse_parse_parse"]);
    assert!(found.hits[0].exact && !found.hits[1].exact);
    assert!(found.hits[0].score > 1000.0 && found.hits[1].score <= 500.0);
}

#[test]
fn a_path_word_finds_every_definition_of_its_file_and_named_counts_those_whose_name_holds_it() {
    let graph = graph(&[file(
        "tools/config_loader.go",
        vec![
            symbol("run", "run", 1),
            symbol("LoaderState", "LoaderState", 2),
        ],
    )]);

    let found = graph.search("loader", 10);

    assert_eq!(names(&found), ["LoaderState", "run"]);
    assert_eq!((found.total, found.named), (2, 1));
}

#[test]
fn generated_and_vendored_code_ranks_after_a_match_as_good() {
    let graph = graph(&[
        file(
            "a.go",
            vec![flagged("Codec", SymbolFlags::empty().with_generated(true))],
        ),
        file(
            "b.go",
            vec![flagged("Codec", SymbolFlags::empty().with_vendored(true))],
        ),
        file(
            "c.go",
            vec![flagged(
                "Codec",
                SymbolFlags::empty()
                    .with_generated(true)
                    .with_vendored(true),
            )],
        ),
        file("d.go", vec![flagged("Codec", SymbolFlags::empty())]),
    ]);

    let found = graph.search("codec", 10);

    assert_eq!(places(&found), ["d.go:1", "b.go:1", "a.go:1", "c.go:1"]);
    let scores: Vec<f64> = found.hits.iter().map(|hit| hit.score).collect();
    assert!((scores[1] / scores[0] - 0.75).abs() < 1e-9);
    assert!((scores[2] / scores[0] - 0.5).abs() < 1e-9);
    assert!((scores[3] / scores[0] - 0.375).abs() < 1e-9);
}

#[test]
fn a_definition_holding_more_of_the_query_s_words_ranks_first() {
    let graph = graph(&[
        file(
            "a.go",
            vec![symbol("parseParseParse", "parseParseParse", 1)],
        ),
        file(
            "b.go",
            vec![symbol("parseConfigLater", "parseConfigLater", 1)],
        ),
        file("c.go", vec![symbol("config", "config", 1)]),
    ]);

    let found = graph.search("parse config", 10);

    assert_eq!(names(&found)[0], "parseConfigLater");
}

#[test]
fn every_time_a_definition_holds_a_word_counts_however_often_it_repeats() {
    let repeated = |times: usize| vec!["x"; times].join("_");
    let (fewer, more) = (repeated(128), repeated(129));
    let graph = graph(&[
        file("a.go", vec![symbol(&fewer, &fewer, 1)]),
        file("b.go", vec![symbol(&more, &more, 1)]),
    ]);

    let found = graph.search("x", 10);

    assert_eq!(places(&found), ["b.go:1", "a.go:1"]);
    assert!(found.hits[0].score > found.hits[1].score);
}

#[test]
fn the_query_s_words_in_order_and_side_by_side_rank_first() {
    let graph = graph(&[
        file("a.go", vec![symbol("fastPathLong", "fastPathLong", 1)]),
        file("b.go", vec![symbol("dFastLongLen", "dFastLongLen", 1)]),
        file("c.go", vec![symbol("longFastSwap", "longFastSwap", 1)]),
    ]);

    let found = graph.search("FastLong", 10);

    assert_eq!(
        names(&found),
        ["dFastLongLen", "fastPathLong", "longFastSwap"]
    );
}

#[test]
fn a_phrase_is_found_within_the_qualified_name_not_where_the_name_follows_it() {
    let graph = graph(&[file(
        "a.go",
        vec![
            symbol("longFast", "longFast", 1),
            symbol("fastLong", "fastLong", 2),
        ],
    )]);

    let found = graph.search("fast long", 10);

    assert_eq!(names(&found), ["fastLong", "longFast"]);
    assert!(
        found.hits[0].score > found.hits[1].score,
        "only fastLong spells the phrase"
    );
}

#[test]
fn a_word_of_a_definition_s_own_name_outweighs_a_word_of_its_owner() {
    let graph = graph(&[file(
        "auth.go",
        vec![
            symbol(
                "GetProperty",
                "auth.CredentialsPropertyProvider.GetProperty",
                2,
            ),
            symbol(
                "CredentialsPropertyProvider",
                "auth.CredentialsPropertyProvider",
                1,
            ),
        ],
    )]);

    let found = graph.search("CredentialsProperty", 10);

    assert_eq!(names(&found)[0], "auth.CredentialsPropertyProvider");
}

#[test]
fn equal_scores_rank_by_path_then_line() {
    let graph = graph(&[
        file("z/run.go", vec![symbol("Start", "Start", 3)]),
        file(
            "a/run.go",
            vec![symbol("Start", "Start", 9), symbol("Start", "Start", 2)],
        ),
    ]);

    assert_eq!(
        places(&graph.search("start", 10)),
        ["a/run.go:2", "a/run.go:9", "z/run.go:3"]
    );
}

#[test]
fn the_words_no_definition_holds_and_those_past_the_eighth_are_reported() {
    let graph = graph(&[file("a.go", vec![symbol("parseConfig", "parseConfig", 1)])]);

    let found = graph.search(
        "parse missing one two three four five six seven eight parse seven Eight",
        10,
    );

    assert_eq!(names(&found), ["parseConfig"]);
    assert_eq!(
        found.unmatched_words,
        ["missing", "one", "two", "three", "four", "five", "six"]
    );
    assert_eq!(found.dropped_words, 2);
}

#[test]
fn a_word_repeated_in_the_query_counts_once() {
    let graph = graph(&[file(
        "a.go",
        vec![
            symbol("parseConfig", "parseConfig", 1),
            symbol("render", "render", 2),
        ],
    )]);

    let once = graph.search("parse", 10);
    let twice = graph.search("parse Parse parse", 10);

    assert_eq!(once.hits[0].score, twice.hits[0].score);
    assert_eq!(twice.total, 1);
}

#[test]
fn a_limit_keeps_the_best_and_the_total_counts_every_match() {
    let graph = graph(&[
        file("a.go", vec![symbol("Run", "Run", 1)]),
        file("b.go", vec![symbol("runAll", "runAll", 1)]),
        file("c.go", vec![symbol("runOnce", "runOnce", 1)]),
    ]);

    let best = graph.search("run", 2);
    let counted = graph.search("run", 0);

    assert_eq!(best.hits.len(), 2);
    assert_eq!(best.hits[0].definition.name, "Run");
    assert_eq!(best.total, 3);
    assert_eq!(
        (counted.hits.len(), counted.total, counted.named),
        (0, 3, 3)
    );
}

#[test]
fn an_ascii_word_finds_a_name_that_lowercases_to_it_from_outside_ascii() {
    let graph = graph(&[file(
        "a.go",
        vec![symbol("\u{212A}ernel", "\u{212A}ernel", 1)],
    )]);

    let found = graph.search("kernel", 10);

    assert_eq!(names(&found), ["\u{212A}ernel"]);
    assert_eq!(found.total, 1);
}

#[test]
fn a_word_outside_ascii_finds_the_name_that_holds_it() {
    let graph = graph(&[file(
        "a.go",
        vec![symbol("CaféBar", "CaféBar", 1), symbol("cafe", "cafe", 2)],
    )]);

    assert_eq!(names(&graph.search("CAFÉ", 10)), ["CaféBar"]);
}

#[test]
fn a_file_removed_or_replaced_is_searched_by_what_it_holds_now() {
    let mut graph = graph(&[
        file("a.go", vec![symbol("oldParser", "oldParser", 1)]),
        file("b.go", vec![symbol("keepParser", "keepParser", 1)]),
    ]);
    assert_eq!(graph.search("parser", 10).total, 2);

    graph.insert(&file("a.go", vec![symbol("newRender", "newRender", 1)]));
    assert_eq!(names(&graph.search("parser", 10)), ["keepParser"]);
    assert_eq!(names(&graph.search("render", 10)), ["newRender"]);

    graph.remove("b.go");
    assert_eq!(graph.search("parser", 10).total, 0);
}

#[test]
fn a_query_with_no_word_finds_only_a_name_it_spells_exactly() {
    let graph = graph(&[file(
        "a.js",
        vec![symbol("$", "$", 1), symbol("dollar", "dollar", 2)],
    )]);

    let found = graph.search("$", 10);
    let nothing = graph.search("!", 10);

    assert_eq!(names(&found), ["$"]);
    assert!(found.hits[0].exact);
    assert_eq!((nothing.total, nothing.hits.len()), (0, 0));
}

#[test]
fn a_name_with_a_combining_mark_is_found_by_either_spelling() {
    let graph = graph(&[file(
        "a.go",
        vec![
            symbol("i\u{307}stanbul", "i\u{307}stanbul", 1),
            symbol("İzmir", "İzmir", 2),
        ],
    )]);

    for (query, wanted) in [
        ("İstanbul", "i\u{307}stanbul"),
        ("istanbul", "i\u{307}stanbul"),
        ("i\u{307}zmir", "İzmir"),
        ("izmir", "İzmir"),
    ] {
        let found = graph.search(query, 10);
        assert_eq!(names(&found), [wanted], "{query}");
    }
}
