//! Digests the crate's own source, which the extraction's fingerprint folds in: any change to the code that walks a
//! Quirrel tree changes what the same bytes extract to.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

include!("src/fnv.rs");

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
