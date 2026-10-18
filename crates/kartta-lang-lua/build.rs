//! Hands the crate the identity of its own source, which the extraction's fingerprint folds in.

fn main() -> std::io::Result<()> {
    kartta_lang::fingerprint::set_source_id()
}
