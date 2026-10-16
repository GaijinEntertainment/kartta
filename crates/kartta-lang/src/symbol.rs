use std::fmt;

/// A symbol's position in the [`RawExtraction::symbols`](crate::RawExtraction::symbols) of its own file.
///
/// A record that names another symbol of the same file — the caller of a call, the holder of a local — names it by
/// this id. The graph rebases every id when it merges the file, so an id means nothing outside its extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolId(pub u32);

/// The caller of a call made outside any symbol, at file scope. The graph drops such a call, so it never becomes an
/// edge from nowhere.
pub const NO_CALLER_ID: SymbolId = SymbolId(u32::MAX);

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
}

impl SymbolKind {
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
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// What a symbol is for besides its kind: test code, generated code, a program's entry point.
///
/// A language sets these by its own conventions — a Go `_test.go` file, a Rust `#[test]`, a generated-code banner —
/// so they are heuristic: they rank answers, a caller that is test code below one that is not, and never decide what
/// an answer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SymbolFlags(u8);

impl SymbolFlags {
    const TEST: u8 = 1 << 0;
    const ENTRY: u8 = 1 << 1;
    const GENERATED: u8 = 1 << 2;

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

    /// Whether the symbol is a program's entry point.
    #[must_use]
    pub const fn is_entry(self) -> bool {
        self.0 & Self::ENTRY != 0
    }

    /// Whether the symbol is generated code.
    #[must_use]
    pub const fn is_generated(self) -> bool {
        self.0 & Self::GENERATED != 0
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

    /// The flags set in either: how a resolver adds what a file's path shows to what its bytes showed.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) const fn bits(self) -> u8 {
        self.0
    }

    pub(crate) const ALL: u8 = Self::TEST | Self::ENTRY | Self::GENERATED;

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
    /// The declaration's source text up to its body, as written, or `None` where the language keeps none for this
    /// kind.
    pub signature: Option<String>,
    /// What the symbol is for besides its kind.
    pub flags: SymbolFlags,
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
            flags: SymbolFlags::empty(),
        }
    }
}
