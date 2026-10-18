use kartta_lang::tree::text;
use kartta_lang::{Access, NO_CALLER_ID, Receiver, RefSite, SymbolId};
use tree_sitter::Node;

use super::Emitter;
use super::syntax::{PREDECLARED, first_operand, is_pointer_indirection, line, next_in_preorder};

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
                if let Some((package, field)) = self.import_qualifier(node) {
                    self.push_use(owner, node, (field, Some(package)));
                }
                true
            }
            _ => true,
        }
    }

    pub(super) fn record_type_operand(&mut self, node: Node<'_>, owner: SymbolId) {
        match node.kind() {
            "identifier" => {
                if self.names_package_type(self.borrowed(node), node.start_byte()) {
                    self.push_use(owner, node, (node, None));
                }
            }
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
        self.raw.references.push(RefSite {
            site_of: owner,
            name: text(name, self.source),
            receiver: qualifier.map_or(Receiver::Bare, |qualifier| {
                Receiver::Qualifier(text(qualifier, self.source))
            }),
            access: Access::Read,
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
        let mut cursor = node.walk();
        loop {
            let current = cursor.node();
            let descend = !current.is_named() || self.record_use(current, owner);
            if !next_in_preorder(&mut cursor, descend) {
                return;
            }
        }
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
