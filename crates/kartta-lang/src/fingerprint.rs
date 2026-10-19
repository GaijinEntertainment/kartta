//! The identity of a crate's code and of the dependency versions it builds against.
//!
//! A language's [fingerprint](crate::Language::fingerprint) folds it in, so that it changes with the code that extracts
//! a file, with no version kept by hand.
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
/// The identity is the crate's name, its version and a digest of three things: its `Cargo.toml`, the versions a
/// `Cargo.lock` in a directory above the crate resolves for its `[dependencies]`, and the files under its `src`. Cargo
/// runs the script again when one of them changes. The dependencies a language extracts through, its grammar and
/// tree-sitter, are among them, so updating either changes the identity, whether the manifest or only the lock moves.
/// A build that sees no lock, such as one from a registry, digests the manifest's version requirements alone.
///
/// # Errors
///
/// The error of reading the manifest, the lock or a file under `src`, of a variable Cargo did not set, or of writing
/// to Cargo.
pub fn set_source_id() -> io::Result<()> {
    let manifest_dir = PathBuf::from(var("CARGO_MANIFEST_DIR")?);
    let manifest = fs::read_to_string(manifest_dir.join("Cargo.toml"))?;
    let lock = manifest_dir
        .ancestors()
        .map(|dir| dir.join("Cargo.lock"))
        .find(|lock| lock.is_file());
    let resolved = match &lock {
        Some(lock) => resolved_versions(&manifest, &fs::read_to_string(lock)?),
        None => String::new(),
    };

    let root = manifest_dir.join("src");
    let mut files = Vec::new();
    collect(&root, &mut files)?;
    files.sort();
    let mut digest = head(&manifest, &resolved);
    for file in &files {
        let relative = file.strip_prefix(&root).unwrap_or(file);
        digest = fnv(digest, relative.to_string_lossy().as_bytes());
        digest = fnv(digest, &[0]);
        digest = fnv(digest, &fs::read(file)?);
        digest = fnv(digest, &[0xff]);
    }

    let mut cargo = io::stdout().lock();
    writeln!(cargo, "cargo:rerun-if-changed=Cargo.toml")?;
    writeln!(cargo, "cargo:rerun-if-changed=src")?;
    if let Some(lock) = &lock {
        writeln!(cargo, "cargo:rerun-if-changed={}", lock.display())?;
    }
    writeln!(
        cargo,
        "cargo:rustc-env=SOURCE_ID={} {} {digest:016x}",
        var("CARGO_PKG_NAME")?.to_string_lossy(),
        var("CARGO_PKG_VERSION")?.to_string_lossy()
    )
}

fn head(manifest: &str, resolved: &str) -> u64 {
    let digest = fnv(fnv(OFFSET, manifest.as_bytes()), &[0xff]);
    fnv(fnv(digest, resolved.as_bytes()), &[0xff])
}

// constraint: a dependency renamed through `package` is not matched, since the lock records the package's own name;
// no language crate renames its grammar or tree-sitter
fn resolved_versions(manifest: &str, lock: &str) -> String {
    let mut dependencies = Vec::new();
    let mut in_dependencies = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]";
            if let Some(name) = line
                .strip_prefix("[dependencies.")
                .and_then(|table| table.strip_suffix(']'))
            {
                dependencies.push(name.trim());
            }
        } else if in_dependencies
            && !line.starts_with('#')
            && let Some((name, _)) = line.split_once('=')
        {
            dependencies.push(name.trim());
        }
    }

    let mut resolved = String::new();
    let mut package = None;
    for line in lock.lines() {
        if let Some(name) = quoted(line, "name") {
            package = Some(name);
        } else if let Some(version) = quoted(line, "version")
            && let Some(name) = package.take()
            && dependencies.contains(&name)
        {
            resolved.push_str(name);
            resolved.push(' ');
            resolved.push_str(version);
            resolved.push('\n');
        }
    }
    resolved
}

fn quoted<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')
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

#[cfg(test)]
mod tests {
    use super::{head, resolved_versions};

    const MANIFEST: &str = "[package]\nname = \"kartta-lang-cpp\"\n\n[dependencies]\n# the grammar\n\
        tree-sitter = \"0.27.1\"\n\n[dependencies.tree-sitter-cpp]\nversion = \"0.23.4\"\n\n\
        [dev-dependencies]\ntempfile = \"3.27.0\"\n";

    fn lock(runtime: &str, grammar: &str) -> String {
        format!(
            "version = 4\n\n[[package]]\nname = \"tempfile\"\nversion = \"3.27.0\"\n\n\
             [[package]]\nname = \"tree-sitter\"\nversion = \"{runtime}\"\nsource = \"registry\"\n\n\
             [[package]]\nname = \"tree-sitter-cpp\"\nversion = \"{grammar}\"\nchecksum = \"00\"\n"
        )
    }

    #[test]
    fn a_crate_s_source_identity_changes_with_the_grammar_version_its_manifest_pins() {
        assert_ne!(
            head(MANIFEST, ""),
            head(&MANIFEST.replace("0.23.4", "0.23.5"), ""),
            "a grammar bump with the crate's own code unchanged must change the identity"
        );
    }

    #[test]
    fn the_lock_resolves_each_dependency_the_manifest_names_and_no_other() {
        assert_eq!(
            resolved_versions(MANIFEST, &lock("0.27.1", "0.23.4")),
            "tree-sitter 0.27.1\ntree-sitter-cpp 0.23.4\n",
            "a table-form dependency is named too, and the dev-dependency and the lock's own version are left out"
        );
    }

    #[test]
    fn an_update_only_the_lock_records_changes_the_identity() {
        let identity = |runtime: &str, grammar: &str| {
            head(
                MANIFEST,
                &resolved_versions(MANIFEST, &lock(runtime, grammar)),
            )
        };

        assert_ne!(
            identity("0.27.1", "0.23.4"),
            identity("0.27.1", "0.23.5"),
            "the grammar"
        );
        assert_ne!(
            identity("0.27.1", "0.23.4"),
            identity("0.27.2", "0.23.4"),
            "tree-sitter"
        );
    }
}
