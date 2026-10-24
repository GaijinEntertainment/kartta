use std::collections::{HashMap, HashSet};
use std::ops::Range;

use kartta_lang::SymbolKind;
use kartta_lang::tree::named_child_nodes;
use tree_sitter::Node;

use super::calls::value_receiver;
use super::syntax::type_base;
use super::{Emitter, GLOB_FORM};

// constraint: a method call on a parameter whose type its function declares names that type; `written` is the type's
// path in the source, `value` the parameter's name, which the receiver falls back to where the file cannot tell the
// type is one the tree may hold, and `module` the chain of the inline module around the call, whose `use`s it sees
pub(super) struct DeclaredReceiver {
    pub(super) written: Range<usize>,
    pub(super) value: Range<usize>,
    pub(super) module: Option<String>,
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
    // or trait around it, a path through one, a tuple and any other pattern than a name, `mut` or not, name none
    pub(super) fn push_parameter_types(&mut self, function: Node<'_>) -> Range<usize> {
        let start = self.parameter_types.len();
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
            let name = match pattern.kind() {
                "identifier" => Some(pattern),
                "mut_pattern" => {
                    named_child_nodes(pattern).find(|inner| inner.kind() == "identifier")
                }
                _ => None,
            };
            let source = self.source;
            if let Some(name) = name
                && let Some(named) = named_type(declared, function, source)
            {
                self.parameter_types
                    .push((&source[name.byte_range()], named.byte_range()));
            }
        }
        start..self.parameter_types.len()
    }

    // constraint: the receiver of a method call on a parameter of a declared type is the type's last segment, `Self`
    // the type of the impl or trait around the call; the bool tells the latter, which follows that type's owner and
    // waits for no `use`, so it carries no `DeclaredReceiver`
    pub(super) fn declared_receiver(
        &self,
        value: Node<'_>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let written = self.declared_type(&self.source[value.byte_range()])?;
        self.receiver_of(written, value)
    }

    // constraint: a field of `self` names the type its declaration does, as a parameter's does, where the struct the
    // impl around the call names by a bare name is declared beside the impl, in its module or block
    pub(super) fn field_receiver(
        &self,
        field: Node<'_>,
    ) -> Option<(String, bool, Option<DeclaredReceiver>)> {
        let written = field_type(field, self.source)?;
        self.receiver_of(written.byte_range(), field)
    }

    fn receiver_of(
        &self,
        written: Range<usize>,
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
        };
        Some((owner, false, Some(declared)))
    }

    // constraint: a declared type names its receiver only where the file shows it may be a type of the tree: a `use`
    // brings the name, or the first segment of its path, from outside the toolchain's crates, a path starts at the
    // file's crate or module or a module it declares, the file declares a type of the name no `use` brings, or, for a
    // name neither a `use` nor the prelude brings, a glob the file keeps may, one of its top level from outside the
    // toolchain's crates; the standard library's types, which crates of a tree shadow by name, `File` or `Vec`, keep
    // the value's receiver; a `use` inside a function body reaches no signature, one an inline module holds reaches
    // the signatures of that module, and one outside every block the others
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
        let source = self.source;
        for (call, declared) in self.declared_receivers.drain(..) {
            let module = declared.module.as_deref();
            let imported = |name: &str| {
                (imported.get(&(module, name)))
                    .or_else(|| imported.get(&(None, name)))
                    .copied()
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
                    || types.contains(written) || (glob && !PRELUDE.contains(&written)),
                    outside_sysroot,
                ),
            };
            if !shown {
                self.raw.calls[call].receiver = Some(value_receiver(&source[declared.value]));
            }
        }
    }
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
        "type_identifier" => (!is_type_parameter(function, declared, source)).then_some(declared),
        "scoped_type_identifier" => {
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
