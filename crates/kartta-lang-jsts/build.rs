//! Digests the crate's own sources, so the extraction fingerprint changes with the walk that produces it.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "src/fnv.rs"]
mod fnv;

fn main() -> Result<(), Box<dyn Error>> {
    let mut sources = Vec::new();
    collect_sources(Path::new("src"), &mut sources)?;
    sources.sort();

    let mut digest = fnv::OFFSET;
    for source in &sources {
        let relative = source.to_string_lossy().replace('\\', "/");
        digest = fnv::field(digest, relative.as_bytes());
        digest = fnv::field(digest, &fs::read(source)?);
    }

    let out =
        PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR for a build script")?);
    let [a, b, c, d] = [48, 32, 16, 0].map(|shift| (digest >> shift) & 0xffff);
    fs::write(
        out.join("source_digest.rs"),
        format!("const SOURCE_DIGEST: u64 = 0x{a:04x}_{b:04x}_{c:04x}_{d:04x};\n"),
    )?;

    println!("cargo:rerun-if-changed=src");
    Ok(())
}

fn collect_sources(dir: &Path, sources: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_sources(&path, sources)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
    Ok(())
}
