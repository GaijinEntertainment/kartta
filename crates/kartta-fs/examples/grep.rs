//! Indexes a real tree and searches its content, timing each search and reporting where every candidate went.
//!
//! ```text
//! cargo run --release --example grep -- ~/some/repo 'fn main' [--glob '*.rs'] [--limit 100] [--count]
//! ```
//!
//! Each search runs twice, so the second shows what a warm page cache, and the sizes and binary verdicts the first search
//! kept, leave of it. Compare against `rg` over the same tree with the in-tree ignore rules only:
//!
//! ```text
//! rg --no-require-git --no-ignore-global --no-ignore-exclude --no-ignore-parent --hidden -g '!.git' --stats 'fn main'
//! ```
#![expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "an operator-facing probe reports to stdout by definition, and setup it cannot complete has nothing to \
              report"
)]

use std::path::PathBuf;
use std::time::Instant;

use kartta_fs::{BuildOptions, Case, Filter, GrepOptions, Index, Matches, Patterns};

struct Request {
    root: PathBuf,
    pattern: String,
    glob: Option<String>,
    limit: usize,
    count: bool,
}

const USAGE: &str = "usage: grep <path> <pattern> [--glob G] [--limit N] [--count]";

fn main() {
    let request = parse().expect(USAGE);

    let started = Instant::now();
    let outcome = Index::build(&request.root, &BuildOptions::default()).expect("build the index");
    println!(
        "{} entries indexed in {:.2?}, {} build failures",
        outcome.index.len(),
        started.elapsed(),
        outcome.errors.len()
    );
    let index = outcome.index;

    let patterns = Patterns::new([&request.pattern], Case::Sensitive).expect("compile the pattern");
    let filter = request
        .glob
        .as_ref()
        .map(|glob| Filter::new([glob]).expect("compile the glob"));
    let mut options = GrepOptions::default().limit(request.limit);
    if let Some(filter) = &filter {
        options = options.filter(filter);
    }
    if request.count {
        options = options.count_all();
    }

    for round in ["first", "second"] {
        let started = Instant::now();
        let found = index.grep(&patterns, &options);
        let elapsed = started.elapsed();
        println!("\n{round} search in {elapsed:.2?}");
        report(&found);
    }
}

fn parse() -> Option<Request> {
    let mut args = std::env::args().skip(1);
    let mut request = Request {
        root: PathBuf::from(args.next()?),
        pattern: args.next()?,
        glob: None,
        limit: 0,
        count: false,
    };

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--glob" => request.glob = Some(args.next()?),
            "--limit" => request.limit = args.next()?.parse().ok()?,
            "--count" => request.count = true,
            _ => return None,
        }
    }

    Some(request)
}

fn report(found: &Matches<'_>) {
    println!(
        "  {} matching lines in {} files; truncated {}, totals {:?} lines / {:?} files",
        found.lines(),
        found.files().len(),
        found.truncated(),
        found.total_lines(),
        found.total_files(),
    );
    println!(
        "  candidates {}: searched {}, binary {}, oversized {}, failed {}, unsearched {}; excluded {}, symlinks {}",
        found.candidates(),
        found.searched(),
        found.binary(),
        found.oversized(),
        found.failures().len(),
        found.unsearched(),
        found.excluded(),
        found.symlinks(),
    );
}
