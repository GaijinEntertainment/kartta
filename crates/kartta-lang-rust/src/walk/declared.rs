use std::collections::{HashMap, HashSet};
use std::ops::Range;

use kartta_lang::SymbolKind;
use kartta_lang::tree::named_child_nodes;
use tree_sitter::Node;

use super::blocks::block_use;
use super::calls::value_receiver;
use super::syntax::type_base;
use super::{Emitter, GLOB_FORM};

// constraint: a method call on a parameter whose type its function declares names that type; `written` is the type's
// path in the source, `value` the parameter's name, which the receiver falls back to where the file cannot tell the
// type is one the tree may hold, and `module` the chain of the inline module around the call, whose `use`s it sees,
// `module_end` that module's end, which keys the `use`s of its blocks, and `local` whether a `let` names the type
pub(super) struct DeclaredReceiver {
    pub(super) written: Range<usize>,
    pub(super) value: Range<usize>,
    pub(super) module: Option<String>,
    pub(super) module_end: Option<usize>,
    pub(super) local: bool,
}

// constraint: the crates of the toolchain, whose types no crate of a tree declares, so a namesake the graph holds is
// another type
const SYSROOT: [&str; 5] = ["std", "core", "alloc", "test", "proc_macro"];

// constraint: the types the standard prelude names, which no glob brings into a file, so a namesake the tree holds,
// `allocator_api2::vec::Vec`, is another type
const PRELUDE: [&str; 5] = ["Box", "Option", "Result", "String", "Vec"];

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
                declared.get_or_insert(named.byte_range());
            }
        }
    }

    // constraint: a `let` of a name, `mut` or not, names the type it declares as a parameter does, or the type its
    // initializer constructs; any other pattern names none
    pub(super) fn local_type(&self, local: Node<'_>) -> Option<Range<usize>> {
        plain_name(local.child_by_field_name("pattern")?)?;
        let declared = local.child_by_field_name("type");
        let source = self.source;
        let written = match declared {
            Some(_) => declared,
            None => constructed_path(local.child_by_field_name("value")?, false, source),
        }?;
        let function = enclosing_function(local)?;
        let named = if declared.is_some() {
            named_type(written, function, source)
        } else {
            let named = named_type(type_base(written), function, source)?;
            names_a_type(named, source).then_some(named)
        };
        named.map(|named| named.byte_range())
    }

    // constraint: the receiver of a method call on a parameter of a declared type is the type's last segment, `Self`
    // the type of the impl or trait around the call; the bool tells the latter, which follows that type's owner and
    // waits for no `use`, so it carries no `DeclaredReceiver`
    pub(super) fn declared_receiver(
        &self,
        value: Node<'_>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let (written, local) = self.declared_type(&self.source[value.byte_range()])?;
        self.receiver_of(written, local, value)
    }

    // constraint: a field of `self` names the type its declaration does, as a parameter's does, where the struct the
    // impl around the call names by a bare name is declared beside the impl, in its module or block
    pub(super) fn field_receiver(
        &self,
        field: Node<'_>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let written = field_type(field, self.source)?;
        self.receiver_of(written.byte_range(), false, field)
    }

    fn receiver_of(
        &self,
        written: Range<usize>,
        local: bool,
        value: Node<'_>,
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
            value: value.byte_range(),
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
        for (call, declared) in self.declared_receivers.drain(..) {
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
            if !shown {
                self.raw.calls[call].receiver = Some(value_receiver(&source[declared.value]));
            }
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

// constraint: the path of the type an initializer constructs: `T::f()`, whose type `?`, `.unwrap()` or `.expect(..)`
// leaves where `f` fails, with `.map_err(..)` between them, and which `.map_err(..)` alone leaves a `Result`; a
// function named for a builder constructs that builder, another type; `f`'s return type goes unread, so any other
// function that returns another type still names `T`, an approximation ADR 3.55 accepts by its measured precision
fn constructed_path<'t>(value: Node<'t>, unwrapped: bool, source: &[u8]) -> Option<Node<'t>> {
    if value.kind() == "try_expression" {
        return constructed_path(value.named_child(0)?, true, source);
    }
    if value.kind() != "call_expression" {
        return None;
    }
    let function = value.child_by_field_name("function")?;
    if function.kind() == "field_expression" {
        let method = &source[function.child_by_field_name("field")?.byte_range()];
        let unwraps = matches!(method, b"unwrap" | b"expect");
        return (unwraps || (unwrapped && method == b"map_err"))
            .then(|| function.child_by_field_name("value"))
            .flatten()
            .and_then(|inner| constructed_path(inner, true, source));
    }
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

fn field_type<'t>(field: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
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
    named_type(declared.child_by_field_name("type")?, structure, source)
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
