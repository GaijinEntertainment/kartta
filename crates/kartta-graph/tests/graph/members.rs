//! Uses on a script language's own receiver, on a receiver that names an owner, and of a value a file imports; and the
//! tiers a C++ data site binds through: the own class, a base the lookup certainly finds, the type a declaration
//! spells, the file, its includes and the files those reach, for fields and for namespace values.

use kartta_graph::{Graph, SiteQuery};
use kartta_lang::{
    Access, CallsOnValues, Conventions, Extraction, IncludeReach, InheritedMembers, OwnMembers,
    ReExport, Receiver, RefSite, Symbol, SymbolId, SymbolKind, UntypedMembers, ValueUses,
};

use crate::{graph_of as graph, including};

fn script() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::Unreached;
    conventions.owner_chain_separator = Some(".");
    conventions.value_uses = ValueUses::Imported;
    conventions
}

fn class(name: &str, lines: [u32; 2], parents: &[&str]) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Class, lines[0], lines[1]);
    symbol.parents = parents.iter().map(|&parent| parent.to_owned()).collect();
    symbol
}

fn member(owner: &str, name: &str, kind: SymbolKind, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, kind, line, line);
    symbol.qualified_name = format!("{owner}.{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

fn free(name: &str, kind: SymbolKind, lines: [u32; 2]) -> Symbol {
    Symbol::new(name, kind, lines[0], lines[1])
}

fn using(site_of: u32, name: &str, receiver: Receiver, line: u32) -> RefSite {
    RefSite {
        site_of: SymbolId(site_of),
        name: name.to_owned(),
        receiver,
        access: Access::Read,
        line,
    }
}

fn file(path: &str, symbols: Vec<Symbol>, includes: &[&str]) -> Extraction {
    including(crate::file(path, symbols), includes)
}

fn sites_of(graph: &Graph, name: &str) -> (Vec<(String, u32, u8)>, usize) {
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    let sites = references
        .sites
        .iter()
        .map(|site| {
            (
                site.site_of.qualified_name.to_owned(),
                site.line,
                site.confidence,
            )
        })
        .collect();
    (sites, references.ambiguous)
}

fn base_module() -> Extraction {
    file(
        "lib/base.py",
        vec![
            class("Base", [1, 9], &[]),
            member("Base", "size", SymbolKind::Field, 3),
        ],
        &[],
    )
}

#[test]
fn an_own_receiver_binds_the_class_s_member_then_one_a_base_found_without_a_guess_declares() {
    let mut pool = file(
        "app/pool.py",
        vec![
            class("Pool", [1, 20], &["Base"]),
            member("Pool", "free", SymbolKind::Field, 2),
            member("Pool", "grow", SymbolKind::Method, 4),
            free("reset", SymbolKind::Function, [6, 8]),
            class("Widget", [30, 40], &["Outside", "Base"]),
            member("Widget", "draw", SymbolKind::Method, 31),
        ],
        &["lib/base.py"],
    );
    pool.local_symbols = vec![SymbolId(3)];
    pool.references = vec![
        using(2, "free", Receiver::Own, 5),
        using(3, "free", Receiver::Own, 7),
        using(2, "size", Receiver::Own, 9),
        using(5, "size", Receiver::Own, 32),
    ];

    let graph = graph(script(), &[base_module(), pool]);

    assert_eq!(
        sites_of(&graph, "Pool.free"),
        (
            vec![("Pool.grow".to_owned(), 5, 85), ("reset".to_owned(), 7, 85)],
            0
        ),
        "a function local to a method reaches the method's class"
    );
    assert_eq!(
        sites_of(&graph, "Base.size"),
        (vec![("Pool.grow".to_owned(), 9, 80)], 1),
        "a base outside the graph written before the one that declares the member may hold it"
    );
}

#[test]
fn an_own_receiver_binds_a_member_any_number_of_bases_up_and_a_cycle_of_bases_ends() {
    let mut symbols = vec![
        class("Level0", [1, 9], &[]),
        member("Level0", "depth", SymbolKind::Field, 2),
    ];
    for level in 1..=10 {
        let start = level * 10;
        let base = format!("Level{}", level - 1);
        symbols.push(class(
            &format!("Level{level}"),
            [start, start + 9],
            &[&base],
        ));
    }
    symbols.push(member("Level10", "read", SymbolKind::Method, 101));
    symbols.push(class("Ring", [200, 209], &["Loop"]));
    symbols.push(member("Ring", "spin", SymbolKind::Method, 201));
    symbols.push(class("Loop", [210, 219], &["Ring"]));
    let mut chain = file("app/chain.py", symbols, &[]);
    chain.references = vec![
        using(12, "depth", Receiver::Own, 102),
        using(14, "depth", Receiver::Own, 202),
    ];
    let graph = graph(script(), &[chain]);

    assert_eq!(
        sites_of(&graph, "Level0.depth"),
        (vec![("Level10.read".to_owned(), 102, 80)], 0),
        "a cycle of bases that declares no member of the name binds nothing and is no guess"
    );
}

#[test]
fn a_receiver_that_names_an_owner_binds_its_member_where_the_file_or_one_include_declares_it() {
    let mut shape = free("Shape", SymbolKind::Interface, [6, 9]);
    shape.qualified_name = "Outer.Shape".to_owned();
    let props = file(
        "ui/props.ts",
        vec![
            free("Props", SymbolKind::Interface, [1, 4]),
            member("Props", "title", SymbolKind::Field, 2),
            shape,
            member("Outer.Shape", "area", SymbolKind::Field, 7),
        ],
        &[],
    );
    let mut card = file(
        "ui/card.ts",
        vec![free("Card", SymbolKind::Function, [1, 9])],
        &["ui/props.ts"],
    );
    card.references = vec![
        using(0, "title", Receiver::Path("Props".to_owned()), 2),
        using(0, "title", Receiver::Path("HTMLElement".to_owned()), 3),
        using(0, "title", Receiver::Path(String::new()), 4),
        using(0, "area", Receiver::Path("Shape".to_owned()), 5),
    ];

    let graph = graph(script(), &[props, card]);

    assert_eq!(
        sites_of(&graph, "Props.title"),
        (vec![("Card".to_owned(), 2, 85)], 1),
        "a type outside the graph binds nothing and is no candidate; a value of no known type may be one"
    );
    assert_eq!(
        sites_of(&graph, "Outer.Shape.area").0,
        [("Card".to_owned(), 5, 85)],
        "a path that spells the end of an owner's chain names that owner"
    );
}

#[test]
fn a_member_of_a_value_of_no_known_type_binds_the_file_s_one_field_only_where_the_language_says_so()
{
    let mut card = file(
        "ui/card.ts",
        vec![
            free("Props", SymbolKind::Interface, [1, 3]),
            member("Props", "title", SymbolKind::Field, 2),
            free("Card", SymbolKind::Function, [5, 9]),
        ],
        &[],
    );
    card.references = vec![using(2, "title", Receiver::Path(String::new()), 6)];
    let mut unbound = script();
    unbound.untyped_members = UntypedMembers::Unbound;

    assert_eq!(
        sites_of(&graph(script(), std::slice::from_ref(&card)), "Props.title"),
        (vec![("Card".to_owned(), 6, 80)], 0)
    );
    assert_eq!(
        sites_of(&graph(unbound, &[card]), "Props.title"),
        (Vec::new(), 1)
    );
}

#[test]
fn a_receiver_that_ends_an_included_module_s_name_binds_that_module_s_own_definition() {
    let mut limit = free("LIMIT", SymbolKind::Variable, [1, 1]);
    limit.qualified_name = "lib.consts.LIMIT".to_owned();
    let mut size = member("Box", "LIMIT", SymbolKind::Field, 4);
    size.qualified_name = "lib.consts.Box.LIMIT".to_owned();
    let consts = file("lib/consts.py", vec![limit, size], &[]);
    let mut other = free("LIMIT", SymbolKind::Variable, [1, 1]);
    other.qualified_name = "app.other.LIMIT".to_owned();
    let other = file("app/other.py", vec![other], &[]);
    let mut main = file(
        "app/main.py",
        vec![free("run", SymbolKind::Function, [1, 9])],
        &["lib/consts.py", "app/other.py"],
    );
    main.references = vec![
        using(0, "LIMIT", Receiver::Path("consts".to_owned()), 2),
        using(0, "LIMIT", Receiver::Path("lib.consts".to_owned()), 3),
        using(0, "LIMIT", Receiver::Path("c".to_owned()), 4),
    ];
    let graph = graph(script(), &[consts, other, main]);

    assert_eq!(
        sites_of(&graph, "lib.consts.LIMIT"),
        (
            vec![("run".to_owned(), 2, 85), ("run".to_owned(), 3, 85)],
            0
        ),
        "a module named by an alias it does not end is no owner"
    );
}

#[test]
fn a_bare_use_binds_the_one_included_module_that_declares_the_name_or_a_re_export_of_it() {
    let constants = file(
        "src/constants.ts",
        vec![
            free("LIMIT", SymbolKind::Constant, [1, 1]),
            free("NAME", SymbolKind::Constant, [2, 2]),
        ],
        &[],
    );
    let other = file(
        "src/other.ts",
        vec![free("NAME", SymbolKind::Constant, [1, 1])],
        &[],
    );
    let re_export = |local: &str, source: &str, from: &str| ReExport {
        local_name: local.to_owned(),
        source_name: source.to_owned(),
        source_path: from.to_owned(),
        source_file: Some(from.to_owned()),
    };
    let mut barrel = file("src/index.ts", Vec::new(), &["src/constants.ts"]);
    barrel.re_exports = vec![
        re_export("MAX", "LIMIT", "src/constants.ts"),
        re_export("NAME", "NAME", "src/constants.ts"),
    ];
    let mut again = file("src/again.ts", Vec::new(), &["src/other.ts"]);
    again.re_exports = vec![re_export("NAME", "NAME", "src/other.ts")];
    let mut both = file(
        "app/both.ts",
        vec![free("show", SymbolKind::Function, [1, 9])],
        &["src/index.ts", "src/again.ts"],
    );
    both.references = vec![using(0, "NAME", Receiver::Bare, 2)];
    let mut user = file(
        "src/user.ts",
        vec![free("run", SymbolKind::Function, [1, 9])],
        &["src/constants.ts", "src/other.ts"],
    );
    user.references = vec![
        using(0, "LIMIT", Receiver::Bare, 2),
        using(0, "NAME", Receiver::Bare, 3),
    ];
    let mut far = file(
        "app/far.ts",
        vec![free("go", SymbolKind::Function, [1, 9])],
        &["src/index.ts"],
    );
    far.references = vec![using(0, "MAX", Receiver::Bare, 2)];
    let files = [constants, other, barrel, again, both, user, far];

    assert_eq!(
        sites_of(&graph(script(), &files), "LIMIT").0,
        [("go".to_owned(), 2, 65), ("run".to_owned(), 2, 70)],
        "a re-export binds the definition it renames"
    );
    assert!(
        sites_of(&graph(script(), &files), "NAME").0.is_empty(),
        "two included modules or two barrels that offer the name bind neither"
    );
    let mut declared = script();
    declared.value_uses = ValueUses::Declared;
    assert!(
        sites_of(&graph(declared, &files), "LIMIT").0.is_empty(),
        "a language whose uses do not reach includes binds none through them"
    );
}

#[test]
fn an_imported_value_binds_an_export_or_a_re_export_before_an_unexported_namesake() {
    let mut limit = free("LIMIT", SymbolKind::Constant, [1, 1]);
    limit.exported = true;
    let constants = file("src/constants.ts", vec![limit], &[]);
    let private = file(
        "src/private.ts",
        vec![free("LIMIT", SymbolKind::Constant, [1, 1])],
        &[],
    );
    let mut barrel = file("src/index.ts", Vec::new(), &["src/constants.ts"]);
    barrel.re_exports = vec![ReExport {
        local_name: "LIMIT".to_owned(),
        source_name: "LIMIT".to_owned(),
        source_path: "src/constants.ts".to_owned(),
        source_file: Some("src/constants.ts".to_owned()),
    }];
    let mut direct = file(
        "app/direct.ts",
        vec![free("go", SymbolKind::Function, [1, 9])],
        &["src/constants.ts", "src/private.ts"],
    );
    direct.references = vec![using(0, "LIMIT", Receiver::Bare, 3)];
    let mut renamed = file(
        "app/renamed.ts",
        vec![free("run", SymbolKind::Function, [1, 9])],
        &["src/index.ts", "src/private.ts"],
    );
    renamed.references = vec![using(0, "LIMIT", Receiver::Bare, 2)];
    let graph = graph(script(), &[constants, private, barrel, direct, renamed]);

    assert_eq!(
        sites_of(&graph, "LIMIT"),
        (vec![("go".to_owned(), 3, 70), ("run".to_owned(), 2, 65)], 0)
    );
}

fn cpp() -> Conventions {
    let mut conventions = Conventions::default();
    conventions.own_members = OwnMembers::First;
    conventions.inherited_members = InheritedMembers::Found;
    conventions.values_are_call_targets = false;
    conventions.scope_separator = Some("::");
    conventions.include_reach = IncludeReach::Transitive;
    conventions.calls_on_values = CallsOnValues::NoNearestGuess;
    conventions.value_uses = ValueUses::Included;
    conventions.untyped_members = UntypedMembers::IncludedField;
    conventions
}

fn cpp_member(owner: &str, name: &str, kind: SymbolKind, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, kind, line, line);
    symbol.qualified_name = format!("{owner}::{name}");
    symbol.owner = Some(owner.to_owned());
    symbol
}

fn field(owner: &str, name: &str, line: u32) -> Symbol {
    cpp_member(owner, name, SymbolKind::Field, line)
}

fn value(name: &str, line: u32) -> Symbol {
    Symbol::new(name, SymbolKind::Variable, line, line)
}

fn with_uses(
    mut extraction: Extraction,
    site_of: u32,
    sites: &[(&str, Receiver, u32)],
) -> Extraction {
    extraction.references = sites
        .iter()
        .map(|(name, receiver, line)| using(site_of, name, receiver.clone(), *line))
        .collect();
    extraction
}

fn path(owner: &str) -> Receiver {
    Receiver::Path(owner.to_owned())
}

fn lines_of(graph: &Graph, name: &str) -> Vec<(u32, u8)> {
    lines_in(graph, name).1
}

fn lines_in<'g>(graph: &'g Graph, name: &str) -> (&'g str, Vec<(u32, u8)>) {
    let references = graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol");
    let sites = references
        .sites
        .iter()
        .map(|site| (site.line, site.confidence))
        .collect();
    (references.symbol.file, sites)
}

fn ambiguous(graph: &Graph, name: &str) -> usize {
    graph
        .references(name, SiteQuery::new(usize::MAX))
        .expect("the name selects a symbol")
        .ambiguous
}

#[test]
fn a_member_of_the_own_receiver_binds_the_own_class_s_field_the_file_sees() {
    let widget = file(
        "inc/widget.h",
        vec![class("Widget", [1, 5], &[]), field("Widget", "width", 2)],
        &[],
    );
    let copy = file(
        "vendor/widget.h",
        vec![class("Widget", [1, 5], &[]), field("Widget", "width", 2)],
        &[],
    );
    let user = with_uses(
        file(
            "src/widget.cpp",
            vec![
                cpp_member("Widget", "draw", SymbolKind::Method, 1),
                field("Other", "width", 9),
            ],
            &["inc/widget.h"],
        ),
        0,
        &[("width", Receiver::Own, 2), ("width", Receiver::Bare, 3)],
    );

    let graph = graph(cpp(), &[copy, widget, user]);

    assert_eq!(
        lines_in(&graph, "Widget::width"),
        ("inc/widget.h", vec![(2, 85), (3, 85)])
    );
    assert_eq!(lines_of(&graph, "Other::width"), []);
}

#[test]
fn a_class_the_file_neither_includes_nor_reaches_binds_none_of_its_fields() {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![cpp_member("Widget", "draw", SymbolKind::Method, 1)],
            &[],
        ),
        0,
        &[("width", Receiver::Bare, 2), ("limit", path("Config"), 3)],
    );
    let vendor = file(
        "vendor/widget.h",
        vec![
            class("Widget", [1, 5], &[]),
            field("Widget", "width", 2),
            class("Config", [6, 8], &[]),
            field("Config", "limit", 7),
        ],
        &[],
    );

    let graph = graph(cpp(), &[user, vendor]);

    assert_eq!(lines_of(&graph, "Widget::width"), []);
    assert_eq!(ambiguous(&graph, "Widget::width"), 1);
    assert_eq!(lines_of(&graph, "Config::limit"), []);
    assert_eq!(ambiguous(&graph, "Config::limit"), 1);
}

fn hierarchy(bases: &[&str]) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (at, base) in (0u32..).zip(bases) {
        symbols.push(class(base, [at * 10 + 1, at * 10 + 5], &[]));
        symbols.push(field(base, "count", at * 10 + 2));
    }
    symbols.push(class("Child", [50, 60], bases));
    symbols.push(cpp_member("Child", "run", SymbolKind::Method, 51));
    symbols
}

#[test]
fn a_member_its_class_lacks_binds_the_field_of_the_one_base_that_declares_it() {
    let symbols = hierarchy(&["Base"]);
    let run = u32::try_from(symbols.len() - 1).expect("few symbols");
    let child = with_uses(
        file("src/child.h", symbols, &[]),
        run,
        &[("count", Receiver::Bare, 52), ("count", Receiver::Own, 53)],
    );

    let graph = graph(cpp(), &[child]);

    assert_eq!(lines_of(&graph, "Base::count"), [(52, 85), (53, 85)]);
}

#[test]
fn a_member_two_bases_declare_binds_neither_and_counts_as_ambiguous() {
    let symbols = hierarchy(&["Left", "Right"]);
    let run = u32::try_from(symbols.len() - 1).expect("few symbols");
    let child = with_uses(
        file("src/child.h", symbols, &[]),
        run,
        &[("count", Receiver::Bare, 52)],
    );

    let graph = graph(cpp(), &[child]);

    assert_eq!(lines_of(&graph, "Left::count"), []);
    assert_eq!(ambiguous(&graph, "Left::count"), 1);
}

#[test]
fn a_bare_name_reads_the_value_its_file_then_its_includes_then_their_includes_declare() {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![
                Symbol::new("run", SymbolKind::Function, 1, 9),
                value("local_limit", 10),
            ],
            &["inc/config.h"],
        ),
        0,
        &[
            ("local_limit", Receiver::Bare, 2),
            ("limit", Receiver::Bare, 3),
            ("deep", Receiver::Bare, 4),
            ("clash", Receiver::Bare, 5),
        ],
    );
    let config = file(
        "inc/config.h",
        vec![value("limit", 1), value("clash", 2)],
        &["inc/core.h"],
    );
    let mut other_clash = value("clash", 1);
    other_clash.qualified_name = "ns::clash".to_owned();
    let core = file("inc/core.h", vec![value("deep", 1), value("limit", 2)], &[]);
    let elsewhere = file("lib/other.h", vec![value("deep", 1), other_clash], &[]);

    let graph = graph(cpp(), &[user, config, core, elsewhere]);

    assert_eq!(lines_of(&graph, "local_limit"), [(2, 80)]);
    assert_eq!(
        lines_in(&graph, "limit"),
        ("inc/config.h", vec![(3, 70)]),
        "the direct includes bind before the files they reach"
    );
    assert_eq!(lines_in(&graph, "deep"), ("inc/core.h", vec![(4, 70)]));
    assert_eq!(lines_in(&graph, "clash"), ("inc/config.h", vec![(5, 70)]));
}

#[test]
fn two_values_of_a_name_in_one_tier_bind_neither() {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![Symbol::new("run", SymbolKind::Function, 1, 9)],
            &["inc/a.h", "inc/b.h"],
        ),
        0,
        &[("limit", Receiver::Bare, 2)],
    );
    let mut namespaced = value("limit", 1);
    namespaced.qualified_name = "ns::limit".to_owned();
    let graph = graph(
        cpp(),
        &[
            user,
            file("inc/a.h", vec![value("limit", 1)], &[]),
            file("inc/b.h", vec![namespaced], &[]),
        ],
    );

    assert_eq!(lines_of(&graph, "limit"), []);
    assert_eq!(lines_of(&graph, "ns::limit"), []);
}

#[test]
fn a_declaration_and_a_definition_of_one_value_in_one_tier_count_once() {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![Symbol::new("run", SymbolKind::Function, 1, 9)],
            &["inc/config.h", "inc/config_impl.h"],
        ),
        0,
        &[("limit", Receiver::Bare, 2)],
    );
    let graph = graph(
        cpp(),
        &[
            user,
            file("inc/config.h", vec![value("limit", 1)], &[]),
            file("inc/config_impl.h", vec![value("limit", 3)], &[]),
        ],
    );

    assert_eq!(lines_of(&graph, "limit"), [(2, 70)]);
}

#[test]
fn a_member_on_a_declared_value_binds_the_field_of_the_class_its_declaration_names() {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![
                Symbol::new("run", SymbolKind::Function, 1, 9),
                field("Local", "first", 10),
                Symbol::new("Alias", SymbolKind::TypeAlias, 11, 11),
                field("Other", "size", 12),
            ],
            &["inc/config.h"],
        ),
        0,
        &[
            ("limit", path("Config"), 2),
            ("first", path("pair"), 3),
            ("first", path("Alias"), 4),
            ("size", path(""), 5),
        ],
    );
    let config = file(
        "inc/config.h",
        vec![
            class("Config", [1, 3], &[]),
            field("Config", "limit", 2),
            field("Limits", "limit", 5),
        ],
        &[],
    );

    let graph = graph(cpp(), &[user, config]);

    assert_eq!(lines_of(&graph, "Config::limit"), [(2, 85)]);
    assert_eq!(
        lines_of(&graph, "Local::first"),
        [(4, 80)],
        "a class outside the graph binds no namesake, and a type the graph holds without the field leaves it to \
         the file"
    );
    assert_eq!(lines_of(&graph, "Other::size"), [(5, 80)]);
}

#[test]
fn a_member_its_declared_class_lacks_binds_the_field_of_the_one_base_that_declares_it() {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![
                Symbol::new("run", SymbolKind::Function, 1, 9),
                field("Local", "at", 10),
            ],
            &["inc/ast.h"],
        ),
        0,
        &[("at", path("ExprFor"), 2), ("size", path("ExprFor"), 3)],
    );
    let ast = file(
        "inc/ast.h",
        vec![
            class("Expression", [1, 3], &[]),
            field("Expression", "at", 2),
            class("ExprFor", [5, 7], &["Expression"]),
            field("Visitor", "size", 9),
        ],
        &[],
    );

    let graph = graph(cpp(), &[user, ast]);

    assert_eq!(
        lines_of(&graph, "Expression::at"),
        [(2, 85)],
        "a base's field binds before the one field of the name in the file"
    );
    assert_eq!(
        lines_of(&graph, "Visitor::size"),
        [(3, 70)],
        "a class whose bases hold no field of the name leaves it to the include tiers"
    );
}

fn reached_through_includes(fields: usize) -> Graph {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![Symbol::new("run", SymbolKind::Function, 1, 9)],
            &["inc/a.h"],
        ),
        0,
        &[("deep", path(""), 2)],
    );
    let mut files = vec![
        user,
        file("inc/a.h", Vec::new(), &["inc/core.h"]),
        file("inc/core.h", vec![field("Node", "deep", 1)], &[]),
    ];
    files.extend((1..fields).map(|at| {
        file(
            &format!("lib/other{at}.h"),
            vec![field("Pane", "deep", 1)],
            &[],
        )
    }));
    graph(cpp(), &files)
}

#[test]
fn a_field_beyond_the_direct_includes_binds_only_for_a_name_at_most_32_fields_share() {
    assert_eq!(
        lines_in(&reached_through_includes(32), "Node::deep"),
        ("inc/core.h", vec![(2, 70)])
    );
    let graph = reached_through_includes(33);
    assert_eq!(lines_of(&graph, "Node::deep"), []);
    assert_eq!(ambiguous(&graph, "Node::deep"), 1);
}

#[test]
fn a_member_on_an_unnamed_value_binds_the_one_field_its_includes_or_their_includes_hold() {
    let user = with_uses(
        file(
            "src/user.cpp",
            vec![Symbol::new("run", SymbolKind::Function, 1, 9)],
            &["inc/a.h", "inc/b.h"],
        ),
        0,
        &[
            ("left", path(""), 2),
            ("deep", path(""), 3),
            ("both", path(""), 4),
        ],
    );
    let graph = graph(
        cpp(),
        &[
            user,
            file(
                "inc/a.h",
                vec![field("Rect", "left", 1), field("Rect", "both", 2)],
                &["inc/core.h"],
            ),
            file("inc/b.h", vec![field("Box", "both", 1)], &[]),
            file("inc/core.h", vec![field("Node", "deep", 1)], &[]),
            file("lib/other.h", vec![field("Pane", "left", 1)], &[]),
        ],
    );

    assert_eq!(lines_of(&graph, "Rect::left"), [(2, 70)]);
    assert_eq!(lines_of(&graph, "Node::deep"), [(3, 70)]);
    assert_eq!(lines_of(&graph, "Rect::both"), []);
    assert_eq!(ambiguous(&graph, "Rect::both"), 1);
}
