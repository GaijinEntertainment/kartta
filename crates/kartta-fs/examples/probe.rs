//! Indexes a real tree and queries it: how much survives the ignore rules, how long the build takes, and what a path
//! query costs once the snapshot exists.
//!
//! A synthetic fixture cannot tell you magnitude — a repository's `.gitignore` files are what decide how much of it is
//! indexed at all, and no generated tree has realistic ones. Run this before trusting any small tree to stand in for
//! real work:
//!
//! ```text
//! cargo run --release --example probe -- ~/some/repo main
//! ```
//!
//! Build failures are reported rather than hidden, because an index that could not read part of the tree answers
//! queries about the part it read as if that were all there is.
#![expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "an operator-facing probe reports to stdout by definition, and setup it cannot complete has nothing to \
              report"
)]

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use kartta_fs::{BuildOptions, EntryKind, Index};

/// How many ranked paths to print.
const SHOWN: usize = 10;

/// How many build failures to print before summarizing the rest.
const REPORTED_ERRORS: usize = 5;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().expect("usage: probe <path> [query]"));
    let query = args.next().map(|arg| arg.to_string_lossy().into_owned());

    let started = Instant::now();
    let outcome = Index::build(&root, &BuildOptions::default()).expect("build the index");
    let elapsed = started.elapsed();

    report_errors(&outcome);
    let index = outcome.into_index();

    let mut counts = [0usize; 4];
    for entry in index.entries() {
        counts[slot(entry.kind())] += 1;
    }

    println!("{} in {elapsed:.2?}", index.root().display());
    println!(
        "  {} entries: {} files, {} dirs, {} symlinks, {} other",
        index.len(),
        counts[slot(EntryKind::File)],
        counts[slot(EntryKind::Dir)],
        counts[slot(EntryKind::Symlink)],
        counts[slot(EntryKind::Other)],
    );

    report_sizes(&index);

    let Some(query) = query else {
        return;
    };

    let started = Instant::now();
    let ranked = index.fuzzy_paths(&query, None, NonZeroUsize::new(SHOWN));
    let elapsed = started.elapsed();

    println!("\n  {query:?} ranked in {elapsed:.2?}");
    for found in &ranked {
        println!("    {:>12}  {:?}", found.score, found.path);
    }
}

/// Totals every file's size, timed and labelled apart from the build.
///
/// This is a `stat` for every file in the tree, serial and cold, because a fresh index has read none of them yet. It
/// is reported as its own cost rather than folded into the build, which is precisely the point the lazy reads make:
/// the work is real, and a caller that never asks for a size never pays it.
fn report_sizes(index: &Index) {
    let started = Instant::now();
    let mut known_bytes = 0u64;
    let mut unknown_sizes = 0usize;

    for inspected in index.inspected() {
        match inspected.size() {
            Some(size) => known_bytes = known_bytes.saturating_add(size),
            None if inspected.entry().kind() == EntryKind::File => unknown_sizes += 1,
            None => {}
        }
    }

    println!(
        "  sizes read on demand in {:.2?}: {known_bytes} known bytes over {unknown_sizes} files of unknown size",
        started.elapsed()
    );
}

/// Where a kind's tally lives. A match rather than a cast, so a new kind fails to compile instead of miscounting.
const fn slot(kind: EntryKind) -> usize {
    match kind {
        EntryKind::File => 0,
        EntryKind::Dir => 1,
        EntryKind::Symlink => 2,
        EntryKind::Other => 3,
    }
}

fn report_errors(outcome: &kartta_fs::BuildOutcome) {
    let errors = outcome.errors();
    if errors.is_empty() {
        return;
    }

    println!("{} build failures, first {REPORTED_ERRORS}:", errors.len());
    for error in errors.iter().take(REPORTED_ERRORS) {
        print_escaped(error);
    }
    println!();
}

/// Prints one failure with every path it carries escaped.
///
/// A failure names a path the repository chose, and a name may hold a newline or a terminal escape sequence. `Display`
/// renders it raw, which lets a hostile name forge a line of this report or drive the terminal; the derived `Debug`
/// quotes and escapes each field instead.
fn print_escaped(error: &kartta_fs::BuildError) {
    println!("  {error:?}");
}
