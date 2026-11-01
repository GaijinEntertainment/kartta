use std::iter::successors;

use kartta_lang::{Receivers, SymbolKind};

use crate::resolve::{Cascade, derived};
use crate::stored::{StoredSymbol, SymbolAt, VALUE_TYPE_SEPARATOR};
use crate::strings::Str;

/// What a receiver that spells a type path binds a member call to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typed {
    /// The member the site names, of the receiver's type or of a type it embeds, or the type an embedded field of the
    /// name holds, which stands for that field.
    Member(SymbolAt),
    /// A type that an import of code outside the graph declares, or a member such a type may give.
    Imported,
    /// A name that nothing the graph holds declares, a builtin type among them.
    Undeclared,
    /// A type the graph holds, with no member of the name the site can bind: a method for a call, any member for a use.
    NoMethod,
    /// A read past a bound, which binds nothing, since a cycle of declarations gives its name no type to bind through.
    PastBound,
    /// A path the graph cannot follow, which leaves the call to the tiers that read no type.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value<'g> {
    Package(Str),
    /// A value of the type.
    Type(SymbolAt),
    /// The type a path's name declares, which a call converts to, and which any other step reads as a value of the
    /// type: a local's path spells the type of the value it holds. A read past its last step never holds one.
    Named(SymbolAt),
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
    /// A read that ran past a bound, as a cycle of declarations does, or that a member walk closed on a cycle of
    /// aliases, which leaves it no type.
    Unbounded,
}

impl Value<'_> {
    const fn held(self) -> Self {
        match self {
            Value::Named(ty) => Value::Type(ty),
            value => value,
        }
    }
}

/// A name a package declares, as the file `viewer` sees it, or as an import does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PackageName {
    dir: Str,
    name: Str,
    viewer: Option<u32>,
}

const MAX_DEPTH: u8 = 16;

const MAX_EVALUATIONS: u16 = 1024;

const UNDERLYING: &str = "~";

enum Step<'p> {
    Select(&'p str),
    Result(usize),
    Element,
    Held,
}

fn head(path: &str) -> (&str, &str) {
    let end = path.find(['.', '(', '[', '~']).unwrap_or(path.len());
    path.split_at(end)
}

fn step(rest: &str) -> Option<(Step<'_>, &str)> {
    if let Some(after) = rest.strip_prefix(UNDERLYING) {
        return Some((Step::Held, after));
    }
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
        let reached = match self.receiver_type(self.body_of(reader), path) {
            Value::Type(ty) => {
                // constraint: a held read spends nothing, so a walk that took over its count would answer by the calls
                // before it
                self.evaluations.set(0);
                let members = self.named(name);
                match self.member(ty, (self.graph.strings.resolve(name), members), 0) {
                    Found::Member(at) if binds(self.graph.sym(at)) => return Typed::Member(at),
                    Found::Member(_) | Found::Nothing => return Typed::NoMethod,
                    Found::Reached(value) => value,
                }
            }
            value => value,
        };
        match reached {
            Value::Imported => Typed::Imported,
            Value::Undeclared => Typed::Undeclared,
            Value::Unbounded => Typed::PastBound,
            Value::Type(_)
            | Value::Named(_)
            | Value::Package(_)
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
            self.evaluations.set(0);
            self.evaluate(body, self.graph.strings.resolve(receiver), 0)
        })
    }

    fn evaluate(&self, body: SymbolAt, path: &'g str, depth: u8) -> Value<'g> {
        let evaluations = self.evaluations.get();
        if depth > MAX_DEPTH || evaluations >= MAX_EVALUATIONS {
            return Value::Unbounded;
        }
        self.evaluations.set(evaluations + 1);
        if path.is_empty() {
            return Value::Unknown;
        }
        if let Some(element) = path.strip_prefix("[]") {
            return Value::Container { body, element };
        }
        let (name, rest) = head(path);
        let value = self
            .anonymous_host(body.file, name)
            .unwrap_or_else(|| self.in_scope(body, name, depth));
        self.along(value, rest, depth)
    }

    fn along(&self, mut value: Value<'g>, mut rest: &'g str, depth: u8) -> Value<'g> {
        loop {
            if let Value::Declarations(read) = value {
                return self.highest_ranked(read, rest, depth);
            }
            if rest.is_empty() || value == Value::Unbounded {
                return value.held();
            }
            let Some((step, after)) = step(rest) else {
                return Value::Unknown;
            };
            value = match step {
                Step::Select(name) => self.select(value, name, depth),
                Step::Result(index) => self.result(value, index, depth),
                Step::Element => self.element(value, depth),
                Step::Held => value.held(),
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
            Value::Type(at) | Value::Named(at) | Value::Callable(at) => {
                (3, strings.resolve(self.graph.sym(at).qualified_name))
            }
            Value::Container { element, .. } => (3, element),
            Value::Package(dir) => (3, strings.resolve(dir)),
            Value::Imported => (2, ""),
            Value::Unknown | Value::Unbounded | Value::Declarations(_) => (1, ""),
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
    fn in_scope(&self, body: SymbolAt, name: &str, depth: u8) -> Value<'g> {
        let file = body.file;
        let Some(key) = self.graph.strings.get(name) else {
            return Value::Undeclared;
        };
        if let Some(target) = self.binding(file, key) {
            return target.map_or(Value::Imported, Value::Package);
        }
        let own = PackageName {
            dir: self.graph.file(file).dir,
            name: key,
            viewer: Some(file),
        };
        if let Some(declared) = self.package_level(own, depth) {
            return declared;
        }
        for dir in self.dot_imported_dirs(file).iter().copied() {
            let read = PackageName {
                dir,
                name: key,
                viewer: None,
            };
            if let Some(declared) = self.package_level(read, depth) {
                return declared;
            }
        }
        self.local_type(body, key).unwrap_or(Value::Undeclared)
    }

    // constraint: two files declare one name only when build constraints keep them apart, so a file's own declaration
    // is the one it is built with
    fn package_level(&self, read: PackageName, depth: u8) -> Option<Value<'g>> {
        let own = read.viewer.and_then(|slot| self.declaration_in(read, slot));
        if let Some(own) = own {
            return Some(self.value_of(own, depth));
        }
        let mut declared = self.declarers(read);
        let first = declared.next()?;
        Some(if declared.next().is_some() {
            Value::Declarations(read)
        } else {
            self.value_of(first, depth)
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
            kind if kind.is_type() => Value::Named(at),
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
                .and_then(|name| {
                    let read = PackageName {
                        dir,
                        name,
                        viewer: None,
                    };
                    self.package_level(read, depth)
                })
                .unwrap_or(Value::Unknown),
            Value::Type(ty) | Value::Named(ty) => self.member_value(ty, name, depth),
            Value::Imported | Value::Undeclared => value,
            Value::Callable(_)
            | Value::Container { .. }
            | Value::Declarations(_)
            | Value::Unknown
            | Value::Unbounded => Value::Unknown,
        }
    }

    // constraint: a call of a value yields the results of the function type under its type, and a call of a type's
    // name converts to that type, whatever is under it
    fn result(&self, value: Value<'g>, index: usize, depth: u8) -> Value<'g> {
        match value {
            Value::Callable(at) => self.value_type(at, index, depth),
            Value::Named(_) if index == 0 => value.held(),
            Value::Type(ty) | Value::Named(ty) => self.called(ty, index, depth),
            Value::Imported | Value::Undeclared => value,
            _ => Value::Unknown,
        }
    }

    fn called(&self, ty: SymbolAt, index: usize, depth: u8) -> Value<'g> {
        match self.underlying(ty, depth) {
            Some(Value::Type(under)) if self.is_function_type(under) => {
                self.value_type(under, index, depth)
            }
            Some(reached @ (Value::Imported | Value::Unbounded)) => reached,
            _ if self.is_function_type(ty) => self.value_type(ty, index, depth),
            _ if index == 0 => Value::Type(ty),
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
        let holder = match value {
            Value::Type(ty) | Value::Named(ty) => match self.underlying(ty, depth) {
                Some(Value::Type(under)) => self.value_type(under, 0, depth),
                Some(reached) => reached,
                None => Value::Unknown,
            },
            value => value,
        };
        match holder {
            Value::Container { body, element } => self.evaluate(body, element, depth + 1),
            Value::Imported | Value::Undeclared | Value::Unbounded => holder,
            _ => Value::Unknown,
        }
    }

    fn member_value(&self, ty: SymbolAt, name: &str, depth: u8) -> Value<'g> {
        let key = self.graph.strings.get(name);
        let members = key.map_or(&[][..], |key| self.named(key));
        match self.member(ty, (name, members), depth) {
            Found::Member(at) => self.value_of(at, depth).held(),
            Found::Reached(value) => value,
            Found::Nothing => Value::Unknown,
        }
    }

    // constraint: an embedded type's members are promoted, the shallowest first, and a member of a type that the graph
    // does not hold, or that a path the graph cannot follow names, may be the one the name reaches. A struct's embedded
    // field is named by its type's name, `x.Base` for an embedded `Base` or `*pkg.Base`, and has no symbol of its own,
    // so its type answers; an interface's embedded interface names no member. An imported type's members are unknown,
    // and one at the depth of a name found here would make the selector illegal, so the import can give only a name no
    // type the graph holds does (ADR 3.201)
    fn member(&self, ty: SymbolAt, (name, members): (&str, &[SymbolAt]), depth: u8) -> Found<'g> {
        let mut walk = Embedded::new(ty);
        while let Some(next) = walk.next() {
            if let Some(at) = self.walked_member(next, members) {
                return Found::Member(at);
            }
            let Walked { ty, .. } = next;
            if let Some(written) = self.embedded_field(ty, name) {
                return match self.evaluate(self.body_of(ty), written, depth + 1) {
                    Value::Type(at) => Found::Member(at),
                    value => Found::Reached(value),
                };
            }
            self.embed(&mut walk, ty, depth);
        }
        if walk.reaches_outside {
            Found::Reached(Value::Imported)
        } else if walk.closes_aliases {
            Found::Reached(Value::Unbounded)
        } else if walk.reaches_unknown {
            Found::Reached(Value::Unknown)
        } else {
            Found::Nothing
        }
    }

    fn embedded_field(&self, ty: SymbolAt, name: &str) -> Option<&'g str> {
        if self.graph.sym(ty).kind == SymbolKind::Interface {
            return None;
        }
        self.graph
            .file(ty.file)
            .parents_of(ty.index)
            .iter()
            .map(|parent| self.graph.strings.resolve(parent.written))
            .find(|written| written.rsplit('.').next() == Some(name))
    }

    fn walked_member(&self, walked: Walked, named: &[SymbolAt]) -> Option<SymbolAt> {
        self.declared_member(walked.ty, named).filter(|&at| {
            !walked.drops_own_methods || self.graph.sym(at).kind != SymbolKind::Method
        })
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
            SymbolKind::TypeAlias => match self.value_type(ty, 0, depth) {
                Value::Type(target) => walk.follow_alias(ty, target),
                Value::Imported => walk.follow(Value::Imported),
                _ => {}
            },
            SymbolKind::DefinedType => match self.underlying(ty, depth) {
                Some(Value::Type(under)) => {
                    let kind = self.graph.sym(under).kind;
                    if matches!(kind, SymbolKind::Struct | SymbolKind::Interface) {
                        walk.descend_in_place(Walked {
                            ty: under,
                            drops_own_methods: kind == SymbolKind::Struct,
                        });
                    }
                }
                Some(Value::Imported) => walk.follow(Value::Imported),
                _ => {}
            },
            _ => {}
        }
        for parent in self.graph.file(ty.file).parents_of(ty.index) {
            let written = self.graph.strings.resolve(parent.written);
            walk.follow(self.evaluate(self.body_of(ty), written, depth + 1));
        }
    }

    // constraint: a type defined over another shares its underlying type: an interface's methods, a struct's fields and
    // what they promote and none of the struct's own methods, a function's results, a container's element. An imported
    // type may be any of them, so what a type over one gives is the imported type's (ADR 3.201)
    fn underlying(&self, ty: SymbolAt, depth: u8) -> Option<Value<'g>> {
        let kind = self.graph.sym(ty).kind;
        if matches!(kind, SymbolKind::Interface | SymbolKind::Struct) {
            return Some(Value::Type(ty));
        }
        let types = self.graph.file(ty.file).value_types(ty.index)?;
        let types = self.graph.strings.resolve(types);
        let path = match kind {
            SymbolKind::DefinedType => match types.strip_prefix(UNDERLYING) {
                Some(named) => named,
                None => return Some(Value::Type(ty)),
            },
            SymbolKind::TypeAlias => types,
            _ => return None,
        };
        match self.evaluate(self.body_of(ty), path, depth + 1) {
            Value::Type(under) => self.underlying(under, depth + 1),
            Value::Container { .. } => Some(Value::Type(ty)),
            reached @ (Value::Imported | Value::Unbounded) => Some(reached),
            _ => None,
        }
    }
}

enum Found<'g> {
    /// The member of the name, or the type that an embedded field of the name holds.
    Member(SymbolAt),
    /// No symbol: the name lies past what the graph holds, or past what a path can follow.
    Reached(Value<'g>),
    Nothing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Walked {
    ty: SymbolAt,
    drops_own_methods: bool,
}

impl Walked {
    const fn whole(ty: SymbolAt) -> Self {
        Self {
            ty,
            drops_own_methods: false,
        }
    }
}

struct Embedded {
    pending: Vec<Walked>,
    walked: Vec<Walked>,
    aliases: Vec<(SymbolAt, SymbolAt)>,
    reaches_outside: bool,
    reaches_unknown: bool,
    closes_aliases: bool,
}

impl Embedded {
    fn new(ty: SymbolAt) -> Self {
        let start = Walked::whole(ty);
        Self {
            pending: vec![start],
            walked: vec![start],
            aliases: Vec::new(),
            reaches_outside: false,
            reaches_unknown: false,
            closes_aliases: false,
        }
    }

    // constraint: a cycle with no type definition in it never ends, so the compiler refuses it, and a walked type two
    // aliases meet at is no cycle (ADR 3.201)
    fn follow_alias(&mut self, alias: SymbolAt, target: SymbolAt) {
        self.aliases.push((alias, target));
        let aliased = |at: &SymbolAt| {
            self.aliases
                .iter()
                .find(|(from, _)| from == at)
                .map(|&(_, to)| to)
        };
        let mut chain = successors(Some(target), aliased).take(self.aliases.len());
        self.closes_aliases |= chain.any(|at| at == alias);
        self.follow(Value::Type(target));
    }

    fn next(&mut self) -> Option<Walked> {
        if self.walked.len() > EMBEDDED_TYPES {
            self.reaches_unknown = true;
            return None;
        }
        (!self.pending.is_empty()).then(|| self.pending.remove(0))
    }

    // constraint: what a defined type holds of the type under it sits at that type's own depth, ahead of the types the
    // walk holds for the next one
    fn descend_in_place(&mut self, under: Walked) {
        if !self.walked.contains(&under) {
            self.walked.push(under);
            self.pending.insert(0, under);
        }
    }

    fn follow(&mut self, value: Value<'_>) {
        match value {
            Value::Type(ty) | Value::Named(ty) => {
                let embedded = Walked::whole(ty);
                if !self.walked.contains(&embedded) {
                    self.walked.push(embedded);
                    self.pending.push(embedded);
                }
            }
            Value::Imported | Value::Undeclared => self.reaches_outside = true,
            Value::Package(_)
            | Value::Callable(_)
            | Value::Container { .. }
            | Value::Declarations(_)
            | Value::Unknown
            | Value::Unbounded => self.reaches_unknown = true,
        }
    }
}

const EMBEDDED_TYPES: usize = 32;
