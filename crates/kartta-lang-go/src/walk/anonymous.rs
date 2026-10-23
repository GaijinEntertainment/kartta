use kartta_lang::SymbolKind;
use kartta_lang::tree::child_nodes;
use tree_sitter::Node;

use super::Emitter;
use super::syntax::{first_operand, parents_of};
use super::types::{bounded, is_container_type};

// constraint: the graph reads a type path's first name up to a `.`, `(` or `[` and finds it among the package's
// declarations (ADR 3.29), so an anonymous struct's name joins the names that lead to it with a rune no Go identifier
// holds and no path step starts with, and its braces keep it apart from the variable it types: `{Server·Limits}`
const JOIN: &str = "\u{b7}";

impl Emitter<'_> {
    pub(super) fn anonymous_name(&self, struct_type: Node<'_>) -> Option<String> {
        self.anonymous.get(&struct_type.id()).cloned()
    }

    // constraint: a body may declare two values of one name, and the graph finds a type local to a body by its name
    // in the file, so a name the file has given already takes a count, which no identifier's name can end in; a name
    // past a type path's bound is given to no struct, as no path could reach it
    fn name_anonymous(&mut self, struct_type: Node<'_>, scope: &str) {
        let joined = scope.replace('.', JOIN);
        let name = if let Some(count) = self.anonymous_counts.get_mut(&joined) {
            *count += 1;
            format!("{{{joined}{JOIN}{count}}}")
        } else {
            let name = format!("{{{joined}}}");
            self.anonymous_counts.insert(joined, 1);
            name
        };
        if let Some(name) = bounded(name) {
            self.anonymous.insert(struct_type.id(), name);
        }
    }

    // constraint: the fields of one declaration share their type, so its struct is named after the first of them
    pub(super) fn name_field_struct<'t>(
        &mut self,
        field: Node<'t>,
        scope: &str,
    ) -> Option<(Node<'t>, String)> {
        let struct_type = anonymous_struct(field.child_by_field_name("type")?)?;
        let mut cursor = field.walk();
        let first = field
            .children_by_field_name("name", &mut cursor)
            .map(|name| self.borrowed(name))
            .find(|&name| name != "_")?;
        let field_scope = format!("{scope}.{first}");
        self.name_anonymous(struct_type, &field_scope);
        Some((struct_type, field_scope))
    }

    pub(super) fn declare_anonymous(&mut self, struct_type: Node<'_>, scope: &str) {
        let Some(name) = self.anonymous_name(struct_type) else {
            return;
        };
        let mut symbol = self.declared(SymbolKind::Struct, struct_type, (None, name.clone()));
        symbol.parents = parents_of(struct_type, self.source);
        self.push_symbol(symbol);
        let fields = child_nodes(struct_type).filter(|field| field.kind() == "field_declaration");
        for field in fields {
            self.field_names(field, (&name, scope));
        }
    }
}

// constraint: `struct{}` holds nothing a use can name, and a set's `map[K]struct{}` or a signal's `chan struct{}`
// spell it everywhere, so only a struct with a field takes a name
fn anonymous_struct(written: Node<'_>) -> Option<Node<'_>> {
    match written.kind() {
        "struct_type" => child_nodes(written)
            .any(|field| field.kind() == "field_declaration")
            .then_some(written),
        "pointer_type" | "parenthesized_type" => anonymous_struct(first_operand(written)?),
        _ if is_container_type(written) => {
            anonymous_struct(written.child_by_field_name("element")?)
        }
        _ => None,
    }
}
