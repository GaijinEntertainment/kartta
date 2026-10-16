use kartta_lang::tree::text;
use kartta_lang::{CallRef, SymbolId};
use tree_sitter::Node;

use super::blocks::{CallForm, path_segments};
use super::syntax::{last_segment, line, path_text, type_base};
use super::{Emitter, open_impls_over};

struct Target {
    callee: String,
    receiver: Option<String>,
    names_own_type: bool,
}

impl Target {
    const fn new(callee: String, receiver: Option<String>) -> Self {
        Self {
            callee,
            receiver,
            names_own_type: false,
        }
    }

    const fn none() -> Self {
        Self::new(String::new(), None)
    }
}

impl Emitter<'_> {
    // constraint: a scoped call's receiver path under the module chain replaces its receiver after the walk when the
    // file declares a type or module of that name, and a `self` or `Self` receiver follows the owner of the impl it
    // names when that owner settles
    pub(super) fn call(&mut self, node: Node<'_>, caller_id: SymbolId) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let target = self.call_target(function);
        let path = receiver_path(function, self.source).and_then(|path| self.module_path(&path));
        let names_own_type = target.names_own_type;
        let call = CallRef {
            caller_id,
            callee: target.callee,
            receiver: target.receiver,
            line: line(node.start_position().row),
        };
        if !self.push_call(call) {
            return;
        }
        if !self.functions.is_empty()
            && let Some(form) = call_form(function, self.source)
        {
            self.record_block_call(node, form);
        }

        let index = self.raw.calls.len() - 1;
        if names_own_type && let Some(own) = &self.raw.calls[index].receiver {
            for at in open_impls_over(&self.types, own) {
                self.impl_owners[at].receivers.push(index);
            }
        }
        if let Some(path) = path {
            for at in open_impls_over(&self.types, &path) {
                self.impl_owners[at].paths.push(self.receiver_paths.len());
            }
            self.receiver_paths.push((index, path));
        }
    }

    // constraint: a scoped callee takes the last path segment before its name as the receiver, `Widget::new()`
    // calling `new` on `Widget`; a member call takes a named operand, `self` as the type of the impl or trait around
    // it, and anything else as the empty receiver
    fn call_target(&self, function: Node<'_>) -> Target {
        match function.kind() {
            "identifier" => Target::new(text(function, self.source), None),
            "scoped_identifier" => {
                let callee = function
                    .child_by_field_name("name")
                    .map_or_else(String::new, |name| text(name, self.source));
                let (receiver, names_own_type) = function
                    .child_by_field_name("path")
                    .map_or((None, false), |path| self.path_receiver(path));
                Target {
                    callee,
                    receiver,
                    names_own_type,
                }
            }
            "generic_function" => function
                .child_by_field_name("function")
                .map_or_else(Target::none, |inner| self.call_target(inner)),
            "field_expression" => self.member_target(function),
            _ => Target::none(),
        }
    }

    fn member_target(&self, function: Node<'_>) -> Target {
        let Some(field) = function.child_by_field_name("field") else {
            return Target::none();
        };
        let value = function.child_by_field_name("value");
        let receiver = value.map(|value| match value.kind() {
            "identifier" => text(value, self.source),
            "self" => self.enclosing_type_name().unwrap_or_default(),
            _ => String::new(),
        });

        Target {
            callee: text(field, self.source),
            receiver,
            names_own_type: value.is_some_and(|value| value.kind() == "self"),
        }
    }

    // constraint: the path keywords `self`, `super` and `crate` name a module, so `self::f()` is a bare call of the
    // module's `f`
    fn path_receiver(&self, path: Node<'_>) -> (Option<String>, bool) {
        let base = if path.kind() == "generic_type" {
            type_base(path)
        } else {
            path
        };
        let terminal = if base.kind() == "scoped_identifier" {
            match base.child_by_field_name("name") {
                Some(name) => name,
                None => return (None, false),
            }
        } else {
            base
        };

        let name = text(terminal, self.source);
        if name == "Self" {
            return (self.enclosing_type_name(), true);
        }
        match terminal.kind() {
            "identifier" | "type_identifier" => (Some(name), false),
            _ => (None, false),
        }
    }

    fn enclosing_type_name(&self) -> Option<String> {
        self.types.last().map(|scope| scope.name.clone())
    }

    pub(super) fn macro_call(&mut self, node: Node<'_>, caller_id: SymbolId) {
        let name = node.child_by_field_name("macro");
        let call = CallRef {
            caller_id,
            callee: name.map_or_else(String::new, |name| last_segment(name, self.source)),
            receiver: None,
            line: line(node.start_position().row),
        };
        if self.push_call(call) && name.is_some_and(|name| name.kind() == "identifier") {
            self.record_block_call(node, CallForm::Macro);
        }
    }

    // constraint: a call with no callee, or one made at file scope before the first symbol, is dropped
    fn push_call(&mut self, call: CallRef) -> bool {
        if call.callee.is_empty() || self.symbol_count == 0 {
            return false;
        }
        self.raw.calls.push(call);
        true
    }
}

fn call_form(function: Node<'_>, source: &[u8]) -> Option<CallForm> {
    let named = if function.kind() == "generic_function" {
        function.child_by_field_name("function")?
    } else {
        function
    };
    match named.kind() {
        "identifier" => Some(CallForm::Plain),
        "scoped_identifier" => named
            .child_by_field_name("path")
            .and_then(|path| path_segments(path, source))
            .map(CallForm::Path),
        _ => None,
    }
}

fn receiver_path(function: Node<'_>, source: &[u8]) -> Option<String> {
    match function.kind() {
        "scoped_identifier" => path_text(function.child_by_field_name("path")?, source),
        "generic_function" => receiver_path(function.child_by_field_name("function")?, source),
        _ => None,
    }
}
