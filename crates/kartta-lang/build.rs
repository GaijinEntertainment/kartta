//! Hands the crate its own source identity, which it exports as `FINGERPRINT`, through the code it exports for every
//! language crate's build script.

#[path = "src/fingerprint.rs"]
pub mod fingerprint;

fn main() -> std::io::Result<()> {
    fingerprint::set_source_id()
}
