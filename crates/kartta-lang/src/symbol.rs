use std::fmt;

/// A symbol's position in the [`RawExtraction::symbols`](crate::RawExtraction::symbols) of its own file.
///
/// A record that names another symbol of the same file — the caller of a call, the holder of a local — names it by
/// this id. The graph rebases every id when it merges the file, so an id means nothing outside its extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolId(pub u32);

/// The `site_of` of a call or a use made outside every symbol, at file scope. Such a site never becomes an edge from
/// nowhere: the graph keeps only the name it spells.
pub const FILE_SCOPE: SymbolId = SymbolId(u32::MAX);

/// What a definition is.
///
/// The set is shared by every language, so a language picks the kind whose role matches rather than its own keyword:
/// a Rust trait is an [`SymbolKind::Interface`], an implementable contract, and a PHP trait a [`SymbolKind::Trait`], a
/// reusable bundle of methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SymbolKind {
    /// A free function.
    Function,
    /// A function declared on a type.
    Method,
    /// A class.
    Class,
    /// A struct, or a union.
    Struct,
    /// A namespace or a module that names a scope.
    Namespace,
    /// An interface, or any contract a type implements.
    Interface,
    /// A variable.
    Variable,
    /// A constant.
    Constant,
    /// An enum type.
    Enum,
    /// A named alias of another type.
    TypeAlias,
    /// A bundle of methods that types mix in.
    Trait,
    /// A macro definition.
    Macro,
    /// A data member of a type.
    Field,
    /// One variant of an enum type.
    EnumVariant,
    /// A new type defined over another type and of none of the kinds above, as Go's `type Celsius float64` or
    /// `type HandlerFunc func(w Writer)`: it has a method set of its own, where an alias names the type it aliases.
    DefinedType,
}

impl SymbolKind {
    /// Every kind, in the order the enum declares them: a slice, whose type a new kind leaves as it is.
    pub const ALL: &[Self] = &[
        Self::Function,
        Self::Method,
        Self::Class,
        Self::Struct,
        Self::Namespace,
        Self::Interface,
        Self::Variable,
        Self::Constant,
        Self::Enum,
        Self::TypeAlias,
        Self::Trait,
        Self::Macro,
        Self::Field,
        Self::EnumVariant,
        Self::DefinedType,
    ];

    /// A short label for the kind, as answers print it: `func`, `iface`, `variant`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Function => "func",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Namespace => "ns",
            Self::Interface => "iface",
            Self::Variable => "var",
            Self::Constant => "const",
            Self::Enum => "enum",
            Self::TypeAlias => "alias",
            Self::Trait => "trait",
            Self::Macro => "macro",
            Self::Field => "field",
            Self::EnumVariant => "variant",
            Self::DefinedType => "type",
        }
    }

    /// Whether a symbol of this kind is a type, whose uses as one a graph records where its
    /// [`TypeUses`](crate::TypeUses) convention says: a class, a struct, an interface, a trait, an enum, a type alias
    /// or a defined type.
    #[must_use]
    pub const fn is_type(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Struct
                | Self::Interface
                | Self::Trait
                | Self::Enum
                | Self::TypeAlias
                | Self::DefinedType
        )
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// What a symbol is for besides its kind: test code, generated code, vendored code, an entry point, a fallback, a
/// definition reflection reaches or a macro invocation writes, a label.
///
/// A language sets these by its own conventions — a Go `_test.go` file, a Rust `#[test]`, a generated-code banner, a
/// `vendor` directory — so they are heuristic: they rank answers, a caller that is test code below one that is not,
/// and a query that leaves flagged symbols out, as a dead-code query does, counts what it left out. A fallback does
/// neither: it makes a call bound to it uncertain, so the namesakes such a call may run are not judged dead. A
/// definition reflection reaches ranks nothing either: a dead-code query counts it as reflected where no recorded use
/// reaches it. A definition a macro invocation writes binds a call only where no written definition does. A label
/// keeps every call and use of its name off the symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SymbolFlags(u8);

impl SymbolFlags {
    const TEST: u8 = 1 << 0;
    const ENTRY: u8 = 1 << 1;
    const GENERATED: u8 = 1 << 2;
    const VENDORED: u8 = 1 << 3;
    const FALLBACK: u8 = 1 << 4;
    const REFLECTED: u8 = 1 << 5;
    const MADE: u8 = 1 << 6;
    const LABEL: u8 = 1 << 7;

    /// No flag set: an ordinary definition.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Whether the symbol is test code.
    #[must_use]
    pub const fn is_test(self) -> bool {
        self.0 & Self::TEST != 0
    }

    /// Whether the symbol is an entry point: a definition that runs with no call that spells its name, as a program's
    /// `main`, which the runtime starts, a function an annotation registers with what calls it, and a method the
    /// language calls for an operator, a statement, an attribute read or an object it makes or ends.
    #[must_use]
    pub const fn is_entry(self) -> bool {
        self.0 & Self::ENTRY != 0
    }

    /// Whether the symbol is generated code.
    #[must_use]
    pub const fn is_generated(self) -> bool {
        self.0 & Self::GENERATED != 0
    }

    /// Whether the symbol is third-party code the repository carries, as a Go `vendor` directory or an npm
    /// `node_modules` holds.
    #[must_use]
    pub const fn is_vendored(self) -> bool {
        self.0 & Self::VENDORED != 0
    }

    /// Whether the symbol is a fallback: a definition a build makes only where it holds no other of the name, as a
    /// DSHL `define_macro_if_not_defined`, so a call bound to it may run a namesake a build defines first.
    #[must_use]
    pub const fn is_fallback(self) -> bool {
        self.0 & Self::FALLBACK != 0
    }

    /// Whether the language's runtime reads or writes the symbol by its name, through reflection, as Go's
    /// `encoding/json` does a field its struct tag names, so the uses the extraction records are not all of them.
    #[must_use]
    pub const fn is_reflected(self) -> bool {
        self.0 & Self::REFLECTED != 0
    }

    /// Whether a macro invocation writes the symbol, as a Rust `macro_rules!` arm's `struct $name;` does, so a written
    /// definition of its name is likelier the one a call means.
    #[must_use]
    pub const fn is_made(self) -> bool {
        self.0 & Self::MADE != 0
    }

    /// Whether the symbol's name is a label that no code spells to reach it, as a test registration's description,
    /// which a test runner calls through the registration, so no call or use of the name binds the symbol.
    #[must_use]
    pub const fn is_label(self) -> bool {
        self.0 & Self::LABEL != 0
    }

    /// These flags with test code set to `yes`.
    #[must_use]
    pub const fn with_test(self, yes: bool) -> Self {
        self.set(Self::TEST, yes)
    }

    /// These flags with the entry point set to `yes`.
    #[must_use]
    pub const fn with_entry(self, yes: bool) -> Self {
        self.set(Self::ENTRY, yes)
    }

    /// These flags with generated code set to `yes`.
    #[must_use]
    pub const fn with_generated(self, yes: bool) -> Self {
        self.set(Self::GENERATED, yes)
    }

    /// These flags with vendored code set to `yes`.
    #[must_use]
    pub const fn with_vendored(self, yes: bool) -> Self {
        self.set(Self::VENDORED, yes)
    }

    /// These flags with the fallback set to `yes`.
    #[must_use]
    pub const fn with_fallback(self, yes: bool) -> Self {
        self.set(Self::FALLBACK, yes)
    }

    /// These flags with reflection set to `yes`.
    #[must_use]
    pub const fn with_reflected(self, yes: bool) -> Self {
        self.set(Self::REFLECTED, yes)
    }

    /// These flags with a macro invocation's making set to `yes`.
    #[must_use]
    pub const fn with_made(self, yes: bool) -> Self {
        self.set(Self::MADE, yes)
    }

    /// These flags with the label set to `yes`.
    #[must_use]
    pub const fn with_label(self, yes: bool) -> Self {
        self.set(Self::LABEL, yes)
    }

    /// The flags set in either: how a resolver adds what a file's path shows to what its bytes showed.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn bits(self) -> u8 {
        self.0
    }

    pub(crate) const ALL: u8 = Self::TEST
        | Self::ENTRY
        | Self::GENERATED
        | Self::VENDORED
        | Self::FALLBACK
        | Self::REFLECTED
        | Self::MADE
        | Self::LABEL;

    pub(crate) const fn from_bits(bits: u8) -> Self {
        Self(bits & Self::ALL)
    }

    const fn set(self, bit: u8, yes: bool) -> Self {
        if yes {
            Self(self.0 | bit)
        } else {
            Self(self.0 & !bit)
        }
    }
}

/// One definition a language found in a file.
///
/// The file is named once, by the [`Extraction`](crate::Extraction) that holds the symbol. Build one with
/// [`Symbol::new`] and set the fields that apply; the type is `#[non_exhaustive]`, so it gains fields without
/// breaking a language.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Symbol {
    /// The identifier as written: `Parse`.
    pub name: String,
    /// The identifier qualified by what encloses it, as the language writes it: `json.Parse`, `Class::method`. Equal
    /// to `name` where the language qualifies nothing.
    pub qualified_name: String,
    /// The type or namespace that declares a member, as written.
    pub owner: Option<String>,
    /// The types this definition extends or implements, as written; the implementations of a type are found from
    /// these.
    pub parents: Vec<String>,
    /// The one-based line the definition starts on.
    pub line: u32,
    /// The one-based line the definition ends on.
    pub end_line: u32,
    /// What the definition is.
    pub kind: SymbolKind,
    /// Whether the definition is visible outside its module, by the language's rules.
    pub exported: bool,
    /// The declaration's source text up to its body, as written but without its comments, or `None` where the
    /// language keeps none for this kind.
    pub signature: Option<String>,
    /// The types of the values the definition yields, each a type path as
    /// [`Receivers::TypePaths`](crate::Receivers::TypePaths) spells one, read in the scope of the definition's file
    /// and, for a local, of its holder's body: a variable's or a field's type, a function's results in order, an empty
    /// path for one the language cannot name, and the type an alias names. Empty where the language records none, and
    /// for a function none of whose results has a path, since an absent result reads as an empty path does. Where
    /// receivers are [`Receivers::Owners`](crate::Receivers::Owners), a field's is the owner its declaration names,
    /// spelled as a receiver names one, after a `*` where the field holds a pointer to a value of it, and for a value
    /// of a class template, after a `*`, the owner its first template argument names, which its `operator->` reaches.
    /// Where they are [`Receivers::ReturnPaths`](crate::Receivers::ReturnPaths), a function's is the owner its return
    /// type names, after a `?` where an `Option`, a `Result` or a `Promise` holds it. Where they are
    /// [`Receivers::TypePaths`](crate::Receivers::TypePaths), a type defined over a named type records that type after
    /// a `~`, and a function type, defined or aliased, records its results after one `()` path, which tells them from
    /// the path a type or an alias names. Where they are
    /// [`Receivers::InheritedFields`](crate::Receivers::InheritedFields), a field's is the class it holds, written from
    /// the root, or a receiver the language lists for the caller's own type. Where they are
    /// [`Receivers::PastedBodies`](crate::Receivers::PastedBodies), a definition in such a body records its name. Under
    /// [`ReceiverMembers::InheritedUpToAnyField`](crate::ReceiverMembers::InheritedUpToAnyField), one empty path marks a
    /// field that a class only declares and holds no value of. Where they are
    /// [`BareReceivers::Constants`](crate::BareReceivers::Constants), a constant's is the class it holds, spelled as a
    /// call's receiver names one, after which a call on the constant binds as on that receiver, and one empty path
    /// marks a value of a class the extraction cannot name, a call on which binds only as a guess. A constant with
    /// none keeps the tiers of any value.
    pub value_types: Vec<String>,
    /// What the symbol is for besides its kind.
    pub flags: SymbolFlags,
    /// The most arguments the definition takes, besides the object a method is called on, where the language counts
    /// them; `u8::MAX` for any number. A call that passes more ([`CallRef::arguments`](crate::CallRef), which also says
    /// how a language may shift both counts) does not bind the definition.
    pub most_arguments: u8,
    /// The fewest arguments a call must pass the definition, besides the object a method is called on, where the
    /// language counts them; `0` where it does not. A call that counts its arguments and passes fewer
    /// ([`CallRef::arguments`](crate::CallRef)) does not bind the definition.
    pub fewest_arguments: u8,
}

impl Symbol {
    /// A symbol named `name`, of `kind`, spanning `line` to `end_line`, qualified by nothing, owned by nothing, and
    /// not exported.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: SymbolKind, line: u32, end_line: u32) -> Self {
        let name = name.into();

        Self {
            qualified_name: name.clone(),
            name,
            owner: None,
            parents: Vec::new(),
            line,
            end_line,
            kind,
            exported: false,
            signature: None,
            value_types: Vec::new(),
            flags: SymbolFlags::empty(),
            most_arguments: u8::MAX,
            fewest_arguments: 0,
        }
    }
}
