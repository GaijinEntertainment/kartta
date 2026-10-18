//! What a change to the symbols of a segment or of a set of regions reaches: their direct calls, the callers a walk
//! over them reaches, where those walks meet, and the files around them.

use std::ops::RangeInclusive;

use kartta_graph::{Change, DirCount, DirectCalls, Graph, Limits, ReachBand, Region, Segment};
use kartta_lang::{
    Callable, CallableRole, Extraction, IncludeRef, Symbol, SymbolFlags, SymbolId, SymbolKind,
};

struct File {
    extraction: Extraction,
}

impl File {
    fn new(path: &str) -> Self {
        Self {
            extraction: crate::file(path, Vec::new()),
        }
    }

    fn function(mut self, name: &str, lines: (u32, u32)) -> Self {
        self.extraction
            .symbols
            .push(Symbol::new(name, SymbolKind::Function, lines.0, lines.1));
        self
    }

    fn calls(mut self, from: u32, name: &str, line: u32) -> Self {
        self.extraction.calls.push(crate::call(from, name, line));
        self
    }

    fn flagged_as_test(mut self) -> Self {
        if let Some(symbol) = self.extraction.symbols.last_mut() {
            symbol.flags = SymbolFlags::default().with_test(true);
        }
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
    let extractions: Vec<Extraction> = files.into_iter().map(|file| file.extraction).collect();
    crate::graph(&extractions)
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

fn drawing() -> Graph {
    graph(vec![
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
    ])
}

fn fan_in(callers: &[(&str, u32)]) -> Graph {
    let mut files = vec![File::new("lib/target.c").function("target", (1, 2))];
    for &(path, count) in callers {
        let mut file = File::new(path);
        for index in 0..count {
            file = file
                .function(&format!("caller{index:02}"), (index * 3 + 1, index * 3 + 2))
                .calls(index, "target", index * 3 + 1);
        }
        files.push(file);
    }
    graph(files)
}

#[test]
fn a_declaration_answers_with_the_calls_of_the_definition_it_links_to() {
    let graph = drawing();

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

fn seeds<'g>(change: &Change<'g>) -> Vec<&'g str> {
    change.seeds.iter().map(|seed| seed.symbol.name).collect()
}

fn hotspots<'g>(change: &Change<'g>) -> Vec<(&'g str, usize, u32, bool)> {
    change
        .hotspots
        .iter()
        .map(|hotspot| {
            (
                hotspot.symbol.name,
                hotspot.hit_count,
                hotspot.min_depth,
                hotspot.also_changed,
            )
        })
        .collect()
}

#[test]
fn a_change_counts_each_symbol_it_touches_once_and_where_their_walks_meet() {
    let graph = chain();
    let regions = [
        Region::new("lib/core.c", 1..=3),
        Region::new("lib/core.c", 5..=9),
        Region::new("lib/core.c", 6..=6),
    ];

    let change = graph.analyze_change(&regions, Limits::new(3));

    assert_eq!(change.changed_symbols, 2);
    assert_eq!(change.changed_files, 1);
    assert_eq!(seeds(&change), ["core", "helper"]);
    assert_eq!(change.seeds[0].reach, 3);
    assert_eq!(change.seeds[1].reach, 2);
    assert_eq!(change.transitive_sum, 5);
    assert_eq!(change.footprint, 3, "helper, mid and top");
    assert_eq!(change.footprint_files, 3);
    assert_eq!(
        hotspots(&change),
        [("mid", 2, 1, false), ("top", 2, 2, false)]
    );
    assert_eq!(change.total_hotspots, 2);
}

#[test]
fn a_hotspot_that_is_itself_changed_is_marked_so() {
    let graph = chain();
    let regions = [
        Region::new("lib/core.c", 1..=u32::MAX),
        Region::new("app/mid.c", 1..=4),
    ];

    let change = graph.analyze_change(&regions, Limits::new(3));

    assert_eq!(change.changed_symbols, 3);
    assert_eq!(change.changed_files, 2);
    assert_eq!(
        hotspots(&change),
        [("top", 3, 1, false), ("mid", 2, 1, true)]
    );
}

#[test]
fn regions_the_graph_does_not_hold_or_that_overlap_no_symbol_are_counted() {
    let graph = chain();
    let regions = [
        Region::new("lib/absent.c", 1..=9),
        Region::new("lib/core.c", 4..=4),
        Region::new("lib/core.c", RangeInclusive::new(9, 5)),
        Region::new("lib/base.h", 1..=u32::MAX),
    ];

    let change = graph.analyze_change(&regions, Limits::new(3));

    assert_eq!(change.unheld_regions, 1);
    assert_eq!(change.regions_outside_symbols, 3);
    assert_eq!(change.changed_symbols, 0);
    assert_eq!(change.changed_files, 0);
}

#[test]
fn a_seed_lists_up_to_thirty_direct_callers_and_counts_more_by_directory() {
    let listed = fan_in(&[("app/a.c", 30)]);
    let counted = fan_in(&[("app/a.c", 20), ("tools/t.c", 10), ("main.c", 1)]);
    let target = [Region::new("lib/target.c", 1..=2)];

    let few = listed.analyze_change(&target, Limits::new(1));
    let many = counted.analyze_change(&target, Limits::new(1));
    let dirs: Vec<(&str, usize)> = many.seeds[0]
        .consumer_dirs
        .iter()
        .map(|dir| (dir.dir, dir.count))
        .collect();

    assert_eq!(few.seeds[0].caller_count, 30);
    assert_eq!(few.seeds[0].consumers.len(), 30);
    assert_eq!(few.seeds[0].consumer_dirs, Vec::<DirCount<'_>>::new());
    assert_eq!(many.seeds[0].caller_count, 31);
    assert_eq!(many.seeds[0].consumers, Vec::new());
    assert_eq!(dirs, [("app", 20), ("tools", 10), (".", 1)]);
}

#[test]
fn a_seed_lists_its_test_callers_after_the_rest() {
    let graph = graph(vec![
        File::new("lib/target.c").function("target", (1, 2)),
        File::new("a/check.c")
            .function("check", (1, 2))
            .flagged_as_test()
            .calls(0, "target", 1),
        File::new("b/use.c")
            .function("use", (1, 2))
            .calls(0, "target", 1),
    ]);

    let change = graph.analyze_change(&[Region::new("lib/target.c", 1..=2)], Limits::new(1));
    let consumers: Vec<&str> = change.seeds[0]
        .consumers
        .iter()
        .map(|consumer| consumer.symbol.name)
        .collect();

    assert_eq!(consumers, ["use", "check"]);
}

#[test]
fn the_confidence_floor_leaves_weak_callers_out_of_a_seed_and_counts_them() {
    let graph = graph(vec![
        File::new("lib/core.c")
            .function("core", (1, 3))
            .function("near", (5, 7))
            .calls(1, "core", 6),
        File::new("far/away.c")
            .function("away", (1, 3))
            .calls(0, "core", 2),
    ]);

    let change = graph.analyze_change(
        &[Region::new("lib/core.c", 1..=3)],
        Limits::new(2).with_min_confidence(70),
    );
    let seed = &change.seeds[0];

    assert_eq!(seed.caller_count, 1);
    assert_eq!(seed.consumers[0].symbol.name, "near");
    assert_eq!(seed.reach, 1);
    assert_eq!(seed.below_confidence, 1);
}

#[test]
fn a_declaration_and_its_definition_are_one_changed_symbol_answered_first_in_path_order() {
    let graph = drawing();
    let regions = [
        Region::new("lib/draw.h", 1..=1),
        Region::new("lib/draw.cpp", 3..=6),
    ];

    let change = graph.analyze_change(&regions, Limits::new(3));

    assert_eq!(change.changed_symbols, 1);
    assert_eq!(change.changed_files, 2);
    assert_eq!(change.seeds[0].symbol.file, "lib/draw.cpp");
    assert_eq!(change.seeds[0].caller_count, 1);
}

#[test]
fn a_seed_the_symbol_cap_stops_counts_what_it_turned_away() {
    let graph = fan_in(&[("app/callers.c", 4)]);

    let change = graph.analyze_change(
        &[Region::new("lib/target.c", 1..=2)],
        Limits::new(2).with_max_symbols(3),
    );

    assert_eq!(change.truncated_seeds, 1);
    assert!(change.seeds[0].truncated);
    assert_eq!(change.seeds[0].turned_away, 2);
    assert_eq!(change.seeds[0].caller_count, 4);
}

#[test]
fn a_change_lists_at_most_three_hundred_seeds_test_code_last_and_counts_them_all() {
    let mut many = File::new("lib/many.c")
        .function("f000", (1, 1))
        .flagged_as_test();
    for index in 1..301 {
        many = many.function(&format!("f{index:03}"), (index + 1, index + 1));
    }
    let caller = File::new("app/caller.c")
        .function("caller", (1, 2))
        .calls(0, "f000", 1);
    let graph = graph(vec![many, caller]);

    let change = graph.analyze_change(&[Region::new("lib/many.c", 1..=u32::MAX)], Limits::new(1));

    assert_eq!(change.changed_symbols, 301);
    assert_eq!(change.seeds.len(), 300);
    assert_eq!(change.seeds[0].symbol.name, "f001");
    assert!(seeds(&change).iter().all(|&name| name != "f000"));
}

#[test]
fn a_change_of_many_symbols_keeps_the_widest_reaching_seeds_wherever_they_sit() {
    let mut many = File::new("lib/many.c");
    for index in 0..700 {
        many = many.function(&format!("f{index:03}"), (index + 1, index + 1));
    }
    let caller = File::new("app/caller.c")
        .function("caller", (1, 4))
        .calls(0, "f650", 2)
        .calls(0, "f050", 3)
        .calls(0, "f350", 4);
    let graph = graph(vec![many, caller]);

    let change = graph.analyze_change(&[Region::new("lib/many.c", 1..=u32::MAX)], Limits::new(1));

    assert_eq!(change.changed_symbols, 700);
    assert_eq!(change.seeds.len(), 300);
    assert_eq!(seeds(&change)[..4], ["f050", "f350", "f650", "f000"]);
    assert_eq!(change.transitive_sum, 3);
}

#[test]
fn merged_changes_sum_their_counts_and_rank_their_seeds_together() {
    let chain = chain();
    let fan = fan_in(&[("app/a.c", 5)]);

    let merged = Change::merge([
        chain.analyze_change(&[Region::new("lib/core.c", 5..=9)], Limits::new(3)),
        fan.analyze_change(
            &[
                Region::new("lib/target.c", 1..=2),
                Region::new("lib/absent.c", 1..=1),
            ],
            Limits::new(3),
        ),
    ]);

    assert_eq!(merged.changed_symbols, 2);
    assert_eq!(merged.unheld_regions, 1);
    assert_eq!(merged.transitive_sum, 7);
    assert_eq!(seeds(&merged), ["target", "helper"]);
}

#[test]
fn merged_changes_rank_their_hotspots_together_and_sum_their_totals() {
    let first = graph(vec![
        File::new("lib/a.c")
            .function("s1", (1, 1))
            .function("s2", (2, 2)),
        File::new("app/x.c")
            .function("x", (1, 3))
            .calls(0, "s1", 2)
            .calls(0, "s2", 3),
    ]);
    let second = graph(vec![
        File::new("lib/b.c")
            .function("t1", (1, 1))
            .function("t2", (2, 2))
            .function("t3", (3, 3)),
        File::new("app/y.c")
            .function("y", (1, 4))
            .calls(0, "t1", 2)
            .calls(0, "t2", 3)
            .calls(0, "t3", 4)
            .function("z", (5, 7))
            .calls(1, "t1", 6)
            .calls(1, "t2", 7),
    ]);

    let merged = Change::merge([
        first.analyze_change(&[Region::new("lib/a.c", 1..=2)], Limits::new(3)),
        second.analyze_change(&[Region::new("lib/b.c", 1..=3)], Limits::new(3)),
    ]);

    assert_eq!(
        hotspots(&merged),
        [("y", 3, 1, false), ("x", 2, 1, false), ("z", 2, 1, false)]
    );
    assert_eq!(merged.total_hotspots, 3);
}
