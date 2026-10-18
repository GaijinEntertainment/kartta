//! Every language kartta ships, as one set to build an engine with, or a selection of them by name.
//!
//! kartta's engine names no language of its own: `kartta::Engine::build` takes the languages to build with, each a
//! value implementing [`Language`], and kartta's own sixteen live in fourteen language crates, `kartta-lang-jsts`
//! holding three. [`all`] gives every one of them in the order the engine should be handed them, and [`select`] gives
//! the ones a list names, for a consumer that takes the list from its own caller, so neither has to name those crates.
//!
//! The languages are the ones [`NAMES`] lists, each under the [name](Language::name) it reports:
//!
//! | Name         | Crate                 | Files                                                                    |
//! |--------------|-----------------------|--------------------------------------------------------------------------|
//! | `go`         | `kartta-lang-go`      | `.go`                                                                    |
//! | `cpp`        | `kartta-lang-cpp`     | `.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`, `.hpp`, `.hxx`, `.inl`, `.inc` |
//! | `typescript` | `kartta-lang-jsts`    | `.ts`, `.mts`, `.cts`                                                    |
//! | `tsx`        | `kartta-lang-jsts`    | `.tsx`                                                                   |
//! | `javascript` | `kartta-lang-jsts`    | `.js`, `.jsx`, `.mjs`, `.cjs`                                            |
//! | `daslang`    | `kartta-lang-daslang` | `.das`                                                                   |
//! | `quirrel`    | `kartta-lang-quirrel` | `.nut`                                                                   |
//! | `blk`        | `kartta-lang-blk`     | `.blk`                                                                   |
//! | `rust`       | `kartta-lang-rust`    | `.rs`                                                                    |
//! | `python`     | `kartta-lang-python`  | `.py`, `.pyi`                                                            |
//! | `bash`       | `kartta-lang-bash`    | `.sh`, `.bash`                                                           |
//! | `php`        | `kartta-lang-php`     | `.php`, `.phtml`                                                         |
//! | `ruby`       | `kartta-lang-ruby`    | `.rb`                                                                    |
//! | `lua`        | `kartta-lang-lua`     | `.lua`                                                                   |
//! | `puppet`     | `kartta-lang-puppet`  | `.pp`                                                                    |
//! | `dshl`       | `kartta-lang-dshl`    | `.dshl`                                                                  |
//!
//! Each claims its files by extension, in any case, and no two claim the same file. The engine hands a file to the
//! first language that claims it, so the order decides only the order the engine lists the graphs and counts of the
//! languages in, and that a consumer's own language put after them gets only the files none of them claims.
//!
//! Every grammar is compiled into a consumer of this crate whatever it selects; one that wants fewer grammars in its
//! build depends on the language crates it wants instead.
//!
//! # Examples
//!
//! ```
//! use kartta::{BuildOptions, Engine};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let tree = tempfile::tempdir()?;
//! std::fs::write(tree.path().join("main.go"), "package main\n\nfunc main() {}\n")?;
//! std::fs::write(tree.path().join("build.sh"), "make all\n")?;
//!
//! let engine = Engine::build(tree.path(), &BuildOptions::default(), kartta_languages::all(), None)?.into_engine();
//! assert!(engine.graph_of("main.go").is_some() && engine.graph_of("build.sh").is_some());
//!
//! let engine = Engine::build(tree.path(), &BuildOptions::default(), kartta_languages::select(["go"])?, None)?
//!     .into_engine();
//! assert!(engine.graph_of("main.go").is_some() && engine.graph_of("build.sh").is_none());
//! # Ok(())
//! # }
//! ```

use std::fmt;

use kartta_lang::Language;
use kartta_lang_bash::Bash;
use kartta_lang_blk::Blk;
use kartta_lang_cpp::Cpp;
use kartta_lang_daslang::Daslang;
use kartta_lang_dshl::Dshl;
use kartta_lang_go::Go;
use kartta_lang_jsts::{JavaScript, Tsx, TypeScript};
use kartta_lang_lua::Lua;
use kartta_lang_php::Php;
use kartta_lang_puppet::Puppet;
use kartta_lang_python::Python;
use kartta_lang_quirrel::Quirrel;
use kartta_lang_ruby::Ruby;
use kartta_lang_rust::Rust;

type Make = fn() -> Box<dyn Language>;

const LANGUAGES: [(&str, Make); 16] = [
    ("go", || Box::new(Go::new())),
    ("cpp", || Box::new(Cpp::new())),
    ("typescript", || Box::new(TypeScript::new())),
    ("tsx", || Box::new(Tsx::new())),
    ("javascript", || Box::new(JavaScript::new())),
    ("daslang", || Box::new(Daslang::new())),
    ("quirrel", || Box::new(Quirrel::new())),
    ("blk", || Box::new(Blk::new())),
    ("rust", || Box::new(Rust::new())),
    ("python", || Box::new(Python::new())),
    ("bash", || Box::new(Bash::new())),
    ("php", || Box::new(Php::new())),
    ("ruby", || Box::new(Ruby::new())),
    ("lua", || Box::new(Lua::new())),
    ("puppet", || Box::new(Puppet::new())),
    ("dshl", || Box::new(Dshl::new())),
];

/// The name of every language kartta ships, in the order [`all`] gives them: the names [`select`] takes.
pub const NAMES: &[&str] = &{
    let mut names = [""; LANGUAGES.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = LANGUAGES[at].0;
        at += 1;
    }
    names
};

/// Every language kartta ships, in the order of [`NAMES`].
///
/// Each language is a value without state, so building the set allocates only the vector that holds it.
#[must_use]
pub fn all() -> Vec<Box<dyn Language>> {
    LANGUAGES.iter().map(|(_, make)| make()).collect()
}

/// The languages `names` names, in the order of [`NAMES`] whatever order `names` gives them in, and each once however
/// often it is named.
///
/// A name is matched exactly, as [`NAMES`] spells it: `go`, not `Go` or `golang`. No name selects no language, which
/// builds an engine that indexes the tree and holds no graph.
///
/// TypeScript, TSX and JavaScript are three languages of one family, `jsts`, and each is selected by its own name.
/// Naming `typescript` without `tsx` builds the `.ts`, `.mts` and `.cts` files into the family's graph and leaves the
/// `.tsx` files out of every graph: the index still holds them, but nothing they define is in a graph, so a call from a
/// `.ts` file into a component of one binds to nothing and counts as unresolved. Naming any other part of the family
/// leaves the rest out the same way.
///
/// # Errors
///
/// [`UnknownLanguages`] when a name is not one of [`NAMES`], listing every such name; nothing is selected then.
pub fn select<I>(names: I) -> Result<Vec<Box<dyn Language>>, UnknownLanguages>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut selected = [false; LANGUAGES.len()];
    let mut unknown: Vec<String> = Vec::new();
    for name in names {
        let name = name.as_ref();
        match LANGUAGES.iter().position(|(known, _)| *known == name) {
            Some(at) => selected[at] = true,
            None if !unknown.iter().any(|seen| seen == name) => unknown.push(name.to_owned()),
            None => {}
        }
    }
    if !unknown.is_empty() {
        return Err(UnknownLanguages { names: unknown });
    }

    Ok(LANGUAGES
        .iter()
        .zip(selected)
        .filter(|&(_, wanted)| wanted)
        .map(|((_, make), _)| make())
        .collect())
}

/// The names given to [`select`] that are not among [`NAMES`]. Its message names each of them and lists [`NAMES`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnknownLanguages {
    names: Vec<String>,
}

impl UnknownLanguages {
    /// Each unknown name, once, in the order it was first given.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

impl fmt::Display for UnknownLanguages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let noun = if self.names.len() == 1 {
            "language"
        } else {
            "languages"
        };
        write!(f, "unknown {noun} ")?;
        for (at, name) in self.names.iter().enumerate() {
            let separator = if at == 0 { "" } else { ", " };
            write!(f, "{separator}{name:?}")?;
        }
        write!(f, "; the known languages are {}", NAMES.join(", "))
    }
}

impl std::error::Error for UnknownLanguages {}
