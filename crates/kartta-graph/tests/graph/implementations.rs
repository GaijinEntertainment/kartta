//! The types that name a type as a parent: how a written parent meets the type it names, transitively, and what the
//! depth bound and the limit left out.

use kartta_graph::{Graph, Implementations, Limits, OutsideImplementations, Selector};
use kartta_lang::{Conventions, Extraction, ImportBinding, Symbol, SymbolKind, ValueUses};

use crate::{file, graph_of as graph, including};

fn ty(name: &str, qualified: &str, line: u32, parents: &[&str]) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Class, line, line + 5);
    qualified.clone_into(&mut symbol.qualified_name);
    symbol.parents = parents.iter().map(|&parent| parent.to_owned()).collect();
    symbol
}

fn found(implementations: &Implementations<'_>) -> Vec<(String, u32)> {
    implementations
        .found
        .iter()
        .map(|implementor| {
            (
                format!(
                    "{}:{}",
                    implementor.symbol.file, implementor.symbol.qualified_name
                ),
                implementor.depth,
            )
        })
        .collect()
}

fn cpp() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.scope_separator = Some("::");
    conventions
}

fn hierarchy() -> Graph {
    graph(
        cpp(),
        &[
            file("base.h", vec![ty("Base", "Base", 1, &[])]),
            including(
                file("shape.h", vec![ty("Shape", "Shape", 1, &["Base"])]),
                &["base.h"],
            ),
            including(
                file(
                    "circle.h",
                    vec![
                        ty("Circle", "Circle", 1, &["Shape"]),
                        ty("Ring", "Ring", 9, &["Circle"]),
                    ],
                ),
                &["shape.h"],
            ),
            including(
                file("square.h", vec![ty("Square", "Square", 1, &["Shape"])]),
                &["shape.h"],
            ),
        ],
    )
}

#[test]
fn implementors_come_nearest_first_then_in_path_order() {
    let graph = hierarchy();

    let direct = graph
        .implementations("Shape", 1, 10)
        .expect("Shape is a type");
    let deep = graph
        .implementations("Base", 3, 10)
        .expect("Base is a type");

    assert_eq!(direct.root.name, "Shape");
    assert_eq!(
        found(&direct),
        [
            ("circle.h:Circle".to_owned(), 1),
            ("square.h:Square".to_owned(), 1)
        ]
    );
    assert_eq!(direct.unexpanded, 2);
    assert_eq!(
        found(&deep),
        [
            ("shape.h:Shape".to_owned(), 1),
            ("circle.h:Circle".to_owned(), 2),
            ("square.h:Square".to_owned(), 2),
            ("circle.h:Ring".to_owned(), 3),
        ]
    );
    assert_eq!(deep.unexpanded, 1);
}

#[test]
fn a_type_reached_through_two_parents_is_listed_once() {
    let mut graph = hierarchy();
    graph.insert(&including(
        file("tile.h", vec![ty("Tile", "Tile", 1, &["Circle", "Square"])]),
        &["circle.h", "square.h"],
    ));

    let deep = graph
        .implementations("Shape", 3, 10)
        .expect("Shape is a type");

    assert_eq!(
        found(&deep),
        [
            ("circle.h:Circle".to_owned(), 1),
            ("square.h:Square".to_owned(), 1),
            ("circle.h:Ring".to_owned(), 2),
            ("tile.h:Tile".to_owned(), 2),
        ]
    );
    assert_eq!(deep.total, 4);
}

#[test]
fn the_limit_bounds_the_answer_and_the_total_counts_every_implementor() {
    let graph = hierarchy();

    let limited = graph.implementations("Base", 3, 2).expect("Base is a type");
    let none = graph
        .implementations("Base", 0, 10)
        .expect("Base is a type");

    assert_eq!(
        found(&limited),
        [
            ("shape.h:Shape".to_owned(), 1),
            ("circle.h:Circle".to_owned(), 2)
        ]
    );
    assert_eq!(limited.total, 4);
    assert_eq!((none.total, none.unexpanded), (0, 1));
}

#[test]
fn a_bare_parent_names_the_nearest_type_of_its_name() {
    let graph = graph(
        cpp(),
        &[
            file("a/base.h", vec![ty("Base", "a::Base", 1, &[])]),
            file("b/base.h", vec![ty("Base", "b::Base", 1, &[])]),
            including(
                file("a/child.h", vec![ty("Child", "a::Child", 1, &["Base"])]),
                &["a/base.h"],
            ),
        ],
    );

    let included = graph.implementations("a::Base", 1, 10).expect("a type");
    let elsewhere = graph.implementations("b::Base", 1, 10).expect("a type");

    assert_eq!(found(&included), [("a/child.h:a::Child".to_owned(), 1)]);
    assert_eq!(found(&elsewhere), []);
}

#[test]
fn a_qualified_parent_names_only_a_type_whose_qualified_name_ends_with_it() {
    let graph = graph(
        cpp(),
        &[
            file("a.h", vec![ty("Base", "ns::Base", 1, &[])]),
            file("b.h", vec![ty("Base", "other::Base", 1, &[])]),
            file(
                "c.h",
                vec![
                    ty("Spelled", "Spelled", 1, &["ns::Base"]),
                    ty("Rooted", "Rooted", 9, &["::Base"]),
                ],
            ),
            file("d.h", vec![ty("Base", "Base", 1, &[])]),
        ],
    );

    let ns = graph.implementations("ns::Base", 1, 10).expect("a type");
    let other = graph.implementations("other::Base", 1, 10).expect("a type");
    let global = graph.implementations("Base", 1, 10).expect("a type");

    assert_eq!(found(&ns), [("c.h:Spelled".to_owned(), 1)]);
    assert_eq!(found(&other), []);
    assert_eq!(global.root.file, "d.h");
    assert_eq!(found(&global), [("c.h:Rooted".to_owned(), 1)]);
}

#[test]
fn a_parent_names_the_type_the_scopes_enclosing_its_child_reach_first() {
    let graph = graph(
        cpp(),
        &[
            file(
                "ast/visitor.h",
                vec![
                    ty("Visitor", "sq::Visitor", 1, &[]),
                    ty("Task", "sq::cm::Task", 9, &[]),
                ],
            ),
            file(
                "das/visitor.h",
                vec![
                    ty("Visitor", "das::Visitor", 1, &[]),
                    ty("Task", "nv::cm::Task", 9, &[]),
                ],
            ),
            file("global/visitor.h", vec![ty("Visitor", "Visitor", 1, &[])]),
            file(
                "lint/collector.h",
                vec![
                    ty("Collector", "sq::lint::Collector", 1, &["Visitor"]),
                    ty("Runner", "sq::lint::Runner", 9, &["cm::Task"]),
                    ty("Walker", "Walker", 17, &["Visitor"]),
                ],
            ),
        ],
    );

    let enclosing = graph.implementations("sq::Visitor", 1, 10).expect("a type");
    let unrelated = graph
        .implementations("das::Visitor", 1, 10)
        .expect("a type");
    let global = graph.implementations("Visitor", 1, 10).expect("a type");
    let qualified = graph
        .implementations("sq::cm::Task", 1, 10)
        .expect("a type");
    let namesake = graph
        .implementations("nv::cm::Task", 1, 10)
        .expect("a type");

    assert_eq!(
        found(&enclosing),
        [("lint/collector.h:sq::lint::Collector".to_owned(), 1)]
    );
    assert_eq!(found(&unrelated), []);
    assert_eq!(found(&global), [("lint/collector.h:Walker".to_owned(), 1)]);
    assert_eq!(
        found(&qualified),
        [("lint/collector.h:sq::lint::Runner".to_owned(), 1)]
    );
    assert_eq!(found(&namesake), []);
}

fn traits_beside_namesake_structs() -> Graph {
    let mut alias = ty("true_type", "eastl::true_type", 3, &[]);
    alias.kind = SymbolKind::TypeAlias;
    let doctest = |path: &str| {
        file(
            path,
            vec![
                ty("true_type", "doctest::detail::true_type", 3, &[]),
                ty("is_rvalue", "doctest::detail::is_rvalue", 4, &["true_type"]),
            ],
        )
    };
    graph(
        cpp(),
        &[
            file(
                "eastl/include/EASTL/type_traits.h",
                vec![
                    ty("integral_constant", "eastl::integral_constant", 2, &[]),
                    alias,
                    ty("is_void", "eastl::is_void", 4, &["true_type"]),
                    ty("is_const", "eastl::is_const", 5, &["eastl::true_type"]),
                    ty(
                        "bool_constant",
                        "eastl::bool_constant",
                        6,
                        &["integral_constant"],
                    ),
                ],
            ),
            including(
                file(
                    "game/tag.h",
                    vec![ty("Tag", "Tag", 3, &["eastl::true_type"])],
                ),
                &["eastl/include/EASTL/type_traits.h"],
            ),
            doctest("libs/a/doctest/doctest.h"),
            doctest("libs/b/doctest/doctest.h"),
        ],
    )
}

#[test]
fn a_parent_names_the_alias_its_scopes_reach_and_not_the_aliased_type() {
    let graph = traits_beside_namesake_structs();

    let aliased = graph
        .implementations("eastl::true_type", 1, 10)
        .expect("an alias");
    let aliasing = graph
        .implementations("eastl::integral_constant", 3, 10)
        .expect("a struct");
    let selected = graph.implementations("true_type", 1, 10).expect("a type");

    assert_eq!(
        found(&aliased),
        [
            (
                "eastl/include/EASTL/type_traits.h:eastl::is_void".to_owned(),
                1
            ),
            (
                "eastl/include/EASTL/type_traits.h:eastl::is_const".to_owned(),
                1
            ),
            ("game/tag.h:Tag".to_owned(), 1),
        ]
    );
    assert_eq!(
        found(&aliasing),
        [(
            "eastl/include/EASTL/type_traits.h:eastl::bool_constant".to_owned(),
            1
        )]
    );
    assert_eq!(
        (selected.root.qualified_name, selected.passed_over),
        ("eastl::true_type", 2)
    );
}

#[test]
fn a_struct_that_namesakes_an_alias_keeps_only_its_own_copys_implementors() {
    let graph = traits_beside_namesake_structs();

    let copy = |prefix: &str| {
        let copy = graph
            .implementations(Selector::new("true_type", Some(prefix)), 1, 10)
            .expect("a struct of the copy");
        found(&copy)
    };

    assert_eq!(
        copy("libs/a"),
        [(
            "libs/a/doctest/doctest.h:doctest::detail::is_rvalue".to_owned(),
            1
        )]
    );
    assert_eq!(
        copy("libs/b"),
        [(
            "libs/b/doctest/doctest.h:doctest::detail::is_rvalue".to_owned(),
            1
        )]
    );
}

#[test]
fn a_package_parent_names_a_type_of_its_own_directory_or_of_the_directory_it_imports() {
    let mut conventions = Conventions::default();
    conventions.directory_packages = true;
    let mut user = file(
        "app/user.go",
        vec![
            ty("Local", "app.Local", 1, &["Reader"]),
            ty("Imported", "app.Imported", 9, &["io.Reader"]),
        ],
    );
    user.bindings.push(ImportBinding::new(
        None,
        Some("io".to_owned()),
        "io".to_owned(),
    ));
    let graph = graph(
        conventions,
        &[
            file("app/reader.go", vec![ty("Reader", "app.Reader", 1, &[])]),
            file("io/reader.go", vec![ty("Reader", "io.Reader", 1, &[])]),
            user,
        ],
    );

    let own = graph.implementations("app.Reader", 1, 10).expect("a type");
    let imported = graph.implementations("io.Reader", 1, 10).expect("a type");

    assert_eq!(found(&own), [("app/user.go:app.Local".to_owned(), 1)]);
    assert_eq!(
        found(&imported),
        [("app/user.go:app.Imported".to_owned(), 1)]
    );
}

#[test]
fn an_owner_chain_parent_is_looked_up_from_the_scope_around_the_type_that_names_it() {
    let mut conventions = Conventions::default();
    conventions.owner_chain_separator = Some("::");
    let graph = graph(
        conventions,
        &[
            file("base.rb", vec![ty("Base", "Base", 1, &[])]),
            file("geo/base.rb", vec![ty("Base", "Geo::Base", 1, &[])]),
            file(
                "geo/shape.rb",
                vec![ty("Shape", "Geo::Shape", 1, &["Base"])],
            ),
        ],
    );

    let nested = graph.implementations("Geo::Base", 1, 10).expect("a type");
    let top = graph.implementations("Base", 1, 10).expect("a type");

    assert_eq!(found(&nested), [("geo/shape.rb:Geo::Shape".to_owned(), 1)]);
    assert_eq!(top.root.qualified_name, "Base");
    assert_eq!(found(&top), []);
}

#[test]
fn an_owner_chain_parent_its_lookup_does_not_reach_names_no_type_of_its_last_segment() {
    let mut conventions = Conventions::default();
    conventions.owner_chain_separator = Some(".");
    let graph = graph(
        conventions,
        &[
            file("other/base.py", vec![ty("Base", "other.Base", 1, &[])]),
            file("pkg/m.py", vec![ty("Base", "pkg.m.Base", 1, &[])]),
            file(
                "app/child.py",
                vec![
                    ty("Bare", "app.Bare", 1, &["Base"]),
                    ty("Spelled", "app.Spelled", 9, &["m.Base"]),
                ],
            ),
        ],
    );

    let other = graph.implementations("other.Base", 1, 10).expect("a type");
    let spelled = graph.implementations("pkg.m.Base", 1, 10).expect("a type");

    assert_eq!(found(&other), []);
    assert_eq!(
        found(&spelled),
        [("app/child.py:app.Spelled".to_owned(), 1)]
    );
}

#[test]
fn an_imported_parent_names_a_type_of_its_name_only_where_its_file_declares_or_imports_it() {
    let mut conventions = Conventions::default();
    conventions.owner_chain_separator = Some(".");
    conventions.value_uses = ValueUses::Imported;
    let graph = graph(
        conventions,
        &[
            file(
                "audit/filters.ts",
                vec![
                    ty("MatchValue", "MatchValue", 1, &[]),
                    ty("RepoMatchValue", "RepoMatchValue", 9, &["MatchValue"]),
                ],
            ),
            file(
                "logs/item.tsx",
                vec![ty("MatchValue", "MatchValue", 1, &[])],
            ),
            including(file("logs/index.ts", Vec::new()), &["logs/item.tsx"]),
            including(
                file(
                    "logs/list.tsx",
                    vec![ty("LogMatch", "LogMatch", 1, &["MatchValue"])],
                ),
                &["logs/index.ts"],
            ),
            file(
                "misc/loose.ts",
                vec![ty("Loose", "Loose", 1, &["MatchValue"])],
            ),
        ],
    );
    let implementors = |path| {
        let selector = Selector::new("MatchValue", Some(path));
        found(&graph.implementations(selector, 1, 10).expect("a type"))
    };

    assert_eq!(
        implementors("audit/filters.ts"),
        [("audit/filters.ts:RepoMatchValue".to_owned(), 1)]
    );
    assert_eq!(
        implementors("logs/item.tsx"),
        [("logs/list.tsx:LogMatch".to_owned(), 1)]
    );
}

#[test]
fn only_a_type_that_records_a_parent_is_an_implementor() {
    let mut function = Symbol::new("run", SymbolKind::Function, 9, 12);
    function.parents.push("Base".to_owned());
    let graph = graph(
        cpp(),
        &[file(
            "a.h",
            vec![
                ty("Base", "Base", 1, &[]),
                ty("Derived", "Derived", 3, &["Base"]),
                function,
            ],
        )],
    );

    let implementations = graph.implementations("Base", 1, 10).expect("a type");

    assert_eq!(found(&implementations), [("a.h:Derived".to_owned(), 1)]);
}

#[test]
fn a_name_several_types_share_selects_the_most_implemented_and_counts_the_rest() {
    let graph = graph(
        cpp(),
        &[
            file("a.h", vec![ty("Node", "a::Node", 1, &[])]),
            file("b.h", vec![ty("Node", "b::Node", 1, &[])]),
            file(
                "c.h",
                vec![
                    ty("Leaf", "Leaf", 1, &["b::Node"]),
                    ty("Branch", "Branch", 9, &["b::Node"]),
                ],
            ),
        ],
    );

    let selected = graph.implementations("Node", 1, 10).expect("a type");
    let passed_over = graph
        .implementations(Selector::new("Node", Some("a.h")), 1, 10)
        .expect("a type in a.h");

    assert_eq!(selected.root.qualified_name, "b::Node");
    assert_eq!(selected.passed_over, 1);
    assert_eq!(
        selected
            .candidates
            .iter()
            .map(|candidate| (candidate.symbol.qualified_name, candidate.count))
            .collect::<Vec<_>>(),
        [("a::Node", 0)]
    );
    assert_eq!(selected.total, 2);
    assert_eq!(
        (passed_over.root.qualified_name, passed_over.total),
        ("a::Node", 0)
    );
}

#[test]
fn a_name_several_types_end_with_selects_alike_in_every_query() {
    let graph = graph(
        cpp(),
        &[
            file("a.h", vec![ty("Node", "a::ns::Node", 1, &[])]),
            file("b.h", vec![ty("Node", "b::ns::Node", 1, &[])]),
            file("c.h", vec![ty("Leaf", "Leaf", 1, &["b::ns::Node"])]),
        ],
    );
    let narrowed = Selector::new("ns::Node", Some("b.h"));

    assert!(graph.callers("ns::Node", Limits::new(1)).is_none());
    assert!(graph.implementations("ns::Node", 1, 10).is_none());
    assert_eq!(
        graph
            .callers(narrowed, Limits::new(1))
            .expect("one Node is in b.h")
            .root
            .qualified_name,
        "b::ns::Node",
    );
    assert_eq!(
        graph
            .implementations(narrowed, 1, 10)
            .expect("one Node is in b.h")
            .root
            .qualified_name,
        "b::ns::Node",
    );
}

#[test]
fn the_types_a_name_passes_over_are_listed_the_most_implemented_first() {
    let graph = graph(
        cpp(),
        &[
            file("a.h", vec![ty("Node", "a::Node", 1, &[])]),
            file("b.h", vec![ty("Node", "b::Node", 1, &[])]),
            file("c.h", vec![ty("Node", "c::Node", 1, &[])]),
            file("d.h", vec![ty("Node", "d::Node", 1, &[])]),
            file(
                "e.h",
                vec![
                    ty("Leaf", "Leaf", 1, &["c::Node"]),
                    ty("Branch", "Branch", 5, &["c::Node"]),
                    ty("Root", "Root", 9, &["c::Node"]),
                    ty("Stem", "Stem", 13, &["b::Node"]),
                    ty("Twig", "Twig", 17, &["d::Node"]),
                    ty("Bud", "Bud", 21, &["d::Node"]),
                ],
            ),
        ],
    );

    let selected = graph.implementations("Node", 1, 10).expect("a type");
    let listed: Vec<(&str, usize)> = selected
        .candidates
        .iter()
        .map(|candidate| (candidate.symbol.qualified_name, candidate.count))
        .collect();

    assert_eq!(
        (selected.root.qualified_name, selected.passed_over),
        ("c::Node", 3)
    );
    assert_eq!(listed, [("d::Node", 2), ("b::Node", 1), ("a::Node", 0)]);
}

#[test]
fn the_types_a_name_passes_over_are_listed_up_to_the_candidate_limit() {
    let types: Vec<Extraction> = (0..7)
        .map(|copy| {
            file(
                &format!("n{copy}.h"),
                vec![ty("Node", &format!("n{copy}::Node"), 1, &[])],
            )
        })
        .collect();
    let graph = graph(cpp(), &types);
    let every = Selector::from("Node");

    let whole = graph.implementations(every, 1, 10).expect("a type");
    let cut = graph
        .implementations(every.with_candidate_limit(5), 1, 10)
        .expect("a type");

    assert_eq!((whole.candidates.len(), whole.passed_over), (6, 6));
    assert_eq!((cut.candidates.len(), cut.passed_over), (5, 6));
}

#[test]
fn a_line_selects_one_of_several_types_of_a_name_in_one_file() {
    let graph = graph(
        cpp(),
        &[file(
            "box.h",
            vec![
                ty("Box", "Box", 1, &[]),
                ty("Box", "Box", 9, &[]),
                ty("Crate", "Crate", 17, &["Box"]),
            ],
        )],
    );
    let on = |line| Selector::new("Box", Some("box.h")).with_line(line);

    let by_file = graph.implementations(on(None), 1, 10).expect("a type");
    let by_line = graph.implementations(on(Some(9)), 1, 10).expect("a type");

    assert_eq!((by_file.root.line, by_file.passed_over), (1, 1));
    assert_eq!((by_line.root.line, by_line.passed_over), (9, 0));
    assert!(graph.implementations(on(Some(17)), 1, 10).is_none());
}

#[test]
fn types_named_as_a_parent_equally_often_select_the_first_in_path_order() {
    let graph = graph(
        cpp(),
        &[
            file("a.h", vec![ty("Node", "a::Node", 1, &[])]),
            file("b.h", vec![ty("Node", "b::Node", 1, &[])]),
            file(
                "c.h",
                vec![
                    ty("Leaf", "Leaf", 1, &["a::Node"]),
                    ty("Branch", "Branch", 9, &["b::Node"]),
                ],
            ),
        ],
    );

    let selected = graph.implementations("Node", 1, 10).expect("a type");

    assert_eq!(selected.root.file, "a.h");
    assert_eq!(found(&selected), [("c.h:Leaf".to_owned(), 1)]);
}

#[test]
fn copies_of_one_qualified_name_are_counted_by_the_parents_each_copy_is_nearest_for() {
    let graph = graph(
        cpp(),
        &[
            file("a/node.h", vec![ty("Node", "ns::Node", 1, &[])]),
            file("b/node.h", vec![ty("Node", "ns::Node", 1, &[])]),
            file("c/node.h", vec![ty("Node", "ns::Node", 1, &[])]),
            file("global.h", vec![ty("Node", "Node", 1, &[])]),
            including(
                file(
                    "d/leaf.h",
                    vec![
                        ty("Leaf", "ns::Leaf", 1, &["Node"]),
                        ty("Twig", "ns::Twig", 9, &["Node"]),
                    ],
                ),
                &["b/node.h"],
            ),
            file("e/root.h", vec![ty("Root", "Root", 1, &["Node"])]),
        ],
    );
    let counted = |implementations: &Implementations<'_>| {
        implementations
            .candidates
            .iter()
            .map(|candidate| (candidate.symbol.file.to_owned(), candidate.count))
            .collect::<Vec<_>>()
    };

    let qualified = graph.implementations("ns::Node", 1, 10).expect("a type");
    let bare = graph.implementations("Node", 1, 10).expect("a type");

    assert_eq!(
        (qualified.root.file, qualified.passed_over),
        ("b/node.h", 2)
    );
    assert_eq!(
        found(&qualified),
        [
            ("d/leaf.h:ns::Leaf".to_owned(), 1),
            ("d/leaf.h:ns::Twig".to_owned(), 1)
        ]
    );
    assert_eq!(
        counted(&qualified),
        [("a/node.h".to_owned(), 0), ("c/node.h".to_owned(), 0)]
    );
    assert_eq!((bare.root.file, bare.passed_over), ("global.h", 3));
    assert_eq!(found(&bare), [("e/root.h:Root".to_owned(), 1)]);
    assert_eq!(
        counted(&bare),
        [
            ("b/node.h".to_owned(), 2),
            ("a/node.h".to_owned(), 0),
            ("c/node.h".to_owned(), 0)
        ]
    );
}

#[test]
fn a_type_that_names_one_parent_twice_counts_once_toward_selecting_it() {
    let graph = graph(
        cpp(),
        &[
            file("a.h", vec![ty("Node", "a::Node", 1, &[])]),
            file("b.h", vec![ty("Node", "b::Node", 1, &[])]),
            file(
                "c.h",
                vec![
                    ty("Leaf", "Leaf", 1, &["a::Node", "a::Node"]),
                    ty("Branch", "Branch", 9, &["b::Node"]),
                    ty("Twig", "Twig", 17, &["b::Node"]),
                ],
            ),
        ],
    );

    let selected = graph.implementations("Node", 1, 10).expect("a type");
    let twice = graph.implementations("a::Node", 1, 10).expect("a type");

    assert_eq!(selected.root.file, "b.h");
    assert_eq!(found(&twice), [("c.h:Leaf".to_owned(), 1)]);
}

#[test]
fn a_name_that_selects_no_type_answers_none() {
    let mut function = Symbol::new("run", SymbolKind::Function, 1, 2);
    function.parents.push("Base".to_owned());
    let graph = graph(cpp(), &[file("a.h", vec![function])]);

    assert_eq!(graph.implementations("run", 1, 10), None);
    assert_eq!(graph.implementations("absent", 1, 10), None);
}

fn outside(found: &OutsideImplementations<'_>) -> Vec<String> {
    (found.found.iter())
        .map(|definition| format!("{}:{}", definition.file, definition.qualified_name))
        .collect()
}

fn rust() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.owner_chain_separator = Some("::");
    conventions
}

#[test]
fn a_parent_the_graph_holds_no_type_of_lists_the_types_whose_parents_end_with_it() {
    let mut function = Symbol::new("show", SymbolKind::Function, 30, 31);
    function.parents.push("Display".to_owned());
    let graph = graph(
        rust(),
        &[file(
            "src/show.rs",
            vec![
                ty("Bare", "Bare", 1, &["Display"]),
                ty("Module", "Module", 9, &["fmt::Display"]),
                ty("Path", "Path", 17, &["std::fmt::Display"]),
                ty("Debugged", "Debugged", 25, &["fmt::Debug"]),
                function,
            ],
        )],
    );

    let bare = graph.implementations_outside("Display", 10);
    let module = graph.implementations_outside("fmt.Display", 10);
    let path = graph.implementations_outside("std::fmt::Display", 10);
    let other_module = graph.implementations_outside("io::Display", 10);
    let limited = graph.implementations_outside("Display", 2);

    let every = ["src/show.rs:Bare", "src/show.rs:Module", "src/show.rs:Path"];
    assert_eq!(outside(&bare), every);
    assert_eq!(outside(&module), every[1..]);
    assert_eq!(outside(&path), every[2..]);
    assert_eq!(outside(&other_module), [] as [&str; 0]);
    assert_eq!(outside(&limited), every[..2]);
    assert_eq!(limited.total, 3);
    assert_eq!(bare.namesakes, 0);
    assert_eq!(graph.implementations("Display", 1, 10), None);
}

#[test]
fn a_parent_that_names_a_type_the_graph_holds_is_not_outside_it() {
    let graph = graph(
        rust(),
        &[
            file(
                "src/error.rs",
                vec![
                    ty("Error", "Error", 1, &[]),
                    ty("Own", "Own", 9, &["Error"]),
                    ty("Std", "Std", 17, &["std::error::Error"]),
                ],
            ),
            file(
                "src/own.rs",
                vec![ty("Error", "own::Error", 1, &["std::error::Error"])],
            ),
        ],
    );

    let bare = graph.implementations_outside("Error", 10);
    let path = graph.implementations_outside("std::error::Error", 10);

    let outside_only = ["src/error.rs:Std", "src/own.rs:own::Error"];
    assert_eq!(outside(&bare), outside_only);
    assert_eq!(outside(&path), outside_only);
    assert_eq!(bare.namesakes, 1);
    assert_eq!(graph.implementations_outside("Shape", 10).total, 0);
    assert_eq!(hierarchy().implementations_outside("Shape", 10).total, 0);
}

#[test]
fn a_type_whose_parent_shares_its_name_names_no_type_of_itself() {
    let graph = graph(
        cpp(),
        &[file("app/foo.h", vec![ty("Foo", "app::Foo", 1, &["Foo"])])],
    );

    assert_eq!(
        outside(&graph.implementations_outside("Foo", 10)),
        ["app/foo.h:app::Foo"]
    );
}

#[test]
fn a_parent_that_names_a_defined_type_of_its_package_is_not_outside_the_graph() {
    let mut conventions = Conventions::default();
    conventions.directory_packages = true;
    let mut header = Symbol::new("header", SymbolKind::DefinedType, 1, 1);
    "app.header".clone_into(&mut header.qualified_name);
    let graph = graph(
        conventions,
        &[
            file("app/header.go", vec![header]),
            file(
                "app/files.go",
                vec![
                    ty("Files", "app.Files", 1, &["header"]),
                    ty("Locked", "app.Locked", 9, &["sync.Mutex"]),
                ],
            ),
        ],
    );

    assert_eq!(graph.implementations_outside("header", 10).total, 0);
    assert_eq!(
        outside(&graph.implementations_outside("sync.Mutex", 10)),
        ["app/files.go:app.Locked"]
    );
}

#[test]
fn a_replaced_or_removed_file_stops_naming_its_parents() {
    let mut graph = hierarchy();

    graph.insert(&file("square.h", vec![ty("Square", "Square", 1, &[])]));
    graph.remove("circle.h");
    let implementations = graph.implementations("Shape", 1, 10).expect("a type");

    assert_eq!(found(&implementations), []);
    assert_eq!(implementations.total, 0);
}
