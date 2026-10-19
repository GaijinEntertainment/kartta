use kartta_lang::tree::{line, preorder, text};
use kartta_lang::{Access, NO_CALLER_ID, Receiver, SymbolId, UseRef};
use tree_sitter::Node;

use super::Emitter;
use super::locals::declares_left;
use super::syntax::{PREDECLARED, first_operand, is_pointer_indirection};
use super::types::{element_of, is_container_path, is_container_type};

impl<'a> Emitter<'a> {
    pub(super) fn record_use(&mut self, node: Node<'_>, owner: SymbolId) -> bool {
        match node.kind() {
            "type_identifier" => {
                if self.names_package_type(self.borrowed(node), node.start_byte()) {
                    self.push_use(owner, node, (node, None));
                }
                false
            }
            "qualified_identifier" => {
                if let (Some(package), Some(name)) = (
                    node.child_by_field_name("package"),
                    node.child_by_field_name("name"),
                ) {
                    self.push_use(owner, node, (name, Some(package)));
                }
                false
            }
            "selector_expression" => {
                if self.named_by_call == Some(node.id()) {
                    return true;
                }
                let access = access_of(node, node.parent());
                if let Some((package, field)) = self.import_qualifier(node) {
                    let qualifier = Receiver::Qualifier(text(package, self.source));
                    self.push_site(owner, node, (text(field, self.source), qualifier, access));
                } else if let (Some(operand), Some(field)) = (
                    node.child_by_field_name("operand"),
                    node.child_by_field_name("field"),
                ) {
                    let path = Receiver::Path(self.value_path(operand).unwrap_or_default());
                    self.push_site(owner, node, (text(field, self.source), path, access));
                }
                true
            }
            _ => true,
        }
    }

    // constraint: a name the file's walk cannot see declared is kept, since a sibling file of the package may declare
    // it, and the graph binds only the ones some file declares
    pub(super) fn record_identifier(&mut self, node: Node<'_>, owner: SymbolId) {
        let name = self.borrowed(node);
        let skipped = name == "_"
            || PREDECLARED.contains(&name)
            || self.named_by_call == Some(node.id())
            || self.literal_key == Some(node.id())
            || self.type_params.iter().any(|(_, param)| param == name)
            || self.is_local(name, node.start_byte());
        if skipped {
            return;
        }
        let Some(parent) = node.parent() else {
            return;
        };
        let names_an_import =
            parent.kind() == "selector_expression" && self.import_qualifier(parent).is_some();
        if !names_an_import && !declares(node, parent) {
            let access = access_of(node, Some(parent));
            self.push_site(owner, node, (name.to_owned(), Receiver::Bare, access));
        }
    }

    // constraint: a key of a struct literal names a field the literal writes, and a key of a map, slice or array
    // literal is a value; a literal of an anonymous struct, or of a type the walk cannot name, keeps none
    pub(super) fn record_literal_key(&mut self, keyed: Node<'_>, owner: SymbolId) {
        let Some(key) = keyed
            .child_by_field_name("key")
            .filter(|key| key.kind() == "identifier")
        else {
            return;
        };
        let keys = keyed
            .parent()
            .map_or(Keys::Unknown, |value| self.keys_of(value));
        match keys {
            Keys::Fields(path) => {
                let field = (text(key, self.source), Receiver::Path(path), Access::Write);
                self.push_site(owner, key, field);
                self.literal_key = Some(key.id());
            }
            Keys::Unknown => self.literal_key = Some(key.id()),
            Keys::Values(_) => {}
        }
    }

    // constraint: Go leaves a literal's type unwritten only for an element of a slice, array or map literal, which the
    // enclosing literal's element type gives
    fn keys_of(&self, value: Node<'_>) -> Keys {
        let Some(holder) = value.parent() else {
            return Keys::Unknown;
        };
        match holder.kind() {
            "composite_literal" => match holder.child_by_field_name("type") {
                Some(written) if written.kind() == "struct_type" => Keys::Unknown,
                Some(written) if is_container_type(written) => {
                    Keys::Values(self.type_path(written))
                }
                Some(written) => self.type_path(written).map_or(Keys::Unknown, Keys::Fields),
                None => Keys::Unknown,
            },
            "literal_value" => self.element_keys(holder),
            "keyed_element"
                if holder
                    .child_by_field_name("element")
                    .is_some_and(|element| element.id() == value.id()) =>
            {
                holder
                    .parent()
                    .map_or(Keys::Unknown, |outer| self.element_keys(outer))
            }
            _ => Keys::Unknown,
        }
    }

    fn element_keys(&self, outer: Node<'_>) -> Keys {
        match self.keys_of(outer) {
            Keys::Fields(path) | Keys::Values(Some(path)) => match element_of(&path) {
                Some(element) if is_container_path(&element) => Keys::Values(Some(element)),
                Some(element) => Keys::Fields(element),
                None => Keys::Unknown,
            },
            Keys::Values(None) | Keys::Unknown => Keys::Unknown,
        }
    }

    pub(super) fn record_type_operand(&mut self, node: Node<'_>, owner: SymbolId) {
        match node.kind() {
            "selector_expression" => self.record_qualified_type_operand(node, owner),
            "unary_expression" => {
                if is_pointer_indirection(node)
                    && let Some(operand) = node.child_by_field_name("operand")
                {
                    self.record_type_operand(operand, owner);
                }
            }
            "parenthesized_expression" => {
                if let Some(inner) = first_operand(node) {
                    self.record_type_operand(inner, owner);
                }
            }
            "index_expression" => {
                if let Some(operand) = node.child_by_field_name("operand") {
                    self.record_type_operand(operand, owner);
                }
                let mut cursor = node.walk();
                for argument in node.children_by_field_name("index", &mut cursor) {
                    self.record_type_operand(argument, owner);
                }
            }
            _ => {}
        }
    }

    // constraint: the walk records a selector on an import name itself when it reaches it, so only a selector whose
    // operand names no import of the file is recorded here
    fn record_qualified_type_operand(&mut self, node: Node<'_>, owner: SymbolId) {
        if let (Some(package), Some(name)) = (
            node.child_by_field_name("operand"),
            node.child_by_field_name("field"),
        ) && package.kind() == "identifier"
            && !self.is_value(self.borrowed(package), node.start_byte())
            && self.import_qualifier(node).is_none()
        {
            self.push_use(owner, node, (name, Some(package)));
        }
    }

    fn push_use(
        &mut self,
        owner: SymbolId,
        site: Node<'_>,
        (name, qualifier): (Node<'_>, Option<Node<'_>>),
    ) {
        if self.named_by_call == Some(site.id()) {
            return;
        }
        let receiver = qualifier.map_or(Receiver::Bare, |qualifier| {
            Receiver::Qualifier(text(qualifier, self.source))
        });
        self.push_site(
            owner,
            site,
            (text(name, self.source), receiver, Access::Read),
        );
    }

    fn push_site(
        &mut self,
        owner: SymbolId,
        site: Node<'_>,
        (name, receiver, access): (String, Receiver, Access),
    ) {
        self.raw.uses.push(UseRef {
            site_of: owner,
            name,
            receiver,
            access,
            line: line(site.start_position().row),
        });
    }

    // constraint: an import binds the name its package clause declares, which the import path need not spell, so a
    // qualifier that names nothing the file declares is kept for the graph, which binds it only to an import
    fn import_qualifier<'t>(&self, selector: Node<'t>) -> Option<(Node<'t>, Node<'t>)> {
        let operand = selector
            .child_by_field_name("operand")
            .filter(|operand| operand.kind() == "identifier")?;
        let field = selector.child_by_field_name("field")?;
        let qualifier = self.borrowed(operand);
        let may_name_an_import = self.import_names.iter().any(|name| name == qualifier)
            || (!self.package_values.contains(qualifier) && !PREDECLARED.contains(&qualifier));

        (may_name_an_import && !self.is_local(qualifier, selector.start_byte()))
            .then_some((operand, field))
    }

    fn names_package_type(&self, name: &str, at: usize) -> bool {
        !PREDECLARED.contains(&name)
            && !self.type_params.iter().any(|(_, param)| param == name)
            && !self.is_local(name, at)
    }

    pub(super) fn record_uses_in(&mut self, node: Node<'_>, owner: SymbolId) {
        preorder(node, |current| {
            !current.is_named() || self.record_use(current, owner)
        });
    }

    pub(super) fn use_owner(&self) -> SymbolId {
        self.use_owners
            .last()
            .map_or(NO_CALLER_ID, |scope| scope.id)
    }

    pub(super) fn borrowed(&self, node: Node<'_>) -> &'a str {
        node.utf8_text(self.source).unwrap_or_default()
    }
}

enum Keys {
    Fields(String),
    Values(Option<String>),
    Unknown,
}

fn declares(name: Node<'_>, parent: Node<'_>) -> bool {
    match parent.kind() {
        "parameter_declaration" | "function_declaration" | "var_spec" | "const_spec" => true,
        "type_switch_statement" => parent
            .child_by_field_name("name")
            .is_some_and(|declared| declared.id() == name.id()),
        "expression_list" => parent.parent().is_some_and(|statement| {
            matches!(
                statement.kind(),
                "short_var_declaration" | "range_clause" | "receive_statement"
            ) && declares_left(statement)
                && is_left(statement, parent)
        }),
        _ => false,
    }
}

// constraint: only the outermost expression on the left of an assignment is assigned; `a.b` in `a.b.c = v` is read
fn access_of(site: Node<'_>, parent: Option<Node<'_>>) -> Access {
    let (mut at, mut holder) = (site, parent);
    while let Some(statement) = holder {
        match statement.kind() {
            "expression_list" | "parenthesized_expression" => {
                at = statement;
                holder = statement.parent();
            }
            "inc_dec_statement" => return Access::Update,
            "assignment" if is_left(statement, at) => {
                let plain = statement
                    .child_by_field_name("operator")
                    .is_none_or(|operator| operator.kind() == "=");
                return if plain { Access::Write } else { Access::Update };
            }
            "range_clause" | "receive_statement" if is_left(statement, at) => {
                return Access::Write;
            }
            _ => return Access::Read,
        }
    }
    Access::Read
}

fn is_left(statement: Node<'_>, side: Node<'_>) -> bool {
    statement
        .child_by_field_name("left")
        .is_some_and(|left| left.id() == side.id())
}
