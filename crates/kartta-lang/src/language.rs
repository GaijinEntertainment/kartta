use std::any::Any;
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
    ///
    /// The languages of one family build interchangeable [resolvers](Language::resolver): an engine builds one for
    /// the family, from the first of its languages in the order the engine was given them, and resolves every file of
    /// the family through it.
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
        Conventions::DEFAULT
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

    /// The resolver for one build over `repository`, which the build asks of the first language of the
    /// [family](Language::family) once and then shares between its threads and every file of the family. A language
    /// that reads configuration — a `go.mod`, a `tsconfig.json` — reads it here, and the resolver owns what it read:
    /// it keeps no borrow of `repository`, which each [`Resolver::resolve`] is handed again.
    fn resolver(&self, repository: &dyn Repository) -> Box<dyn Resolver>;
}

/// Applies a file's path and the repository to what its language extracted from the bytes.
///
/// An engine that follows a changing tree resolves a file again when an answer the repository gave its resolution
/// changes, and it learns which answers those were by watching the repository while [`Resolver::resolve`] runs. So
/// `resolve` asks the repository on the thread that called it, and asks again for each file what it needs rather than
/// answering one file from what it asked for another. It reads no file's content: the engine learns that a file's
/// content changed only when it reads that file itself. Configuration is read by [`Language::resolver`] as it builds
/// the resolver, which the engine watches apart: when a change reaches what it read, the engine builds the resolver
/// again, and resolves every file of the family again when what the resolver derived changed, as
/// [`Resolver::resolves_as`] tells.
pub trait Resolver: Any + Send + Sync {
    /// The extraction of the file at `path`, from `raw`: its imports and includes resolved against `repository`, its
    /// symbols' flags completed by what the path shows.
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction;

    /// Whether this resolver, built again over a changed tree, resolves every file as `kept`, built before the change,
    /// still does over the same tree, so the engine resolves none of the family's files again for the change to its
    /// configuration. A resolver that holds what it derived from its configuration answers whether `kept` is its own
    /// type holding the same, the bytes it derived nothing from left out.
    ///
    /// The default answers `false`, so a change to any byte the resolver read resolves every file of its family again.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::any::Any;
    ///
    /// use kartta_lang::{Extraction, RawExtraction, Repository, Resolver};
    ///
    /// #[derive(PartialEq)]
    /// struct Modules(Vec<String>);
    ///
    /// impl Resolver for Modules {
    ///     fn resolve(&self, raw: RawExtraction, path: &str, _: &dyn Repository) -> Extraction {
    ///         raw.into_extraction(path).0
    ///     }
    ///
    ///     fn resolves_as(&self, kept: &dyn Resolver) -> bool {
    ///         (kept as &dyn Any).downcast_ref::<Self>() == Some(self)
    ///     }
    /// }
    ///
    /// let kept = Modules(vec!["example.com/m".to_owned()]);
    /// assert!(Modules(vec!["example.com/m".to_owned()]).resolves_as(&kept));
    /// assert!(!Modules(vec!["example.com/n".to_owned()]).resolves_as(&kept));
    /// ```
    fn resolves_as(&self, kept: &dyn Resolver) -> bool {
        let _ = kept;
        false
    }
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
///
/// # Examples
///
/// A language states its conventions as a `const` that sets the fields it changes on the default, which leaves a field
/// added later at its default too:
///
/// ```
/// use kartta_lang::{Conventions, OwnMembers, TypeUses};
///
/// const CONVENTIONS: Conventions = {
///     let mut conventions = Conventions::DEFAULT;
///     conventions.own_members = OwnMembers::Unreached;
///     conventions.type_uses = TypeUses::Nowhere;
///     conventions
/// };
///
/// assert_eq!(CONVENTIONS.own_members, OwnMembers::Unreached);
/// assert_eq!(CONVENTIONS.values_are_call_targets, Conventions::DEFAULT.values_are_call_targets);
/// ```
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
    /// Whether a call can bind a namespace of the name it calls.
    pub namespace_calls: NamespaceCalls,
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
    /// symbol, and a base written `ns::Base` names the owner `Base`. A receiver names the owner of its last name,
    /// `Widget` for `b::Widget`, and of the classes of that name nearest the caller, the one its spelling names from
    /// the caller's scope outward. `None` where a language writes no such names.
    pub scope_separator: Option<&'static str>,
    /// Whether a package is the files of one directory that declare it, as a Go package is: they see each other's
    /// package-level symbols with no import, and an import names the directory.
    pub directory_packages: bool,
    /// How far through its includes a file's calls bind the names they define.
    pub include_reach: IncludeReach,
    /// What a member call, `x.f()`, can call besides a member.
    pub member_calls: MemberCalls,
    /// The methods the language's runtime gives its own values, a string's, an array's or a table's, in ascending
    /// order: a member call of one, `line.trim()`, binds no free definition, and a member of the name only where scope
    /// reaches it, the caller's file, its imports, package or includes, or the caller's own owner, or as the name's one
    /// callable where its receiver spells that callable's owner, `session.seek()` for a `Session`, because a call on
    /// such a value reaches the runtime, not a function of the repository. Empty where a language names none.
    pub runtime_methods: &'static [&'static str],
    /// Whether a member call on a value, `x.f()` with an `x` that names no owner, guesses the callable of its name
    /// nearest the caller.
    pub calls_on_values: CallsOnValues,
    /// What a member call's receiver spells.
    pub receivers: Receivers,
    /// The receivers a member call writes for a value of the caller's own type: its own receiver, as `self` and `cls`
    /// in Python, and `other`, as a comparison names its second operand. Where the language lists them, a member call
    /// on any other name that names no owner, a value's, binds no member of the caller's own owner. A language that
    /// records its own receiver as the name of the caller's owner, as Rust does `self` and `Self`, lists only `other`,
    /// since a call on that name binds through the owner it names. `None` keeps the caller's own owner for every name.
    pub own_type_receivers: Option<&'static [&'static str]>,
    /// Where the language records a use of a type.
    pub type_uses: TypeUses,
    /// Where the language records a read or a write of a field, a variable or a constant.
    pub data_uses: DataUses,
    /// Where a bare use of a name that its own file does not declare binds.
    pub value_uses: ValueUses,
    /// What a member use on a value of a type the language cannot name binds.
    pub untyped_members: UntypedMembers,
}

impl Conventions {
    /// The default conventions, which [`Conventions::default`] returns, as a value a `const` can start from.
    pub const DEFAULT: Self = Self {
        own_members: OwnMembers::Fallback,
        inherited_members: InheritedMembers::Unreached,
        values_are_call_targets: true,
        namespace_calls: NamespaceCalls::Bound,
        owner_chain_separator: None,
        bare_call_reaches_enclosing_owners: false,
        scope_separator: None,
        directory_packages: false,
        include_reach: IncludeReach::Direct,
        member_calls: MemberCalls::NearbyFreeFunctions,
        runtime_methods: &[],
        calls_on_values: CallsOnValues::NearestGuessed,
        receivers: Receivers::Owners,
        own_type_receivers: None,
        type_uses: TypeUses::Partial,
        data_uses: DataUses::Partial,
        value_uses: ValueUses::Declared,
        untyped_members: UntypedMembers::FileField,
    };
}

impl Default for Conventions {
    fn default() -> Self {
        Self::DEFAULT
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
    /// of an owner of that name. Where scopes nest, a name the caller's body does not declare is spelled after the
    /// operator that takes the member, `.lock` or `->lock`, and the graph binds a member of the owner the
    /// [value type](crate::Symbol::value_types) of the caller's owner's field of that name names, else of a base's.
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
/// one callable of the name outside vendored code is left to guess at, a member call takes a member alone under each,
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
    /// A free function only through a receiver that names a module, a name whose last segment starts in lower case, as
    /// the receiver of a Rust path call `walk::run()` does, and then as [`MemberCalls::NearbyFreeFunctions`] takes one.
    /// A call on a value, the empty receiver or one of [`Conventions::own_type_receivers`], and one through a type, a
    /// receiver written capitalised as `Poll::Ready(x)` is or as the owner a call on `self` records, take a member
    /// alone.
    ModulePaths,
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

/// Whether a call can bind a namespace of the name it calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NamespaceCalls {
    /// It can: a namespace is a candidate as any callable of its name is.
    #[default]
    Bound,
    /// It cannot, where a call never names a namespace, as no Rust call names a `mod`: a call binds another definition
    /// of the name or none. The namespace still counts among the definitions of its name where a call binds only the
    /// one callable of a name, as a definition the call's shape rules out does, so a function named like a namespace
    /// is not the one callable of its name.
    Unbound,
}

/// Where a language records a use of a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TypeUses {
    /// Nowhere code names a type as one, such as a declaration, a signature, a generic argument or an import: a type's
    /// sites are the calls that name it and, where the language records them, its reads as a value, as a Rust unit
    /// struct is read.
    Nowhere,
    /// Where some code names a type, as Python's extraction records an annotation inside a body: code can name a type
    /// where the language records no use, so a type that no use names may still be named.
    #[default]
    Partial,
    /// Wherever code names a type that is not local to a body, as Go's extraction records a use in a declaration or a
    /// composite literal and a call in a conversion: such a type that no call and no use names is named nowhere the
    /// language reads. A local type stays unjudged, because no use binds a local.
    Every,
}

/// Where a language records a read or a write of a field, a variable or a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DataUses {
    /// Where some code reads or writes one, or nowhere: code can read or write one where the language records no use,
    /// as a Python module's top level reads a variable and a C macro's expansion a field, so such a definition that no
    /// use names may still be used.
    #[default]
    Partial,
    /// Wherever code reads or writes one that is not local to a body, as Go's extraction records a selector, a
    /// composite literal's key and a package-level name: such a definition that no use names is read and written
    /// nowhere the language reads but through a call on it, which the extraction records as a call, or by its
    /// runtime's reflection. A local stays unjudged, because no use binds a local.
    Every,
}

/// Where a bare use of a name that its own file does not declare binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ValueUses {
    /// In the use's package, where packages are directories, or a dot-imported one.
    #[default]
    Declared,
    /// In the one file of those the use's file includes that exports the name outside every body and owner, else in
    /// the definition a re-export chain from them renames, else in the one included file that declares the name
    /// without exporting it: a module imports a name from the module that exports it.
    Imported,
    /// In the one variable, constant or function of the name outside every owner and body that the use's own file
    /// declares, else that the files it includes declare, else, for a name few of them share, that the files those
    /// reach declare, as an include pastes the included text in place: a function named without a call is taken as a
    /// value. A type or a macro of the name binds no use, and a declaration and its definition count once.
    Included,
}

/// What a member use on a value of a type the language cannot name binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UntypedMembers {
    /// The one field of the name its own file declares outside other bodies.
    #[default]
    FileField,
    /// Nothing, where values are most often built from literals and inline types, so that the file's one field of the
    /// name is a namesake of the member a value holds.
    Unbound,
    /// The one field of the name its own file declares outside other bodies, else the one the files it includes
    /// declare, else, for a name few fields share, the one the files those reach declare, as an include pastes the
    /// included text in place.
    IncludedField,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_returns_the_const_default() {
        assert_eq!(Conventions::default(), Conventions::DEFAULT);
    }
}
