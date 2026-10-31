use std::collections::{HashMap, HashSet};
use std::ops::Range;

use kartta_lang::SymbolKind;
use kartta_lang::tree::named_child_nodes;
use tree_sitter::Node;

use super::blocks::block_use;
use super::calls::{own_field, value_receiver};
use super::syntax::type_base;
use super::{Emitter, GLOB_FORM};

// constraint: a method call on a parameter whose type its function declares names that type; `written` is the type's
// path in the source, `value` the parameter's name, which the receiver falls back to where the file cannot tell the
// type is one the tree may hold, and which a chain has none of, and `module` the chain of the inline module around the
// call, whose `use`s it sees, `module_end` that module's end, which keys the `use`s of its blocks, and `local` whether
// a `let` or a call names the type
#[derive(Clone)]
pub(super) struct DeclaredReceiver {
    pub(super) written: Range<usize>,
    pub(super) value: Option<Range<usize>>,
    pub(super) module: Option<String>,
    pub(super) module_end: Option<usize>,
    pub(super) local: bool,
}

// constraint: what a `DeclaredReceiver` decides once the walk ends: a call's receiver, or a function's returned type
pub(super) enum Kept {
    Call(usize),
    Returned(usize),
}

// constraint: what a binding holds: the path of its type in the source, or, for a `let` of a method call, the place in
// `returned_locals` of the path the graph reads
#[derive(Clone)]
pub(super) enum LocalType {
    Written(Range<usize>),
    Returned(usize),
}

// constraint: the crates of the toolchain, whose types no crate of a tree declares, so a namesake the graph holds is
// another type
const SYSROOT: [&str; 5] = ["std", "core", "alloc", "test", "proc_macro"];

// constraint: the types the standard prelude names, which no glob brings into a file, so a namesake the tree holds,
// `allocator_api2::vec::Vec`, is another type
const PRELUDE: [&str; 5] = ["Box", "Option", "Result", "String", "Vec"];

// constraint: the most calls a receiver's path spells, which covers a builder's chain and bounds what it stores
const MAX_RETURNS: usize = 8;

impl Emitter<'_> {
    // constraint: a parameter declared `x: T`, `&T`, `&mut T`, `T<'a, U>`, `Box<T>`, `Rc<T>`, `Arc<T>`, `Self`, `impl
    // Trait` or `dyn Trait` names that type for a method call on it; a type parameter of the function or of the impl
    // or trait around it, a path through one, a tuple and any other pattern than a name, `mut` or not, name none; the
    // function's own region is the last one open
    pub(super) fn parameter_types(&mut self, function: Node<'_>) {
        let parameters = function
            .child_by_field_name("parameters")
            .into_iter()
            .flat_map(named_child_nodes)
            .filter(|parameter| parameter.kind() == "parameter");
        for parameter in parameters {
            let (Some(pattern), Some(declared)) = (
                parameter.child_by_field_name("pattern"),
                parameter.child_by_field_name("type"),
            ) else {
                continue;
            };
            let source = self.source;
            if let Some(name) = plain_name(pattern)
                && let Some(named) = named_type(declared, function, source)
                && let Some(region) = self.regions.last_mut()
                && let Some(declared) = region.names.get_mut(&source[name.byte_range()])
            {
                declared.get_or_insert(LocalType::Written(named.byte_range()));
            }
        }
    }

    // constraint: a `let` of a name, `mut` or not, names the type it declares as a parameter does, the type its
    // initializer constructs, or what its method call returns; any other pattern names none
    pub(super) fn local_type(&mut self, local: Node<'_>) -> Option<LocalType> {
        plain_name(local.child_by_field_name("pattern")?)?;
        let source = self.source;
        if let Some(declared) = local.child_by_field_name("type") {
            let named = named_type(declared, enclosing_function(local)?, source)?;
            return Some(LocalType::Written(named.byte_range()));
        }
        let (held, unwrapped) = peeled(local.child_by_field_name("value")?, false, source);
        if let Some(path) = constructed_path(held, source) {
            let named = constructed_name(path, enclosing_function(local)?, source)?;
            return Some(LocalType::Written(named.byte_range()));
        }
        let returned = self.returned_receiver(held, unwrapped, 0)?;
        self.returned_locals.push(returned);
        Some(LocalType::Returned(self.returned_locals.len() - 1))
    }

    // constraint: a method call on a chain names the type the chain holds: the type `T::f()` constructs, read as a
    // `let`'s initializer is, what a method call returns, or what a name or a field of `self` declared `Option<T>` or
    // `Result<T, E>` holds once the chain unwraps it; any other chain names none and keeps the empty receiver
    pub(super) fn chain_receiver(
        &self,
        chain: Node<'_>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        self.chain_at(chain, 0)
    }

    fn chain_at(
        &self,
        chain: Node<'_>,
        calls: usize,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let source = self.source;
        let (held, unwrapped) = peeled(chain, false, source);
        if let Some(path) = constructed_path(held, source) {
            let named = constructed_name(path, enclosing_function(chain)?, source)?;
            return self.receiver_of(named.byte_range(), true, None);
        }
        if held.kind() == "call_expression" {
            return self.returned_receiver(held, unwrapped, calls);
        }
        if !unwrapped {
            return None;
        }
        if held.kind() == "identifier" {
            let (LocalType::Written(written), local) =
                self.declared_type(&source[held.byte_range()])?
            else {
                return None;
            };
            let function = enclosing_function(chain)?;
            let declared = function.descendant_for_byte_range(written.start, written.end)?;
            let named = held_type(declared, function, source)?;
            return self.receiver_of(named.byte_range(), local, None);
        }
        let (declared, structure) = field_type(own_field(held)?, source)?;
        let named = held_type(declared, structure, source)?;
        self.receiver_of(named.byte_range(), false, None)
    }

    // constraint: a method call on `x.m()` names what `m` returns, which only the graph can read: the receiver spells
    // the path, `T::m()`, with `?` after a call the chain unwraps, from the type `x` names by the rules of any other
    // operand; `calls` counts the calls outside this one, as a path stops at `MAX_RETURNS` since it grows with each
    fn returned_receiver(
        &self,
        call: Node<'_>,
        unwrapped: bool,
        calls: usize,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        if calls == MAX_RETURNS {
            return None;
        }
        let source = self.source;
        let function = call.child_by_field_name("function")?;
        let function = if function.kind() == "generic_function" {
            function.child_by_field_name("function")?
        } else {
            function
        };
        if function.kind() != "field_expression" {
            return None;
        }
        let method = function.child_by_field_name("field")?;
        let operand = function.child_by_field_name("value")?;
        let (owner, own, declared) = match operand.kind() {
            "identifier" => self.declared_receiver(operand)?,
            "self" => (self.enclosing_type_name()?, true, None),
            _ => match own_field(operand) {
                Some(field) => self.field_receiver(field)?,
                None => self.chain_at(operand, calls + 1)?,
            },
        };
        let owner = owner.split('|').next().unwrap_or(&owner);
        let method = String::from_utf8_lossy(&source[method.byte_range()]);
        let mark = if unwrapped { "?" } else { "" };
        let path = format!("{owner}::{method}(){mark}");
        let declared = declared.map(|declared| DeclaredReceiver {
            value: None,
            ..declared
        });
        Some((path, own, declared))
    }

    // constraint: what a function gives the graph to read a chain through: the last segment of the type its return type
    // names, after a `?` where an `Option` or a `Result` holds it; an `async` function returns a future, which `.await`
    // alone opens, and no chain reads through it
    pub(super) fn returned_value(
        &self,
        function: Node<'_>,
    ) -> Option<(String, Option<DeclaredReceiver>)> {
        let source = self.source;
        let returned = unpinned(function.child_by_field_name("return_type")?, source);
        let is_async = named_child_nodes(function)
            .find(|child| child.kind() == "function_modifiers")
            .is_some_and(|modifiers| {
                source[modifiers.byte_range()]
                    .windows(5)
                    .any(|word| word == b"async")
            });
        if is_async {
            return None;
        }
        let named = named_type(returned, function, source)?;
        let (named, mark) = if is_a_wrapper(named, source) {
            (held_type(named, function, source)?, "?")
        } else {
            (named, "")
        };
        let (owner, _, declared) = self.receiver_of(named.byte_range(), true, None)?;
        Some((format!("{mark}{owner}"), declared))
    }

    // constraint: the receiver of a method call on a parameter of a declared type is the type's last segment, `Self`
    // the type of the impl or trait around the call; the bool tells the latter, which follows that type's owner and
    // waits for no `use`, so it carries no `DeclaredReceiver`
    pub(super) fn declared_receiver(
        &self,
        value: Node<'_>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let (declared, local) = self.declared_type(&self.source[value.byte_range()])?;
        match declared {
            LocalType::Written(written) => self.receiver_of(written, local, Some(value)),
            LocalType::Returned(at) => {
                let (path, own, declared) = &self.returned_locals[at];
                let spelled = value_receiver(&self.source[value.byte_range()]);
                let declared = declared.clone().map(|declared| DeclaredReceiver {
                    value: Some(value.byte_range()),
                    ..declared
                });
                Some((format!("{path}|{spelled}"), *own, declared))
            }
        }
    }

    // constraint: a field of `self` names the type its declaration does, as a parameter's does, where the struct the
    // impl around the call names by a bare name is declared beside the impl, in its module or block
    pub(super) fn field_receiver(
        &self,
        field: Node<'_>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let (written, _) = field_type(field, self.source)?;
        self.receiver_of(written.byte_range(), false, Some(field))
    }

    fn receiver_of(
        &self,
        written: Range<usize>,
        local: bool,
        value: Option<Node<'_>>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let spelled = String::from_utf8_lossy(&self.source[written.clone()]);
        if spelled == "Self" {
            return Some((self.enclosing_type_name()?, true, None));
        }
        let owner = spelled
            .rsplit("::")
            .next()
            .unwrap_or(&spelled)
            .trim()
            .to_owned();
        let declared = DeclaredReceiver {
            written,
            value: value.map(|value| value.byte_range()),
            module: self.modules.last().map(|module| module.chain.clone()),
            module_end: self.modules.last().map(|module| module.end),
            local,
        };
        Some((owner, false, Some(declared)))
    }

    // constraint: a declared type names its receiver only where the file shows it may be a type of the tree: a `use`
    // brings the name, or the first segment of its path, from outside the toolchain's crates, a path starts at the
    // file's crate or module or a module it declares, the file declares a type of the name no `use` brings, or, for a
    // name neither a `use` nor the prelude brings, a glob the file keeps may, one of its top level from outside the
    // toolchain's crates, for a parameter or a field and never for a `let`; the standard library's types, which crates
    // of a tree shadow by name, `File` or `Vec`, keep the value's receiver; a `use` in a block around where the type is
    // written decides first, so one inside a function body reaches that body's `let`s and no signature, then one an
    // inline module holds reaches the signatures of that module, and one outside every block the others
    pub(super) fn keep_declared_receivers(&mut self) {
        if self.declared_receivers.is_empty() {
            return;
        }
        let directives = &self.raw.directives;
        let mut imported: HashMap<(Option<&str>, &str), &str> = HashMap::new();
        for import in self.imports.iter().filter(|import| !import.in_body) {
            let directive = &directives[import.directive];
            if let Some(name) = directive.names.first() {
                imported
                    .entry((import.module.as_deref(), name))
                    .or_insert_with(|| head(&directive.spec));
            }
        }
        let glob = directives
            .iter()
            .any(|directive| directive.form == GLOB_FORM);
        let mut types = HashSet::new();
        let mut modules = HashSet::new();
        for symbol in &self.raw.symbols {
            match symbol.kind {
                SymbolKind::Struct
                | SymbolKind::Enum
                | SymbolKind::Interface
                | SymbolKind::TypeAlias => {
                    types.insert(symbol.name.as_str());
                }
                SymbolKind::Namespace => {
                    modules.insert(symbol.name.as_str());
                }
                _ => {}
            }
        }
        let outside_sysroot = |head: &str| !SYSROOT.contains(&head);
        let (source, block_items) = (self.source, &self.block_items);
        let mut unshown_returns = Vec::new();
        for (kept, declared) in self.declared_receivers.drain(..) {
            let module = declared.module.as_deref();
            let (at, module_end) = (declared.written.start, declared.module_end);
            let imported = |name: &str| {
                block_use(block_items, name, at, module_end)
                    .map(|directive| head(&directives[directive].spec))
                    .or_else(|| imported.get(&(module, name)).copied())
                    .or_else(|| imported.get(&(None, name)).copied())
            };
            let written = String::from_utf8_lossy(&source[declared.written]);
            let written = written.trim_start_matches("::");
            let shown = match written.split_once("::") {
                Some((first, _)) => {
                    let first = first.trim();
                    matches!(first, "crate" | "self" | "super")
                        || modules.contains(first)
                        || outside_sysroot(imported(first).unwrap_or(first))
                }
                None => imported(written).map_or_else(
                    || {
                        types.contains(written)
                            || (glob && !declared.local && !PRELUDE.contains(&written))
                    },
                    outside_sysroot,
                ),
            };
            match kept {
                _ if shown => {}
                Kept::Call(call) => {
                    self.raw.calls[call].receiver = Some(
                        declared
                            .value
                            .map_or_else(String::new, |value| value_receiver(&source[value])),
                    );
                }
                Kept::Returned(symbol) => unshown_returns.push(symbol),
            }
        }
        for symbol in unshown_returns {
            self.raw.symbols[symbol].value_types.clear();
        }
    }
}

fn plain_name(pattern: Node<'_>) -> Option<Node<'_>> {
    match pattern.kind() {
        "identifier" => Some(pattern),
        "mut_pattern" => named_child_nodes(pattern).find(|inner| inner.kind() == "identifier"),
        _ => None,
    }
}

fn enclosing_function(node: Node<'_>) -> Option<Node<'_>> {
    let mut around = node.parent()?;
    while around.kind() != "function_item" {
        around = around.parent()?;
    }
    Some(around)
}

// constraint: what a value holds under the methods of a `Result` or an `Option` that leave what they hold, and whether
// it is unwrapped: `?`, `.unwrap()` and `.expect(..)` take what `f` holds where `f` fails, with `.map_err(..)`,
// `.as_ref()` or `.as_mut()` between them, which alone leave a `Result` or an `Option`
fn peeled<'t>(value: Node<'t>, unwrapped: bool, source: &[u8]) -> (Node<'t>, bool) {
    let operand = match value.kind() {
        "try_expression" => value.named_child(0),
        "call_expression" => value
            .child_by_field_name("function")
            .filter(|function| function.kind() == "field_expression")
            .and_then(|function| {
                let method = &source[function.child_by_field_name("field")?.byte_range()];
                let keeps = matches!(method, b"map_err" | b"as_ref" | b"as_mut");
                (matches!(method, b"unwrap" | b"expect") || (unwrapped && keeps))
                    .then(|| function.child_by_field_name("value"))?
            }),
        _ => None,
    };
    operand.map_or((value, unwrapped), |operand| peeled(operand, true, source))
}

// constraint: the path of the type a call constructs: `T::f()`; a function named for a builder constructs that builder,
// another type; `f`'s return type goes unread, so any other function that returns another type still names `T`, an
// approximation ADR 3.55 accepts by its measured precision
fn constructed_path<'t>(value: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    if value.kind() != "call_expression" {
        return None;
    }
    let function = value.child_by_field_name("function")?;
    let named = if function.kind() == "generic_function" {
        function.child_by_field_name("function")?
    } else {
        function
    };
    let name = &source[named.child_by_field_name("name")?.byte_range()];
    (named.kind() == "scoped_identifier" && !name.ends_with(b"builder"))
        .then(|| named.child_by_field_name("path"))
        .flatten()
}

fn constructed_name<'t>(path: Node<'t>, function: Node<'_>, source: &[u8]) -> Option<Node<'t>> {
    let named = named_type(type_base(path), function, source)?;
    names_a_type(named, source).then_some(named)
}

// constraint: the type a declared `Option<T>` or `Result<T, E>` holds, through a path or an alias of the name; `named`
// is the node `named_type` gives for the declaration, the base of its generic type
fn held_type<'t>(named: Node<'t>, around: Node<'_>, source: &[u8]) -> Option<Node<'t>> {
    if !is_a_wrapper(named, source) {
        return None;
    }
    let generic = named
        .parent()
        .filter(|parent| parent.kind() == "generic_type")?;
    let held = named_child_nodes(generic.child_by_field_name("type_arguments")?).next()?;
    named_type(unpinned(held, source), around, source)
}

// constraint: a method call on a `Pin<P>` reaches what `P` points to
fn unpinned<'t>(returned: Node<'t>, source: &[u8]) -> Node<'t> {
    Some(returned)
        .filter(|returned| returned.kind() == "generic_type")
        .filter(|generic| {
            generic
                .child_by_field_name("type")
                .is_some_and(|base| &source[base.byte_range()] == b"Pin")
        })
        .and_then(|generic| {
            generic
                .child_by_field_name("type_arguments")?
                .named_child(0)
        })
        .unwrap_or(returned)
}

fn is_a_wrapper(named: Node<'_>, source: &[u8]) -> bool {
    let last = named.child_by_field_name("name").unwrap_or(named);
    matches!(&source[last.byte_range()], b"Option" | b"Result")
}

// constraint: a capitalised last segment names a type, and a lowercase one the module a free function lives in; `Box`,
// `Rc` and `Arc` hand a method call to what they point to, which their function does not name
fn names_a_type(path: Node<'_>, source: &[u8]) -> bool {
    let last = &source[path
        .child_by_field_name("name")
        .unwrap_or(path)
        .byte_range()];
    last.first().is_some_and(u8::is_ascii_uppercase) && !matches!(last, b"Box" | b"Rc" | b"Arc")
}

fn head(path: &str) -> &str {
    path.split("::").next().unwrap_or(path).trim()
}

fn field_type<'t>(field: Node<'t>, source: &[u8]) -> Option<(Node<'t>, Node<'t>)> {
    let mut around = field.parent()?;
    while around.kind() != "impl_item" {
        if around.kind() == "trait_item" {
            return None;
        }
        around = around.parent()?;
    }
    let owner = type_base(around.child_by_field_name("type")?);
    if owner.kind() != "type_identifier" {
        return None;
    }
    let spelled = |node: Node<'_>| &source[node.byte_range()];
    let mut cursor = around.walk();
    let structure = (around.parent()?.named_children(&mut cursor))
        .filter(|item| matches!(item.kind(), "struct_item" | "union_item"))
        .find(|item| {
            (item.child_by_field_name("name")).is_some_and(|name| spelled(name) == spelled(owner))
        })?;
    let declared = (structure
        .child_by_field_name("body")?
        .named_children(&mut cursor))
    .filter(|declared| declared.kind() == "field_declaration")
    .find(|declared| {
        (declared.child_by_field_name("name")).is_some_and(|name| spelled(name) == spelled(field))
    })?;
    let named = named_type(declared.child_by_field_name("type")?, structure, source)?;
    Some((named, structure))
}

// constraint: the node whose text is the path of the type a parameter names, a type or a trait
fn named_type<'t>(declared: Node<'t>, function: Node<'_>, source: &[u8]) -> Option<Node<'t>> {
    match declared.kind() {
        "reference_type" => named_type(declared.child_by_field_name("type")?, function, source),
        "type_identifier" | "identifier" => {
            (!is_type_parameter(function, declared, source)).then_some(declared)
        }
        "scoped_type_identifier" | "scoped_identifier" => {
            let path = declared.child_by_field_name("path")?;
            let through_a_type = &source[path.byte_range()] == b"Self"
                || (path.kind() == "identifier" && is_type_parameter(function, path, source));
            (!through_a_type).then_some(declared)
        }
        "generic_type" => {
            let base = declared.child_by_field_name("type")?;
            if matches!(&source[base.byte_range()], b"Box" | b"Rc" | b"Arc") {
                let pointee =
                    named_child_nodes(declared.child_by_field_name("type_arguments")?).next()?;
                return named_type(pointee, function, source);
            }
            named_type(base, function, source)
        }
        "abstract_type" | "dynamic_type" => {
            let named = declared.child_by_field_name("trait")?;
            let named = if named.kind() == "generic_type" {
                named.child_by_field_name("type")?
            } else {
                named
            };
            matches!(named.kind(), "type_identifier" | "scoped_type_identifier").then_some(named)
        }
        _ => None,
    }
}

// constraint: a type parameter is declared by the function or by the impl or trait whose body holds it
fn is_type_parameter(function: Node<'_>, name: Node<'_>, source: &[u8]) -> bool {
    let spelled = &source[name.byte_range()];
    let owner = function
        .parent()
        .and_then(|body| body.parent())
        .filter(|owner| matches!(owner.kind(), "impl_item" | "trait_item"));
    [Some(function), owner]
        .into_iter()
        .flatten()
        .filter_map(|scope| scope.child_by_field_name("type_parameters"))
        .flat_map(named_child_nodes)
        .filter_map(|parameter| parameter.child_by_field_name("name"))
        .any(|parameter| &source[parameter.byte_range()] == spelled)
}
