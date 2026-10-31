use kartta_lang::tree::{line, named_child_nodes};
use kartta_lang::{CallRef, LocalCall, SymbolId};
use tree_sitter::Node;

use super::Emitter;
use super::locals::LocalKind;
use super::syntax::{
    INSPECTING_CALLS, SERIALIZING_CALLS, TYPE_ONLY_KINDS, call_target, first_operand,
    is_pointer_indirection,
};
use super::types::inside_containers;

impl Emitter<'_> {
    pub(super) fn call(&mut self, node: Node<'_>, site_of: SymbolId) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let Some(invoked) = self.invoked(function) else {
            return;
        };
        self.named_by_call = Some(invoked.id());

        let target = self.spelled_receiver(invoked, call_target(invoked, self.source));
        let callee = target.0.as_str();
        if invoked.kind() == "identifier" && matches!(callee, "new" | "make") {
            self.record_allocated_type(node);
        }
        let serializes = SERIALIZING_CALLS.contains(&callee);
        if serializes || INSPECTING_CALLS.contains(&callee) {
            self.note_reflected_values(node, serializes);
        }
        if invoked.kind() == "identifier"
            && let Some(local) = self.innermost_local(callee, invoked.start_byte())
        {
            if let LocalKind::ValueSymbol(symbol) | LocalKind::TypeSymbol(symbol) = local.kind {
                self.raw.local_calls.push(LocalCall {
                    site_of,
                    local: symbol,
                    line: line(node.start_position().row),
                });
            }
            return;
        }
        self.push_call(target, node, site_of);
    }

    // constraint: the receiver is the type path of the operand's value, and the empty receiver of a member call on a
    // value of no known type where the walk names none; a local type's name is kept as written
    fn spelled_receiver(
        &self,
        invoked: Node<'_>,
        (callee, receiver): (String, Option<String>),
    ) -> (String, Option<String>) {
        let Some(operand) = invoked.child_by_field_name("operand") else {
            return (callee, receiver);
        };
        let local_type = operand.kind() == "identifier"
            && self
                .innermost_local(self.borrowed(operand), operand.start_byte())
                .is_some_and(|local| matches!(local.kind, LocalKind::TypeSymbol(_)));
        if local_type {
            return (callee, receiver);
        }
        (callee, Some(self.value_path(operand).unwrap_or_default()))
    }

    // constraint: a conversion, `(T)(x)` or `(*T)(x)`, and a call of a parenthesized name, `(f)(x)`, have one tree, so
    // the operand in the parentheses is recorded as a use and the call as none
    fn invoked<'t>(&mut self, function: Node<'t>) -> Option<Node<'t>> {
        match function.kind() {
            "index_expression" => {
                let generic = self.instantiated(function)?;
                let owner = self.use_owner();
                let mut cursor = function.walk();
                for argument in function.children_by_field_name("index", &mut cursor) {
                    self.record_type_operand(argument, owner);
                }
                Some(generic)
            }
            "parenthesized_expression" => {
                if let Some(inner) = first_operand(function) {
                    let owner = self.use_owner();
                    self.record_type_operand(inner, owner);
                }
                None
            }
            _ => Some(function),
        }
    }

    // constraint: a type a body declares has fields local to it, which no use binds, so a value of one hands no
    // package-level namesake's fields over
    fn note_reflected_values(&mut self, call: Node<'_>, honors_skip_tags: bool) {
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        for argument in named_child_nodes(arguments) {
            let Some(path) = self.value_path(argument) else {
                continue;
            };
            let of_a_local_type = self
                .innermost_local(inside_containers(&path), argument.start_byte())
                .is_some_and(|local| matches!(local.kind, LocalKind::TypeSymbol(_)));
            if !of_a_local_type {
                self.reflected_values.push((path, honors_skip_tags));
            }
        }
    }

    fn record_allocated_type(&mut self, call: Node<'_>) {
        if let Some(operand) = call
            .child_by_field_name("arguments")
            .and_then(first_operand)
        {
            let owner = self.use_owner();
            self.record_type_operand(operand, owner);
        }
    }

    // constraint: the walk reads one file, so a package-level variable that another file of the package declares
    // passes as a generic function
    fn instantiated<'t>(&self, index: Node<'t>) -> Option<Node<'t>> {
        let operand = index.child_by_field_name("operand")?;
        let at = index.start_byte();
        let generic = match operand.kind() {
            "identifier" => {
                let name = self.borrowed(operand);
                !self.is_local(name, at) && !self.package_values.contains(name)
            }
            "selector_expression" => true,
            _ => false,
        };

        (generic && self.has_type_arguments(index, at)).then_some(operand)
    }

    fn has_type_arguments(&self, index: Node<'_>, at: usize) -> bool {
        let mut cursor = index.walk();
        let mut arguments = index.children_by_field_name("index", &mut cursor);

        arguments
            .next()
            .is_some_and(|first| arguments.next().is_some() || self.is_type_argument(first, at))
    }

    // constraint: the syntax cannot tell a type from a value, so a name the file does not declare as a value is taken
    // as a type argument
    fn is_type_argument(&self, node: Node<'_>, at: usize) -> bool {
        match node.kind() {
            "identifier" => !self.is_value(self.borrowed(node), at),
            "selector_expression" => node.child_by_field_name("operand").is_some_and(|operand| {
                operand.kind() == "identifier" && !self.is_value(self.borrowed(operand), at)
            }),
            "unary_expression" => {
                is_pointer_indirection(node)
                    && node
                        .child_by_field_name("operand")
                        .is_some_and(|operand| self.is_type_argument(operand, at))
            }
            "parenthesized_expression" => {
                first_operand(node).is_some_and(|inner| self.is_type_argument(inner, at))
            }
            "index_expression" => {
                node.child_by_field_name("operand")
                    .is_some_and(|operand| self.is_type_argument(operand, at))
                    && self.has_type_arguments(node, at)
            }
            kind => TYPE_ONLY_KINDS.contains(&kind),
        }
    }

    fn push_call(
        &mut self,
        (callee, receiver): (String, Option<String>),
        node: Node<'_>,
        site_of: SymbolId,
    ) {
        if callee.is_empty() || self.symbol_count == 0 {
            return;
        }

        self.raw.calls.push(CallRef {
            site_of,
            callee,
            receiver,
            line: line(node.start_position().row),
            invokes_a_macro: false,
            calls_a_method: false,
            arguments: 0,
        });
    }
}
