use kartta_lang::tree::{signature_from, text};
use kartta_lang::{RawDirective, RawReExport, SymbolKind};
use tree_sitter::Node;

use super::{Binding, Emitter, Scope};
use crate::syntax::{child_nodes, start_line, unquote};

const IMPORT_FORM: u32 = 0;

impl Emitter<'_> {
    // constraint: an import of a bare specifier keeps the names it binds, for the resolver to tell which hold a package
    // outside the repository; a relative, a root-relative and a `#` specifier name a file of the repository even where
    // none resolves
    pub(super) fn import_statement(&mut self, node: Node<'_>) {
        let Some(source) = node.child_by_field_name("source") else {
            return;
        };
        let specifier = unquote(text(source, self.source));
        let mut directive = RawDirective::new(IMPORT_FORM, specifier, start_line(node));
        if !directive.spec.starts_with(['.', '/', '#']) {
            imported_names(node, &mut |name| {
                directive.names.push(text(name, self.source));
            });
        }
        self.raw.directives.push(directive);
    }

    pub(super) fn declare_imported(&mut self, node: Node<'_>) {
        imported_names(node, &mut |name| {
            self.declare(name.byte_range(), usize::MAX, Binding::Imported);
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

        self.push_directive(specifier, start_line(node));
    }

    pub(super) fn push_directive(&mut self, specifier: String, line: u32) {
        self.raw
            .directives
            .push(RawDirective::new(IMPORT_FORM, specifier, line));
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
            let id = self.raw.push_symbol(symbol);
            self.scopes
                .push(Scope::spanning(target, target, id, Some(id)));
        }
    }
}

// constraint: an import binds its default name, its namespace's name, and each named import by its alias, else by
// its name
fn imported_names(node: Node<'_>, bound: &mut impl FnMut(Node<'_>)) {
    match node.kind() {
        "identifier" => bound(node),
        "import_specifier" => {
            if let Some(name) = node
                .child_by_field_name("alias")
                .or_else(|| node.child_by_field_name("name"))
            {
                bound(name);
            }
        }
        "import_statement" | "import_clause" | "namespace_import" | "named_imports" => {
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
