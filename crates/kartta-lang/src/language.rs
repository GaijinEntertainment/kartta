use std::any::Any;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::io;

use crate::doc::DocComments;
use crate::extraction::{Extraction, RawExtraction};
use crate::symbol::SymbolKind;

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

    /// Why the language leaves out of its graph the file at `path`, a file of its kind that it does not
    /// [claim](Language::claims), as a clause an answer can quote: Go's `no Go build reads a file whose name starts with
    /// _ or .`. `None`, the default, for every other file.
    fn declines(&self, path: &str) -> Option<&str> {
        let _ = path;
        None
    }

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
/// The struct is `#[non_exhaustive]` so that a field the graph comes to need breaks no language crate, one kartta does
/// not ship included: outside `kartta-lang` no struct literal or `..` update builds it. A language cannot tell at compile
/// time that a field it should set was added; the field keeps its default until the language states it.
///
/// Each enum among its fields is `#[non_exhaustive]` for the same reason, so that a rule a language comes to need is a
/// new variant in a minor release. A language that states a new variant needs a graph of that release: an older one
/// reads the variant as the enum's default where it matches and as none of the others where it compares, and an unknown
/// [`Receiver`](crate::Receiver) as an empty [`Path`](crate::Receiver::Path).
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
    /// The separator that joins a class's own methods to their owner, `.` in Ruby's `Formula.build`, where one owner
    /// can hold a class method and an instance method of one name, `Formula.build` beside `Formula#build`. A member
    /// call's receiver or a use's receiver path that ends in it is the class itself: `Formula.` names the owner
    /// `Formula`, and `.` alone a class the language cannot name. Of an owner's methods of the name, a receiver on the
    /// class reaches those joined with this separator first, and any other receiver the others first; an owner that
    /// holds only one kind offers it to both. `None` where the language keeps no such methods apart.
    pub class_member_separator: Option<&'static str>,
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
    /// How the extraction writes each entry of `Symbol::parents`.
    pub parent_spelling: ParentSpelling,
    /// Whether a package is the files of one directory that declare it, as a Go package is: they see each other's
    /// package-level symbols with no import, and an import names the directory.
    pub directory_packages: bool,
    /// How far through its includes a file's calls bind the names they define.
    pub include_reach: IncludeReach,
    /// The module every file sees with no include, where only includes put a definition in a file's scope: a call with
    /// no receiver binds by its name alone only a definition of a file its own file reaches through includes, however
    /// deep, or of the file whose package this names, and a bare use under [`ValueUses::Expanded`] that the use's file
    /// and includes leave open binds its definition. `None` where the language's scope reaches further.
    pub module_seen_everywhere: Option<&'static str>,
    /// What a member call, `x.f()`, can call besides a member.
    pub member_calls: MemberCalls,
    /// The methods the language's runtime gives its own values, a string's, an array's or a table's, in ascending
    /// order: a member call of one, `line.trim()`, binds no free definition, and a member of the name only where scope
    /// reaches it, the caller's file, its imports, package or includes, or the caller's own owner, or as the name's one
    /// callable where its receiver spells that callable's owner, `session.seek()` for a `Session`, because a call on
    /// such a value reaches the runtime, not a function of the repository. Empty where a language names none.
    pub runtime_methods: &'static [&'static str],
    /// The methods the language's standard library declares, in ascending order: a method call of one, marked by
    /// `CallRef::calls_a_method`, on a value whose type the graph does not follow, `found.start()`, binds the one
    /// callable of its name only as a guess, unless its receiver spells that callable's owner, because the value is as
    /// often one of the library's or of a crate outside the tree as one of the repository's. Empty where a language
    /// names none.
    pub library_methods: &'static [&'static str],
    /// The methods of the standard library's interfaces that a repository's type can declare, in ascending order,
    /// where a type implements an interface without naming it: the library may call such a method on a value of the
    /// type, so a dead-code query counts a type's method of one as dispatched. Empty where a language names none.
    pub library_interface_methods: &'static [&'static str],
    /// The character that ends the name of the method an attribute write calls, Ruby's `=` in `f=`, where the language
    /// names one so. A call of such a method on a value whose type the graph does not follow, `x.f = 1`, binds only as
    /// a guess, because many types declare a writer of one name. `None` where a language names none.
    pub setter_suffix: Option<char>,
    /// Which members of the files a file includes a member call of one of the
    /// [`Conventions::runtime_methods`] binds, and under [`IncludedRuntimeMembers::SpelledOwnOrExported`] any method
    /// call on a value.
    pub included_runtime_members: IncludedRuntimeMembers,
    /// What an owner that no type of its name declares is.
    pub untyped_owners: UntypedOwners,
    /// Whether a member call on a value, `x.f()` with an `x` that names no owner, guesses the callable of its name
    /// nearest the caller.
    pub calls_on_values: CallsOnValues,
    /// What a member call's receiver spells.
    pub receivers: Receivers,
    /// What a receiver written as a bare name, of letters, digits and `_`, names.
    pub bare_receivers: BareReceivers,
    /// The receivers a member call writes for a value of the caller's own type: its own receiver, as `self` and `cls`
    /// in Python, and `other`, as a comparison names its second operand. Where the language lists them, a member call
    /// on any other name that names no owner, a value's, binds as [`Conventions::unlisted_receivers`] says. A language
    /// that records its own receiver as the name of the caller's owner, as Rust does `self` and `Self`, lists only
    /// `other`, since a call on that name binds through the owner it names. `None` keeps the caller's own owner for
    /// every name.
    pub own_type_receivers: Option<&'static [&'static str]>,
    /// What a member call on a name outside [`Conventions::own_type_receivers`] binds of the caller's own owner.
    pub unlisted_receivers: UnlistedReceivers,
    /// The receivers a member call writes for the caller's base class, as Quirrel's `base`, daslang's `super` and PHP's
    /// `parent`. A member call on one binds the member of the name that the first base the caller's class declares
    /// holds, else the one the nearest class above that base holds, and nothing where the graph holds neither, so never
    /// a member of the caller's own class. Empty where a language writes none.
    pub base_receivers: &'static [&'static str],
    /// The method a construction runs where a class declares it under one name, as PHP's `__construct`. A call of it on
    /// a receiver written from the root binds what that class's table holds, and nothing where the table holds none, so
    /// never another class's. A construction records it right after a call of the class's last name on the class
    /// written from the root; on a class the graph does not hold, that call alone counts among the calls that bound
    /// nothing. `None` where a language has no such name.
    pub constructor: Option<&'static str>,
    /// What a member call on a class the graph holds reaches of the types above that class.
    pub receiver_members: ReceiverMembers,
    /// Where the language records a use of a type.
    pub type_uses: TypeUses,
    /// Where the language records a read or a write of a field, a variable or a constant.
    pub data_uses: DataUses,
    /// The kinds among field, variable and constant whose reads and writes the language records nowhere, as Rust
    /// records no access to a field: a definition of such a kind has no use but one that spells its name as something
    /// else, a callable's, would, so no use of it does not show that nothing reads it. Empty where the language records
    /// such uses of every kind it extracts.
    pub unrecorded_data: &'static [SymbolKind],
    /// Where a bare use of a name that its own file does not declare binds.
    pub value_uses: ValueUses,
    /// The names the runtime answers before [`module_seen_everywhere`](Self::module_seen_everywhere) does, in functions
    /// no file holds: such a use binds nothing past the use's file and includes.
    pub native_names: &'static [&'static str],
    /// What a member use on a value of a type the language cannot name binds.
    pub untyped_members: UntypedMembers,
    /// Which calls bind apart from the definitions other calls of their name bind.
    pub calls_apart: CallsApart,
    /// What a call reaches where the tier that binds it holds several definitions of one qualified name equal.
    pub twins: Twins,
}

impl Conventions {
    /// The default conventions, which [`Conventions::default`] returns, as a value a `const` can start from.
    pub const DEFAULT: Self = Self {
        own_members: OwnMembers::Fallback,
        inherited_members: InheritedMembers::Unreached,
        values_are_call_targets: true,
        namespace_calls: NamespaceCalls::Bound,
        owner_chain_separator: None,
        class_member_separator: None,
        bare_call_reaches_enclosing_owners: false,
        scope_separator: None,
        parent_spelling: ParentSpelling::Written,
        directory_packages: false,
        include_reach: IncludeReach::Direct,
        module_seen_everywhere: None,
        member_calls: MemberCalls::NearbyFreeFunctions,
        runtime_methods: &[],
        library_methods: &[],
        library_interface_methods: &[],
        setter_suffix: None,
        included_runtime_members: IncludedRuntimeMembers::Any,
        untyped_owners: UntypedOwners::Types,
        calls_on_values: CallsOnValues::NearestGuessed,
        receivers: Receivers::Owners,
        bare_receivers: BareReceivers::Any,
        own_type_receivers: None,
        unlisted_receivers: UnlistedReceivers::NoOwnMember,
        base_receivers: &[],
        constructor: None,
        receiver_members: ReceiverMembers::Declared,
        type_uses: TypeUses::Partial,
        data_uses: DataUses::Partial,
        unrecorded_data: &[],
        value_uses: ValueUses::Declared,
        native_names: &[],
        untyped_members: UntypedMembers::FileField,
        calls_apart: CallsApart::None,
        twins: Twins::First,
    };
}

impl Default for Conventions {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What a member call on a receiver a language does not list among its [`Conventions::own_type_receivers`] binds of the
/// caller's own owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum UnlistedReceivers {
    /// Nothing: the call goes on to the tiers after the caller's own owner.
    #[default]
    NoOwnMember,
    /// Its member, as on a listed receiver, and as a guess on one field of a listed receiver, `this.slot`, which holds
    /// any class: the list then says only which receivers name the caller's class.
    OwnMemberFieldsAsGuess,
}

/// How the extraction writes each entry of `Symbol::parents`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ParentSpelling {
    /// As the file spells it, which the graph reads through the language's scopes.
    #[default]
    Written,
    /// From the root, as the qualified name the language compiles it to: a base is the type of exactly that qualified
    /// name, and the graph holds none where that name is a type outside the tree, whatever namesake a file reaches. A
    /// receiver that starts with a separator reads the same way.
    FromRoot,
}

/// What a member call on a class the graph holds, named by the receiver or one of
/// [`Conventions::own_type_receivers`], reaches of the types above that class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ReceiverMembers {
    /// Nothing the receiver's tier reads, as a language with no opt-in stands, and as C++ and Go stay: C++ looks its
    /// bases up under [`InheritedMembers::Found`], and Go through [`Receivers::TypePaths`].
    #[default]
    Declared,
    /// The member the nearest type above the class holds, where the class holds none, ahead of a namesake of the
    /// class: the `parents` of each type breadth-first, a level's traits first, as PHP's method table takes a class's
    /// own method, then its traits', then its parent's, and an interface's declaration only where no body above the
    /// class holds the member.
    Inherited,
    /// [`ReceiverMembers::Inherited`], where a field of the name that the walk meets ahead of the member hides it and the
    /// call binds nothing, as a class's values and methods share one table: Quirrel's `_members`. A field with no member
    /// above it leaves the call to the name tiers, since a Quirrel class hands on a function of its file as `f = f`.
    InheritedUpToFields,
    /// [`ReceiverMembers::InheritedUpToFields`], where a field answers with no member above it too, as a call on a field
    /// runs what the field holds, and standing in for Python's C3 order. A field with
    /// [`value_types`](crate::Symbol::value_types) only declares the name, so the walk goes past it.
    InheritedUpToAnyField,
}

/// How far through its includes a file's calls bind the names they define.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum IncludeReach {
    /// The files it includes itself: an import names one module, and what that module imports in turn is not in the
    /// importing file's scope.
    #[default]
    Direct,
    /// Every file it reaches through includes, however deep, as a C or C++ `#include` pastes the included text in
    /// place. A call whose receiver names an owner or an import binds through that name, not through includes. A
    /// name's one callable binds a call only where the caller's file, or a file that includes it directly, reaches that
    /// callable or a declaration of it.
    Transitive,
    /// Every file it reaches through includes, however deep, as a DSHL script joins its includes, under
    /// [`IncludeReach::Transitive`]'s rule for a call past the direct includes, unless a copy of the name sits nearer
    /// the call than the file reached. A name's one callable binds a call wherever it is, since no name comes from
    /// outside the repository and it can sit in a file behind an include the resolver cannot follow. For the same
    /// reason, a definition a call's arguments turn away shows no overload the graph lacks, so what is left binds as
    /// though the name held nothing else.
    Script,
}

/// What a member call's receiver spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
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
    /// As [`Receivers::Owners`], where a receiver can also spell a chain of calls from an owner, which the extraction
    /// cannot follow past its file: `T::m()::n()?` calls `m` on `T`, then `n` on what `m` returns, and `?` takes what
    /// an `Option`, a `Result` or a `Promise` holds. A chain can start from a bare call, `f()::n()`, which binds as the
    /// call would. The graph finds each method as the owner tier does, reads the owner it returns from its
    /// [value type](crate::Symbol::value_types), and binds the call as one on the last owner. A path that reaches no
    /// type leaves the receiver the value after its `|`, `T::m()|.value` for `.value`, and without one the empty
    /// receiver.
    ReturnPaths,
    /// As [`Receivers::Owners`], where a receiver can also spell a property the extraction cannot read from its file,
    /// `->name`. The graph walks up from the caller's class to the nearest type that declares a field of the name,
    /// a type's traits before the class it extends. Where the caller's class can read that field, its
    /// [value type](crate::Symbol::value_types) is the call's receiver. A field with no value type, one private to a
    /// type above the caller's class, and a name no type declares leave the empty receiver.
    InheritedFields,
    /// As [`Receivers::Owners`], where a bare call can also spell the named bodies a library pastes into the scope
    /// around it, which the extraction cannot read from its file: `+` and the bodies' names one a line, nearest first.
    /// A line of `@` and a filter stands for every body some file's top level pastes in by that filter, which a file
    /// records as a use of the body's name, and one of the body's name, `@` and the filter per filter.
    /// The call binds the definition of its name whose [value type](crate::Symbol::value_types) is the first such body
    /// that holds one, nearest the caller where several files define it, even one local to its file; a name no body
    /// holds binds as a call with no receiver. A file's top level records such a read as a use of the line, `#` and the
    /// call's name, per line.
    PastedBodies,
}

/// What a receiver written as a bare name, of letters, digits and `_`, names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum BareReceivers {
    /// A value, a module or a type alike, so a bare name that no type the graph holds is named can still hold a value
    /// of one.
    #[default]
    Any,
    /// A module where exactly one other file declares a module of that name, as daslang's `mod::f()` qualifies a call,
    /// whether or not the caller's file requires the module: the call binds that module's definition of the name or
    /// nothing. A call not marked [`CallRef::calls_a_method`](crate::CallRef) through a name no symbol holds names a
    /// module outside the graph and binds nothing. Any other bare name reads as under [`BareReceivers::Any`].
    Modules,
    /// A class, as a PHP static call writes `Mockery::mock()` while a value is written `$mock`, except for the scope
    /// keywords listed, `self`, `static` and `parent`, compared without case. A call on any other bare name that no
    /// type the graph holds is named calls a member of a class outside the graph and binds nothing, as does a receiver
    /// written from the root that names no type. A receiver the graph has a type of reads the class the caller's file
    /// holds or includes, else the one of the caller's scope, ahead of a namesake farther off.
    Classes(&'static [&'static str]),
    /// A value, but for a name that starts in upper case, which names a constant: a class, a module or a value bound
    /// once, as Ruby writes them. A call through a receiver whose last segment is such a name, and of which the graph
    /// holds neither a type nor a value, as `Hash.new` where the tree defines no `Hash`, calls code outside the graph:
    /// once no member of that owner binds it, it binds nothing.
    Constants,
}

/// What a member call, `x.f()`, can call besides a member.
///
/// A free function is a definition with no owner. Beyond the caller's directory and the one above it, where only the
/// one callable of the name outside vendored code is left to guess at, a member call takes a member alone under each,
/// so a lone free `trim` elsewhere in the tree is not guessed for `line.trim()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum MemberCalls {
    /// A free function the caller's file sees, the one definition of its name, or one in the caller's directory or the
    /// one above it, where no member of the name is a candidate: what a value holds can be read as free, an object's
    /// property, a table's slot, a module that a name holds or a path a call spells.
    #[default]
    NearbyFreeFunctions,
    /// As [`MemberCalls::NearbyFreeFunctions`], but a receiver that names a type the caller's file holds or includes
    /// takes a member alone, unless that file is generated, whose names say nothing of a type.
    NearbyFreeFunctionsUnlessTyped,
    /// Nothing: a Go member call calls a method or a field of its value and reaches a package's function only through
    /// the import of the package, which binds before any tier that takes a member.
    Members,
    /// A free function only through a receiver that names a module, a name whose last segment starts in lower case, as
    /// the receiver of a Rust path call `walk::run()` does, and then as [`MemberCalls::NearbyFreeFunctions`] takes one,
    /// but for the caller's own file, where it takes only the item whose qualified name ends with the receiver,
    /// `walk::run` of an inline `mod walk`; such a call takes a member only where the receiver names its owner. A call
    /// on a value, the empty receiver or one of [`Conventions::own_type_receivers`], and one through a type, a
    /// receiver written capitalised as `Poll::Ready(x)` is or as the owner a call on `self` records, take a member
    /// alone. A call through a capitalised receiver of which the graph holds neither a type nor a value of its last
    /// segment's name, `Vec::new()` or `tempfile::NamedTempFile::new()`, calls code outside the graph: once no member
    /// of that owner binds it, it binds nothing.
    ModulePaths,
}

/// Whether a member call on a value, `x.f()` with an `x` that names no owner, guesses the callable of its name nearest
/// the caller once nothing the caller's file sees binds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CallsOnValues {
    /// The callable in the caller's directory or the one above it, then the one outside vendored code.
    #[default]
    NearestGuessed,
    /// None near the caller, where such a value's type is most often declared far from it, as a C++ operand's
    /// container, smart pointer or library type is, so the nearest namesake is most often not what the value holds.
    /// The one callable of the name still binds.
    NoNearestGuess,
}

/// Which calls of a language bind apart from the definitions other calls of their name bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CallsApart {
    /// None: a call can bind a macro or anything else of its name, as a dshl one does.
    #[default]
    None,
    /// Those marked [`CallRef::binds_apart`](crate::CallRef::binds_apart): such a call binds only a macro, as Rust's
    /// `name!()` beside `name()` does, and any other call no macro.
    Marked,
}

/// What a call reaches where the tier that binds it, its caller's file or the type its receiver names, holds several
/// definitions of one qualified name and kind equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Twins {
    /// The first of them, as an overload or a namesake copy is one definition among others.
    #[default]
    First,
    /// Every one of them, as each branch of a Rust `cfg` defines a name the others define too and some build compiles
    /// each: those in the bound one's file, and, where the caller's file reaches it through a file it includes, those
    /// in every file it includes; of members, the exported ones, and in one file those of a type it defines twice.
    Every,
    /// Every one of them in the other files of its package, as a Go build constraint keeps files that declare one name
    /// apart and some build selects each, unless the caller's own file holds one, the one that file is built with, and
    /// none in a file [`BuiltApart`] keeps from the caller's. A use reaches them as a call does.
    Package(BuiltApart),
}

/// Says from two files' [build constraints](crate::Extraction::build_constraint), the empty one for a file with none,
/// whether no build compiles both files. A pair it cannot tell within its bound reads as built together.
#[derive(Debug, Clone, Copy)]
pub struct BuiltApart(pub fn(&str, &str) -> bool);

impl PartialEq for BuiltApart {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::fn_addr_eq(self.0, other.0)
    }
}

impl Eq for BuiltApart {}

impl Hash for BuiltApart {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Whether a call can bind a namespace of the name it calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
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
#[non_exhaustive]
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
#[non_exhaustive]
pub enum DataUses {
    /// Where some code reads or writes one, or nowhere: code can read or write one where the language records no use,
    /// as a Python framework reads a class's attribute by its name and a C macro's expansion a field, so such a
    /// definition that no use names may still be used.
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
#[non_exhaustive]
pub enum ValueUses {
    /// In the use's package, where packages are directories, or a dot-imported one.
    #[default]
    Declared,
    /// In the one file of those the use's file includes that exports the name outside every body and owner, else in
    /// the definition a re-export chain from them renames, else in the one included file that declares the name
    /// without exporting it: a module imports a name from the module that exports it.
    Imported,
    /// As [`ValueUses::Imported`], else, where no file the use's file includes declares the name, in the one file of
    /// those its includes reach in turn that declares the name outside every body and owner, whatever its kind: a file
    /// and everything it includes compile as one script. A use that would bind an enum, or that two files tie on, binds
    /// the one variable of the name those files hold, its own included, where they hold one: an enum there names the
    /// ranges of the variable of its name. A use all that leaves unbound binds the one file that declares the name, of
    /// all the files that compile in a script with the use's file: those that share a file no file includes with it. Of
    /// several, the one that shares the most leading directories with the use's file, at least two, binds.
    ImportedThenReached,
    /// As [`Imported`](Self::Imported), else in [`Conventions::module_seen_everywhere`], else in the one definition
    /// that the files including the use's file bind the name to that way, the file itself first, else in the one those
    /// files and every file a re-export hands the use's file to bind it to: a macro's quoted code names what the file
    /// that expands the macro reaches. Two definitions among those files bind none. A name of
    /// [`Conventions::native_names`] stops after `Imported`.
    Expanded,
    /// In the one variable, constant or function of the name outside every owner and body that the use's own file
    /// declares, else that the files it includes declare, else, for a name few of them share, that the files those
    /// reach declare, as an include pastes the included text in place: a function named without a call is taken as a
    /// value. A type or a macro of the name binds no use, and a declaration and its definition count once.
    Included,
}

/// Which members of the files a file includes a member call of a runtime method, `obj.setValue(1)`, binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum IncludedRuntimeMembers {
    /// Any member of the name, as the members of the caller's own file are.
    #[default]
    Any,
    /// A member whose owner the receiver spells at its last segment in any case, `settings.setValue(1)` for
    /// `Settings`, whose owner the caller's own owner is or derives from, or whose owner is an interface or a type
    /// alias, where the values such a call is made on are most often the native values a host or the runtime hands
    /// the code, so that an included class's member of the name is a namesake of the runtime's method, while an
    /// interface or a type alias, which makes no value, is included to type the values the caller holds.
    SpelledOrOwn,
    /// As [`IncludedRuntimeMembers::SpelledOrOwn`] for a runtime method's call, and for any other method call on a
    /// value, `pool.prec(1)`, as well where the member is not exported, as a type's private method is out of the
    /// including module's reach and a trait impl's member is one of the trait's namesakes, where no tier above follows
    /// the value's type. A file whose one member of the name the call turns away leaves the call unbound, as
    /// two files that offer one each do, since the member it keeps elsewhere is a namesake no likelier the callee.
    SpelledOwnOrExported,
}

/// What an owner that no type of its name declares is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum UntypedOwners {
    /// A type the graph does not hold, which a member call reaches on any value of it.
    #[default]
    Types,
    /// One value, a table a module binds, which a member call reaches through a receiver that spells it, `t.f()` for
    /// the table `t`, or from a member of it. Where members of several owners in one scope answer a member call on any
    /// other value, `events.f()`, the members of such owners drop out, so the member of the class the value holds binds
    /// past the table's namesake; a member that no other owner ties still binds.
    Tables,
}

/// What a member use on a value of a type the language cannot name binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
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
#[non_exhaustive]
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
#[non_exhaustive]
pub enum InheritedMembers {
    /// A bare call reaches no inherited member.
    #[default]
    Unreached,
    /// The `parents` of a type also name types its member lookup does not search, as a Ruby `extend` does, so an
    /// inherited member is one candidate among those of its name.
    Admitted,
    /// The `parents` of a type are the bases its member lookup searches, as C++ class-scope lookup does, so an
    /// inherited member a base certainly declares is taken when the caller's owner declares none of the name, and
    /// when the class a call's receiver names declares none of it.
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
