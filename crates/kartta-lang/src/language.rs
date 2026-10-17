use std::fmt;
use std::io;

use crate::doc::DocComments;
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

    /// Where the language writes the comments that document a definition from the lines above it; none by default.
    fn doc_comments(&self) -> DocComments {
        DocComments::NONE
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

    /// The regular files whose path equals `path` when ASCII letters are compared without case, `path` itself among
    /// them when it is one, in index order. A byte outside ASCII matches only itself.
    ///
    /// This is for a language whose loader ignores letter case, such as a game engine reading its files from an
    /// archive that lowercases every path. The default answers `path` alone when it is a file, for a repository that
    /// cannot compare paths without case.
    fn files_ignoring_case<'a>(&'a self, path: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        Box::new(self.contains(path).then(|| path.to_owned()).into_iter())
    }

    /// The content of the file at `path`.
    ///
    /// # Errors
    ///
    /// The error of reading it.
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
}

/// The conventions a language's calls resolve under and its uses are recorded by, which the graph cannot infer from its
/// extractions. The default states those of the languages that change none.
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
    /// How far through its includes a file's calls bind the names they define.
    pub include_reach: IncludeReach,
    /// What a member call, `x.f()`, can call besides a member.
    pub member_calls: MemberCalls,
    /// The methods the language's runtime gives its own values, a string's, an array's or a table's, in ascending
    /// order: a member call of one, `line.trim()`, binds a member of the name and no free definition, because a call on
    /// such a value reaches the runtime, not a function of the repository. Empty where a language names none.
    pub runtime_methods: &'static [&'static str],
    /// Whether a member call on a value, `x.f()` with an `x` that names no owner, guesses the callable of its name
    /// nearest the caller.
    pub calls_on_values: CallsOnValues,
    /// What a member call's receiver spells.
    pub receivers: Receivers,
    /// Where the language records a use of a type.
    pub type_uses: TypeUses,
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
            include_reach: IncludeReach::Direct,
            member_calls: MemberCalls::NearbyFreeFunctions,
            runtime_methods: &[],
            calls_on_values: CallsOnValues::NearestGuessed,
            receivers: Receivers::Owners,
            type_uses: TypeUses::Partial,
        }
    }
}

/// How far through its includes a file's calls bind the names they define.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IncludeReach {
    /// The files it includes itself: an import names one module, and what that module imports in turn is not in the
    /// importing file's scope.
    #[default]
    Direct,
    /// Every file it reaches through includes, however deep, as a C or C++ `#include` pastes the included text in
    /// place. A call whose receiver names an owner or an import binds through that name, not through includes.
    Transitive,
}

/// What a member call's receiver spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Receivers {
    /// The owner of the member the call reaches, as the language writes owners, or an import: the graph binds a member
    /// of an owner of that name.
    #[default]
    Owners,
    /// A type path, which names the receiver's type in the scope of the calling file. It starts with a name that the
    /// file's package declares or that an import of the file binds, `Server` or `http`, and goes on in steps: `.name`
    /// selects a member of a package or a field or a method of a type, `()` takes the first result of calling what
    /// precedes and `()#n` its result `n`, counted from zero, so `()` is `()#0` and `pkg.Open()#1` is the second
    /// result of `pkg.Open`, and `[]` takes an element of a container. A path that starts with `[]` is a container of
    /// what follows, `[]Item`. The graph follows the paths that definitions give as their
    /// [value types](crate::Symbol::value_types) and binds a member of the type the receiver reaches or of a type that
    /// type embeds; a path that leaves the code the graph holds binds nothing.
    TypePaths,
}

/// What a member call, `x.f()`, can call besides a member.
///
/// A free function is a definition with no owner. Beyond the caller's directory and the one above it, where only the
/// one callable of the name outside vendored code is left to guess at, a member call takes a member alone under either,
/// so a lone free `trim` elsewhere in the tree is not guessed for `line.trim()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MemberCalls {
    /// A free function the caller's file sees, the one definition of its name, or one in the caller's directory or the
    /// one above it, where no member of the name is a candidate: what a value holds can be read as free, an object's
    /// property, a table's slot, a module that a name holds or a path a call spells.
    #[default]
    NearbyFreeFunctions,
    /// Nothing: a Go member call calls a method or a field of its value and reaches a package's function only through
    /// the import of the package, which binds before any tier that takes a member.
    Members,
}

/// Whether a member call on a value, `x.f()` with an `x` that names no owner, guesses the callable of its name nearest
/// the caller once nothing the caller's file sees binds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CallsOnValues {
    /// The callable in the caller's directory or the one above it, then the one outside vendored code.
    #[default]
    NearestGuessed,
    /// None near the caller, where such a value's type is most often declared far from it, as a C++ operand's
    /// container, smart pointer or library type is, so the nearest namesake is most often not what the value holds.
    /// The one callable of the name still binds.
    NoNearestGuess,
}

/// Where a language records a use of a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TypeUses {
    /// Code can name a type where the language records no use, so a type that no use names may still be named.
    #[default]
    Partial,
    /// Wherever code names a type that is not local to a body, as Go's extraction records a use in a declaration or a
    /// composite literal and a call in a conversion: such a type that no call and no use names is named nowhere the
    /// language reads. A local type stays unjudged, because no use binds a local.
    Every,
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
