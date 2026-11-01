use kartta_lang::tree::{signature_from, text};
use kartta_lang::{RawDirective, RawReExport, SymbolKind};
use tree_sitter::Node;

use super::{Binding, Emitter, Scope};
use crate::syntax::{child_nodes, named_child_nodes, parents, start_line, unquote};

const IMPORT_FORM: u32 = 0;

impl Emitter<'_> {
    pub(super) fn import_statement(&mut self, node: Node<'_>) {
        let Some(source) = node.child_by_field_name("source") else {
            return;
        };
        let specifier = unquote(text(source, self.source));
        self.push_directive(specifier, start_line(node), Some(node));
    }

    /// Declares the names `binder`, an import or a declarator that [`binds_a_require`], binds until `until`.
    pub(super) fn declare_imported(&mut self, binder: Node<'_>, until: usize) {
        imported_names(binder, &mut |name| {
            self.declare(name.byte_range(), until, Binding::Definition);
        });
    }

    pub(super) fn export_statement(&mut self, node: Node<'_>) {
        self.exported_ids = (
            node.child_by_field_name("declaration")
                .map(|declaration| declaration.id()),
            node.child_by_field_name("value").map(|value| value.id()),
        );

        let Some(source) = node.child_by_field_name("source") else {
            let local_names = export_specifiers(node)
                .filter_map(|export| export.child_by_field_name("name"))
                .map(|name| text(name, self.source));
            self.exported_names.extend(local_names);
            self.default_export(node);
            return;
        };

        let specifier = unquote(text(source, self.source));
        for export in export_specifiers(node) {
            let Some(name) = export.child_by_field_name("name") else {
                continue;
            };
            let source_name = text(name, self.source);
            let local_name = (export.child_by_field_name("alias"))
                .map_or_else(|| source_name.clone(), |alias| text(alias, self.source));
            self.raw
                .re_exports
                .push(re_export(local_name, source_name, specifier.clone()));
        }
        let names_a_clause = child_nodes(node).any(|clause| clause.kind() == "export_clause");
        if !names_a_clause {
            let everything = re_export("*".to_owned(), "*".to_owned(), specifier.clone());
            self.raw.re_exports.push(everything);
        }

        self.push_directive(specifier, start_line(node), None);
    }

    // constraint: an import or a `require` of a bare specifier keeps the names its binder binds, for the resolver to
    // tell which hold a package outside the repository; a relative, a root-relative and a `#` specifier name a file of
    // the repository even where none resolves
    pub(super) fn push_directive(
        &mut self,
        specifier: String,
        line: u32,
        binder: Option<Node<'_>>,
    ) {
        let mut directive = RawDirective::new(IMPORT_FORM, specifier, line);
        if let Some(binder) = binder
            && !directive.spec.starts_with(['.', '/', '#'])
        {
            imported_names(binder, &mut |name| {
                directive.names.push(text(name, self.source));
            });
        }
        self.raw.directives.push(directive);
    }

    fn default_export(&mut self, node: Node<'_>) {
        if !child_nodes(node).any(|child| child.kind() == "default") {
            return;
        }
        let Some(target) =
            (node.child_by_field_name("value")).or_else(|| node.child_by_field_name("declaration"))
        else {
            return;
        };

        let anonymous = target.child_by_field_name("name").is_none();
        let kind = match target.kind() {
            "arrow_function" | "function_expression" | "function" | "generator_function" => {
                SymbolKind::Function
            }
            "function_declaration" | "generator_function_declaration" if anonymous => {
                SymbolKind::Function
            }
            "class" | "class_declaration" if anonymous => SymbolKind::Class,
            _ => return,
        };

        let mut symbol = self.unowned("default".to_owned(), kind, node);
        symbol.exported = true;
        if kind == SymbolKind::Function {
            symbol.signature = signature_from(node.start_byte(), target, self.source);
            let id = self.raw.push_symbol(symbol);
            let this_of = self.lexical_this(target);
            self.scopes
                .push(Scope::spanning(target, target, id, this_of));
        } else {
            symbol.parents = parents(target, self.source);
            let id = self.raw.push_symbol(symbol);
            self.scopes
                .push(Scope::spanning(target, target, id, Some(id)));
        }
    }
}

/// Whether `declarator` takes the module a `require` of a string names, as `const fs = require("fs")` does.
pub(super) fn binds_a_require(declarator: Node<'_>, source: &[u8]) -> bool {
    declarator
        .child_by_field_name("value")
        .filter(|value| value.kind() == "call_expression")
        .is_some_and(|call| {
            call.child_by_field_name("function")
                .is_some_and(|function| {
                    function.kind() == "identifier" && &source[function.byte_range()] == b"require"
                })
                && (call.child_by_field_name("arguments")).is_some_and(|arguments| {
                    named_child_nodes(arguments).any(|argument| argument.kind() == "string")
                })
        })
}

// constraint: an import binds its default name, its namespace's name, and each named import by its alias, else by
// its name; a declarator that takes a `require` binds each name of its pattern, a property key and a default value
// none
fn imported_names(node: Node<'_>, bound: &mut impl FnMut(Node<'_>)) {
    let field = |name| node.child_by_field_name(name);
    let inner = match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => return bound(node),
        "import_specifier" => field("alias").or_else(|| field("name")),
        "variable_declarator" => field("name"),
        "pair_pattern" => field("value"),
        "assignment_pattern" | "object_assignment_pattern" => field("left"),
        _ => None,
    };
    if let Some(inner) = inner {
        return imported_names(inner, bound);
    }
    match node.kind() {
        "import_statement" | "import_clause" | "namespace_import" | "named_imports"
        | "object_pattern" | "array_pattern" | "rest_pattern" => {
            for child in child_nodes(node) {
                imported_names(child, bound);
            }
        }
        _ => {}
    }
}

fn export_specifiers(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    (child_nodes(node).filter(|clause| clause.kind() == "export_clause"))
        .flat_map(child_nodes)
        .filter(|specifier| specifier.kind() == "export_specifier")
}

const fn re_export(local_name: String, source_name: String, source_path: String) -> RawReExport {
    RawReExport {
        local_name,
        source_name,
        source_path,
    }
}
