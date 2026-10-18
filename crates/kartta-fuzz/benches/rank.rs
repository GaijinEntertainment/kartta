//! Regression benchmark: how long ranking a repository-sized path list takes, across query selectivity.
//!
//! Cost is dominated by how many candidates survive the subsequence prefilter, not by how many are supplied, so the
//! arms span the range: a single character almost everything matches, a word most things do not, and a query nothing
//! matches at all, which measures the prefilter alone.
//!
//! The corpus is generated and deterministic, so criterion's run-to-run comparison is meaningful. It is also the same
//! corpus the profiler runs against — `cargo bench --bench rank -- --profile-time 10` under a sampling profiler
//! reports where the time goes without criterion's statistics in the way.
use std::hint::black_box;
use std::num::NonZeroUsize;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use kartta_fuzz::{Ranked, Ranking};

/// Corpus size. Large enough that per-candidate costs dominate setup and that the profile resembles a real repository,
/// small enough that the whole suite runs in well under a minute.
const CORPUS_LEN: usize = 250_000;

/// Queries spanning the selectivity range, each with the share of the corpus it is expected to keep.
const QUERIES: &[&str] = &[
    "e",             // nearly everything survives: the alignment dominates
    "ren",           // a large fraction survives
    "render",        // a moderate fraction
    "render_pass",   // selective
    "engine render", // two terms, each scored separately
    "qqzz",          // nothing survives: the prefilter alone
];

/// Builds a deterministic path corpus shaped like a repository: two to four directory levels, `snake_case` and
/// `camelCase` file stems, a realistic spread of extensions and name lengths.
///
/// Generated rather than committed because a corpus large enough to be representative is too large to keep in the
/// repository, and a smaller one would profile the wrong thing.
fn corpus(len: usize) -> Vec<String> {
    const TOP: &[&str] = &[
        "src", "crates", "internal", "pkg", "cmd", "tests", "docs", "vendor", "lib", "tools",
    ];
    const MID: &[&str] = &[
        "engine",
        "renderer",
        "network",
        "physics",
        "audio",
        "input",
        "asset_pipeline",
        "scene_graph",
        "shader_cache",
        "memory",
        "config",
        "parser",
        "codec",
        "client",
        "server",
        "telemetry",
    ];
    const STEM: &[&str] = &[
        "mod",
        "lib",
        "main",
        "handler",
        "service",
        "model",
        "types",
        "utils",
        "index",
        "render_pass",
        "state_machine",
        "fooBar",
        "initContext",
        "registry",
        "descriptor_set",
        "commandBuffer",
    ];
    const EXT: &[&str] = &[
        "rs", "go", "cpp", "hpp", "toml", "json", "md", "py", "ts", "nut", "das",
    ];

    // A small linear congruential generator keeps the corpus deterministic without a dependency.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(state >> 33).unwrap_or(0) % bound
    };

    (0..len)
        .map(|i| {
            let depth = 2 + next(3);
            let mut path = String::with_capacity(64);
            path.push_str(TOP[next(TOP.len())]);
            for _ in 1..depth {
                path.push('/');
                path.push_str(MID[next(MID.len())]);
            }
            path.push('/');
            path.push_str(STEM[next(STEM.len())]);
            if i % 3 == 0 {
                path.push('_');
                path.push_str(&(i % 997).to_string());
            }
            path.push('.');
            path.push_str(EXT[next(EXT.len())]);
            path
        })
        .collect()
}

/// Ranks `candidates` against `query` the way a caller does: a fresh ranking, every candidate offered by position.
fn ranked(candidates: &[&str], query: &str, limit: Option<NonZeroUsize>) -> Vec<Ranked> {
    let mut ranking = Ranking::new(query, limit);
    for (position, candidate) in candidates.iter().enumerate() {
        if !ranking.offer(position, candidate) {
            break;
        }
    }
    ranking.finish()
}

fn rank(c: &mut Criterion) {
    let paths = corpus(CORPUS_LEN);
    let candidates: Vec<&str> = paths.iter().map(String::as_str).collect();

    let mut group = c.benchmark_group("rank");
    group.throughput(Throughput::Elements(CORPUS_LEN as u64));

    for &query in QUERIES {
        let survivors = ranked(&candidates, query, None).len();

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{query} ({survivors})")),
            &query,
            |b, &query| {
                b.iter(|| black_box(ranked(black_box(&candidates), black_box(query), None)));
            },
        );
    }

    group.finish();
}

/// The same corpus and queries, but with a `limit`, the way a caller actually calls it.
///
/// A tool answering a language model asks for a page of results, never the whole ranking, so this is the path that
/// matters in production — and the one where the bounded heap replaces collecting every survivor.
fn rank_limited(c: &mut Criterion) {
    let paths = corpus(CORPUS_LEN);
    let candidates: Vec<&str> = paths.iter().map(String::as_str).collect();
    let limit = NonZeroUsize::new(100);

    let mut group = c.benchmark_group("rank_limited");
    group.throughput(Throughput::Elements(CORPUS_LEN as u64));

    for &query in QUERIES {
        group.bench_with_input(BenchmarkId::from_parameter(query), &query, |b, &query| {
            b.iter(|| black_box(ranked(black_box(&candidates), black_box(query), limit)));
        });
    }

    group.finish();
}

criterion_group!(benches, rank, rank_limited);
criterion_main!(benches);
