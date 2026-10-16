//! Digests the crate's own sources into its extraction fingerprint, so a change to the code that extracts a file
//! names a new cache namespace without a version kept by hand.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[path = "src/fnv.rs"]
mod fnv;

fn main() -> io::Result<()> {
    let root = Path::new("src");
    let mut sources = Vec::new();
    collect(root, root, &mut sources)?;
    sources.sort();

    let mut digest = fnv::OFFSET_BASIS;
    for (relative, path) in &sources {
        let content = fs::read(path)?;
        digest = fnv::fold(digest, relative.as_bytes());
        digest = fnv::fold(digest, &[0]);
        digest = fnv::fold(digest, &(content.len() as u64).to_le_bytes());
        digest = fnv::fold(digest, &content);
    }

    let out = env::var_os("OUT_DIR")
        .ok_or_else(|| io::Error::other("cargo sets OUT_DIR for a build script"))?;
    fs::write(
        Path::new(&out).join("source_digest.rs"),
        format!(
            "const SOURCE_DIGEST: [u8; 8] = {:?};\n",
            digest.to_le_bytes()
        ),
    )?;
    println!("cargo::rerun-if-changed=src");
    Ok(())
}

fn collect(root: &Path, dir: &Path, sources: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, sources)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            let relative: Vec<_> = relative
                .iter()
                .map(|component| component.to_string_lossy())
                .collect();
            sources.push((relative.join("/"), path));
        }
    }
    Ok(())
}
