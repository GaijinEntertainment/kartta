use kartta_lang::{Receivers, SymbolKind};

use crate::resolve::{Cascade, derived};
use crate::stored::{StoredSymbol, SymbolAt, VALUE_TYPE_SEPARATOR};
use crate::strings::Str;

/// What a receiver that spells a type path binds a member call to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typed {
    /// The member the call names, of the receiver's type or of a type it embeds.
    Member(SymbolAt),
    /// A type that an import of code outside the graph declares, or a member such a type may give.
    Imported,
    /// A name that nothing the graph holds declares, a builtin type among them.
    Undeclared,
    /// A type the graph holds, with no member of the name the site can bind: a method for a call, any member for a use.
    NoMethod,
    /// A path the graph cannot follow, which leaves the call to the tiers that read no type.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value<'g> {
    Package(Str),
    Type(SymbolAt),
    Callable(SymbolAt),
    Container {
        body: SymbolAt,
        element: &'g str,
    },
    /// A name several files of a package declare, read where no file of them is the reader's own.
    Declarations(PackageName),
    Imported,
    Undeclared,
    Unknown,
}

/// A name a package declares, as the file `viewer` sees it, or as an import does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PackageName {
    dir: Str,
    name: Str,
    viewer: Option<u32>,
}

const MAX_DEPTH: u8 = 16;

const UNDERLYING: &str = "~";

enum Step<'p> {
    Select(&'p str),
    Result(usize),
    Element,
}

fn head(path: &str) -> (&str, &str) {
    let end = path.find(['.', '(', '[']).unwrap_or(path.len());
    path.split_at(end)
}

fn step(rest: &str) -> Option<(Step<'_>, &str)> {
    if let Some(after) = rest.strip_prefix('.') {
        let (name, rest) = head(after);
        return (!name.is_empty()).then_some((Step::Select(name), rest));
    }
    if let Some(after) = rest.strip_prefix("()") {
        let Some(numbered) = after.strip_prefix('#') else {
            return Some((Step::Result(0), after));
        };
        let digits = numbered
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(numbered.len());
        let index = numbered[..digits].parse().ok()?;
        return Some((Step::Result(index), &numbered[digits..]));
    }
    rest.strip_prefix("[]").map(|after| (Step::Element, after))
}

impl<'g> Cascade<'g> {
    pub(crate) fn reads_type_paths(&self) -> bool {
        self.conventions.receivers == Receivers::TypePaths
    }

    pub(crate) fn typed_member(
        &self,
        (reader, path): (SymbolAt, Str),
        name: Str,
        binds: impl Fn(&StoredSymbol) -> bool,
    ) -> Typed {
        match self.receiver_type(self.body_of(reader), path) {
            Value::Type(ty) => match self.member(ty, name, 0) {
                Found::Member(at) if binds(self.graph.sym(at)) => Typed::Member(at),
                Found::Member(_) | Found::Nothing => Typed::NoMethod,
                Found::Outside => Typed::Imported,
                Found::Unknown => Typed::Unknown,
            },
            Value::Imported => Typed::Imported,
            Value::Undeclared => Typed::Undeclared,
            Value::Package(_)
            | Value::Callable(_)
            | Value::Container { .. }
            | Value::Declarations(_)
            | Value::Unknown => Typed::Unknown,
        }
    }

    // constraint: a local's holder is the outermost declaration around it, so a closure's body is its enclosing
    // function's
    fn body_of(&self, at: SymbolAt) -> SymbolAt {
        self.holder(at).unwrap_or(at)
    }

    fn receiver_type(&self, body: SymbolAt, receiver: Str) -> Value<'g> {
        derived(&self.receiver_types, (body, receiver), || {
            self.evaluate(body, self.graph.strings.resolve(receiver), 0)
        })
    }

    fn evaluate(&self, body: SymbolAt, path: &'g str, depth: u8) -> Value<'g> {
        if depth > MAX_DEPTH || path.is_empty() {
            return Value::Unknown;
        }
        if let Some(element) = path.strip_prefix("[]") {
            return Value::Container { body, element };
        }
        let (name, rest) = head(path);
        let value = self
            .anonymous_host(body.file, name)
            .unwrap_or_else(|| self.in_scope(body, name));
        self.along(value, rest, depth)
    }

    fn along(&self, mut value: Value<'g>, mut rest: &'g str, depth: u8) -> Value<'g> {
        loop {
            if let Value::Declarations(read) = value {
                return self.highest_ranked(read, rest, depth);
            }
            if rest.is_empty() {
                return value;
            }
            let Some((step, after)) = step(rest) else {
                return Value::Unknown;
            };
            value = match step {
                Step::Select(name) => self.select(value, name, depth),
                Step::Result(index) => self.result(value, index, depth),
                Step::Element => self.element(value, depth),
            };
            if matches!(value, Value::Unknown | Value::Imported | Value::Undeclared) {
                return value;
            }
            rest = after;
        }
    }

    // constraint: a build compiles one of the declarations and the graph cannot tell which, so the path goes on through
    // each, and what they reach decides by what it is and never by the path of its file
    fn highest_ranked(&self, read: PackageName, rest: &'g str, depth: u8) -> Value<'g> {
        self.declarers(read)
            .map(|at| self.along(self.value_of(at, depth), rest, depth))
            .max_by_key(|&value| self.value_rank(value, depth))
            .unwrap_or(Value::Unknown)
    }

    // constraint: an alias of one name in each file can name a held type in one and an import's in another
    fn value_rank(&self, value: Value<'g>, depth: u8) -> (u8, &'g str) {
        let strings = &self.graph.strings;
        match value {
            Value::Type(at)
                if self.graph.sym(at).kind == SymbolKind::TypeAlias && depth < MAX_DEPTH =>
            {
                self.value_rank(self.value_type(at, 0, depth), depth + 1)
            }
            Value::Type(at) | Value::Callable(at) => {
                (3, strings.resolve(self.graph.sym(at).qualified_name))
            }
            Value::Container { element, .. } => (3, element),
            Value::Package(dir) => (3, strings.resolve(dir)),
            Value::Imported => (2, ""),
            Value::Unknown | Value::Declarations(_) => (1, ""),
            Value::Undeclared => (0, ""),
        }
    }

    // constraint: a path names an anonymous struct by the index, in braces, of the field or the variable of its file
    // that the struct types, which owns the struct's fields; a path is read in the file that spells it (ADR 3.170)
    fn anonymous_host(&self, file: u32, name: &str) -> Option<Value<'g>> {
        let index: u32 = name.strip_prefix('{')?.strip_suffix('}')?.parse().ok()?;
        let held = (index as usize) < self.graph.file(file).symbols.len();
        Some(if held {
            Value::Type(SymbolAt::new(file, index))
        } else {
            Value::Unknown
        })
    }

    // constraint: a name of a type position that neither the file's package nor an import declares is predeclared,
    // from a dot import of code outside the graph, or from a file the graph does not hold
    fn in_scope(&self, body: SymbolAt, name: &str) -> Value<'g> {
        let file = body.file;
        let Some(key) = self.graph.strings.get(name) else {
            return Value::Undeclared;
        };
        if let Some(target) = self.binding(file, key) {
            return target.map_or(Value::Imported, Value::Package);
        }
        let own = self.graph.file(file).dir;
        if let Some(declared) = self.package_level(own, key, Some(file)) {
            return declared;
        }
        for dir in self.dot_imported_dirs(file).iter().copied() {
            if let Some(declared) = self.package_level(dir, key, None) {
                return declared;
            }
        }
        self.local_type(body, key).unwrap_or(Value::Undeclared)
    }

    // constraint: two files declare one name only when build constraints keep them apart, so a file's own declaration
    // is the one it is built with
    fn package_level(&self, dir: Str, name: Str, viewer: Option<u32>) -> Option<Value<'g>> {
        let read = PackageName { dir, name, viewer };
        let own = viewer.and_then(|slot| self.declaration_in(read, slot));
        if let Some(own) = own {
            return Some(self.value_of(own, 0));
        }
        let mut declared = self.declarers(read);
        let first = declared.next()?;
        Some(if declared.next().is_some() {
            Value::Declarations(read)
        } else {
            self.value_of(first, 0)
        })
    }

    // constraint: a file sees the package its own package clause names, its test files only from a test file; an
    // import sees the package's other files
    fn declarers(&self, read: PackageName) -> impl Iterator<Item = SymbolAt> {
        let package = read.viewer.map(|slot| self.graph.file(slot).package);
        (self.graph.package_files(read.dir).iter())
            .filter(move |&&slot| package.is_none_or(|own| self.graph.file(slot).package == own))
            .filter_map(move |&slot| self.declaration_in(read, slot))
    }

    fn declaration_in(&self, read: PackageName, slot: u32) -> Option<SymbolAt> {
        let sees_tests = read.viewer.is_some_and(|viewer| {
            let symbols = &self.graph.file(viewer).symbols;
            symbols.first().is_some_and(|first| first.flags.is_test())
        });
        let named = self.named(read.name);
        self.graph.in_file(named, slot).iter().copied().find(|&at| {
            let symbol = self.graph.sym(at);
            symbol.owner.is_none()
                && self.holder(at).is_none()
                && (sees_tests || !symbol.flags.is_test())
        })
    }

    // constraint: a type declared in a body is named only inside that body, and a body rarely declares two types of one
    // name
    fn local_type(&self, body: SymbolAt, name: Str) -> Option<Value<'g>> {
        self.graph
            .in_file(self.named(name), body.file)
            .iter()
            .copied()
            .find(|&at| self.graph.sym(at).kind.is_type() && self.holder(at) == Some(body))
            .map(Value::Type)
    }

    fn value_of(&self, at: SymbolAt, depth: u8) -> Value<'g> {
        match self.graph.sym(at).kind {
            SymbolKind::Function | SymbolKind::Method => Value::Callable(at),
            kind if kind.is_type() => Value::Type(at),
            _ => self.value_type(at, 0, depth),
        }
    }

    fn value_type(&self, at: SymbolAt, index: usize, depth: u8) -> Value<'g> {
        let Some(types) = self.graph.file(at.file).value_types(at.index) else {
            return Value::Unknown;
        };
        let types = self.graph.strings.resolve(types);
        types
            .split(VALUE_TYPE_SEPARATOR)
            .nth(index)
            .map_or(Value::Unknown, |path| {
                self.evaluate(self.body_of(at), path, depth + 1)
            })
    }

    fn select(&self, value: Value<'g>, name: &str, depth: u8) -> Value<'g> {
        match value {
            Value::Package(dir) => self
                .graph
                .strings
                .get(name)
                .and_then(|key| self.package_level(dir, key, None))
                .unwrap_or(Value::Unknown),
            Value::Type(ty) => self.member_value(ty, name, depth),
            Value::Imported | Value::Undeclared => value,
            Value::Callable(_)
            | Value::Container { .. }
            | Value::Declarations(_)
            | Value::Unknown => Value::Unknown,
        }
    }

    // constraint: calling a function type yields its results, which its value types give, and calling any other type
    // converts a value to it
    fn result(&self, value: Value<'g>, index: usize, depth: u8) -> Value<'g> {
        match value {
            Value::Callable(at) => self.value_type(at, index, depth),
            Value::Type(ty) if self.is_function_type(ty) => self.value_type(ty, index, depth),
            Value::Type(_) if index == 0 => value,
            Value::Imported | Value::Undeclared => value,
            _ => Value::Unknown,
        }
    }

    fn is_function_type(&self, ty: SymbolAt) -> bool {
        matches!(
            self.graph.sym(ty).kind,
            SymbolKind::TypeAlias | SymbolKind::DefinedType
        ) && self
            .graph
            .file(ty.file)
            .value_types(ty.index)
            .is_some_and(|types| {
                let types = self.graph.strings.resolve(types);
                !types.starts_with("[]") && !types.starts_with(UNDERLYING)
            })
    }

    fn element(&self, value: Value<'g>, depth: u8) -> Value<'g> {
        match value {
            Value::Container { body, element } => self.evaluate(body, element, depth + 1),
            Value::Type(ty) => match self.value_type(ty, 0, depth) {
                Value::Container { body, element } => self.evaluate(body, element, depth + 1),
                _ => Value::Unknown,
            },
            Value::Imported | Value::Undeclared => value,
            _ => Value::Unknown,
        }
    }

    fn member_value(&self, ty: SymbolAt, name: &str, depth: u8) -> Value<'g> {
        let Some(key) = self.graph.strings.get(name) else {
            return Value::Unknown;
        };
        match self.member(ty, key, depth) {
            Found::Member(at) => self.value_of(at, depth),
            Found::Outside => Value::Imported,
            Found::Nothing | Found::Unknown => self.embedded_named(ty, name, depth),
        }
    }

    // constraint: an embedded field is named by its type's name, `x.Base` for an embedded `Base` or `*pkg.Base`
    fn embedded_named(&self, ty: SymbolAt, name: &str, depth: u8) -> Value<'g> {
        let mut walk = Embedded::new(ty);
        while let Some(next) = walk.next() {
            let named = self
                .graph
                .file(next.file)
                .parents_of(next.index)
                .iter()
                .map(|parent| self.graph.strings.resolve(parent.written))
                .find(|written| written.rsplit('.').next() == Some(name));
            if let Some(written) = named {
                return self.evaluate(self.body_of(next), written, depth + 1);
            }
            self.embed(&mut walk, next, depth);
        }
        Value::Unknown
    }

    // constraint: an embedded type's members are promoted, the shallowest first, and a member of a type that the graph
    // does not hold, or that a path the graph cannot follow names, may be the one the name reaches
    fn member(&self, ty: SymbolAt, name: Str, depth: u8) -> Found {
        let named = self.named(name);
        let mut walk = Embedded::new(ty);
        while let Some(next) = walk.next() {
            if let Some(at) = self.declared_member(next, named) {
                return Found::Member(at);
            }
            self.embed(&mut walk, next, depth);
        }
        if walk.reaches_outside {
            Found::Outside
        } else if walk.reaches_unknown {
            Found::Unknown
        } else {
            Found::Nothing
        }
    }

    // constraint: a type's members are declared in its own package and qualified by its name, so a member of its name
    // in another directory, in the external test package beside it, or of an anonymous struct a field or a variable of
    // the type's name owns, belongs to a namesake
    fn declared_member(&self, ty: SymbolAt, named: &[SymbolAt]) -> Option<SymbolAt> {
        let symbol = self.graph.sym(ty);
        if !symbol.kind.is_type() {
            return self.anonymous_member(ty, named);
        }
        let owner = symbol.name;
        let qualified = self.graph.strings.resolve(symbol.qualified_name);
        let holder = self.holder(ty);
        let file = self.graph.file(ty.file);
        let in_package = |slot: &&u32| self.graph.file(**slot).package == file.package;
        let qualified_by_type = |member: &StoredSymbol| {
            self.graph
                .strings
                .resolve(member.qualified_name)
                .strip_prefix(qualified)
                .and_then(|rest| rest.strip_prefix('.'))
                == Some(self.graph.strings.resolve(member.name))
        };
        self.graph
            .package_files(file.dir)
            .iter()
            .filter(in_package)
            .find_map(|&slot| {
                self.graph.in_file(named, slot).iter().copied().find(|&at| {
                    let member = self.graph.sym(at);
                    member.owner == Some(owner)
                        && self.holder(at) == holder
                        && qualified_by_type(member)
                })
            })
    }

    // constraint: the fields of an anonymous struct are owned by the field or the variable it types and declared inside
    // that one's lines, which tells them apart from the fields of a namesake's struct
    fn anonymous_member(&self, host: SymbolAt, named: &[SymbolAt]) -> Option<SymbolAt> {
        let symbol = self.graph.sym(host);
        let lines = symbol.line..=symbol.end_line;
        self.graph
            .in_file(named, host.file)
            .iter()
            .copied()
            .find(|&at| {
                let member = self.graph.sym(at);
                member.owner == Some(symbol.name) && lines.contains(&member.line)
            })
    }

    fn embed(&self, walk: &mut Embedded, ty: SymbolAt, depth: u8) {
        match self.graph.sym(ty).kind {
            SymbolKind::TypeAlias => {
                let aliased = self.value_type(ty, 0, depth);
                if matches!(aliased, Value::Type(_) | Value::Imported) {
                    walk.follow(aliased);
                }
            }
            SymbolKind::DefinedType => {
                if let Some(under) = self.interface_under(ty, depth) {
                    walk.follow(under);
                }
            }
            _ => {}
        }
        for parent in self.graph.file(ty.file).parents_of(ty.index) {
            let written = self.graph.strings.resolve(parent.written);
            walk.follow(self.evaluate(self.body_of(ty), written, depth + 1));
        }
    }

    // constraint: a type defined over another takes none of its methods, an interface's excepted, whose methods are the
    // method set of every type defined over it. An imported type may be an interface, and a struct's fields stay, so a
    // member of a type over one is the imported type's
    fn interface_under(&self, ty: SymbolAt, depth: u8) -> Option<Value<'g>> {
        let kind = self.graph.sym(ty).kind;
        if kind == SymbolKind::Interface {
            return Some(Value::Type(ty));
        }
        let types = self.graph.file(ty.file).value_types(ty.index)?;
        let types = self.graph.strings.resolve(types);
        let path = match kind {
            SymbolKind::DefinedType => types.strip_prefix(UNDERLYING)?,
            SymbolKind::TypeAlias => types,
            _ => return None,
        };
        match self.evaluate(self.body_of(ty), path, depth + 1) {
            Value::Type(under) => self.interface_under(under, depth + 1),
            Value::Imported => Some(Value::Imported),
            _ => None,
        }
    }
}

enum Found {
    Member(SymbolAt),
    Outside,
    Nothing,
    Unknown,
}

struct Embedded {
    pending: Vec<SymbolAt>,
    walked: Vec<SymbolAt>,
    reaches_outside: bool,
    reaches_unknown: bool,
}

impl Embedded {
    fn new(ty: SymbolAt) -> Self {
        Self {
            pending: vec![ty],
            walked: vec![ty],
            reaches_outside: false,
            reaches_unknown: false,
        }
    }

    fn next(&mut self) -> Option<SymbolAt> {
        if self.walked.len() > EMBEDDED_TYPES {
            self.reaches_unknown = true;
            return None;
        }
        (!self.pending.is_empty()).then(|| self.pending.remove(0))
    }

    fn follow(&mut self, value: Value<'_>) {
        match value {
            Value::Type(at) if !self.walked.contains(&at) => {
                self.walked.push(at);
                self.pending.push(at);
            }
            Value::Type(_) => {}
            Value::Imported | Value::Undeclared => self.reaches_outside = true,
            Value::Package(_)
            | Value::Callable(_)
            | Value::Container { .. }
            | Value::Declarations(_)
            | Value::Unknown => self.reaches_unknown = true,
        }
    }
}

const EMBEDDED_TYPES: usize = 32;
