use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::RawExtraction;
use kartta_lang::path::base;
use tree_sitter::{Language, Parser, Tree};

use crate::{fnv, walk};

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

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
        (self.parse(source)).map_or_else(RawExtraction::default, |tree| walk::walk(&tree, source))
    }

    fn parse(self, source: &[u8]) -> Option<Tree> {
        thread_local! {
            static PARSERS: RefCell<[Option<Parser>; 3]> = const { RefCell::new([None, None, None]) };
        }

        PARSERS.with_borrow_mut(|parsers| {
            let slot = &mut parsers[self as usize];
            if slot.is_none() {
                *slot = self.parser();
            }
            slot.as_mut()?.parse(source, None)
        })
    }

    pub(crate) fn fingerprint(self) -> &'static [u8] {
        static FINGERPRINTS: [OnceLock<[u8; 8]>; 3] = [const { OnceLock::new() }; 3];

        FINGERPRINTS[self as usize].get_or_init(|| self.digest())
    }

    fn parser(self) -> Option<Parser> {
        let mut parser = Parser::new();
        parser.set_language(&self.grammar()).ok()?;
        Some(parser)
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

    fn digest(self) -> [u8; 8] {
        let grammar = self.grammar();
        let identity: [&[u8]; 5] = [
            env!("CARGO_PKG_NAME").as_bytes(),
            env!("CARGO_PKG_VERSION").as_bytes(),
            self.name().as_bytes(),
            &SOURCE_DIGEST.to_le_bytes(),
            self.node_types().as_bytes(),
        ];
        let shape = [
            grammar.abi_version(),
            grammar.node_kind_count(),
            grammar.parse_state_count(),
            grammar.field_count(),
        ];
        let version = grammar.metadata().map_or([0; 3], |version| {
            [
                version.major_version,
                version.minor_version,
                version.patch_version,
            ]
        });

        let digest = identity
            .iter()
            .fold(fnv::OFFSET, |digest, field| fnv::field(digest, field));
        let digest = fnv::field(digest, &version);
        let digest = (shape.iter()).fold(digest, |digest, count| {
            fnv::field(digest, &count.to_le_bytes())
        });
        digest.to_le_bytes()
    }
}
