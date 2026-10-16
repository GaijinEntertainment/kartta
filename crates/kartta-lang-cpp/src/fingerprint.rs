use std::sync::OnceLock;

use tree_sitter::Language;

use crate::fnv;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

pub(crate) fn fingerprint() -> &'static [u8] {
    static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

    FINGERPRINT.get_or_init(|| {
        let grammar = Language::from(tree_sitter_cpp::LANGUAGE);
        let mut fingerprint = Vec::new();
        for text in [env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")] {
            fingerprint.extend_from_slice(text.as_bytes());
            fingerprint.push(0);
        }
        fingerprint.extend_from_slice(&SOURCE_DIGEST);
        if let Some(version) = grammar.metadata() {
            fingerprint.extend_from_slice(&[
                version.major_version,
                version.minor_version,
                version.patch_version,
            ]);
        }
        for word in [
            fnv::fold(fnv::OFFSET_BASIS, tree_sitter_cpp::NODE_TYPES.as_bytes()),
            grammar.abi_version() as u64,
            grammar.node_kind_count() as u64,
            grammar.parse_state_count() as u64,
            grammar.field_count() as u64,
        ] {
            fingerprint.extend_from_slice(&word.to_le_bytes());
        }
        fingerprint
    })
}
