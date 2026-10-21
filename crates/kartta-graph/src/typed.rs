use kartta_lang::{Receivers, SymbolKind};

use crate::resolve::{Cascade, derived};
use crate::stored::{SymbolAt, VALUE_TYPE_SEPARATOR};
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
    Container { file: u32, element: &'g str },
    Imported,
    Undeclared,
    Unknown,
}

const MAX_DEPTH: u8 = 16;

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
        (file, path): (u32, Str),
        name: Str,
        binds: impl Fn(SymbolKind) -> bool,
    ) -> Typed {
        match self.receiver_type(file, path) {
            Value::Type(ty) => match self.member(ty, name, 0) {
                Found::Member(at) if binds(self.graph.sym(at).kind) => Typed::Member(at),
                Found::Member(_) | Found::Nothing => Typed::NoMethod,
                Found::Outside => Typed::Imported,
                Found::Unknown => Typed::Unknown,
            },
            Value::Imported => Typed::Imported,
            Value::Undeclared => Typed::Undeclared,
            Value::Package(_) | Value::Callable(_) | Value::Container { .. } | Value::Unknown => {
                Typed::Unknown
            }
        }
    }

    fn receiver_type(&self, file: u32, receiver: Str) -> Value<'g> {
        derived(&self.receiver_types, (file, receiver), || {
            self.evaluate(file, self.graph.strings.resolve(receiver), 0)
        })
    }

    fn evaluate(&self, file: u32, path: &'g str, depth: u8) -> Value<'g> {
        if depth > MAX_DEPTH || path.is_empty() {
            return Value::Unknown;
        }
        if let Some(element) = path.strip_prefix("[]") {
            return Value::Container { file, element };
        }
        let (name, mut rest) = head(path);
        let mut value = self.in_scope(file, name);
        while !rest.is_empty() {
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
        value
    }

    // constraint: a name of a type position that neither the file's package nor an import declares is predeclared,
    // from a dot import of code outside the graph, or from a file the graph does not hold
    fn in_scope(&self, file: u32, name: &str) -> Value<'g> {
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
        self.local_type(file, key).unwrap_or(Value::Undeclared)
    }

    // constraint: a file sees the package its own package clause names, its test files only from a test file; an
    // import sees the package's other files. Two files declare one name only when build constraints keep them apart,
    // so a file's own declaration is the one it is built with
    fn package_level(&self, dir: Str, name: Str, viewer: Option<u32>) -> Option<Value<'g>> {
        let named = self.named(name);
        let viewer = viewer.map(|slot| {
            let file = self.graph.file(slot);
            let test = file
                .symbols
                .first()
                .is_some_and(|symbol| symbol.flags.is_test());
            (slot, file.package, test)
        });
        let sees_tests = viewer.is_some_and(|(_, _, test)| test);
        let declared_in = |slot: u32| {
            self.graph.in_file(named, slot).iter().copied().find(|&at| {
                let symbol = self.graph.sym(at);
                symbol.owner.is_none()
                    && self.holder(at).is_none()
                    && (sees_tests || !symbol.flags.is_test())
            })
        };
        let seen = |slot: &&u32| {
            viewer.is_none_or(|(_, package, _)| self.graph.file(**slot).package == package)
        };
        let own = viewer.and_then(|(slot, _, _)| declared_in(slot));
        let declared = own.or_else(|| {
            self.graph
                .package_files(dir)
                .iter()
                .filter(seen)
                .find_map(|&slot| declared_in(slot))
        })?;
        Some(self.value_of(declared, 0))
    }

    // constraint: a type declared in a body is named only inside that body's file, and a file rarely declares two
    // local types of one name
    fn local_type(&self, file: u32, name: Str) -> Option<Value<'g>> {
        self.graph
            .in_file(self.named(name), file)
            .iter()
            .copied()
            .find(|&at| self.graph.sym(at).kind.is_type() && self.holder(at).is_some())
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
                self.evaluate(at.file, path, depth + 1)
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
            Value::Callable(_) | Value::Container { .. } | Value::Unknown => Value::Unknown,
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
            .is_some_and(|types| !self.graph.strings.resolve(types).starts_with("[]"))
    }

    fn element(&self, value: Value<'g>, depth: u8) -> Value<'g> {
        match value {
            Value::Container { file, element } => self.evaluate(file, element, depth + 1),
            Value::Type(ty) => match self.value_type(ty, 0, depth) {
                Value::Container { file, element } => self.evaluate(file, element, depth + 1),
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
                return self.evaluate(next.file, written, depth + 1);
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

    // constraint: a type's members are declared in its own package, so a member of its name in another directory, or in
    // the external test package beside it, belongs to a namesake
    fn declared_member(&self, ty: SymbolAt, named: &[SymbolAt]) -> Option<SymbolAt> {
        let owner = self.graph.sym(ty).name;
        let holder = self.holder(ty);
        let file = self.graph.file(ty.file);
        let in_package = |slot: &&u32| self.graph.file(**slot).package == file.package;
        self.graph
            .package_files(file.dir)
            .iter()
            .filter(in_package)
            .find_map(|&slot| {
                self.graph.in_file(named, slot).iter().copied().find(|&at| {
                    self.graph.sym(at).owner == Some(owner) && self.holder(at) == holder
                })
            })
    }

    fn embed(&self, walk: &mut Embedded, ty: SymbolAt, depth: u8) {
        if self.graph.sym(ty).kind == SymbolKind::TypeAlias
            && let Value::Type(aliased) = self.value_type(ty, 0, depth)
        {
            walk.follow(Value::Type(aliased));
        }
        for parent in self.graph.file(ty.file).parents_of(ty.index) {
            let written = self.graph.strings.resolve(parent.written);
            walk.follow(self.evaluate(ty.file, written, depth + 1));
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
            Value::Package(_) | Value::Callable(_) | Value::Container { .. } | Value::Unknown => {
                self.reaches_unknown = true;
            }
        }
    }
}

const EMBEDDED_TYPES: usize = 32;
