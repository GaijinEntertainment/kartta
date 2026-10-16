use std::fmt;
use std::io;

use crate::extraction::{Extraction, RawExtraction};

/// A language the code graph understands: which files are its, what it extracts from their bytes, and how it
/// resolves what it extracted against the repository.
///
/// The engine names no language of its own. It is handed the languages to build with, and each claims its files,
/// extracts them on the build's threads, and resolves its imports through the [`Resolver`] it builds over a read-only
/// [`Repository`]. A crate that implements this trait plugs a language in without a change to kartta.
///
/// Extraction is split in two so that its expensive half can be shared. [`Language::extract`] sees only the bytes, so
/// a cache keyed by their hash and by [`Language::fingerprint`] can hand its result to every file with the same
/// content, in any tree; [`Resolver::resolve`] applies the path and the repository, which no cache shares.
pub trait Language: Send + Sync + fmt::Debug {
    /// The language's name: `go`, `cpp`. Answers report it, and the cache files the language's extractions under it.
    fn name(&self) -> &str;

    /// The family whose files resolve against each other in one graph; the language's own name unless several
    /// languages share one, as TypeScript and JavaScript files call each other.
    fn family(&self) -> &str {
        self.name()
    }

    /// Whether the file at `path`, root-relative with forward slashes, is this language's.
    fn claims(&self, path: &str) -> bool;

    /// Identifies what [`Language::extract`] computes from the same bytes: it changes whenever the grammar or the
    /// language's extraction does, and a cache keeps the extractions of each fingerprint apart. The code of this crate,
    /// which shapes the result too, is identified by [`FINGERPRINT`](crate::FINGERPRINT), which a cache folds in
    /// beside it.
    fn fingerprint(&self) -> &[u8];

    /// The conventions the graph resolves this language's calls under.
    fn conventions(&self) -> Conventions {
        Conventions::default()
    }

    /// What `source`, one file's whole content, holds, from the bytes alone.
    ///
    /// Runs on the build's threads, many files at a time. A file the parser gives no tree for yields an empty
    /// extraction; a tree with syntax errors, which a recovering parser such as tree-sitter returns, is extracted as
    /// far as the language can read it.
    fn extract(&self, source: &[u8]) -> RawExtraction;

    /// The resolver for one build over `repository`, which the build asks once and then shares between its threads.
    /// A language that reads configuration — a `go.mod`, a `tsconfig.json` — reads it here.
    fn resolver<'r>(&'r self, repository: &'r dyn Repository) -> Box<dyn Resolver + 'r>;
}

/// Applies a file's path and the repository to what its language extracted from the bytes.
pub trait Resolver: Send + Sync {
    /// The extraction of the file at `path`, from `raw`: its imports and includes resolved, its symbols' flags
    /// completed by what the path shows.
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction;
}

/// The repository as a resolver sees it: the files a build indexes, and the bytes of any of them.
///
/// Paths are root-relative with forward slashes, and `.` and the empty path both name the root directory, as
/// [`path::dir`](crate::path::dir) and [`path::join`](crate::path::join) spell it. An index may keep its paths in a
/// compact form it rebuilds on reading, so the lookups hand out owned paths rather than borrowing them.
pub trait Repository: Sync {
    /// Whether `path` is a regular file of the repository.
    fn contains(&self, path: &str) -> bool;

    /// The regular files directly in `dir`, in index order.
    fn files_in_dir<'a>(&'a self, dir: &str) -> Box<dyn Iterator<Item = String> + 'a>;

    /// The regular files whose last component is `name`, in index order.
    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a>;

    /// The content of the file at `path`.
    ///
    /// # Errors
    ///
    /// The error of reading it.
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
}

/// The conventions a language's calls resolve under, which the graph cannot infer from its extractions. The default
/// states those of the languages that change none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Conventions {
    /// When a bare call inside a member reaches the members of that member's own owner. A bare call inside a member of
    /// a namespace reaches the namespace's members whatever this says, because a namespace is a lexical scope.
    pub own_members: OwnMembers,
    /// What a bare call that reaches no member of the caller's own owner takes from the owners that owner derives
    /// from.
    pub inherited_members: InheritedMembers,
    /// Whether a variable or a constant can be called. True where a function-valued binding is a callable form of the
    /// language; false where such a name is data, and a call of it is some other symbol's.
    pub values_are_call_targets: bool,
    /// The separator that joins an owner's chain of names, `.` in `A.Worker` and `::` in `a::Worker`, where an owner
    /// is such a chain: a receiver that spells the chain's end at a separator, `Worker` or `m::Worker` for
    /// `a::m::Worker`, names that owner. `None` where an owner is a single name.
    pub owner_chain_separator: Option<&'static str>,
    /// Whether a bare call reaches a member of an owner whose chain encloses the caller's own owner at
    /// [`Conventions::owner_chain_separator`], as a TypeScript `namespace` is a lexical scope of what it holds.
    pub bare_call_reaches_enclosing_owners: bool,
    /// The separator a qualified name joins its scopes with, `::` in `ns::Widget::draw`, where a member's qualified
    /// name keeps only its owner, `Widget::draw`, and an owner is a single name: a callee written with more scopes
    /// than that is read by its trailing `Widget::draw`, a callee written from the root, `::draw`, names a global
    /// symbol, and a base written `ns::Base` names the owner `Base`. `None` where a language writes no such names.
    pub scope_separator: Option<&'static str>,
    /// Whether a package is the files of one directory that declare it, as a Go package is: they see each other's
    /// package-level symbols with no import, and an import names the directory.
    pub directory_packages: bool,
}

impl Default for Conventions {
    fn default() -> Self {
        Self {
            own_members: OwnMembers::Fallback,
            inherited_members: InheritedMembers::Unreached,
            values_are_call_targets: true,
            owner_chain_separator: None,
            bare_call_reaches_enclosing_owners: false,
            scope_separator: None,
            directory_packages: false,
        }
    }
}

/// When a bare call inside a member reaches the members of that member's own owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OwnMembers {
    /// Before a free symbol of the name, as C++ name lookup does inside a member function: the caller's own class is
    /// searched first.
    First,
    /// Only when no free symbol of the name exists, as in a class method whose body is wrapped in `with(self)`.
    #[default]
    Fallback,
    /// Never: a member is reached only through a receiver, `s.len()` or `self.load()`, and a bare name reaches a
    /// package- or module-level symbol.
    Unreached,
}

/// How a bare call reaches the members of the types the caller's owner derives from, as the `parents` of its type
/// name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InheritedMembers {
    /// A bare call reaches no inherited member.
    #[default]
    Unreached,
    /// The `parents` of a type also name types its member lookup does not search, as a Ruby `extend` does, so an
    /// inherited member is one candidate among those of its name.
    Admitted,
    /// The `parents` of a type are the bases its member lookup searches, as C++ class-scope lookup does, so an
    /// inherited member a base certainly declares is taken when the caller's owner declares none of the name.
    Found,
}
