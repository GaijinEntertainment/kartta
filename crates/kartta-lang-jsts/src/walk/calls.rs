use std::collections::HashMap;

use kartta_lang::tree::{signature_from, text};
use kartta_lang::{CallRef, LocalCall, RawExtraction, Symbol, SymbolId, SymbolKind};
use tree_sitter::Node;

use super::{Binding, Emitter, Held, Scope};
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

/// The `arguments` of a bare call of a parameter until [`settle_parameter_calls`] reads it: the walk counts no call's
/// arguments, so every other call holds 0.
const THROUGH_A_PARAMETER: u8 = u8::MAX;

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
            let binder = (node.parent()).filter(|declarator| {
                callee == "require"
                    && declarator.kind() == "variable_declarator"
                    && declarator.child_by_field_name("value") == Some(node)
            });
            self.push_directive(unquote(specifier), start_line(node), binder);
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
    // on a value of no known type, unless its annotation, its context or its `new` spells its type, or a call
    // initializes it, which spells the path of calls the graph follows; an import binding, a class and a namespace keep
    // their name
    fn value_receiver(&self, receiver: Option<String>) -> Option<String> {
        let name = receiver?;
        Some(match self.bindings.get(name.as_bytes()) {
            Some(
                Binding::Value(held)
                | Binding::Local(_, held)
                | Binding::Module(held)
                | Binding::Parameter(held),
            ) => match held {
                Some(Held::Type(typed)) => self.spelled(*typed),
                Some(Held::Constructed(class)) => format!("{}()", self.spelled(*class)),
                Some(Held::Returned(path)) => self.returned[*path].clone(),
                None => String::new(),
            },
            _ => name,
        })
    }

    /// The path of calls a call spells from the owner it is written on: `opened()`, `Kartta::open()`,
    /// `Kartta::open()::child()`, with `?` after a call an `await` takes. A call on a value of no known type spells none.
    pub(super) fn returned_path(&self, value: Node<'_>) -> Option<String> {
        let (call, awaited) = match value.kind() {
            "await_expression" => (value.named_child(0)?, "?"),
            _ => (value, ""),
        };
        if call.kind() != "call_expression" {
            return None;
        }
        let (callee, written) = call_target(call, self.source);
        if callee.is_empty() {
            return None;
        }
        let chained = (call.child_by_field_name("function"))
            .and_then(|function| function.child_by_field_name("object"))
            .filter(|object| object.kind() == "call_expression");
        if let Some(object) = chained {
            let path = self.returned_path(object)?;
            return Some(format!("{path}::{callee}(){awaited}"));
        }
        let owner = self
            .this_owner(call)
            .or_else(|| self.value_receiver(written));
        match owner.as_deref() {
            None => Some(format!("{callee}(){awaited}")),
            Some("") => None,
            Some(owner) => Some(format!("{owner}::{callee}(){awaited}")),
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
    // initializer and the class itself in a static one, both named by the class; a class expression's is named by the
    // owner of its members, the innermost owner around the use, as only a function's barrier stands between them
    pub(super) fn this_class(&self) -> Option<String> {
        let this_of = self.scopes.last()?.this_of?;
        let symbol = self.raw.symbols.get(this_of.0 as usize)?;
        if symbol.kind == SymbolKind::Class {
            Some(symbol.qualified_name.clone())
        } else if names_a_class(symbol.kind) {
            self.types
                .iter()
                .rev()
                .find_map(|scope| scope.owner.clone())
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
        symbol.flags = symbol.flags.with_label(true);
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
        let (mut arguments, mut invokes_a_macro) = (0, false);
        if receiver.is_none() && !self.function_spans.is_empty() {
            match self.bindings.get(callee.as_bytes()) {
                Some(&Binding::Local(local, _)) => {
                    self.raw.local_calls.push(LocalCall {
                        site_of,
                        local,
                        line: start_line(node),
                    });
                    return;
                }
                Some(Binding::Parameter(_)) if self.file_is_generated => invokes_a_macro = true,
                Some(Binding::Parameter(_)) => {
                    self.parameter_calls = true;
                    arguments = THROUGH_A_PARAMETER;
                }
                None => self.locals.defer(self.raw.calls.len(), node.start_byte()),
                Some(_) => {}
            }
        }
        self.raw.calls.push(CallRef {
            site_of,
            callee,
            receiver,
            line: start_line(node),
            invokes_a_macro,
            calls_a_method: false,
            arguments,
        });
    }
}

// constraint: the language has no macro and sets `MacroInvocations::Apart`, so a call marked as a macro invocation binds
// nothing and stays a site, whose name `find_unused` reads as called
/// Settles the bare calls of a parameter: one binds nothing unless the file defines a function or a class of the name
/// that no type or namespace owns, other than the definition that makes the call.
pub(super) fn settle_parameter_calls(raw: &mut RawExtraction) {
    let RawExtraction { calls, symbols, .. } = raw;
    let mut defined: HashMap<&str, (usize, bool)> = HashMap::new();
    let free = |symbol: &&Symbol| {
        symbol.owner.is_none()
            && !symbol.flags.is_label()
            && matches!(symbol.kind, SymbolKind::Function | SymbolKind::Class)
    };
    for (id, symbol) in symbols
        .iter()
        .enumerate()
        .filter(|(_, symbol)| free(symbol))
    {
        defined
            .entry(&symbol.name)
            .and_modify(|(_, several)| *several = true)
            .or_insert((id, false));
    }
    let through_a_parameter = |call: &&mut CallRef| call.arguments == THROUGH_A_PARAMETER;
    for call in calls.iter_mut().filter(through_a_parameter) {
        call.arguments = 0;
        call.invokes_a_macro = !defined
            .get(call.callee.as_str())
            .is_some_and(|&(id, several)| several || id != call.site_of.0 as usize);
    }
}

// constraint: the one symbol other than a class that is the `this` of a scope is the variable or the constant a class
// expression is bound to
pub(super) const fn names_a_class(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class | SymbolKind::Constant | SymbolKind::Variable
    )
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
