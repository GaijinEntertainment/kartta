use kartta_lang::{Access, FILE_SCOPE, Receiver, SymbolId, UseRef};
use tree_sitter::Node;

use super::{Binding, Emitter, IDENTIFIER, line};

/// A read or a write of a name, or of an attribute taken on a name, which the scope that binds the name decides once
/// it closes, as Python binds a name for the whole of that scope.
pub(super) struct NameUse<'a> {
    caller: SymbolId,
    name: &'a [u8],
    attribute: Option<&'a [u8]>,
    access: Access,
    line: u32,
    at: usize,
    attribute_object: bool,
}

impl NameUse<'_> {
    // constraint: a use that shares its bytes with a binding is that binding's target, which only an update reads
    fn is_target(&self, bindings: &[(&[u8], Binding)]) -> bool {
        self.access != Access::Update
            && bindings
                .iter()
                .any(|&(name, _)| std::ptr::eq(name, self.name))
    }
}

impl<'a> Emitter<'a> {
    // constraint: a called attribute is a call site already
    pub(super) fn attribute_use(&mut self, attribute: Node<'_>, caller: SymbolId) {
        if is_callee(attribute) {
            return;
        }
        let (Some(object), Some(name)) = (
            attribute.child_by_field_name("object"),
            attribute.child_by_field_name("attribute"),
        ) else {
            return;
        };
        let source = self.source;
        let attribute_name = &source[name.byte_range()];
        let used = NameUse {
            caller,
            name: &source[object.byte_range()],
            attribute: Some(attribute_name),
            access: self.access_of(attribute),
            line: line(name.start_position().row),
            at: object.start_byte(),
            attribute_object: false,
        };
        if self.own_receiver(object).is_some() {
            self.push_site(&used, attribute_name, Receiver::Own);
        } else if object.kind() == IDENTIFIER {
            self.defer(used);
        } else {
            self.push_site(&used, attribute_name, Receiver::Path(String::new()));
        }
    }

    // constraint: a name the module binds is not used where it is bound, as one a function binds is not, but an
    // update reads the value it binds again
    pub(super) fn name_use(&mut self, identifier: Node<'_>, caller: SymbolId) {
        if is_callee(identifier) || !reads(identifier) {
            return;
        }
        let access = self.access_of(identifier);
        if caller == FILE_SCOPE
            && access != Access::Update
            && self.module_targets.contains(&identifier.start_byte())
        {
            return;
        }
        self.defer(NameUse {
            caller,
            name: &self.source[identifier.byte_range()],
            attribute: None,
            access,
            line: line(identifier.start_position().row),
            at: identifier.start_byte(),
            attribute_object: is_attribute_object(identifier),
        });
    }

    fn access_of(&self, site: Node<'_>) -> Access {
        if site.start_byte() < self.type_names_end {
            Access::Type
        } else {
            access_of(site)
        }
    }

    fn defer(&mut self, used: NameUse<'a>) {
        match self
            .frames
            .iter_mut()
            .rev()
            .find(|frame| frame.holds(used.at))
        {
            Some(frame) => frame.uses.push(used),
            None => self.module_uses.push(used),
        }
    }

    // constraint: a scope that binds the name holds it, a value of no known type or a member of the class whose body
    // that scope is, unless a `global` declaration there gives it to the module; a scope that does not bind it leaves it
    // to the scope around, and a class body's names reach no function inside it
    pub(super) fn settle_uses(
        &mut self,
        bindings: &[(&[u8], Binding)],
        class: Option<SymbolId>,
        uses: Vec<NameUse<'a>>,
    ) {
        for used in uses {
            let mut bound = bindings
                .iter()
                .filter(|&&(name, _)| name == used.name)
                .peekable();
            if bound.peek().is_none() {
                match self.scope_frame(used.at) {
                    Some(outer) => outer.uses.push(used),
                    None => self.module_uses.push(used),
                }
            } else if bound.any(|&(_, binding)| matches!(binding, Binding::Module)) {
                self.module_uses.push(used);
            } else if let Some(attribute) = used.attribute {
                self.push_site(&used, attribute, Receiver::Path(String::new()));
            } else if let Some(class) =
                class.and_then(|class| self.raw.symbols.get(class.0 as usize))
                && !used.is_target(bindings)
            {
                let owner = class.qualified_name.clone();
                self.push_site(&used, used.name, Receiver::Path(owner));
            }
        }
    }

    // constraint: a body runs once the module has bound its names, so a name no scope around the use binds is the
    // module's wherever the module binds it: a value it assigns is a value of no known type, and a class, a function or
    // a module it imports owns the attributes taken on its name; an imported module read only to take an attribute of
    // it is no use of a symbol
    pub(super) fn record_module_uses(&mut self) {
        for used in std::mem::take(&mut self.module_uses) {
            let binding = self.module_names.get(used.name).copied();
            match (used.attribute, binding) {
                (Some(attribute), Some(Binding::Value)) => {
                    self.push_site(&used, attribute, Receiver::Path(String::new()));
                }
                (Some(attribute), _) => {
                    let owner = String::from_utf8_lossy(used.name).into_owned();
                    self.push_site(&used, attribute, Receiver::Path(owner));
                }
                (None, Some(Binding::Imported)) if used.attribute_object => {}
                (None, Some(_)) => self.push_site(&used, used.name, Receiver::Bare),
                (None, None) => {}
            }
        }
    }

    fn push_site(&mut self, used: &NameUse<'_>, name: &[u8], receiver: Receiver) {
        self.raw.uses.push(UseRef {
            site_of: used.caller,
            name: String::from_utf8_lossy(name).into_owned(),
            receiver,
            access: used.access,
            line: used.line,
        });
    }
}

fn is_callee(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind() == "call" && parent.child_by_field_name("function") == Some(node)
    })
}

fn is_attribute_object(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind() == "attribute" && parent.child_by_field_name("object") == Some(node)
    })
}

fn reads(identifier: Node<'_>) -> bool {
    let Some(parent) = identifier.parent() else {
        return false;
    };
    let in_field = |field| parent.child_by_field_name(field) == Some(identifier);
    match parent.kind() {
        "attribute" => in_field("object"),
        "keyword_argument" => in_field("value"),
        "function_definition" | "class_definition" => !in_field("name"),
        "global_statement"
        | "nonlocal_statement"
        | "dotted_name"
        | "aliased_import"
        | "import_from_statement"
        | "import_statement" => false,
        _ => true,
    }
}

// constraint: only the outermost expression on the left of an assignment is assigned, a target of a tuple included;
// `a.b` in `a.b.c = v` is read
fn access_of(site: Node<'_>) -> Access {
    let mut at = site;
    while let Some(parent) = at.parent() {
        let left = parent.child_by_field_name("left") == Some(at);
        return match parent.kind() {
            "pattern_list" | "tuple_pattern" | "list_pattern" | "parenthesized_expression" => {
                at = parent;
                continue;
            }
            "assignment" if left => Access::Write,
            "augmented_assignment" if left => Access::Update,
            _ => Access::Read,
        };
    }
    Access::Read
}
