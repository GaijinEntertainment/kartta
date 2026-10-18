use std::sync::OnceLock;

use kartta_lang::RawExtraction;
use kartta_lang::path::base;
use kartta_lang::tree::parse;
use tree_sitter::Language;

use crate::walk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    TypeScript,
    Tsx,
    JavaScript,
}

impl Flavor {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
        }
    }

    const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::TypeScript => &["ts", "mts", "cts"],
            Self::Tsx => &["tsx"],
            Self::JavaScript => &["js", "jsx", "mjs", "cjs"],
        }
    }

    pub(crate) fn claims(self, path: &str) -> bool {
        base(path).rsplit_once('.').is_some_and(|(_, extension)| {
            (self.extensions().iter()).any(|own| extension.eq_ignore_ascii_case(own))
        })
    }

    pub(crate) fn extract(self, source: &[u8]) -> RawExtraction {
        parse(&self.grammar(), source)
            .map_or_else(RawExtraction::default, |tree| walk::walk(&tree, source))
    }

    pub(crate) fn fingerprint(self) -> &'static [u8] {
        static FINGERPRINTS: [OnceLock<Vec<u8>>; 3] = [const { OnceLock::new() }; 3];

        FINGERPRINTS[self as usize].get_or_init(|| {
            kartta_lang::tree::fingerprint(env!("SOURCE_ID"), &self.grammar(), self.node_types())
        })
    }

    fn grammar(self) -> Language {
        match self {
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        }
    }

    const fn node_types(self) -> &'static str {
        match self {
            Self::TypeScript => tree_sitter_typescript::TYPESCRIPT_NODE_TYPES,
            Self::Tsx => tree_sitter_typescript::TSX_NODE_TYPES,
            Self::JavaScript => tree_sitter_javascript::NODE_TYPES,
        }
    }
}
