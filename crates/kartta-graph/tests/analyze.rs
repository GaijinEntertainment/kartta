//! What a change to the symbols of a segment reaches: their direct calls, the callers a walk over them reaches, and
//! the files around them.

use std::ops::RangeInclusive;

use kartta_graph::{DirectCalls, Graph, Limits, ReachBand, Segment};
use kartta_lang::{
    CallRef, Callable, CallableRole, Conventions, Extraction, IncludeRef, Symbol, SymbolId,
    SymbolKind,
};

struct File {
    extraction: Extraction,
}

impl File {
    fn new(path: &str) -> Self {
        let mut extraction = Extraction::default();
        path.clone_into(&mut extraction.file);
        Self { extraction }
    }

    fn function(mut self, name: &str, lines: (u32, u32)) -> Self {
        self.extraction
            .symbols
            .push(Symbol::new(name, SymbolKind::Function, lines.0, lines.1));
        self
    }

    fn calls(mut self, from: u32, name: &str, line: u32) -> Self {
        self.extraction.calls.push(CallRef {
            caller_id: SymbolId(from),
            callee: name.to_owned(),
            receiver: None,
            line,
        });
        self
    }

    fn callable(mut self, symbol: u32, key: &str, role: CallableRole) -> Self {
        self.extraction.callables.push(Callable {
            symbol: SymbolId(symbol),
            key: key.to_owned(),
            role,
        });
        self
    }

    fn includes(mut self, target: &str) -> Self {
        self.extraction.includes.push(IncludeRef {
            target: target.to_owned(),
        });
        self
    }
}

fn graph(files: Vec<File>) -> Graph {
    let mut graph = Graph::new(Conventions::default());
    for file in files {
        graph.insert(&file.extraction);
    }
    graph
}

fn names<'g>(segment: &Segment<'g>) -> Vec<&'g str> {
    segment
        .symbols
        .iter()
        .map(|symbol| symbol.definition.name)
        .collect()
}

fn chain() -> Graph {
    graph(vec![
        File::new("lib/core.c")
            .function("core", (1, 3))
            .function("helper", (5, 9))
            .calls(1, "core", 6)
            .calls(1, "missing", 7),
        File::new("app/mid.c")
            .function("mid", (1, 4))
            .calls(0, "helper", 2)
            .includes("lib/core.h"),
        File::new("app/top.c")
            .function("top", (1, 4))
            .calls(0, "mid", 2),
        File::new("lib/core.h").includes("lib/base.h"),
        File::new("lib/base.h"),
        File::new("tools/user.c").includes("lib/core.c"),
    ])
}

#[test]
fn a_segment_holds_the_symbols_its_lines_overlap_in_outline_order() {
    let graph = chain();

    let whole = graph
        .analyze_segment("lib/core.c", 1..=u32::MAX, Limits::new(3))
        .expect("the graph holds lib/core.c");
    let one = graph
        .analyze_segment("lib/core.c", 6..=6, Limits::new(3))
        .expect("held");
    let across = graph
        .analyze_segment("lib/core.c", 3..=5, Limits::new(3))
        .expect("held");
    let between = graph
        .analyze_segment("lib/core.c", 4..=4, Limits::new(3))
        .expect("held");

    assert_eq!(whole.file, "lib/core.c");
    assert_eq!(whole.lines, 1..=u32::MAX);
    assert_eq!(names(&whole), ["core", "helper"]);
    assert_eq!(names(&one), ["helper"]);
    assert_eq!(names(&across), ["core", "helper"]);
    assert_eq!(names(&between), Vec::<&str>::new());
}

#[test]
fn a_range_that_starts_past_its_end_overlaps_no_symbol() {
    let graph = chain();

    let reversed = graph
        .analyze_segment("lib/core.c", RangeInclusive::new(9, 5), Limits::new(3))
        .expect("held");

    assert_eq!(names(&reversed), Vec::<&str>::new());
    assert_eq!(reversed.included_by, ["tools/user.c"]);
}

#[test]
fn a_file_the_graph_does_not_hold_answers_none() {
    let graph = chain();

    assert!(
        graph
            .analyze_segment("lib/absent.c", 1..=9, Limits::new(1))
            .is_none()
    );
}

#[test]
fn each_symbol_lists_its_direct_callers_and_callees_and_counts_what_bound_nothing() {
    let graph = chain();

    let segment = graph
        .analyze_segment("lib/core.c", 5..=9, Limits::new(1))
        .expect("held");
    let helper = &segment.symbols[0];
    let listed = |calls: &DirectCalls<'_>| -> Vec<(String, u32)> {
        calls
            .listed
            .iter()
            .map(|reached| (reached.symbol.name.to_owned(), reached.line))
            .collect()
    };

    assert_eq!(listed(&helper.callers), [("mid".to_owned(), 2)]);
    assert_eq!(helper.callers.total, 1);
    assert_eq!(listed(&helper.callees), [("core".to_owned(), 6)]);
    assert_eq!(helper.callees.total, 1);
    assert_eq!(helper.unresolved, 1);
}

#[test]
fn a_declaration_answers_with_the_calls_of_the_definition_it_links_to() {
    let graph = graph(vec![
        File::new("lib/draw.h").function("draw", (1, 1)).callable(
            0,
            "draw()",
            CallableRole::Declaration,
        ),
        File::new("lib/draw.cpp")
            .function("draw", (3, 6))
            .function("paint", (8, 9))
            .calls(0, "paint", 4)
            .callable(0, "draw()", CallableRole::Definition)
            .includes("lib/draw.h"),
        File::new("app/main.cpp")
            .function("run", (1, 3))
            .calls(0, "draw", 2)
            .includes("lib/draw.h"),
    ]);

    let segment = graph
        .analyze_segment("lib/draw.h", 1..=1, Limits::new(3))
        .expect("held");
    let draw = &segment.symbols[0];

    assert_eq!(draw.definition.file, "lib/draw.h");
    assert_eq!(draw.callers.listed[0].symbol.name, "run");
    assert_eq!(draw.callees.listed[0].symbol.name, "paint");
    assert_eq!(draw.blast.reach, 1);
}

#[test]
fn the_blast_counts_the_callers_reached_by_depth_and_the_files_around_them() {
    let graph = chain();

    let segment = graph
        .analyze_segment("lib/core.c", 5..=9, Limits::new(3))
        .expect("held");
    let blast = &segment.symbols[0].blast;
    let shallow = graph
        .analyze_segment("lib/core.c", 5..=9, Limits::new(1))
        .expect("held");

    assert_eq!(blast.reach, 2);
    assert_eq!(blast.by_depth, [1, 1]);
    assert_eq!(
        blast.affected_files, 3,
        "the callers' files and the includer of lib/core.c"
    );
    assert_eq!(blast.band, ReachBand::Low);
    assert!(!blast.truncated);
    assert_eq!(
        blast.unexpanded, 0,
        "the walk followed top and found no caller"
    );
    assert_eq!(shallow.symbols[0].blast.reach, 1);
    assert_eq!(
        shallow.symbols[0].blast.unexpanded, 1,
        "mid sits at the depth bound"
    );
}

#[test]
fn the_symbol_cap_stops_the_blast_and_counts_what_it_turned_away() {
    let mut target = File::new("lib/target.c").function("target", (1, 2));
    let mut callers = File::new("app/callers.c");
    for index in 0..4 {
        callers = callers
            .function(&format!("caller{index}"), (index * 3 + 1, index * 3 + 2))
            .calls(index, "target", index * 3 + 1);
    }
    target = target.calls(0, "unrelated", 2);
    let graph = graph(vec![target, callers]);

    let segment = graph
        .analyze_segment("lib/target.c", 1..=2, Limits::new(2).with_max_symbols(3))
        .expect("held");
    let blast = &segment.symbols[0].blast;

    assert_eq!(blast.reach, 2, "the cap counts the symbol itself");
    assert!(blast.truncated);
    assert_eq!(blast.turned_away, 2);
    assert_eq!(segment.symbols[0].callers.total, 4);
}

#[test]
fn the_confidence_floor_leaves_out_weak_calls_and_counts_them() {
    let graph = graph(vec![
        File::new("lib/core.c")
            .function("core", (1, 3))
            .function("near", (5, 7))
            .calls(1, "core", 6),
        File::new("far/away.c")
            .function("away", (1, 3))
            .calls(0, "core", 2),
    ]);

    let open = graph
        .analyze_segment("lib/core.c", 1..=3, Limits::new(2))
        .expect("held");
    let strict = graph
        .analyze_segment("lib/core.c", 1..=3, Limits::new(2).with_min_confidence(70))
        .expect("held");

    assert_eq!(open.symbols[0].callers.total, 2);
    assert_eq!(open.symbols[0].callers.below_confidence, 0);
    assert_eq!(strict.symbols[0].callers.total, 1);
    assert_eq!(strict.symbols[0].callers.listed[0].symbol.name, "near");
    assert_eq!(strict.symbols[0].callers.below_confidence, 1);
    assert_eq!(strict.symbols[0].blast.reach, 1);
    assert_eq!(strict.symbols[0].blast.below_confidence, 1);
}

#[test]
fn a_symbol_lists_at_most_fifty_direct_callers_and_counts_them_all() {
    let target = File::new("lib/target.c").function("target", (1, 2));
    let mut callers = File::new("app/callers.c");
    for index in 0..60 {
        callers = callers
            .function(&format!("caller{index:02}"), (index * 3 + 1, index * 3 + 2))
            .calls(index, "target", index * 3 + 1);
    }
    let graph = graph(vec![target, callers]);

    let segment = graph
        .analyze_segment("lib/target.c", 1..=2, Limits::new(1))
        .expect("held");
    let callers = &segment.symbols[0].callers;

    assert_eq!(callers.listed.len(), 50);
    assert_eq!(callers.total, 60);
    assert_eq!(segment.symbols[0].blast.reach, 60);
    assert_eq!(segment.symbols[0].blast.band, ReachBand::High);
}

#[test]
fn a_segment_names_the_files_its_file_includes_and_is_included_by_directly() {
    let graph = chain();

    let header = graph
        .analyze_segment("lib/core.h", 1..=u32::MAX, Limits::new(3))
        .expect("held");
    let source = graph
        .analyze_segment("lib/core.c", 1..=u32::MAX, Limits::new(3))
        .expect("held");

    assert_eq!(header.includes, ["lib/base.h"]);
    assert_eq!(header.included_by, ["app/mid.c"]);
    assert_eq!(source.includes, Vec::<&str>::new());
    assert_eq!(source.included_by, ["tools/user.c"]);
}
