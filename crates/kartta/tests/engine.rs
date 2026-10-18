//! The engine's contract: which graph a file goes into, what a shared cache spares a second build, and what a
//! language's resolver sees of the tree.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and an integration test crate is built \
              without cfg(test) — so the workspace no-panic posture has to be waived here, where a failed expectation \
              is the assertion"
)]

use std::fmt::Write as _;
use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use kartta::{
    BuildError, BuildOptions, CancelToken, Definitions, Engine, Extraction, Language, Limits, Miss,
    RawExtraction, Region, Repository, Resolver, Symbol, SymbolKind, confidence,
};
use kartta_fs::{Content, ContentVisitor, Index, Namespace};
use kartta_lang::{CallRef, SymbolId};
use tempfile::TempDir;

#[derive(Debug)]
struct Defs {
    name: &'static str,
    family: &'static str,
    extension: &'static str,
    extracted: Arc<AtomicUsize>,
}

impl Defs {
    fn new(name: &'static str, family: &'static str, extension: &'static str) -> Self {
        Self {
            name,
            family,
            extension,
            extracted: Arc::default(),
        }
    }
}

impl Language for Defs {
    fn name(&self) -> &str {
        self.name
    }

    fn family(&self) -> &str {
        self.family
    }

    fn claims(&self, path: &str) -> bool {
        path.ends_with(self.extension)
    }

    fn fingerprint(&self) -> &[u8] {
        b"defs 1"
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        self.extracted.fetch_add(1, Ordering::Relaxed);
        let mut raw = RawExtraction::default();
        for (at, line) in String::from_utf8_lossy(source).lines().enumerate() {
            let number = u32::try_from(at + 1).unwrap_or(u32::MAX);
            if let Some(name) = line.strip_prefix("def ") {
                raw.symbols
                    .push(Symbol::new(name, SymbolKind::Function, number, number));
            } else if let Some(callee) = line.strip_prefix("call ")
                && let Some(caller) = raw.symbols.len().checked_sub(1)
            {
                raw.calls.push(CallRef {
                    caller_id: SymbolId(u32::try_from(caller).unwrap_or(u32::MAX)),
                    callee: callee.to_owned(),
                    receiver: None,
                    line: number,
                });
            }
        }
        raw
    }

    fn resolver<'r>(&'r self, _repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(AtPath)
    }
}

struct AtPath;

impl Resolver for AtPath {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        raw.into_extraction(path).0
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Seen {
    in_src: Vec<String>,
    in_root: Vec<String>,
    in_root_spelled_empty: Vec<String>,
    named_b: Vec<String>,
    ignoring_case: [Vec<String>; 3],
    contains: [bool; 3],
    go_mod: Vec<u8>,
}

#[derive(Debug, Default)]
struct Probe {
    seen: Arc<Mutex<Seen>>,
}

impl Language for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, "probe")
    }

    fn fingerprint(&self) -> &[u8] {
        b"probe 1"
    }

    fn extract(&self, _source: &[u8]) -> RawExtraction {
        RawExtraction::default()
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        let seen = Seen {
            in_src: repository.files_in_dir("src").collect(),
            in_root: repository.files_in_dir(".").collect(),
            in_root_spelled_empty: repository.files_in_dir("").collect(),
            named_b: repository.files_named("b.probe").collect(),
            ignoring_case: [
                repository.files_ignoring_case("SRC/Sub/C.Probe").collect(),
                repository.files_ignoring_case("Src/SUB").collect(),
                repository.files_ignoring_case("Ignored.probe").collect(),
            ],
            contains: [
                repository.contains("src/sub/c.probe"),
                repository.contains("src/sub"),
                repository.contains("ignored.probe"),
            ],
            go_mod: repository.read("go.mod").expect("go.mod is indexed"),
        };
        *self.seen.lock().expect("no other thread panicked") = seen;
        Box::new(AtPath)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelAt {
    ResolverBuilt,
    FileResolved,
}

#[derive(Debug)]
struct Cancelling(CancelToken, CancelAt);

struct CancelOnResolve<'t>(&'t CancelToken);

impl Resolver for CancelOnResolve<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        self.0.cancel();
        raw.into_extraction(path).0
    }
}

impl Language for Cancelling {
    fn name(&self) -> &'static str {
        "cancelling"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, "defs")
    }

    fn fingerprint(&self) -> &[u8] {
        b"cancelling 1"
    }

    fn extract(&self, _source: &[u8]) -> RawExtraction {
        RawExtraction::default()
    }

    fn resolver<'r>(&'r self, _repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        match self.1 {
            CancelAt::ResolverBuilt => {
                self.0.cancel();
                Box::new(AtPath)
            }
            CancelAt::FileResolved => Box::new(CancelOnResolve(&self.0)),
        }
    }
}

struct Junk(Namespace);

impl ContentVisitor for Junk {
    fn wants(&self, path: &str) -> bool {
        has_extension(path, "defs")
    }

    fn visit(&self, content: Content<'_>) {
        content.cache(&self.0, b"not an extraction");
    }
}

fn has_extension(path: &str, extension: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|found| found == extension)
}

fn tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");

    for (path, content) in spec {
        let full = dir.path().join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&full, content).expect("write file");
    }

    dir
}

fn build(root: &Path, options: &BuildOptions, languages: Vec<Box<dyn Language>>) -> Engine {
    let outcome = Engine::build(root, options, languages, None).expect("build the engine");
    assert!(
        outcome.errors().is_empty(),
        "unexpected build errors: {:?}",
        outcome.errors()
    );

    outcome.into_engine()
}

fn cached(cache: &Path) -> BuildOptions {
    let mut options = BuildOptions::default();
    options.content_cache = Some(cache.to_path_buf());
    options
}

fn places(definitions: &Definitions<'_>) -> Vec<String> {
    definitions
        .found
        .iter()
        .map(|found| format!("{}:{}", found.definition.file, found.definition.line))
        .collect()
}

#[test]
fn a_claimed_file_is_in_its_family_graph_and_an_unclaimed_one_in_none() {
    let tree = tree(&[
        ("src/a.defs", "other\ndef parse\n"),
        ("notes.md", "def parse\n"),
    ]);

    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(Defs::new("defs", "defs", ".defs"))],
    );

    let graph = engine.graph_of("src/a.defs").expect("defs claims the file");
    assert_eq!(
        places(&graph.definitions("parse", None, 10)),
        ["src/a.defs:2"]
    );
    assert!(engine.graph_of("notes.md").is_none());
    assert!(engine.index().entry("notes.md").is_some());
}

#[test]
fn languages_of_one_family_share_a_graph_and_the_first_to_claim_a_file_takes_it() {
    let tree = tree(&[("one.alpha", "def one\n"), ("two.beta", "def two\n")]);

    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![
            Box::new(Defs::new("alpha", "shared", ".alpha")),
            Box::new(Defs::new("beta", "shared", ".beta")),
            Box::new(Defs::new("gamma", "other", ".alpha")),
        ],
    );

    let files: Vec<(&str, usize)> = engine
        .graphs()
        .map(|(family, graph)| (family, graph.stats().files))
        .collect();
    assert_eq!(files, [("shared", 2), ("other", 0)]);
}

#[test]
fn a_change_across_families_is_analyzed_by_each_graph_and_answered_as_one() {
    let tree = tree(&[
        ("one.alpha", "def one\n"),
        ("two.beta", "def two\n"),
        ("notes.md", "def notes\n"),
    ]);
    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![
            Box::new(Defs::new("alpha", "first", ".alpha")),
            Box::new(Defs::new("beta", "second", ".beta")),
        ],
    );
    let regions = [
        Region::new("two.beta", 1..=1),
        Region::new("one.alpha", 1..=1),
        Region::new("notes.md", 1..=1),
        Region::new("absent.alpha", 1..=1),
    ];

    let change = engine.analyze_change(&regions, Limits::new(3));
    let mut seeds: Vec<&str> = change.seeds.iter().map(|seed| seed.symbol.name).collect();
    seeds.sort_unstable();

    assert_eq!(change.changed_symbols, 2);
    assert_eq!(change.changed_files, 2);
    assert_eq!(seeds, ["one", "two"]);
    assert_eq!(
        change.unheld_regions, 2,
        "notes.md no language claims, absent.alpha no graph holds"
    );
}

fn two_hundred_defs(prefix: &str) -> String {
    let mut source = String::new();
    for index in 0..200 {
        writeln!(source, "def {prefix}{index:03}").expect("a String takes every write");
    }
    source
}

#[test]
fn the_seeds_of_several_families_compete_for_one_limit() {
    let alpha = two_hundred_defs("a");
    let beta = two_hundred_defs("b");
    let tree = tree(&[("many.alpha", &alpha), ("many.beta", &beta)]);
    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![
            Box::new(Defs::new("alpha", "first", ".alpha")),
            Box::new(Defs::new("beta", "second", ".beta")),
        ],
    );
    let regions = [
        Region::new("many.beta", 1..=u32::MAX),
        Region::new("many.alpha", 1..=u32::MAX),
    ];

    let change = engine.analyze_change(&regions, Limits::new(1));
    let seeds: Vec<&str> = change.seeds.iter().map(|seed| seed.symbol.name).collect();

    assert_eq!(change.changed_symbols, 400);
    assert_eq!(seeds.len(), 300);
    assert_eq!(
        (seeds[0], seeds[199], seeds[200], seeds[299]),
        ("a000", "a199", "b000", "b099")
    );
}

#[test]
fn a_second_build_takes_each_extraction_from_the_shared_cache() {
    let tree = tree(&[("a.defs", "def parse\n"), ("b.defs", "def render\n")]);
    let cache = tempfile::tempdir().expect("create cache dir");
    let first = Defs::new("defs", "defs", ".defs");
    let first_extracted = Arc::clone(&first.extracted);
    let second = Defs::new("defs", "defs", ".defs");
    let second_extracted = Arc::clone(&second.extracted);

    build(tree.path(), &cached(cache.path()), vec![Box::new(first)]);
    let engine = build(tree.path(), &cached(cache.path()), vec![Box::new(second)]);

    assert_eq!(first_extracted.load(Ordering::Relaxed), 2);
    assert_eq!(second_extracted.load(Ordering::Relaxed), 0);
    let graph = engine.graph_of("a.defs").expect("defs claims the file");
    assert_eq!(places(&graph.definitions("render", None, 10)), ["b.defs:1"]);
}

#[test]
fn files_extracted_one_after_another_on_one_thread_are_each_extracted_and_cached_once() {
    let tree = tree(&[
        ("a.defs", "def parse\ncall render\n"),
        ("b.defs", "def render\n"),
        ("c.defs", "def main\ncall parse\n"),
    ]);
    let cache = tempfile::tempdir().expect("create cache dir");
    let mut options = cached(cache.path());
    options.threads = NonZeroUsize::new(1);
    let cold = Defs::new("defs", "defs", ".defs");
    let cold_extracted = Arc::clone(&cold.extracted);
    let warm = Defs::new("defs", "defs", ".defs");
    let warm_extracted = Arc::clone(&warm.extracted);

    build(tree.path(), &options, vec![Box::new(cold)]);
    let engine = build(tree.path(), &options, vec![Box::new(warm)]);

    assert_eq!(cold_extracted.load(Ordering::Relaxed), 3);
    assert_eq!(warm_extracted.load(Ordering::Relaxed), 0);
    let graph = engine.graph_of("c.defs").expect("defs claims the file");
    assert_eq!(places(&graph.definitions("main", None, 10)), ["c.defs:1"]);
}

#[test]
fn the_stats_count_the_files_each_language_claimed_read_took_from_the_cache_and_holds() {
    let tree = tree(&[
        ("a.defs", "def parse\n"),
        ("b.defs", "def render\n"),
        ("blob.defs", "def\0\0\0binary\n"),
        ("one.alpha", "def one\n"),
        ("notes.md", "def notes\n"),
    ]);
    let cache = tempfile::tempdir().expect("create cache dir");
    let languages = || -> Vec<Box<dyn Language>> {
        vec![
            Box::new(Defs::new("defs", "defs", ".defs")),
            Box::new(Defs::new("alpha", "defs", ".alpha")),
        ]
    };
    let counts = |engine: &Engine| -> Vec<(String, usize, usize, usize, usize)> {
        engine
            .stats()
            .languages
            .iter()
            .map(|files| {
                let name = format!("{}/{}", files.family, files.name);
                (name, files.claimed, files.read, files.cached, files.held)
            })
            .collect()
    };

    let cold = build(tree.path(), &cached(cache.path()), languages());
    let warm = build(tree.path(), &cached(cache.path()), languages());

    assert_eq!(
        counts(&cold),
        [
            ("defs/defs".to_owned(), 3, 2, 0, 2),
            ("defs/alpha".to_owned(), 1, 1, 0, 1)
        ]
    );
    assert_eq!(
        counts(&warm),
        [
            ("defs/defs".to_owned(), 3, 2, 2, 2),
            ("defs/alpha".to_owned(), 1, 1, 1, 1)
        ]
    );
    let stats = warm.stats();
    let families: Vec<(&str, usize, usize)> = stats
        .families
        .iter()
        .map(|(family, graph)| (*family, graph.files, graph.symbols))
        .collect();
    assert_eq!(families, [("defs", 3, 3)]);
}

#[test]
fn every_family_s_call_sites_resolve_on_the_index_s_threads() {
    let tree = tree(&[
        ("a.defs", "def main\ncall parse\ncall printf\n"),
        ("b.defs", "def parse\n"),
        ("c.other", "def solo\ncall absent\n"),
    ]);
    let mut options = BuildOptions::default();
    options.threads = NonZeroUsize::new(2);
    let engine = build(
        tree.path(),
        &options,
        vec![
            Box::new(Defs::new("defs", "defs", ".defs")),
            Box::new(Defs::new("other", "other", ".other")),
        ],
    );

    let resolution = engine.resolution(1);

    let families: Vec<&str> = resolution.iter().map(|(family, _)| *family).collect();
    assert_eq!(families, ["defs", "other"]);
    let (defs, other) = (&resolution[0].1, &resolution[1].1);
    assert_eq!(
        (defs.sites, defs.bound.as_slice()),
        (2, &[(confidence::UNIQUE, 1)][..])
    );
    assert_eq!(
        (defs.unbound[0].callee, defs.unbound[0].miss),
        ("printf", Miss::Undefined)
    );
    assert_eq!((other.sites, other.misses.undefined), (1, 1));
    assert_eq!(other.unbound[0].caller.name, "solo");
}

#[test]
fn an_extraction_the_cache_holds_malformed_is_extracted_again_from_the_file() {
    let tree = tree(&[("a.defs", "def parse\n")]);
    let cache = tempfile::tempdir().expect("create cache dir");
    let fingerprint = [
        &b"defs 1"[..],
        &kartta_lang::FINGERPRINT,
        RawExtraction::CODEC,
    ]
    .concat();
    let namespace = Namespace::new("graph/defs", &fingerprint);
    Index::build_with(
        tree.path(),
        &cached(cache.path()),
        Some(&Junk(namespace)),
        None,
    )
    .expect("fill the cache");
    let defs = Defs::new("defs", "defs", ".defs");
    let extracted = Arc::clone(&defs.extracted);

    let engine = build(tree.path(), &cached(cache.path()), vec![Box::new(defs)]);

    assert_eq!(extracted.load(Ordering::Relaxed), 1);
    let graph = engine.graph_of("a.defs").expect("defs claims the file");
    assert_eq!(places(&graph.definitions("parse", None, 10)), ["a.defs:1"]);
}

#[test]
fn a_resolver_sees_the_regular_files_the_build_indexed() {
    let tree = tree(&[
        (".gitignore", "ignored.probe\n"),
        ("go.mod", "module example"),
        ("ignored.probe", ""),
        ("other/b.probe", ""),
        ("src/a.probe", ""),
        ("src/b.probe", ""),
        ("src/sub/c.probe", ""),
    ]);
    let probe = Probe::default();
    let seen = Arc::clone(&probe.seen);

    build(tree.path(), &BuildOptions::default(), vec![Box::new(probe)]);

    let seen = seen.lock().expect("no other thread panicked");
    assert_eq!(
        *seen,
        Seen {
            in_src: vec!["src/a.probe".to_owned(), "src/b.probe".to_owned()],
            in_root: vec![".gitignore".to_owned(), "go.mod".to_owned()],
            in_root_spelled_empty: vec![".gitignore".to_owned(), "go.mod".to_owned()],
            named_b: vec!["other/b.probe".to_owned(), "src/b.probe".to_owned()],
            ignoring_case: [vec!["src/sub/c.probe".to_owned()], Vec::new(), Vec::new()],
            contains: [true, false, false],
            go_mod: b"module example".to_vec(),
        }
    );
}

#[test]
fn a_language_that_claims_no_file_is_never_asked_for_a_resolver() {
    let tree = tree(&[("a.defs", "def parse\n")]);
    let probe = Probe::default();
    let seen = Arc::clone(&probe.seen);

    build(
        tree.path(),
        &BuildOptions::default(),
        vec![
            Box::new(Defs::new("defs", "defs", ".defs")),
            Box::new(probe),
        ],
    );

    assert_eq!(
        *seen.lock().expect("no other thread panicked"),
        Seen::default()
    );
}

#[test]
fn a_build_cancelled_after_its_walk_stops_before_it_resolves() {
    let tree = tree(&[("a.defs", "def parse\n")]);
    let cancel = CancelToken::new();

    let outcome = Engine::build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(Cancelling(
            cancel.clone(),
            CancelAt::ResolverBuilt,
        ))],
        Some(&cancel),
    );

    assert!(matches!(outcome, Err(BuildError::Cancelled)));
}

#[test]
fn a_build_cancelled_while_it_resolves_its_last_batch_completes() {
    let tree = tree(&[("a.defs", "def parse\n")]);
    let cancel = CancelToken::new();

    let outcome = Engine::build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(Cancelling(cancel.clone(), CancelAt::FileResolved))],
        Some(&cancel),
    );

    let engine = outcome.expect("every batch was resolved").into_engine();
    let graph = engine
        .graph_of("a.defs")
        .expect("the language claims the file");
    assert_eq!(graph.stats().files, 1);
}

#[test]
fn a_tree_of_more_files_than_one_batch_resolves_whole() {
    let names: Vec<(String, String)> = (0..4100)
        .map(|at| (format!("f{at}.defs"), format!("def f{at}\n")))
        .collect();
    let spec: Vec<(&str, &str)> = names
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .collect();
    let tree = tree(&spec);

    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(Defs::new("defs", "defs", ".defs"))],
    );

    let graph = engine.graph_of("f0.defs").expect("defs claims the file");
    assert_eq!((graph.stats().files, graph.stats().symbols), (4100, 4100));
}
