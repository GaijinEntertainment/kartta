//! The identity of a crate's code, which a language's [fingerprint](crate::Language::fingerprint) folds in so that it
//! changes with the code that extracts a file, with no version kept by hand.
//!
//! A language crate hands its build the identity from its build script, which depends on this crate for it:
//!
//! ```no_run
//! fn main() -> std::io::Result<()> {
//!     kartta_lang::fingerprint::set_source_id()
//! }
//! ```
//!
//! and reads it with `env!("SOURCE_ID")`.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Sets `SOURCE_ID` in the compile-time environment of the crate whose build script calls it.
///
/// The identity is the crate's name, its version and a digest of the files under its `src`, and Cargo runs the script
/// again when one of them changes.
///
/// # Errors
///
/// The error of reading a file under `src`, of a variable Cargo did not set, or of writing to Cargo.
pub fn set_source_id() -> io::Result<()> {
    let root = Path::new(&var("CARGO_MANIFEST_DIR")?).join("src");
    let mut files = Vec::new();
    collect(&root, &mut files)?;
    files.sort();

    let mut digest = OFFSET;
    for file in &files {
        let relative = file.strip_prefix(&root).unwrap_or(file);
        digest = fnv(digest, relative.to_string_lossy().as_bytes());
        digest = fnv(digest, &[0]);
        digest = fnv(digest, &fs::read(file)?);
        digest = fnv(digest, &[0xff]);
    }

    let mut cargo = io::stdout().lock();
    writeln!(cargo, "cargo:rerun-if-changed=src")?;
    writeln!(
        cargo,
        "cargo:rustc-env=SOURCE_ID={} {} {digest:016x}",
        var("CARGO_PKG_NAME")?.to_string_lossy(),
        var("CARGO_PKG_VERSION")?.to_string_lossy()
    )
}

fn var(name: &str) -> io::Result<OsString> {
    env::var_os(name).ok_or_else(|| io::Error::other(format!("cargo did not set {name}")))
}

pub(crate) const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

pub(crate) fn fnv(digest: u64, bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    bytes.iter().fold(digest, |digest, &byte| {
        (digest ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }

    Ok(())
}
