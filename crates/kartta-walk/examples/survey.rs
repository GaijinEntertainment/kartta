//! Surveys a real tree: how big it is, how much of it is readable, and how long it takes to walk at each thread count.
//!
//! `benches/walk.rs` catches regressions on a small generated fixture; this measures magnitude, which a small fixture
//! cannot tell you. Run it before trusting any synthetic tree to stand in for real work:
//!
//! ```text
//! cargo run --release --example survey -- ~/some/tree
//! ```
//!
//! Unreadable paths are reported rather than hidden, because a tree the walker cannot fully read produces timings that
//! describe the permissions as much as the walk.
#![expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "an operator-facing survey tool reports to stdout by definition, and setup it cannot complete has nothing \
              to report"
)]

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit, WalkOptions, WalkOutcome, walk};

/// What a consumer records per entry: the kind, and a size for a regular file whose metadata could be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Record {
    kind: EntryKind,
    size: Option<u64>,
}

struct Sizes;

impl Policy for Sizes {
    type State = ();
    type Node = Record;

    fn enter(&self, _dir: &Dir<'_>, _parent: &()) {}

    fn visit(&self, entry: &Entry<'_>, _state: &()) -> Visit<Record> {
        let kind = entry.kind();
        // Only a regular file is worth a stat, and only its size is kept.
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
        let opts = WalkOptions {
            threads: threads.and_then(NonZeroUsize::new),
            ..Default::default()
        };
        let label = threads.map_or_else(|| "ambient".to_owned(), |n| format!("{n:>2}"));

        let elapsed = best_of(3, || walk_once(&root, &opts));
        println!("  {label} threads: {elapsed:.2?}");
    }
}

/// Prints what the tree holds and how much of it could be read.
fn report_shape(root: &Path) {
    let outcome = walk(root, &Sizes, &(), &WalkOptions::default()).expect("walk the tree");

    let mut dirs = 0_usize;
    let mut files = 0_usize;
    let mut symlinks = 0_usize;
    let mut others = 0_usize;
    let mut unsized_files = 0_usize;
    let mut bytes = 0_u64;

    for node in outcome.tree().nodes() {
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
        outcome.tree().len()
    );
    println!(
        "  {} unreadable paths, {unsized_files} files whose size could not be read",
        outcome.errors().len()
    );

    if let Some(first) = outcome.errors().first() {
        println!("  first unreadable: {first}");
    }
}

/// One walk, reduced to the entry count so the tree is built and then observed.
fn walk_once(root: &Path, opts: &WalkOptions) -> usize {
    let outcome: WalkOutcome<Record> = walk(root, &Sizes, &(), opts).expect("walk the tree");

    outcome.tree().len()
}

/// The fastest of `runs` attempts. A real tree is shared with the rest of the machine, so the minimum is the honest
/// figure — the mean measures whatever else was running.
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
