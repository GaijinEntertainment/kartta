//! BLK for the kartta code graph: the includes of Dagor `DataBlock` text files.
//!
//! [`Blk`] implements [`Language`] over the `tree-sitter-blk` grammar. It claims every file whose extension is `blk`,
//! in any case.
//!
//! # What a file extracts to
//!
//! A BLK file declares no symbols and makes no calls: its blocks and parameters are data. Every `include` statement
//! with a path is a directive, whatever the case of the keyword and whether the path is quoted, its path kept as
//! written without its quotes. A file the engine loads as binary, which its header shows, extracts to nothing, and so
//! does a text that can spell neither the `include` keyword nor a `mountPoints` block name, which is not parsed.
//!
//! # Resolution
//!
//! An include names the file the engine loads for it, and one that names no file is an import, kept as written. The
//! path is read as the engine reads it: the `~` escapes of a quoted path decoded, the block comments and trailing
//! blanks of an unquoted one dropped, and `\` read as `/`.
//!
//! - A path relative to the including file names the file there.
//! - A `#` path is relative to a root the application sets, so it names the one file of the repository that ends with
//!   the path at a directory boundary, and no file when several do.
//! - A `%name/rest` path names `rest` below the directory a `mountPoints` block maps `name` to. The mount table is read
//!   from the first `mountPoints` block at the root of every file named `settings.blk`, `application.blk`,
//!   `aot_config.blk` or `libs_aot_config.blk`; a mount stated in a file of another name is not seen. A child block
//!   maps its name to its `forSource` directory, which names a directory below an ancestor of the stating file, and a
//!   `t` parameter maps its name to a directory relative to the stating file. The mappings stated nearest the
//!   including file decide, and they must place the same file.
//! - An absolute path, a drive-letter one such as `C:/x.blk` included, and a path relative to the working directory,
//!   `:/`, name no file.
//!
//! Every match is case-sensitive.
//!
//! # Fingerprint
//!
//! [`Blk::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to any of them
//! changes the fingerprint. A change to `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys
//! beside it. It cannot see a grammar built from changed sources that keep its version and tables — a patched external
//! scanner — nor a change to the tree-sitter runtime that keeps its version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_blk::Blk;
//!
//! let blk = Blk::new();
//! assert!(blk.claims("gameData/units/tank.blk"));
//!
//! let raw = blk.extract(b"include \"common.blk\"\nweapon{ include '#/develop/gun.blk' }\n");
//! let paths: Vec<&str> = raw.directives.iter().map(|directive| directive.spec.as_str()).collect();
//! assert_eq!(paths, ["common.blk", "#/develop/gun.blk"]);
//! ```

use std::cell::RefCell;
use std::sync::OnceLock;

use kartta_lang::path::base;
use kartta_lang::{Language, RawExtraction, Repository, Resolver};
use tree_sitter::{Parser, Tree};

use crate::fnv::{FNV_OFFSET, fnv};
use crate::resolve::BlkResolver;
use crate::text::source_text;

mod fnv;
mod mounts;
mod resolve;
mod text;
mod walk;

include!(concat!(env!("OUT_DIR"), "/source_digest.rs"));

const NODE_TYPES_DIGEST: u64 = fnv(FNV_OFFSET, tree_sitter_blk::NODE_TYPES.as_bytes());

/// The BLK language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Blk;

impl Blk {
    /// The BLK language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Language for Blk {
    fn name(&self) -> &'static str {
        "blk"
    }

    fn claims(&self, path: &str) -> bool {
        base(path)
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("blk"))
    }

    fn fingerprint(&self) -> &[u8] {
        static FINGERPRINT: OnceLock<Vec<u8>> = OnceLock::new();

        FINGERPRINT.get_or_init(fingerprint)
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        raw_extraction(source)
    }

    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r> {
        Box::new(BlkResolver::new(repository))
    }
}

fn raw_extraction(source: &[u8]) -> RawExtraction {
    let text = source_text(source);
    if text.is_empty() {
        return RawExtraction::default();
    }

    parse(text).map_or_else(RawExtraction::default, |tree| walk::raw_walk(&tree, text))
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_blk::LANGUAGE.into()
}

fn parse(source: &[u8]) -> Option<Tree> {
    thread_local! {
        static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
    }

    PARSER.with_borrow_mut(|parser| {
        if parser.is_none() {
            let mut fresh = Parser::new();
            fresh.set_language(&grammar()).ok()?;
            *parser = Some(fresh);
        }
        parser.as_mut()?.parse(source, None)
    })
}

fn fingerprint() -> Vec<u8> {
    let grammar = grammar();
    let version = grammar.metadata().map_or([0; 3], |version| {
        [
            version.major_version,
            version.minor_version,
            version.patch_version,
        ]
    });
    let counts = [
        grammar.abi_version(),
        grammar.node_kind_count(),
        grammar.parse_state_count(),
        grammar.field_count(),
    ];

    let mut bytes = Vec::new();
    for part in [env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")] {
        bytes.extend_from_slice(part.as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(&SOURCE_DIGEST);
    bytes.extend_from_slice(&NODE_TYPES_DIGEST.to_le_bytes());
    bytes.extend_from_slice(&version);
    for count in counts {
        bytes.extend_from_slice(&u64::try_from(count).unwrap_or(u64::MAX).to_le_bytes());
    }

    bytes
}
