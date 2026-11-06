//! DSHL, the shader language of the Dagor Engine, for the kartta code graph: the shaders, blocks, macros and
//! variables of shader source, the macro invocations between them and the files they include.
//!
//! [`Dshl`] implements [`Language`] over the `tree-sitter-dshl` grammar. It claims every file whose extension is
//! `dshl`, in any case.
//!
//! # What a file extracts to
//!
//! - A `shader`, each of its names, is a class, a `block` an interface, a `macro` a macro, and a render pass, a render
//!   pass group and a framebuffer layout each a class. Each is qualified by its own name: DSHL has one namespace per
//!   compilation, which its includes join. A macro a body declares belongs to no declaration, since the shader compiler
//!   takes every definition into one table. A `define_macro_if_not_defined` is a macro flagged as a fallback: the build
//!   skips it where a game defines the macro first, so a call bound to it may run the game's own.
//! - A variable and a sampler at file scope are variables: the global variables the engine sets by name. Inside a
//!   shader, a block or a macro, a variable is a field of the declaration around it, and a `local` variable is a
//!   temporary and no symbol. A preshader binding, `tex@smp2d = frame_tex`, is a field of its declaration, a `bool` a
//!   constant, and an `interval` an enum whose ranges are its variants.
//! - A `supports` statement makes each block it names a parent of the shader or block around it, of every name of a
//!   shader with several, the way a type implements an interface, and is a use of that block.
//! - A macro invocation, in a body or in a condition, is a call of the macro, made by the declaration around it. Only
//!   a macro is called: a call reaches no variable, constant or field. An invocation at file scope is a call from file
//!   scope, whose name the graph keeps. A call counts what its top-level commas split, and a macro takes exactly its
//!   parameters, since the shader compiler refuses any other count.
//! - A name a condition, an `assume`, a `##if`, `##bool` or `##declare` directive, a preshader value or a variable's
//!   initial value reads is a use, attributed to the declaration around it; so is a shader a `shader ==` condition
//!   names and a render pass a `pass` statement names. Keyword-like values — a sampler state, a render state, a
//!   channel mask — intrinsics and hardware features name nothing the script declares. The range a comparison tests an
//!   interval against, `quality == high`, is no use either: the graph binds no use to an enum variant.
//! - The text of an `hlsl { }` block is HLSL, which the grammar keeps as text: no HLSL declaration becomes a symbol,
//!   and only its `#include` lines are read, as includes.
//!
//! # Resolution
//!
//! The shader compiler looks an `include` up under the include paths of the build configuration that compiles the
//! file, and an HLSL `#include` the same way. Which configuration compiles a file is the build's, not the
//! repository's, so [`Dshl::resolver`] matches a spelling against the files whose path ends in it, and of several
//! takes the one sharing the most directories with the including file; two equally near resolve to neither. A spelling
//! that climbs, `../common.dshl`, is taken relative to the including file. An include that names no file of the
//! repository is kept as an import.
//!
//! A use binds what its own file declares, else what the one file it includes directly declares, else what the one
//! file those include in turn declares, as [`ValueUses::ImportedThenReached`] says; a name two files of one such step
//! declare binds neither. An `interval` names the ranges of the global of its name, so a read it would take, or tie
//! on, binds the one variable of the name those files hold, where they hold one. A use all that leaves unbound binds
//! the one file that declares the name of all the scripts its file compiles in, since an included file reads what its
//! includers declare, and of several the copy nearest the use, since a game keeps its own copy of a global's file.
//!
//! A macro call binds what its own file declares, else the one file it includes directly, else the one file those
//! include in turn, as [`IncludeReach::Script`] says, unless a copy of the macro sits nearer the call, else the macro
//! the tiers by name find. Each step reads only the macros that take the call's arguments, and a call none takes binds
//! nothing.
//!
//! # Fingerprint
//!
//! [`Dshl::fingerprint`] identifies this crate's name and version, a digest of its source, and the grammar: its
//! version, ABI version, node types, and the counts of its node kinds, parse states and fields. A change to
//! `kartta-lang` is covered by `kartta_lang::FINGERPRINT`, which a cache keys beside it. It cannot see a grammar built
//! from changed sources that keep its version and tables, nor a change to the tree-sitter runtime that keeps its
//! version.
//!
//! # Examples
//!
//! ```
//! use kartta_lang::Language;
//! use kartta_lang_dshl::Dshl;
//!
//! let dshl = Dshl::new();
//! assert!(dshl.claims("prog/shaders/water.dshl"));
//!
//! let raw = dshl.extract(b"texture frame_tex;\nshader water { supports global_frame; }\n");
//! let names: Vec<&str> = raw.symbols.iter().map(|symbol| symbol.qualified_name.as_str()).collect();
//! assert_eq!(names, ["frame_tex", "water"]);
//! assert_eq!(raw.symbols[1].parents, ["global_frame"]);
//! ```

use kartta_lang::path::has_extension;
use kartta_lang::{
    Conventions, DocComments, IncludeReach, Language, OwnMembers, RawExtraction, Repository,
    Resolver, SymbolKind, ValueUses,
};

use crate::resolve::DshlResolver;

mod resolve;
mod walk;

/// The DSHL language.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Dshl;

impl Dshl {
    /// The DSHL language.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

const CONVENTIONS: Conventions = {
    let mut conventions = Conventions::DEFAULT;
    conventions.own_members = OwnMembers::Unreached;
    conventions.values_are_call_targets = false;
    conventions.unrecorded_data = &[SymbolKind::Field];
    conventions.value_uses = ValueUses::ImportedThenReached;
    conventions.include_reach = IncludeReach::Script;
    conventions.includers =
        Some("the files whose include, or #include inside an hlsl block, names this file");
    conventions
};

impl Language for Dshl {
    fn name(&self) -> &'static str {
        "dshl"
    }

    fn claims(&self, path: &str) -> bool {
        has_extension(path, &["dshl"])
    }

    fn fingerprint(&self) -> &[u8] {
        kartta_lang::tree_sitter_fingerprint!(grammar(), tree_sitter_dshl::NODE_TYPES)
    }

    fn doc_comments(&self) -> DocComments {
        DocComments::C_LIKE
    }

    fn conventions(&self) -> Conventions {
        CONVENTIONS
    }

    fn extract(&self, source: &[u8]) -> RawExtraction {
        kartta_lang::tree::extract(&grammar(), source, walk::raw_walk)
    }

    fn resolver(&self, _repository: &dyn Repository) -> Box<dyn Resolver> {
        Box::new(DshlResolver)
    }
}

fn grammar() -> tree_sitter::Language {
    tree_sitter_dshl::LANGUAGE.into()
}
