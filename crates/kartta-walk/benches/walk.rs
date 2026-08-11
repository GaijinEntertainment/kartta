//! Regression benchmark: how long this crate takes to reach a tree, across thread counts.
//!
//! The fixture is generated and deterministic, so criterion's run-to-run comparison is meaningful. It is also small,
//! which makes it useless for judging magnitude — `examples/survey.rs` exists for that, against a real tree.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and a benchmark target is built without \
              cfg(test) — a benchmark that cannot set itself up has nothing to measure, so failing loudly is right"
)]

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use kartta_walk::{Dir, Entry, EntryKind, Policy, Visit, WalkOptions, WalkOutcome, walk};
use tempfile::TempDir;

/// Directories per level and files per directory in the generated tree. Three levels of eight give 584 directories and
/// 7020 files — 7604 entries in total, enough that parallelism has something to divide and small enough to generate in a
/// second. Criterion's element throughput is reported against that count.
const FANOUT: usize = 8;
const DEPTH: usize = 3;
const FILES: usize = 12;

/// What a consumer records per entry: the kind, and a size for a regular file whose metadata could be read.
///
/// The size is observed by every arm, because a size nobody reads is a size the measurement cannot claim to have
/// gathered.
#[derive(Clone, Copy, Debug)]
struct Record {
    kind: EntryKind,
    size: Option<u64>,
}

/// What every arm returns, so both halves of each [`Record`] are read rather than merely produced.
#[derive(Debug)]
struct Summary {
    entries: usize,
    dirs: usize,
    bytes: u64,
}

fn summarize(outcome: &WalkOutcome<Record>) -> Summary {
    let tree = outcome.tree();
    let mut dirs = 0;
    let mut bytes = 0;

    for node in tree.nodes() {
        let record = *node.value();

        if record.kind == EntryKind::Dir {
            dirs += 1;
        }
        bytes += record.size.unwrap_or(0);
    }

    Summary {
        entries: tree.len(),
        dirs,
        bytes,
    }
}

/// Records [`Record`] for every entry, descending everywhere — the shape of work a real consumer asks for, including one
/// stat per regular file.
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

/// The tree to walk: `KARTTA_WALK_BENCH_DIR` when set, otherwise a generated one whose guard keeps it alive.
fn bench_root() -> (PathBuf, Option<TempDir>) {
    if let Some(dir) = std::env::var_os("KARTTA_WALK_BENCH_DIR") {
        return (PathBuf::from(dir), None);
    }

    let temp = tempfile::tempdir().expect("create temp dir");
    generate(temp.path(), DEPTH);
    let root = temp.path().to_path_buf();

    (root, Some(temp))
}

/// Writes `FILES` files into `dir`, then `FANOUT` subdirectories built the same way until `depth` runs out.
fn generate(dir: &Path, depth: usize) {
    for file in 0..FILES {
        let path = dir.join(format!("file{file:02}.txt"));
        std::fs::write(path, "x".repeat(file * 8)).expect("write bench file");
    }

    if depth == 0 {
        return;
    }

    for sub in 0..FANOUT {
        let path = dir.join(format!("dir{sub:02}"));
        std::fs::create_dir(&path).expect("create bench directory");
        generate(&path, depth - 1);
    }
}

fn bench_walk(c: &mut Criterion) {
    let (root, _guard) = bench_root();

    let probe = walk(&root, &Sizes, &(), &WalkOptions::default()).expect("probe walk");
    assert!(probe.errors().is_empty(), "the bench tree walks cleanly");

    // Pins the counts the constants above claim, so a change to the generator cannot quietly move the goalposts. Skipped
    // for a caller-supplied tree, whose shape is unknown.
    let summary = summarize(&probe);
    if std::env::var_os("KARTTA_WALK_BENCH_DIR").is_none() {
        let dirs = FANOUT + FANOUT.pow(2) + FANOUT.pow(3);
        assert_eq!(summary.dirs, dirs);
        assert_eq!(summary.entries, dirs + FILES * (dirs + 1));
        assert!(summary.bytes > 0, "the fixture files have content");
    }

    let mut group = c.benchmark_group("walk");
    group.throughput(Throughput::Elements(summary.entries as u64));

    group.bench_function("ambient-pool", |bencher| {
        bencher.iter(|| {
            let outcome =
                walk(black_box(&root), &Sizes, &(), &WalkOptions::default()).expect("walk");

            black_box(summarize(&outcome))
        });
    });

    // With `threads` set, each walk builds its own pool — the cost a caller in a restricted environment actually pays.
    for threads in [1_usize, 2, 4, 8] {
        let opts = WalkOptions {
            threads: NonZeroUsize::new(threads),
            ..Default::default()
        };

        group.bench_with_input(
            BenchmarkId::new("threads", threads),
            &opts,
            |bencher, opts| {
                bencher.iter(|| {
                    let outcome = walk(black_box(&root), &Sizes, &(), opts).expect("walk");

                    black_box(summarize(&outcome))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_walk);
criterion_main!(benches);
