//! Builds an index of a real tree while reading file content, and times what the reads cost.
//!
//! ```text
//! cargo run --release --example read -- ~/some/repo walk [--all]
//! ```
//!
//! The mode is one of `none` — the build alone, `walk` — the build reading every wanted file during the walk, as
//! `Index::build_with` does, and `after` — the build alone and then a second pass reading the same files. The files
//! wanted are source files by extension, the set a code graph parses, or every file with `--all`.
#![expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "an operator-facing probe reports to stdout by definition, and setup it cannot complete has nothing to \
              report"
)]

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use kartta_fs::{BuildOptions, Content, ContentVisitor, EntryKind, Index};
use rayon::prelude::*;

const USAGE: &str = "usage: read <path> <none|walk|after> [--all]";
const PROBE: usize = 64 * 1024;

const SOURCES: &[&str] = &[
    "go", "cpp", "cc", "cxx", "h", "hpp", "hxx", "hh", "c", "inl", "inc", "das", "ts", "mts",
    "cts", "tsx", "js", "jsx", "mjs", "cjs", "sh", "bash", "blk", "lua", "php", "phtml", "pp",
    "py", "pyi", "nut", "rb", "rs",
];

struct Tally {
    all: bool,
    files: AtomicUsize,
    bytes: AtomicU64,
}

impl Tally {
    fn wanted(&self, path: &str) -> bool {
        self.all
            || path
                .rsplit_once('.')
                .is_some_and(|(_, extension)| SOURCES.contains(&extension))
    }

    fn count(&self, bytes: usize) {
        self.files.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

impl ContentVisitor for Tally {
    fn wants(&self, path: &str) -> bool {
        self.wanted(path)
    }

    fn visit(&self, content: Content<'_>) {
        self.count(content.bytes().len());
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect(USAGE));
    let mode = args
        .next()
        .filter(|mode| ["none", "walk", "after"].contains(&mode.as_str()))
        .expect(USAGE);
    let tally = Tally {
        all: args.next().is_some_and(|flag| flag == "--all"),
        files: AtomicUsize::new(0),
        bytes: AtomicU64::new(0),
    };

    let started = Instant::now();
    let outcome = if mode == "walk" {
        Index::build_with(&root, &BuildOptions::default(), Some(&tally), None)
    } else {
        Index::build(&root, &BuildOptions::default())
    }
    .expect("build the index");
    let built = started.elapsed();

    if mode == "after" {
        read_after(outcome.index(), &tally);
    }
    let elapsed = started.elapsed();

    println!(
        "{mode}: {} entries, build {built:.2?}, total {elapsed:.2?}, {} failures; read {} files, {} bytes",
        outcome.index().len(),
        outcome.errors().len(),
        tally.files.into_inner(),
        tally.bytes.into_inner(),
    );
}

fn read_after(index: &Index, tally: &Tally) {
    let binary = BuildOptions::default().binary_extensions;
    let wanted: Vec<usize> = index
        .entries()
        .enumerate()
        .filter(|(_, entry)| {
            entry.kind() == EntryKind::File
                && tally.wanted(entry.path())
                && !binary.matches(entry.path())
        })
        .map(|(position, _)| position)
        .collect();

    wanted.par_iter().for_each_init(
        || index.paths(),
        |paths, &position| {
            if let Some(len) = read_text(&index.root().join(paths.get(position))) {
                tally.count(len);
            }
        },
    );
}

#[expect(
    clippy::verbose_file_reads,
    reason = "the rest is read from the handle whose first 64 KiB are already in; fs::read would reopen the file and \
              read those again"
)]
fn read_text(path: &Path) -> Option<usize> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(PROBE as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.contains(&0) {
        return None;
    }
    if bytes.len() == PROBE {
        file.read_to_end(&mut bytes).ok()?;
    }

    Some(bytes.len())
}
