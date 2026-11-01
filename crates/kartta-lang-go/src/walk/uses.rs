use std::collections::HashMap;

use kartta_lang::tree::{child_nodes, line, named_child_nodes, preorder, text};
use kartta_lang::{Access, FILE_SCOPE, Receiver, SymbolId, UseRef};
use tree_sitter::Node;

use super::Emitter;
use super::locals::{Local, LocalKind, declares_left};
use super::syntax::{PREDECLARED, first_operand, is_exported, is_pointer_indirection};
use super::types::{element_of, is_container_path, is_container_type};

const MAX_NAMES_FOLLOWED: usize = 8;

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
                let holders = std::iter::successors(node.parent(), Node::parent);
                self.record_selector(node, owner, access_of(node, holders));
                true
            }
            _ => true,
        }
    }

    pub(super) fn record_walked_selector(&mut self, node: Node<'_>, owner: SymbolId) {
        let access = access_of(node, self.ancestors.iter().rev().copied());
        self.record_selector(node, owner, access);
    }

    fn record_selector(&mut self, node: Node<'_>, owner: SymbolId, access: Access) {
        if self.named_by_call == Some(node.id()) {
            return;
        }
        let access = if self.takes_type_operand(node) {
            Access::Type
        } else {
            access
        };
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
    }

    // constraint: a name the file's walk cannot see declared is kept, since a sibling file of the package may declare
    // it, and the graph binds only the ones some file declares; a selector's operand that names no import is such a
    // name, since Go lets no package-level name share an import's
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
        let Some((&parent, above)) = self.ancestors.split_last() else {
            return;
        };
        let names_an_import = parent.kind() == "selector_expression"
            && self.import_names.iter().any(|import| import == name);
        if !names_an_import && !declares(node, parent, above) {
            let access = if self.takes_type_operand(node) {
                Access::Type
            } else {
                access_of(node, self.ancestors.iter().rev().copied())
            };
            self.push_site(owner, node, (name.to_owned(), Receiver::Bare, access));
        }
    }

    // constraint: a key of a struct literal names a field the literal writes, and a key of a map, slice or array
    // literal is a value; a literal of a type the walk cannot name, an anonymous struct assigned to no name among them,
    // keeps none
    pub(super) fn record_literal_key(&mut self, keyed: Node<'_>, owner: SymbolId) {
        let Some(key) = keyed
            .child_by_field_name("key")
            .filter(|key| key.kind() == "identifier")
        else {
            return;
        };
        match self.keys_of(&self.literals) {
            Keys::Fields(path) if !self.takes_value_keys(&path, (key, keyed.start_byte())) => {
                let field = (text(key, self.source), Receiver::Path(path), Access::Write);
                self.push_site(owner, key, field);
                self.literal_key = Some(key.id());
            }
            Keys::Unknown => self.literal_key = Some(key.id()),
            Keys::Fields(_) | Keys::Values(_) => {}
        }
    }

    // constraint: a named type the file declares as a map, a slice or an array takes values for keys, and so does
    // another package's type for an unexported key, since Go lets no package set another's unexported field; a type
    // a sibling file declares is left a struct, which the walk cannot tell from a container
    fn takes_value_keys(&self, path: &str, (key, at): (Node<'_>, usize)) -> bool {
        let plain = |name: &str| name.chars().all(|c| c == '_' || c.is_alphanumeric());
        match path.split_once('.') {
            Some((package, name)) => {
                plain(package) && plain(name) && !is_exported(self.borrowed(key))
            }
            None => {
                plain(path)
                    && self
                        .declaration_of(path, at)
                        .and_then(|declared| self.declared_type(declared, at))
                        .is_some_and(is_container_type)
            }
        }
    }

    // constraint: Go leaves a literal's type unwritten only for an element of a slice, array or map literal, which the
    // enclosing literal's element type gives
    fn keys_of(&self, open: &[Node<'_>]) -> Keys {
        let [held @ .., value] = open else {
            return Keys::Unknown;
        };
        let [outer @ .., holder] = held else {
            return Keys::Unknown;
        };
        match holder.kind() {
            "composite_literal" => match holder.child_by_field_name("type") {
                Some(written) if is_container_type(written) => {
                    Keys::Values(self.type_path(written))
                }
                Some(written) => self.type_path(written).map_or(Keys::Unknown, Keys::Fields),
                None => Keys::Unknown,
            },
            "literal_value" => self.element_keys(held),
            "keyed_element"
                if holder
                    .child_by_field_name("element")
                    .is_some_and(|element| element.id() == value.id()) =>
            {
                self.element_keys(outer)
            }
            _ => Keys::Unknown,
        }
    }

    // constraint: a struct literal that names no field sets every field in the order its type declares them, which only
    // the declaring file holds, so a type declared elsewhere records none; the literal is read from its written type
    // down through the elements whose type it elides, since a table holds thousands of them and each node's parent is
    // found from the root
    pub(super) fn record_positional_literal(&mut self, literal: Node<'a>, owner: SymbolId) {
        let (Some(written), Some(value)) = (
            literal.child_by_field_name("type"),
            literal.child_by_field_name("value"),
        ) else {
            return;
        };
        if !named_child_nodes(value).any(|element| elided_or_positional(element).is_some()) {
            return;
        }
        let Some(declared) = self.declared_type(written, literal.start_byte()) else {
            return;
        };
        let keys = if is_container_type(written) {
            Keys::Values(self.type_path(written))
        } else {
            self.type_path(written).map_or(Keys::Unknown, Keys::Fields)
        };
        self.record_positional_level((value, declared), &keys, owner);
    }

    fn record_positional_level(
        &mut self,
        (value, declared): (Node<'a>, Node<'a>),
        keys: &Keys,
        owner: SymbolId,
    ) {
        if declared.kind() == "struct_type" {
            if let Keys::Fields(path) = keys {
                self.record_positional_fields((value, declared), path, owner);
            }
            return;
        }
        let element_keys = elements_of(keys);
        let element = is_container_type(declared)
            .then(|| declared.child_by_field_name("element"))
            .flatten()
            .filter(|_| !matches!(element_keys, Keys::Unknown))
            .and_then(|element| self.declared_type(element, value.start_byte()))
            .filter(|element| element.kind() == "struct_type" || is_container_type(*element));
        let Some(element) = element else {
            return;
        };
        let elided = named_child_nodes(value)
            .filter_map(elided_or_positional)
            .filter(|elided| elided.kind() == "literal_value");
        for elided in elided {
            self.record_positional_level((elided, element), &element_keys, owner);
        }
    }

    // constraint: an embedded or blank field takes a place and names no field the walk declares
    fn record_positional_fields(
        &mut self,
        (value, fields): (Node<'a>, Node<'a>),
        path: &str,
        owner: SymbolId,
    ) {
        let mut elements = named_child_nodes(value).filter(|element| element.kind() != "comment");
        if elements
            .next()
            .is_none_or(|first| first.kind() == "keyed_element")
            || field_places(fields).count() != elements.count() + 1
        {
            return;
        }
        let elements = named_child_nodes(value).filter(|element| element.kind() != "comment");
        for (element, field) in elements.zip(field_places(fields)) {
            if let Some(field) = field.filter(|field| self.borrowed(*field) != "_") {
                let write = (
                    text(field, self.source),
                    Receiver::Path(path.to_owned()),
                    Access::Write,
                );
                self.push_site(owner, element, write);
            }
        }
    }

    // constraint: a type the file declares under a name, an alias or a defined type, has the fields or the elements of
    // the type it is declared as; Go forbids a cycle, which a bound on the names followed stops all the same
    fn declared_type(&self, written: Node<'a>, at: usize) -> Option<Node<'a>> {
        let (mut declared, mut names) = (written, 0);
        loop {
            declared = match declared.kind() {
                "pointer_type" | "parenthesized_type" => first_operand(declared)?,
                "generic_type" => declared.child_by_field_name("type")?,
                "type_identifier" if names < MAX_NAMES_FOLLOWED => {
                    names += 1;
                    self.declaration_of(self.borrowed(declared), at)?
                }
                "type_identifier" => return None,
                _ => return Some(declared),
            };
        }
    }

    pub(super) fn declaration_of(&self, name: &str, at: usize) -> Option<Node<'a>> {
        match self.innermost_local(name, at) {
            Some(Local {
                kind: LocalKind::TypeSymbol(_),
                from,
                ..
            }) => {
                let local = self.root.descendant_for_byte_range(*from, from + 1);
                std::iter::successors(local, Node::parent)
                    .find(|node| matches!(node.kind(), "type_definition" | "alias_declaration"))?
                    .child_by_field_name("type")
            }
            Some(_) => None,
            None => self
                .package_types
                .get_or_init(|| self.package_types())
                .get(name)
                .copied(),
        }
    }

    // constraint: a file's composite literals look the types they name up among its package-level declarations, so the
    // declarations are indexed once, at the first such literal
    fn package_types(&self) -> HashMap<&'a str, Node<'a>> {
        let specs = child_nodes(self.root)
            .filter(|declaration| declaration.kind() == "type_declaration")
            .flat_map(child_nodes);
        let mut types = HashMap::new();
        for spec in specs {
            if let (Some(name), Some(written)) = (
                spec.child_by_field_name("name"),
                spec.child_by_field_name("type"),
            ) {
                types.entry(self.borrowed(name)).or_insert(written);
            }
        }
        types
    }

    fn element_keys(&self, open: &[Node<'_>]) -> Keys {
        elements_of(&self.keys_of(open))
    }

    pub(super) fn record_type_operand(&mut self, node: Node<'_>, owner: SymbolId) {
        match node.kind() {
            "identifier" => self.type_operands.push(node.id()),
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
    // operand names no import of the file is recorded here, and the other is marked a type operand for the walk
    fn record_qualified_type_operand(&mut self, node: Node<'_>, owner: SymbolId) {
        if self.import_qualifier(node).is_some() {
            self.type_operands.push(node.id());
        } else if let (Some(package), Some(name)) = (
            node.child_by_field_name("operand"),
            node.child_by_field_name("field"),
        ) && package.kind() == "identifier"
            && !self.is_value(self.borrowed(package), node.start_byte())
        {
            self.push_use(owner, node, (name, Some(package)));
        }
    }

    fn takes_type_operand(&mut self, node: Node<'_>) -> bool {
        let operand = self.type_operands.iter().position(|&id| id == node.id());
        operand
            .map(|at| self.type_operands.swap_remove(at))
            .is_some()
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
            (text(name, self.source), receiver, Access::Type),
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
        self.use_owners.last().map_or(FILE_SCOPE, |scope| scope.id)
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

// constraint: only an element without a key, or a key's literal whose type its container elides, can hold a positional
// struct literal
fn elided_or_positional(element: Node<'_>) -> Option<Node<'_>> {
    match element.kind() {
        "comment" => None,
        "keyed_element" => element
            .child_by_field_name("element")
            .filter(|value| value.kind() == "literal_value"),
        _ => Some(element),
    }
}

fn elements_of(keys: &Keys) -> Keys {
    match keys {
        Keys::Fields(path) | Keys::Values(Some(path)) => match element_of(path) {
            Some(element) if is_container_path(&element) => Keys::Values(Some(element)),
            Some(element) => Keys::Fields(element),
            None => Keys::Unknown,
        },
        Keys::Values(None) | Keys::Unknown => Keys::Unknown,
    }
}

fn field_places(struct_type: Node<'_>) -> impl Iterator<Item = Option<Node<'_>>> {
    child_nodes(struct_type)
        .filter(|field| field.kind() == "field_declaration")
        .flat_map(|field| {
            let names = (0..field.child_count()).filter_map(move |index| {
                (field.field_name_for_child(index) == Some("name"))
                    .then(|| field.child(index))
                    .flatten()
            });
            let embedded = field.child_by_field_name("name").is_none();
            names.map(Some).chain(embedded.then_some(None))
        })
}

fn declares(name: Node<'_>, parent: Node<'_>, above: &[Node<'_>]) -> bool {
    match parent.kind() {
        "parameter_declaration" | "function_declaration" | "var_spec" | "const_spec" => true,
        "type_switch_statement" => parent
            .child_by_field_name("name")
            .is_some_and(|declared| declared.id() == name.id()),
        "expression_list" => above.last().is_some_and(|&statement| {
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
fn access_of<'t>(site: Node<'t>, holders: impl Iterator<Item = Node<'t>>) -> Access {
    let mut at = site;
    for statement in holders {
        match statement.kind() {
            "expression_list" | "parenthesized_expression" => at = statement,
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
