use std::sync::Arc;

use crate::symbol::{Symbol, SymbolFlags, SymbolId};

/// What a language extracts from one file's bytes alone, before the file's path and the repository are applied.
///
/// It is a function of the language's code and the bytes and of nothing else — no path, no other file, no
/// configuration — which is what lets a cache share it between every file with the same content. Whatever depends on
/// the path or the repository is recorded as a [`RawDirective`] for the [`Resolver`](crate::Resolver) to replay. A
/// symbol's flags here carry only what the bytes show, such as a generated-code banner; the resolver adds what the
/// path shows, such as a test file's name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct RawExtraction {
    /// The definitions in the file, in the order the language found them.
    pub symbols: Vec<Symbol>,
    /// Identities of callables the language can establish without guessing; empty for a language that keeps none.
    pub callables: Vec<Callable>,
    /// In-class declarations folded into out-of-line definitions in the same file; empty for a language that folds
    /// none.
    pub folded_declarations: Vec<FoldedDeclaration>,
    /// The calls in the file, unresolved, except those in `local_calls`.
    pub calls: Vec<CallRef>,
    /// Calls the language's own scoping binds to a symbol in `local_symbols`.
    pub local_calls: Vec<LocalCall>,
    /// Uses of a name outside a call, unresolved; empty for a language that records none.
    pub uses: Vec<UseRef>,
    /// The symbols declared inside a body, local to it; see [`Extraction::local_symbols`].
    pub local_symbols: Vec<SymbolId>,
    /// Re-exports, their specifiers as written; empty for a language without them.
    pub re_exports: Vec<RawReExport>,
    /// The package or module the file declares itself part of; `None` for a language without one.
    pub package: Option<String>,
    /// Compile-time implements assertions the file states.
    pub assertions: Vec<ImplementsAssertion>,
    /// The file's imports and includes, in source order, for the resolver to replay, and whatever else of the file only
    /// its resolver reads.
    pub directives: Vec<RawDirective>,
    /// Symbols that become entry points when the file's path qualifies them, such as a `main` at a crate root.
    pub entry_candidates: Vec<SymbolId>,
}

impl RawExtraction {
    /// Adds `symbol` to [`symbols`](Self::symbols) and returns its id, its position there.
    pub fn push_symbol(&mut self, symbol: Symbol) -> SymbolId {
        let id = SymbolId(u32::try_from(self.symbols.len()).unwrap_or(u32::MAX));
        self.symbols.push(symbol);
        id
    }

    /// The [`Extraction`] of the file at `path`, holding everything this one holds but the directives, which come
    /// back beside it for the resolver to replay, and the entry candidates, which only the resolver can judge.
    ///
    /// The records move rather than copy; only the re-exports are rebuilt, to make room for the file each names, so a
    /// file with none costs no allocation here.
    #[must_use]
    pub fn into_extraction(self, path: &str) -> (Extraction, Vec<RawDirective>) {
        let Self {
            symbols,
            callables,
            folded_declarations,
            calls,
            local_calls,
            uses,
            local_symbols,
            re_exports,
            package,
            assertions,
            directives,
            entry_candidates: _,
        } = self;

        let extraction = Extraction {
            file: path.to_owned(),
            symbols,
            callables,
            folded_declarations,
            calls,
            local_calls,
            uses,
            local_symbols,
            re_exports: re_exports.into_iter().map(ReExport::from).collect(),
            package,
            assertions,
            ..Extraction::default()
        };
        (extraction, directives)
    }
}

/// One import or include as the file wrote it, for the resolver to replay against the path and the repository, or
/// another fact of the file that only its resolver reads, as Go's build constraint.
///
/// Only the language that recorded it reads it: `form` tells its resolver which shape of directive it is, `spec` is
/// the specifier with its delimiters stripped, and `names` the companion strings the form carries, such as an alias.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RawDirective {
    /// Which of its language's directive shapes this is.
    pub form: u32,
    /// The specifier, as the resolver wants it.
    pub spec: String,
    /// The strings that come with the form.
    pub names: Vec<String>,
    /// The one-based line of the directive; 1 for a fact of the whole file, which several lines may state.
    pub line: u32,
}

impl RawDirective {
    /// A directive with no companion names.
    #[inline]
    #[must_use]
    pub const fn new(form: u32, spec: String, line: u32) -> Self {
        Self {
            form,
            spec,
            names: Vec::new(),
            line,
        }
    }
}

/// Everything a language found in one file, resolved against the file's path and the repository: what the graph
/// merges.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Extraction {
    /// The file's root-relative, forward-slash path.
    pub file: String,
    /// The definitions in the file, flags complete.
    pub symbols: Vec<Symbol>,
    /// Identities of callables. A declaration joins only the one definition with the same key whose file includes the
    /// declaration's file, directly or not, or, where includes are transitive, that the declaration's file includes;
    /// a file's includes start with itself.
    pub callables: Vec<Callable>,
    /// In-class declarations folded into out-of-line definitions in the file.
    pub folded_declarations: Vec<FoldedDeclaration>,
    /// The calls in the file, unresolved, except those in `local_calls`.
    pub calls: Vec<CallRef>,
    /// Calls the language's own scoping binds to a symbol in `local_symbols`.
    pub local_calls: Vec<LocalCall>,
    /// Uses of a name outside a call, unresolved.
    pub uses: Vec<UseRef>,
    /// The symbols declared inside a function, method or function-literal body: each is local to that body.
    ///
    /// No use binds a local, and a call in `calls` binds one only from inside its holder, the declaration whose
    /// body declares it. The holder is the nearest symbol before the local in `symbols` that this list does not name:
    /// it must be the caller of the calls in the body, or the holder of that caller where this list names the caller
    /// too. A local with no such symbol before it is its own holder, and no call of another symbol binds it.
    pub local_symbols: Vec<SymbolId>,
    /// The files this file includes, resolved.
    pub includes: Vec<IncludeRef>,
    /// Re-exports, their specifiers as written.
    pub re_exports: Vec<ReExport>,
    /// Module dependencies that name no file of the repository, kept as written.
    pub imports: Vec<ImportRef>,
    /// The names under which the file refers to the modules it imports, and where those modules are.
    pub bindings: Vec<ImportBinding>,
    /// The package or module the file declares itself part of, or that its resolver places it in, as a Cargo package
    /// holds its files. Of the owners of one name that a call's receiver names, one the caller's package declares
    /// binds before any outside the caller's directory, where a package is not a directory.
    pub package: Option<String>,
    /// The packages the file's package is built with, one per line, each as [`package`](Self::package) names it, where
    /// the resolver reads them, as Cargo manifests list a package's dependencies and theirs. Where they are read, a
    /// call binds a definition of another package by its name alone only where that package is among them and the
    /// definition is no test code; what an import or a path names binds wherever it lies. The files of one package
    /// share one list, so it is joined once per package.
    pub dependencies: Option<Arc<str>>,
    /// Compile-time implements assertions the file states.
    pub assertions: Vec<ImplementsAssertion>,
    /// Files of `includes` that this file pulls in as test-only modules. A file is test code, with every symbol in
    /// it, when every file that includes it lists it here or is such a file itself; one that a production file
    /// includes is not. Under [`IncludeReach::Transitive`](crate::IncludeReach) it must also include a test file
    /// itself, and the source of the same stem beside it, which nothing includes, does not count against it and is
    /// test code with it.
    pub test_includes: Vec<String>,
    /// What keeps the file from some builds, in the form the language's [`BuiltApart`](crate::BuiltApart) reads; `None`
    /// where nothing does.
    pub build_constraint: Option<String>,
}

impl Extraction {
    /// Adds `flags` to the flags of every symbol, as a resolver does with what the file's path shows.
    pub fn flag_symbols(&mut self, flags: SymbolFlags) {
        if flags == SymbolFlags::empty() {
            return;
        }
        for symbol in &mut self.symbols {
            symbol.flags = symbol.flags.union(flags);
        }
    }
}

/// A call, unresolved.
///
/// Build one with [`CallRef::new`] and set the fields that apply. The type is `#[non_exhaustive]`, as every record of
/// this module except [`FoldedDeclaration`] is, so it gains fields without breaking a language.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CallRef {
    /// The symbol whose body makes the call, or [`FILE_SCOPE`](crate::FILE_SCOPE) at file scope.
    pub site_of: SymbolId,
    /// The called name as written.
    pub callee: String,
    /// The type of the receiver of a member call, where the language and the call's form name one, spelled as the
    /// language's [`Receivers`](crate::Receivers) convention says. Empty for a member call on an expression the
    /// language cannot name, which is still not a bare call; `None` for a bare call.
    pub receiver: Option<String>,
    /// The one-based line of the call.
    pub line: u32,
    /// Whether the call binds apart from the definitions other calls of its name bind, under
    /// [`CallsApart::Marked`](crate::CallsApart::Marked): only a macro, which in a language with no macro leaves a site
    /// that binds no definition. Rust marks `name!()`, written beside `name()`, and TypeScript a bare call of a
    /// parameter. False for every other call.
    pub binds_apart: bool,
    /// Whether the call is written as a member call where the language reads a member alone through that form, as
    /// Rust's `x.f()` and C++'s `x.f()` and `p->f()` are: such a call binds no variable, constant, type or constructor.
    /// False where a member call can call what a module or a table holds, a class among it, as Python's `models.User()`
    /// does.
    pub calls_a_method: bool,
    /// How many arguments the call passes at least, the object of a member call among them, where the language counts
    /// them, or `0`. A call binds no definition that takes fewer ([`Symbol::most_arguments`](crate::Symbol)), and a
    /// count past `0` binds none that requires more ([`Symbol::fewest_arguments`](crate::Symbol)), so such a count
    /// holds all the arguments the call passes, up to `u8::MAX`. The graph only compares a count with a definition's, so
    /// a language in which a call of no arguments turns definitions away counts one past the written count on both
    /// sides, as DSHL does.
    pub arguments: u8,
}

impl CallRef {
    /// A call of `callee` from `site_of` on `line`, with [`binds_apart`](Self::binds_apart) and
    /// [`calls_a_method`](Self::calls_a_method) false and [`arguments`](Self::arguments) 0. Set the fields that apply.
    #[inline]
    #[must_use]
    pub const fn new(
        site_of: SymbolId,
        callee: String,
        receiver: Option<String>,
        line: u32,
    ) -> Self {
        Self {
            site_of,
            callee,
            receiver,
            line,
            binds_apart: false,
            calls_a_method: false,
            arguments: 0,
        }
    }
}

/// A call the language's own scoping binds to a local symbol in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct LocalCall {
    /// The symbol whose body makes the call, or [`FILE_SCOPE`](crate::FILE_SCOPE) at file scope.
    pub site_of: SymbolId,
    /// The called local, a symbol in the extraction's `local_symbols`.
    pub local: SymbolId,
    /// The one-based line of the call.
    pub line: u32,
}

impl LocalCall {
    /// A call of `local` from `site_of` on `line`.
    #[inline]
    #[must_use]
    pub const fn new(site_of: SymbolId, local: SymbolId, line: u32) -> Self {
        Self {
            site_of,
            local,
            line,
        }
    }
}

/// A use of a name outside a call, unresolved: a type named as one, a read or a write of a value, a member of a
/// value taken without a call.
///
/// Its [`Receiver`] decides where the name is looked up. A bare or qualified name binds no enum variant, no local
/// symbol and no field but one a class scope holds for a [`Receiver::Bare`], and a language records no use that a local
/// binding of the name covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct UseRef {
    /// The innermost symbol declared around the use, or [`FILE_SCOPE`](crate::FILE_SCOPE) at file scope, where
    /// the use binds no edge and the graph keeps only the name it spells.
    pub site_of: SymbolId,
    /// The name as written, without what it is written on.
    pub name: String,
    /// What the name is written on.
    pub receiver: Receiver,
    /// How the use touches what the name binds.
    pub access: Access,
    /// The one-based line of the use.
    pub line: u32,
}

impl UseRef {
    /// A read of `name`, written on `receiver`, from `site_of` on `line`. Set [`access`](Self::access) for any other
    /// use.
    #[inline]
    #[must_use]
    pub const fn new(site_of: SymbolId, name: String, receiver: Receiver, line: u32) -> Self {
        Self {
            site_of,
            name,
            receiver,
            access: Access::Read,
            line,
        }
    }
}

/// What a use's name is written on, which decides where the graph looks the name up: a guess would be worse than no
/// reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Receiver {
    /// Nothing: a bare name. Under [`OwnMembers::First`](crate::OwnMembers::First) it first binds the field of the
    /// enclosing member's class or of a base. Otherwise it binds as [`ValueUses`](crate::ValueUses) says: under
    /// [`Declared`](crate::ValueUses::Declared), a symbol of the same file, the same package or a dot-imported package,
    /// else the one an import of a module's file takes a bare call of the name to;
    /// under [`Imported`](crate::ValueUses::Imported), those, else an included file's export, a re-export, or an
    /// unexported namesake of one included file; under
    /// [`ImportedThenReached`](crate::ValueUses::ImportedThenReached), those, else a declaration of one file its
    /// includes reach in turn, the one variable they hold before an enum or a tie, else the one file that declares the
    /// name of those that compile in a script with the file, of several the nearest; under
    /// [`Expanded`](crate::ValueUses::Expanded), those of [`Imported`](crate::ValueUses::Imported), else the definition
    /// in [`Conventions::module_seen_everywhere`](crate::Conventions::module_seen_everywhere), else the one definition
    /// the files including the file bind the name to, else the one those files and every file a re-export hands the
    /// file to bind it to; under [`Included`](crate::ValueUses::Included), the variable, constant or function of the
    /// name in the file, then in its includes, then in the files those reach.
    Bare,
    /// A qualifier as written, `pkg` in `pkg.Name`, which binds only through the file's imports.
    Qualifier(String),
    /// The enclosing member's own receiver: `this`, `self`, or none written where the language reads a bare name as a
    /// member of it. The name binds a member of the enclosing member's owner.
    Own,
    /// A value of the type the path spells, as the language's [`Receivers`](crate::Receivers) convention says, or
    /// empty where the language cannot name that type. The name binds a member: a field, or a method taken as a value.
    Path(String),
}

/// How a use touches what its name binds.
///
/// Exhaustive on purpose: a use reads, assigns, does both or does neither, so the set is closed, and a `match` on it
/// in another crate stays checked by the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Access {
    /// Reads it: takes its value, or a member through it, as `EventType.Created` reads the type `EventType`.
    Read,
    /// Assigns it without reading it: the left side of `=`, a key of a literal that builds its owner.
    Write,
    /// Reads and assigns it, as `+=` and `++` do.
    Update,
    /// Names it as a type, as a declaration, a signature, an annotation or a generic argument does, which reads no
    /// value.
    Type,
}

impl Access {
    /// The word for the access in an answer: `read`, `write`, `update` or `type`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Update => "update",
            Self::Type => "type",
        }
    }

    /// Whether the use reads the value: a read or an update.
    #[must_use]
    pub const fn reads(self) -> bool {
        matches!(self, Self::Read | Self::Update)
    }

    /// Whether the use assigns the value: a write or an update.
    #[must_use]
    pub const fn writes(self) -> bool {
        matches!(self, Self::Write | Self::Update)
    }
}

/// A file this file includes: its root-relative path, resolved by the language. A target outside the graph's files
/// adds no edge.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct IncludeRef {
    /// The included file's root-relative, forward-slash path.
    pub target: String,
}

impl IncludeRef {
    /// An include of the file at `target`.
    #[inline]
    #[must_use]
    pub const fn new(target: String) -> Self {
        Self { target }
    }
}

/// A module dependency that names no file of the repository — a standard library module, an external package —
/// kept as written for the questions about what a file depends on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ImportRef {
    /// The module as written, its delimiters stripped.
    pub module: String,
    /// The names a from-style import takes; empty when the module itself is what binds.
    pub names: Vec<String>,
    /// The one-based line of the import.
    pub line: u32,
}

impl ImportRef {
    /// An import of `module`, taking `names`, on `line`.
    #[inline]
    #[must_use]
    pub const fn new(module: String, names: Vec<String>, line: u32) -> Self {
        Self {
            module,
            names,
            line,
        }
    }
}

/// The name under which the file refers to a module it imports, and where in the repository that module lives.
///
/// A binding whose `target_dir` is `None` names code outside the repository: knowing that lets a call through it stop
/// rather than guess.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ImportBinding {
    /// The name as the import writes it — an alias, `.` for an import into the file's own scope, `_` for one that binds
    /// nothing — or `None` when the import binds the module's own name.
    pub alias: Option<String>,
    /// The imported module's root-relative directory, or its file where a module is one file, as a Rust module is, or
    /// `None` when it lies outside the repository. A call through a file binds what that file exports at its top
    /// level, and so does a call of the bound name itself.
    pub target_dir: Option<String>,
    /// The name the import binds when no alias is written and the module's declared name is unknown: the specifier's
    /// last path segment, or the segment before it where the language's convention skips a version suffix, `yaml` for
    /// Go's `gopkg.in/yaml.v3`.
    pub last_segment: String,
}

impl ImportBinding {
    /// A binding of the module at `target_dir`, under `alias` where the import writes one.
    #[inline]
    #[must_use]
    pub const fn new(
        alias: Option<String>,
        target_dir: Option<String>,
        last_segment: String,
    ) -> Self {
        Self {
            alias,
            target_dir,
            last_segment,
        }
    }
}

/// A compile-time assertion that a type implements an interface. It declares nothing of the type, so it is no parent
/// the type's members come from.
///
/// The graph reads both names from the asserting file by the rules it reads a parent the file writes by: where a
/// package is a directory, a bare name in the file's own package and a qualified one in the package its import binds.
/// A bare type name the asserting file declares names that type. Any other type name that reaches no type, or several,
/// adds nothing. A name with a file beside it, which only a resolver sets, is read from that file instead, as a Rust
/// path that reaches a module is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ImplementsAssertion {
    /// The implementing type as written: `T` or `pkg.T`.
    pub type_name: String,
    /// The interface as written: `I` or `pkg.I`.
    pub interface: String,
    /// The root-relative path of the file `type_name` is read from, where it is not the asserting file.
    pub type_file: Option<String>,
    /// The root-relative path of the file `interface` is read from, where it is not the asserting file.
    pub interface_file: Option<String>,
}

impl ImplementsAssertion {
    /// An assertion that `type_name` implements `interface`, each read from the asserting file.
    #[must_use]
    pub const fn new(type_name: String, interface: String) -> Self {
        Self {
            type_name,
            interface,
            type_file: None,
            interface_file: None,
        }
    }
}

/// A name a module re-exports from another, as the bytes write it: `export { Foo } from './foo'`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RawReExport {
    /// The name as re-exported, or `*` for all of them.
    pub local_name: String,
    /// The name in the module it comes from.
    pub source_name: String,
    /// That module's specifier as written.
    pub source_path: String,
}

impl RawReExport {
    /// A re-export of `source_name`, from the module `source_path` spells, as `local_name`.
    #[inline]
    #[must_use]
    pub const fn new(local_name: String, source_name: String, source_path: String) -> Self {
        Self {
            local_name,
            source_name,
            source_path,
        }
    }
}

/// A re-export with the file its specifier names, which only the resolver can know.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ReExport {
    /// The name as re-exported, or `*` for all of them.
    pub local_name: String,
    /// The name in the module it comes from.
    pub source_name: String,
    /// That module's specifier as written.
    pub source_path: String,
    /// The root-relative path of the file the specifier names, or `None` where it names no file of the repository.
    pub source_file: Option<String>,
}

impl From<RawReExport> for ReExport {
    fn from(raw: RawReExport) -> Self {
        let RawReExport {
            local_name,
            source_name,
            source_path,
        } = raw;
        Self {
            local_name,
            source_name,
            source_path,
            source_file: None,
        }
    }
}

/// The identity of a callable, established by the language without guessing: one record at most per symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Callable {
    /// The callable's symbol.
    pub symbol: SymbolId,
    /// The language's identity for it, scope and overload included.
    pub key: String,
    /// Whether this occurrence declares the callable or defines it.
    pub role: CallableRole,
}

impl Callable {
    /// The identity `key` of `symbol`, the occurrence `role` says.
    #[inline]
    #[must_use]
    pub const fn new(symbol: SymbolId, key: String, role: CallableRole) -> Self {
        Self { symbol, key, role }
    }
}

/// Which occurrence of a callable a symbol is.
///
/// Exhaustive on purpose: an occurrence has a body or has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallableRole {
    /// A declaration, without a body.
    Declaration,
    /// An occurrence with a body.
    Definition,
}

/// An in-class declaration of a member that the language folds into the out-of-line definition of the same callable
/// in the same file, so the file keeps one symbol per callable.
///
/// Exhaustive on purpose: the two fields locate the declaration and the definition that replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FoldedDeclaration {
    /// The out-of-line definition.
    pub definition: SymbolId,
    /// The one-based line of the folded declaration, in the class's body.
    pub line: u32,
}

/// A doctest is a crate of its own, so each of these fails to compile for as long as the record stays
/// `#[non_exhaustive]`: a field added in a minor release breaks no language crate.
///
/// ```compile_fail,E0639
/// use kartta_lang::{CallRef, SymbolId};
/// let _ = CallRef {
///     site_of: SymbolId(0),
///     callee: String::new(),
///     receiver: None,
///     line: 1,
///     binds_apart: false,
///     calls_a_method: false,
///     arguments: 0,
/// };
/// ```
///
/// ```compile_fail,E0639
/// use kartta_lang::{SymbolId, LocalCall};
/// let _ = LocalCall { site_of: SymbolId(0), local: SymbolId(1), line: 1 };
/// ```
///
/// ```compile_fail,E0639
/// use kartta_lang::{Access, Receiver, SymbolId, UseRef};
/// let _ = UseRef {
///     site_of: SymbolId(0),
///     name: String::new(),
///     receiver: Receiver::Bare,
///     access: Access::Read,
///     line: 1,
/// };
/// ```
///
/// ```compile_fail,E0639
/// let _ = kartta_lang::RawDirective { form: 0, spec: String::new(), names: Vec::new(), line: 1 };
/// ```
///
/// ```compile_fail,E0639
/// let _ = kartta_lang::IncludeRef { target: String::new() };
/// ```
///
/// ```compile_fail,E0639
/// let _ = kartta_lang::ImportRef { module: String::new(), names: Vec::new(), line: 1 };
/// ```
///
/// ```compile_fail,E0639
/// let _ = kartta_lang::ImportBinding { alias: None, target_dir: None, last_segment: String::new() };
/// ```
///
/// ```compile_fail,E0639
/// let _ = kartta_lang::RawReExport {
///     local_name: String::new(),
///     source_name: String::new(),
///     source_path: String::new(),
/// };
/// ```
///
/// ```compile_fail,E0639
/// let _ = kartta_lang::ReExport {
///     local_name: String::new(),
///     source_name: String::new(),
///     source_path: String::new(),
///     source_file: None,
/// };
/// ```
///
/// ```compile_fail,E0639
/// use kartta_lang::{Callable, CallableRole, SymbolId};
/// let _ = Callable { symbol: SymbolId(0), key: String::new(), role: CallableRole::Definition };
/// ```
///
/// ```compile_fail,E0639
/// let _ = kartta_lang::ImplementsAssertion {
///     type_name: String::new(),
///     interface: String::new(),
///     type_file: None,
///     interface_file: None,
/// };
/// ```
#[cfg(doctest)]
struct OpenRecords;
