//! Digests the crate's own source, which the crate exports as `FINGERPRINT`: the helpers a language extracts with and
//! the codec that encodes an extraction both shape what a cache holds for the same bytes.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=src");

    let manifest =
        env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| missing("CARGO_MANIFEST_DIR"))?;
    let out = env::var_os("OUT_DIR").ok_or_else(|| missing("OUT_DIR"))?;
    let root = Path::new(&manifest).join("src");

    let mut files = Vec::new();
    collect(&root, &mut files)?;
    files.sort();

    let mut digest = FNV_OFFSET;
    for file in &files {
        let relative = file.strip_prefix(&root).unwrap_or(file);
        digest = fnv(digest, relative.to_string_lossy().as_bytes());
        digest = fnv(digest, &[0]);
        digest = fnv(digest, &fs::read(file)?);
        digest = fnv(digest, &[0xff]);
    }

    fs::write(
        Path::new(&out).join("source_digest.rs"),
        format!(
            "const SOURCE_DIGEST: [u8; 8] = {:?};\n",
            digest.to_le_bytes()
        ),
    )
}

fn fnv(digest: u64, bytes: &[u8]) -> u64 {
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

fn missing(variable: &str) -> io::Error {
    io::Error::other(format!("cargo did not set {variable}"))
}
