use kartta_lang::tree::{signature_from, text};
use kartta_lang::{CallRef, SymbolId, SymbolKind};
use tree_sitter::Node;

use super::{Binding, Emitter, Scope};
use crate::syntax::{
    call_target, first_string_argument, function_value, named_child_nodes, start_line, trim_quotes,
    unquote,
};

const TEST_REGISTRARS: [&str; 11] = [
    "describe",
    "it",
    "test",
    "suite",
    "bench",
    "xdescribe",
    "xit",
    "xtest",
    "fdescribe",
    "fit",
    "ftest",
];

const TEST_MODIFIERS: [&str; 6] = [
    "skip",
    "only",
    "todo",
    "concurrent",
    "sequential",
    "failing",
];

const TEST_HOOKS: [&str; 4] = ["beforeEach", "afterEach", "beforeAll", "afterAll"];

impl Emitter<'_> {
    pub(super) fn jsx_call(&mut self, node: Node<'_>, site_of: SymbolId) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };

        let target = match name.kind() {
            "identifier" | "jsx_identifier" => (text(name, self.source), None),
            "member_expression" | "nested_identifier" => {
                let parts: Vec<Node<'_>> = named_child_nodes(name).collect();
                let Some(last) = parts.last() else {
                    return;
                };
                let receiver = match parts.as_slice() {
                    [head, _]
                        if matches!(
                            head.kind(),
                            "identifier" | "jsx_identifier" | "property_identifier"
                        ) =>
                    {
                        text(*head, self.source)
                    }
                    _ => String::new(),
                };
                (text(*last, self.source), Some(receiver))
            }
            _ => return,
        };

        if target
            .0
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_uppercase)
        {
            let (callee, receiver) = target;
            self.push_call((callee, self.value_receiver(receiver)), node, site_of);
        }
    }

    pub(super) fn new_call(&mut self, node: Node<'_>, site_of: SymbolId) {
        let Some(constructor) = node.child_by_field_name("constructor") else {
            return;
        };

        let target = match constructor.kind() {
            "identifier" => (text(constructor, self.source), None),
            "member_expression" => {
                let receiver = (constructor.child_by_field_name("object"))
                    .filter(|object| object.kind() == "identifier")
                    .map(|object| text(object, self.source));
                let Some(property) = constructor.child_by_field_name("property") else {
                    return;
                };
                (text(property, self.source), self.value_receiver(receiver))
            }
            _ => return,
        };
        self.push_call(target, node, site_of);
    }

    pub(super) fn call(&mut self, node: Node<'_>, site_of: SymbolId) {
        let (callee, written) = call_target(node, self.source);
        let owner = self.this_owner(node);
        let on_this = owner.is_some();
        let receiver = owner.or(written);

        if receiver.is_none()
            && matches!(callee.as_str(), "require" | "import")
            && let Some(specifier) = first_string_argument(node, self.source)
        {
            self.push_directive(unquote(specifier), start_line(node));
        }

        let recorded = if on_this {
            receiver.clone()
        } else {
            self.value_receiver(receiver.clone())
        };
        self.push_call((callee.clone(), recorded), node, site_of);
        self.test_registration(node, &callee, receiver.as_deref());
    }

    // constraint: a receiver the caller declares as a value names no type, so it is the empty receiver of a member call
    // on a value of no known type, unless it is a parameter its context types; an import binding, a class and a
    // namespace keep their name
    fn value_receiver(&self, receiver: Option<String>) -> Option<String> {
        let name = receiver?;
        match self.bindings.get(name.as_bytes()) {
            Some(Binding::Contextual(typed)) => Some(self.spelled(*typed)),
            _ if self.names_a_declared_value(&name) => Some(String::new()),
            _ => Some(name),
        }
    }

    fn this_owner(&self, call: Node<'_>) -> Option<String> {
        let on_this = (call.child_by_field_name("function"))
            .filter(|function| function.kind() == "member_expression")
            .and_then(|function| function.child_by_field_name("object"))
            .is_some_and(|object| object.kind() == "this");
        if on_this { self.this_class() } else { None }
    }

    // constraint: `this` is the instance of the class around the member whose body holds the use, an arrow function
    // keeping the `this` of the scope it is written in; a function expression or a function outside a class gives a
    // `this` of no known type; in a class body outside every member, `this` is an instance of the class in a field's
    // initializer and the class itself in a static one, both named by the class
    pub(super) fn this_class(&self) -> Option<String> {
        let this_of = self.scopes.last()?.this_of?;
        let symbol = self.raw.symbols.get(this_of.0 as usize)?;
        if symbol.kind == SymbolKind::Class {
            Some(symbol.qualified_name.clone())
        } else {
            symbol.owner.clone()
        }
    }

    fn test_registration(&mut self, node: Node<'_>, callee: &str, receiver: Option<&str>) {
        let name = match receiver {
            None if TEST_HOOKS.contains(&callee) => callee.to_owned(),
            _ if registers_a_test(callee, receiver) => match description(node, self.source) {
                Some(description) => description,
                None => return,
            },
            _ => return,
        };
        let Some(body) = test_body(node, self.source) else {
            return;
        };

        let mut symbol = self.unowned(name, SymbolKind::Function, node);
        symbol.signature = signature_from(node.start_byte(), body, self.source);
        let id = self.raw.push_symbol(symbol);
        let this_of = self.lexical_this(body);
        self.scopes.push(Scope::spanning(body, body, id, this_of));
    }

    fn push_call(
        &mut self,
        (callee, receiver): (String, Option<String>),
        node: Node<'_>,
        site_of: SymbolId,
    ) {
        if callee.is_empty() {
            return;
        }
        self.raw.calls.push(CallRef {
            site_of,
            callee,
            receiver,
            line: start_line(node),
        });
    }
}

fn registers_a_test(callee: &str, receiver: Option<&str>) -> bool {
    receiver.map_or_else(
        || TEST_REGISTRARS.contains(&callee),
        |registrar| TEST_REGISTRARS.contains(&registrar) && TEST_MODIFIERS.contains(&callee),
    )
}

fn description(call: Node<'_>, source: &[u8]) -> Option<String> {
    let literal = first_string_argument(call, source)?;
    let description = trim_quotes(&literal);
    (!description.is_empty()).then(|| description.to_owned())
}

fn test_body<'t>(call: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    named_child_nodes(call.child_by_field_name("arguments")?)
        .find_map(|argument| function_value(argument, source))
}
