use kartta_lang::SymbolId;
use kartta_lang::tree::child_nodes;
use tree_sitter::Node;

use super::Emitter;
use super::locals::Block;
use super::syntax::{first_operand, parents_of};
use super::types::is_container_type;

/// An anonymous struct the walk declares: its type, the scope that qualifies its fields, and the position of the name
/// that owns them among the names of its declaration.
pub(super) type Anonymous<'t> = (Node<'t>, String, usize);

impl<'a> Emitter<'a> {
    // constraint: the graph reads a path's `{N}` as the field or the variable at index N of the path's file, which owns
    // the fields of the struct it types (ADR 3.170), and no Go identifier or path step holds a brace
    pub(super) fn anonymous_name(&self, struct_type: Node<'_>) -> Option<String> {
        self.anonymous
            .get(&struct_type.id())
            .map(|host| format!("{{{}}}", host.0))
    }

    // constraint: the fields of one declaration share their type, so its struct is owned by the first of them, the
    // symbol the walk declares next
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
        self.anonymous
            .insert(struct_type.id(), SymbolId(self.symbol_count));
        Some((struct_type, format!("{scope}.{first}")))
    }

    pub(super) fn first_named(&self, names: &[Node<'_>]) -> Option<&'a str> {
        names
            .iter()
            .map(|&name| self.borrowed(name))
            .find(|&name| name != "_")
    }

    // constraint: a value a body declares qualifies its struct's fields by the callable that holds it too,
    // `app.TestRun.tests.name`, since two bodies of a file often declare one name, `tests`
    pub(super) fn name_value_struct<'t>(
        &mut self,
        written: Node<'t>,
        (name, host): (&str, SymbolId),
    ) -> Option<(Node<'t>, String)> {
        let struct_type = anonymous_struct(written)?;
        let caller = self
            .callers
            .last()
            .filter(|_| !self.blocks.is_empty())
            .map(|scope| {
                self.raw.symbols[scope.id.0 as usize]
                    .qualified_name
                    .as_str()
            })
            .and_then(|qualified| qualified.strip_prefix(self.package.as_str()))
            .and_then(|qualified| qualified.strip_prefix('.'));
        let scope = caller.map_or_else(|| name.to_owned(), |caller| format!("{caller}.{name}"));
        self.anonymous.insert(struct_type.id(), host);
        Some((struct_type, scope))
    }

    // constraint: a literal's type is the type of a name only where the values line up with the names; each name of a
    // `var` spec is a symbol, and a `:=` name of the block given is one only where it declares a value whose literal's
    // struct needs an owner
    pub(super) fn name_literal_structs<'t>(
        &mut self,
        (names, values): (&[Node<'t>], &[Node<'t>]),
        short: Option<Block>,
    ) -> Vec<Anonymous<'t>> {
        if names.len() != values.len() {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut host = self.symbol_count;
        for (position, (&name, &value)) in names.iter().zip(values).enumerate() {
            let name = self.borrowed(name);
            if name == "_" {
                continue;
            }
            let owned = literal_type(value)
                .filter(|_| short.is_none_or(|block| !self.is_reassigned(name, block)))
                .and_then(|written| self.name_value_struct(written, (name, SymbolId(host))));
            if short.is_none() || owned.is_some() {
                host += 1;
            }
            found.extend(owned.map(|(struct_type, scope)| (struct_type, scope, position)));
        }
        found
    }

    // constraint: the symbol that owns an anonymous struct's fields holds the types the struct embeds
    pub(super) fn declare_anonymous(&mut self, struct_type: Node<'_>, scope: &str) {
        let Some(&host) = self.anonymous.get(&struct_type.id()) else {
            return;
        };
        let parents = parents_of(struct_type, self.source);
        let host = &mut self.raw.symbols[host.0 as usize];
        host.parents.extend(parents);
        let owner = host.name.clone();
        let fields = child_nodes(struct_type).filter(|field| field.kind() == "field_declaration");
        for field in fields {
            self.field_names(field, (&owner, scope));
        }
    }
}

pub(super) fn literal_type(value: Node<'_>) -> Option<Node<'_>> {
    match value.kind() {
        "composite_literal" => value.child_by_field_name("type"),
        "parenthesized_expression" => literal_type(first_operand(value)?),
        "unary_expression" if value.child_by_field_name("operator")?.kind() == "&" => {
            literal_type(value.child_by_field_name("operand")?)
        }
        _ => None,
    }
}

// constraint: `struct{}` holds nothing a use can name, and a set's `map[K]struct{}` or a signal's `chan struct{}`
// spell it everywhere, so only a struct with a field takes an owner
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
