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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kartta::{
    BuildError, BuildOptions, CancelToken, Definitions, Engine, Extraction, Language, Limits, Miss,
    RawExtraction, Region, Repository, Resolver, Symbol, SymbolKind, confidence,
};
use kartta_fs::{Content, ContentIndexStorage, ContentVisitor, Index, Namespace};
use kartta_lang::{CallRef, SymbolId};
use tempfile::TempDir;

#[derive(Debug)]
struct Defs {
    name: &'static str,
    family: &'static str,
    extension: &'static str,
    extracted: Arc<AtomicUsize>,
    resolvers: Arc<AtomicUsize>,
    resolves_as_kept: bool,
}

impl Defs {
    fn new(name: &'static str, family: &'static str, extension: &'static str) -> Self {
        Self {
            name,
            family,
            extension,
            extracted: Arc::default(),
            resolvers: Arc::default(),
            resolves_as_kept: false,
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

    fn declines(&self, path: &str) -> Option<&str> {
        path.strip_suffix(".off")
            .is_some_and(|path| path.ends_with(self.extension))
            .then_some("a file switched off is read by no build")
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
                raw.calls.push(CallRef::new(
                    SymbolId(u32::try_from(caller).unwrap_or(u32::MAX)),
                    callee.to_owned(),
                    None,
                    number,
                ));
            }
        }
        raw
    }

    fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver> {
        self.resolvers.fetch_add(1, Ordering::Relaxed);
        let _configuration = repository.read("config.cfg");
        if self.resolves_as_kept {
            Box::new(AsKept)
        } else {
            Box::new(AtPath)
        }
    }
}

struct AtPath;

impl Resolver for AtPath {
    fn resolve(&self, raw: RawExtraction, path: &str, _repository: &dyn Repository) -> Extraction {
        raw.into_extraction(path).0
    }
}

/// A resolver that derives nothing from the configuration it reads, and so resolves as any it was built again from.
struct AsKept;

impl Resolver for AsKept {
    fn resolve(&self, raw: RawExtraction, path: &str, _repository: &dyn Repository) -> Extraction {
        raw.into_extraction(path).0
    }

    fn resolves_as(&self, _kept: &dyn Resolver) -> bool {
        true
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

    fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CancelAt {
    ResolverBuilt,
    FileResolved,
    /// As `FileResolved`, with this file removed once the walk has read it.
    FileResolvedWith(PathBuf),
}

#[derive(Debug)]
struct Cancelling(CancelToken, CancelAt);

struct CancelOnResolve(CancelToken);

impl Resolver for CancelOnResolve {
    fn resolve(&self, raw: RawExtraction, path: &str, _repository: &dyn Repository) -> Extraction {
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

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        match &self.1 {
            CancelAt::ResolverBuilt => {
                self.0.cancel();
                Box::new(AtPath)
            }
            CancelAt::FileResolved => Box::new(CancelOnResolve(self.0.clone())),
            CancelAt::FileResolvedWith(gone) => {
                fs::remove_file(gone).expect("the file goes");
                Box::new(CancelOnResolve(self.0.clone()))
            }
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
        outcome.errors.is_empty(),
        "unexpected build errors: {:?}",
        outcome.errors
    );

    outcome.engine
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

    let (_, graph) = engine.graph_of("src/a.defs").expect("defs claims the file");
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
fn the_first_language_to_decline_a_file_names_itself_and_its_reason() {
    let tree = tree(&[
        ("a.alpha", "def a\n"),
        ("b.alpha.off", "def b\n"),
        ("c.md", "def c\n"),
    ]);

    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![
            Box::new(Defs::new("beta", "other", ".beta")),
            Box::new(Defs::new("alpha", "shared", ".alpha")),
            Box::new(Defs::new("gamma", "other", ".alpha")),
        ],
    );

    assert!(engine.graph_of("b.alpha.off").is_none());
    assert_eq!(
        engine.declined("b.alpha.off"),
        Some(("alpha", "a file switched off is read by no build"))
    );
    assert_eq!(engine.declined("a.alpha"), None);
    assert_eq!(engine.declined("c.md"), None);
}

fn shared_family() -> (Defs, Defs) {
    (
        Defs::new("alpha", "shared", ".alpha"),
        Defs::new("beta", "shared", ".beta"),
    )
}

fn resolvers_built(languages: &(Arc<AtomicUsize>, Arc<AtomicUsize>)) -> (usize, usize) {
    (
        languages.0.load(Ordering::Relaxed),
        languages.1.load(Ordering::Relaxed),
    )
}

#[test]
fn a_family_resolves_through_one_resolver_its_first_language_builds() {
    let tree = tree(&[("one.alpha", "def one\n"), ("two.beta", "def two\n")]);
    let (alpha, beta) = shared_family();
    let built = (Arc::clone(&alpha.resolvers), Arc::clone(&beta.resolvers));

    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(alpha), Box::new(beta)],
    );

    assert_eq!(resolvers_built(&built), (1, 0));
    let (_, graph) = engine.graph_of("two.beta").expect("beta claims the file");
    assert_eq!(graph.stats().files, 2);
}

#[test]
fn a_family_whose_first_language_claims_no_file_resolves_the_files_of_the_others() {
    let tree = tree(&[("two.beta", "def two\n")]);
    let (alpha, beta) = shared_family();

    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(alpha), Box::new(beta)],
    );

    let (family, graph) = engine.graph_of("two.beta").expect("beta claims the file");
    assert_eq!((family, graph.stats().files), ("shared", 1));
    assert_eq!(places(&graph.definitions("two", None, 10)), ["two.beta:1"]);
}

#[test]
fn a_changed_configuration_builds_its_family_s_resolver_again_once_and_names_the_family_once() {
    let tree = tree(&[
        ("config.cfg", "one"),
        ("one.alpha", "def one\n"),
        ("two.beta", "def two\n"),
    ]);
    // constraint: a stamp is trusted only when its file last changed over two seconds before the walk began
    std::thread::sleep(Duration::from_millis(2100));
    let (alpha, beta) = shared_family();
    let built = (Arc::clone(&alpha.resolvers), Arc::clone(&beta.resolvers));
    let mut options = BuildOptions::default();
    options.stamps = true;
    let mut engine = build(tree.path(), &options, vec![Box::new(alpha), Box::new(beta)]);

    fs::write(tree.path().join("config.cfg"), "two").expect("edit the configuration");
    let refresh = engine
        .refresh_paths(["config.cfg"], None)
        .expect("refresh the configuration");

    assert_eq!(refresh.reconfigured, ["shared"]);
    assert_eq!(refresh.resolved_again, 2, "one.alpha and two.beta");
    assert_eq!(resolvers_built(&built), (2, 0));
}

#[test]
fn a_configuration_rebuilt_to_resolve_as_the_kept_one_resolves_nothing_again() {
    let tree = tree(&[
        ("config.cfg", "one"),
        ("one.alpha", "def one\n"),
        ("two.beta", "def two\n"),
    ]);
    // constraint: a stamp is trusted only when its file last changed over two seconds before the walk began
    std::thread::sleep(Duration::from_millis(2100));
    let alpha = Defs {
        resolves_as_kept: true,
        ..Defs::new("alpha", "shared", ".alpha")
    };
    let beta = Defs::new("beta", "shared", ".beta");
    let built = (Arc::clone(&alpha.resolvers), Arc::clone(&beta.resolvers));
    let mut engine = build(
        tree.path(),
        &stamped(),
        vec![Box::new(alpha), Box::new(beta)],
    );

    fs::write(tree.path().join("config.cfg"), "two").expect("edit the configuration");
    let refresh = engine
        .refresh_paths(["config.cfg"], None)
        .expect("refresh the configuration");

    assert!(
        refresh.reconfigured.is_empty(),
        "{:?}",
        refresh.reconfigured
    );
    assert_eq!((refresh.extracted, refresh.resolved_again), (0, 0));
    assert_eq!(
        resolvers_built(&built),
        (2, 0),
        "the resolver is built again to compare"
    );
    let (_, graph) = engine.graph_of("two.beta").expect("beta claims the file");
    assert_eq!(graph.stats().files, 2);
}

fn stamped() -> BuildOptions {
    let mut options = BuildOptions::default();
    options.stamps = true;
    options
}

#[test]
fn a_followed_family_reads_its_configuration_again_only_when_a_refresh_reaches_it() {
    let tree = tree(&[
        ("config.cfg", "one"),
        ("one.alpha", "def one\n"),
        ("two.beta", "def two\n"),
    ]);
    // constraint: a stamp is trusted only when its file last changed over two seconds before the walk began
    std::thread::sleep(Duration::from_millis(2100));
    let (alpha, beta) = shared_family();
    let reads = (Arc::clone(&alpha.resolvers), Arc::clone(&beta.resolvers));
    let mut engine = build(
        tree.path(),
        &stamped(),
        vec![Box::new(alpha), Box::new(beta)],
    );
    let after_build = resolvers_built(&reads);

    fs::write(tree.path().join("one.alpha"), "def one\ndef more\n").expect("edit a source file");
    let source = engine
        .refresh_paths(["one.alpha"], None)
        .expect("refresh the source file");
    let after_source = resolvers_built(&reads);
    fs::write(tree.path().join("config.cfg"), "two").expect("edit the configuration");
    let configuration = engine
        .refresh_paths(["config.cfg"], None)
        .expect("refresh the configuration");

    assert_eq!(after_build, (1, 0), "config.cfg is read once by the build");
    assert_eq!(
        (after_source, source.reconfigured.len(), source.extracted),
        ((1, 0), 0, 1),
        "a refresh of one.alpha reads no configuration"
    );
    let (_, graph) = engine.graph_of("one.alpha").expect("alpha claims the file");
    assert_eq!(
        places(&graph.definitions("more", None, 10)),
        ["one.alpha:2"]
    );
    assert_eq!(configuration.reconfigured, ["shared"]);
    assert_eq!(configuration.resolved_again, 2, "one.alpha and two.beta");
    assert_eq!(resolvers_built(&reads), (2, 0));
}

#[test]
fn a_followed_family_with_no_file_at_the_build_builds_its_resolver_when_it_gains_one() {
    let tree = tree(&[("config.cfg", "one"), ("one.alpha", "def one\n")]);
    std::thread::sleep(Duration::from_millis(2100));
    let alpha = Defs::new("alpha", "first", ".alpha");
    let gamma = Defs::new("gamma", "second", ".gamma");
    let reads = (Arc::clone(&alpha.resolvers), Arc::clone(&gamma.resolvers));
    let mut engine = build(
        tree.path(),
        &stamped(),
        vec![Box::new(alpha), Box::new(gamma)],
    );
    let after_build = resolvers_built(&reads);

    fs::write(tree.path().join("three.gamma"), "def three\n").expect("add a gamma file");
    let gained = engine
        .refresh_paths(["three.gamma"], None)
        .expect("refresh the added file");
    let after_gained = resolvers_built(&reads);
    fs::write(tree.path().join("three.gamma"), "def three\ndef four\n")
        .expect("edit the gamma file");
    engine
        .refresh_paths(["three.gamma"], None)
        .expect("refresh the edited file");

    assert_eq!(after_build, (1, 0), "no gamma file, no gamma resolver");
    assert_eq!((after_gained, gained.extracted), ((1, 1), 1));
    assert!(gained.reconfigured.is_empty(), "{:?}", gained.reconfigured);
    assert_eq!(
        resolvers_built(&reads),
        (1, 1),
        "the gamma resolver is kept"
    );
    let (family, graph) = engine
        .graph_of("three.gamma")
        .expect("gamma claims the file");
    assert_eq!(family, "second");
    assert_eq!(
        places(&graph.definitions("four", None, 10)),
        ["three.gamma:2"]
    );
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

    let change = engine.analyze_change(&regions, Limits::new(3)).change;
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

#[test]
fn a_change_says_why_no_graph_holds_each_region_it_counts_as_unheld() {
    let tree = tree(&[
        ("one.alpha", "def one\n"),
        ("notes.md", "def notes\n"),
        ("old.alpha.off", "def old\n"),
        ("older.alpha.off", "def older\n"),
        ("old.beta.off", "def old\n"),
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
        Region::new("one.alpha", 1..=1),
        Region::new("notes.md", 1..=1),
        Region::new("old.alpha.off", 1..=1),
        Region::new("old.beta.off", 1..=1),
        Region::new("absent.alpha", 1..=1),
        Region::new("older.alpha.off", 1..=1),
        Region::new("absent.md", 1..=1),
        Region::new("absent.alpha.off", 1..=1),
    ];

    let answer = engine.analyze_change(&regions, Limits::new(3));
    let declined: Vec<(&str, &str, usize)> = answer
        .declined
        .iter()
        .map(|declined| (declined.language, declined.reason, declined.regions))
        .collect();

    assert_eq!(answer.change.changed_symbols, 1);
    assert_eq!(answer.change.unheld_regions, 7);
    assert_eq!(
        answer.missing_regions, 3,
        "absent.alpha, absent.md and absent.alpha.off the index does not hold, claimed, declined or neither"
    );
    assert_eq!(answer.unclaimed_regions, 1, "notes.md");
    assert_eq!(
        declined,
        [
            ("alpha", "a file switched off is read by no build", 2),
            ("beta", "a file switched off is read by no build", 1),
        ],
        "old.alpha.off and older.alpha.off under one language, old.beta.off under the next, as the regions first name \
         them"
    );
}

#[cfg(unix)]
#[test]
fn a_change_counts_its_regions_of_symlinks_apart() {
    let tree = tree(&[("one.alpha", "def one\n"), ("notes.md", "notes\n")]);
    std::os::unix::fs::symlink("one.alpha", tree.path().join("link.alpha")).expect("link.alpha");
    std::os::unix::fs::symlink("notes.md", tree.path().join("link.md")).expect("link.md");
    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(Defs::new("alpha", "first", ".alpha"))],
    );
    let regions = [
        Region::new("one.alpha", 1..=1),
        Region::new("link.alpha", 1..=1),
        Region::new("link.md", 1..=1),
    ];

    let answer = engine.analyze_change(&regions, Limits::new(3));

    assert_eq!(answer.change.unheld_regions, 2);
    assert_eq!(
        answer.symlink_regions, 2,
        "link.alpha, which the alpha graph claims, and link.md, which no language claims"
    );
    assert_eq!(answer.unclaimed_regions, 0);
}

#[cfg(unix)]
#[test]
fn a_change_counts_its_regions_of_special_files_apart() {
    let tree = tree(&[("one.alpha", "def one\n")]);
    let _sockets = ["sock.alpha", "sock"].map(|name| {
        std::os::unix::net::UnixListener::bind(tree.path().join(name)).expect("a socket")
    });
    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(Defs::new("alpha", "first", ".alpha"))],
    );
    let regions = [
        Region::new("one.alpha", 1..=1),
        Region::new("sock.alpha", 1..=1),
        Region::new("sock", 1..=1),
    ];

    let answer = engine.analyze_change(&regions, Limits::new(3));

    assert_eq!(answer.change.changed_symbols, 1);
    assert_eq!(answer.change.unheld_regions, 2);
    assert_eq!(
        answer.special_regions, 2,
        "sock.alpha, whose name the alpha graph claims, and sock, which no language claims"
    );
    assert_eq!(answer.unclaimed_regions, 0);
}

#[test]
fn a_change_counts_its_regions_of_directories_apart() {
    let tree = tree(&[
        ("one.alpha", "def one\n"),
        ("pkg/a.alpha", "def a\n"),
        ("lib.alpha/b.alpha", "def b\n"),
    ]);
    let engine = build(
        tree.path(),
        &BuildOptions::default(),
        vec![Box::new(Defs::new("alpha", "first", ".alpha"))],
    );
    let regions = [
        Region::new("one.alpha", 1..=1),
        Region::new("pkg", 1..=1),
        Region::new("lib.alpha", 1..=1),
    ];

    let answer = engine.analyze_change(&regions, Limits::new(3));

    assert_eq!(answer.change.changed_symbols, 1);
    assert_eq!(answer.change.unheld_regions, 2);
    assert_eq!(
        answer.directory_regions, 2,
        "pkg, which no language claims, and lib.alpha, whose name the alpha graph claims"
    );
    assert_eq!(answer.unclaimed_regions, 0);
}

#[cfg(unix)]
#[test]
fn a_symlink_names_the_held_entry_its_target_points_to() {
    let tree = tree(&[
        ("a.alpha", "def a\n"),
        ("pkg/a.alpha", "def a\n"),
        ("pkg/sub/b.alpha", "def b\n"),
    ]);
    let root = tree.path();
    let absolute = root.join("pkg/a.alpha").display().to_string();
    let canonical = fs::canonicalize(root).expect("the root");
    let through_canonical = canonical.join("pkg/a.alpha").display().to_string();
    let links = [
        (
            "canonical.alpha",
            through_canonical.as_str(),
            Some("pkg/a.alpha"),
        ),
        ("link.alpha", "pkg/a.alpha", Some("pkg/a.alpha")),
        ("pkg/sub/up.alpha", "../a.alpha", Some("pkg/a.alpha")),
        ("pkg/here.alpha", "./sub/../a.alpha", Some("pkg/a.alpha")),
        ("dir", "pkg/sub", Some("pkg/sub")),
        ("chained.alpha", "link.alpha", Some("link.alpha")),
        ("absolute.alpha", absolute.as_str(), Some("pkg/a.alpha")),
        ("through.alpha", "dir/../a.alpha", None),
        ("inside.alpha", "dir/b.alpha", None),
        ("outside.alpha", "../a.alpha", None),
        ("gone.alpha", "pkg/none.alpha", None),
    ];
    for (link, target, _) in links {
        std::os::unix::fs::symlink(target, root.join(link)).expect("a symlink");
    }
    let engine = build(
        root,
        &BuildOptions::default(),
        vec![Box::new(Defs::new("alpha", "first", ".alpha"))],
    );

    for (link, target, expected) in links {
        let symlink = engine.symlink(link).expect("a held symlink");
        let resolved = symlink.resolved.as_ref().map(kartta::Entry::path);
        assert_eq!(symlink.target, Some(target), "{link}");
        assert_eq!(resolved, expected, "{link}");
        if let Some(resolved) = resolved {
            assert_eq!(
                fs::canonicalize(root.join(link)).expect("a link the kernel follows"),
                fs::canonicalize(canonical.join(resolved)).expect("the resolved entry"),
                "{link} resolves where the kernel's own lookup lands"
            );
        }
    }
    assert_eq!(
        fs::canonicalize(root.join("through.alpha")).expect("through.alpha"),
        canonical.join("pkg/a.alpha"),
        "the kernel reads `dir/..` as pkg, where a lexical reading of the target names the root's a.alpha"
    );
    assert!(engine.symlink("a.alpha").is_none(), "a file is no symlink");
    assert!(
        engine.symlink("absent.alpha").is_none(),
        "nor is a path the index lacks"
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

    let change = engine.analyze_change(&regions, Limits::new(1)).change;
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
    let (_, graph) = engine.graph_of("a.defs").expect("defs claims the file");
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
    let (_, graph) = engine.graph_of("c.defs").expect("defs claims the file");
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
    let fingerprint = [&b"defs 1"[..], kartta_lang::FINGERPRINT].concat();
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
    let (_, graph) = engine.graph_of("a.defs").expect("defs claims the file");
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

    let stopped = match outcome.expect_err("the cancel stopped the build") {
        BuildError::Stopped(stopped) => Some(stopped),
        _ => None,
    }
    .expect("a cancelled build hands back what it made");
    assert_eq!(stopped.unvisited, 0, "the walk saw every entry");
    assert!(stopped.index.entry("a.defs").is_some());
}

#[test]
fn a_build_cancelled_after_its_walk_keeps_the_content_index_it_finished() {
    let tree = tree(&[("a.defs", "def parse\n")]);
    let cancel = CancelToken::new();
    let mut options = BuildOptions::default();
    options.content_index = Some(ContentIndexStorage::Memory);

    let outcome = Engine::build(
        tree.path(),
        &options,
        vec![Box::new(Cancelling(
            cancel.clone(),
            CancelAt::ResolverBuilt,
        ))],
        Some(&cancel),
    );

    let stopped = match outcome.expect_err("the cancel stopped the build") {
        BuildError::Stopped(stopped) => Some(stopped),
        _ => None,
    }
    .expect("a cancelled build hands back what it made");
    let held = stopped.index.content_index_stats();
    assert_eq!(held.map(|held| held.files()), Some(1));
}

#[test]
fn a_build_cancelled_between_batches_hands_back_the_read_failures_of_those_it_resolved() {
    let names: Vec<(String, String)> = (0..1025)
        .map(|at| (format!("f{at:04}.defs"), "def f\n".to_owned()))
        .collect();
    let spec: Vec<(&str, &str)> = names
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .collect();
    let tree = tree(&spec);
    let cache = tempfile::tempdir().expect("create cache dir");
    let fingerprint = [&b"cancelling 1"[..], kartta_lang::FINGERPRINT].concat();
    let namespace = Namespace::new("graph/cancelling", &fingerprint);
    Index::build_with(
        tree.path(),
        &cached(cache.path()),
        Some(&Junk(namespace)),
        None,
    )
    .expect("fill the cache");
    let cancel = CancelToken::new();
    let gone = tree.path().join("f0000.defs");

    let outcome = Engine::build(
        tree.path(),
        &cached(cache.path()),
        vec![Box::new(Cancelling(
            cancel.clone(),
            CancelAt::FileResolvedWith(gone.clone()),
        ))],
        Some(&cancel),
    );

    let stopped = match outcome.expect_err("the cancel stopped the build") {
        BuildError::Stopped(stopped) => Some(stopped),
        _ => None,
    }
    .expect("a cancelled build hands back what it made");
    let failed: Vec<_> = stopped
        .errors
        .iter()
        .filter_map(BuildError::native_path)
        .collect();
    assert_eq!(failed, [gone.as_path()]);
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

    let engine = outcome.expect("every batch was resolved").engine;
    let (_, graph) = engine
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

    let (_, graph) = engine.graph_of("f0.defs").expect("defs claims the file");
    assert_eq!((graph.stats().files, graph.stats().symbols), (4100, 4100));
}

#[cfg(feature = "watch")]
#[test]
fn a_watched_build_without_stamps_is_refused_before_it_walks() {
    let tree = tree(&[]);
    let missing = tree.path().join("missing");
    let languages: Vec<Box<dyn Language>> = vec![Box::new(Defs::new("defs", "defs", ".defs"))];

    let built = Engine::build_watched(&missing, &BuildOptions::default(), languages, None);

    assert!(
        matches!(built, Err(BuildError::WatchWithoutStamps)),
        "{built:?}"
    );
}

#[cfg(all(feature = "watch", target_os = "linux"))]
#[test]
fn a_refresh_past_the_limit_under_a_watcher_watches_what_it_takes_in() {
    let tree = tree(&[
        ("sub/.gitignore", "out/\n"),
        ("sub/out/deep/old.defs", "def old\n"),
    ]);
    let mut options = BuildOptions::default();
    options.stamps = true;
    let languages: Vec<Box<dyn Language>> = vec![Box::new(Defs::new("defs", "defs", ".defs"))];
    let (outcome, watcher) =
        Engine::build_watched(tree.path(), &options, languages, None).expect("build the engine");
    let (mut engine, mut watcher) = (outcome.engine, watcher.expect("start watching"));
    let limit = engine.refresh_paths_limit();

    fs::write(tree.path().join("sub/.gitignore"), "").expect("show sub/out");
    for at in 0..limit {
        fs::create_dir(tree.path().join(format!("new{at}"))).expect("make a directory");
    }
    let changes = watcher.drain().expect("drain");
    let (refresh, followed) = engine
        .refresh_paths_watched(changes.iter().map(String::as_str), &mut watcher, None)
        .expect("refresh what changed");
    followed.expect("watch what the refresh took in");
    fs::write(tree.path().join("sub/out/deep/new.defs"), "def new\n").expect("write a file");
    let seen = watcher.drain().expect("drain");

    assert!(refresh.widened.is_some(), "{:?}", refresh.widened);
    assert_eq!(seen, ["sub/out/deep/new.defs"]);
}
