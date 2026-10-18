//! How long ranking a repository-sized path list takes, across query selectivity, without and with a limit.

use std::hint::black_box;
use std::num::NonZeroUsize;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use kartta_fuzz::{Ranked, Ranking};

const CORPUS_LEN: usize = 250_000;

const QUERIES: &[&str] = &["e", "ren", "render", "render_pass", "engine render", "qqzz"];

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

    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut lcg = move |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(state >> 33).unwrap_or(0) % bound
    };

    (0..len)
        .map(|i| {
            let depth = 2 + lcg(3);
            let mut path = String::with_capacity(64);
            path.push_str(TOP[lcg(TOP.len())]);
            for _ in 1..depth {
                path.push('/');
                path.push_str(MID[lcg(MID.len())]);
            }
            path.push('/');
            path.push_str(STEM[lcg(STEM.len())]);
            if i % 3 == 0 {
                path.push('_');
                path.push_str(&(i % 997).to_string());
            }
            path.push('.');
            path.push_str(EXT[lcg(EXT.len())]);
            path
        })
        .collect()
}

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
