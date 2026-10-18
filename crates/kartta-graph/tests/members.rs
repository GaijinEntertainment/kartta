//! Uses on a script language's own receiver, on a receiver that names an owner, and of a value a file imports.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the query helpers \
              of an integration test crate are neither; a name that selects nothing is the failure"
)]

use kartta_graph::{Graph, SiteQuery};
use kartta_lang::{
    Access, Conventions, Extraction, IncludeRef, OwnMembers, ReExport, Receiver, RefSite, Symbol,
    SymbolId, SymbolKind, UntypedMembers, ValueUses,
};

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
    let mut extraction = Extraction::default();
    path.clone_into(&mut extraction.file);
    extraction.symbols = symbols;
    extraction.includes = includes
        .iter()
        .map(|&target| IncludeRef {
            target: target.to_owned(),
        })
        .collect();
    extraction
}

fn graph(conventions: Conventions, files: &[Extraction]) -> Graph {
    let mut graph = Graph::new(conventions);
    for extraction in files {
        graph.insert(extraction);
    }
    graph
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
