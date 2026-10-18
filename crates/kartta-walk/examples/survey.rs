//! Surveys a real tree: how big it is, how much of it is readable, and how long it takes to walk at each thread count.
//!
//! This measures magnitude, which a small generated fixture cannot tell you. Run it before trusting any synthetic tree
//! to stand in for real work:
//!
//! ```text
//! cargo run --release --example survey -- ~/some/tree
//! ```
//!
//! Unreadable paths are reported rather than hidden, because a tree the walker cannot fully read produces timings that
//! describe the permissions as much as the walk. Each thread count reports the fastest of three walks: a real tree is
//! shared with the rest of the machine, so the minimum is the honest figure and a mean measures whatever else ran.
#![expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "an operator-facing survey tool reports to stdout by definition, and setup it cannot complete has nothing \
              to report"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit, WalkOptions, WalkOutcome, walk};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Record {
    kind: EntryKind,
    size: Option<u64>,
}

struct Sizes;

impl Policy for Sizes {
    type State = ();
    type Value = Record;

    fn enter(&self, _dir: &Dir<'_>, _parent_state: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<Record> {
        let kind = entry.kind();
        let size = if kind == EntryKind::File {
            entry.metadata().ok().map(|meta| meta.len())
        } else {
            None
        };

        Visit::Keep(Record { kind, size })
    }
}

fn main() {
    let root: PathBuf = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);

    println!("surveying {}", root.display());
    report_shape(&root);

    println!("  --- time to a name-sorted arena tree, best of 3 ---");

    for threads in [None, Some(1), Some(2), Some(4), Some(8), Some(16)] {
        let label = threads.map_or_else(|| "ambient".to_owned(), |n| format!("{n:>2}"));
        let timed = || best_of(3, || walk_once(&root));

        let elapsed = threads.map_or_else(timed, |threads| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("start the thread pool")
                .install(timed)
        });
        println!("  {label} threads: {elapsed:.2?}");
    }
}

fn report_shape(root: &Path) {
    let WalkOutcome { tree, errors, .. } =
        walk(root, &Sizes, &(), &WalkOptions::default()).expect("walk the tree");

    let mut dirs = 0_usize;
    let mut files = 0_usize;
    let mut symlinks = 0_usize;
    let mut others = 0_usize;
    let mut unsized_files = 0_usize;
    let mut bytes = 0_u64;

    for node in tree.nodes() {
        let record = *node.value();

        match record.kind {
            EntryKind::Dir => dirs += 1,
            EntryKind::Symlink => symlinks += 1,
            EntryKind::Other => others += 1,
            EntryKind::File => {
                files += 1;
                match record.size {
                    Some(size) => bytes += size,
                    None => unsized_files += 1,
                }
            }
        }
    }

    println!(
        "  {} entries: {dirs} directories, {files} files, {symlinks} symlinks, {others} other, {bytes} bytes",
        tree.len()
    );
    println!(
        "  {} unreadable paths, {unsized_files} files whose size could not be read",
        errors.len()
    );

    if let Some(first) = errors.first() {
        println!("  first unreadable: {first}");
    }
}

fn walk_once(root: &Path) -> usize {
    let outcome: WalkOutcome<Record> =
        walk(root, &Sizes, &(), &WalkOptions::default()).expect("walk the tree");

    outcome.tree.len()
}

fn best_of(runs: usize, mut once: impl FnMut() -> usize) -> Duration {
    let mut best = Duration::MAX;

    for _ in 0..runs {
        let start = Instant::now();
        let entries = once();
        let elapsed = start.elapsed();

        assert!(entries > 0, "the tree is empty; nothing to measure");
        best = best.min(elapsed);
    }

    best
}
