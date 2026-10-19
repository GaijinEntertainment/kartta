use kartta_lang::{Access, FILE_SCOPE, Receiver, SymbolId, UseRef};
use tree_sitter::Node;

use super::{Binding, Emitter, Span};
use crate::syntax::start_line;

impl Emitter<'_> {
    // constraint: a member called or constructed is a call site already, and a JSX element's name is a call
    pub(super) fn member_use(&mut self, member: Node<'_>, caller: SymbolId) {
        if caller == FILE_SCOPE || is_callee(member) {
            return;
        }
        let (Some(object), Some(property)) = (
            member.child_by_field_name("object"),
            member.child_by_field_name("property"),
        ) else {
            return;
        };
        if !matches!(
            property.kind(),
            "property_identifier" | "private_property_identifier"
        ) {
            return;
        }
        let receiver = match object.kind() {
            "this" => self.this_receiver(),
            "identifier" => Receiver::Path(self.named_owner(object)),
            _ => Receiver::Path(String::new()),
        };
        self.push_use(caller, property, (receiver, access_of(member)));
    }

    // constraint: an identifier names a module's value where the file imports it or declares it outside every body,
    // and a body that binds the name again hides it; a declaration, an import or an export of the name is no use
    pub(super) fn value_use(&mut self, identifier: Node<'_>, caller: SymbolId) {
        if caller == FILE_SCOPE || is_callee(identifier) || declares(identifier) {
            return;
        }
        let binding = self.binding(&self.source[identifier.byte_range()]);
        if !matches!(binding, Some(Binding::Module(_) | Binding::Imported)) {
            return;
        }
        let access = access_of(identifier);
        self.push_use(caller, identifier, (Receiver::Bare, access));
    }

    // constraint: a key of an object literal writes a field only where the literal's type is written beside it, a
    // declaration's or a field's annotation, `as` or `satisfies`
    pub(super) fn literal_keys(&mut self, object: Node<'_>, caller: SymbolId) {
        if caller == FILE_SCOPE {
            return;
        }
        let Some(span) = literal_type(object) else {
            return;
        };
        let path = self.spelled(span);
        let mut cursor = object.walk();
        for entry in object.named_children(&mut cursor) {
            let key = match entry.kind() {
                "pair" => entry
                    .child_by_field_name("key")
                    .and_then(|key| match key.kind() {
                        "property_identifier" => Some(key),
                        "string" => key
                            .named_child(0)
                            .filter(|text| text.kind() == "string_fragment"),
                        _ => None,
                    }),
                "shorthand_property_identifier" => Some(entry),
                _ => None,
            };
            if let Some(key) = key {
                self.push_use(caller, key, (Receiver::Path(path.clone()), Access::Write));
            }
        }
    }

    fn this_receiver(&self) -> Receiver {
        let in_its_own_member =
            (self.scopes.last()).is_some_and(|scope| scope.this_of == Some(scope.id));
        match self.this_class() {
            Some(_) if in_its_own_member => Receiver::Own,
            Some(owner) => Receiver::Path(owner),
            None => Receiver::Path(String::new()),
        }
    }

    // constraint: a value the file declares names the type its declaration spells, or none; any other name, a class,
    // an import or a global, keeps its name, as a call's receiver does
    fn named_owner(&self, object: Node<'_>) -> String {
        let name = &self.source[object.byte_range()];
        match self.binding(name) {
            Some(Binding::Value(typed) | Binding::Module(typed)) => {
                typed.map_or_else(String::new, |span| self.spelled(span))
            }
            Some(Binding::Imported | Binding::Type) | None => {
                String::from_utf8_lossy(name).into_owned()
            }
        }
    }

    fn binding(&self, name: &[u8]) -> Option<&Binding> {
        self.bindings
            .get(name)
            .or_else(|| self.module_bindings.get(name))
    }

    fn spelled(&self, (start, end): Span) -> String {
        String::from_utf8_lossy(&self.source[start..end]).into_owned()
    }

    fn push_use(
        &mut self,
        caller: SymbolId,
        name: Node<'_>,
        (receiver, access): (Receiver, Access),
    ) {
        self.raw.uses.push(UseRef {
            site_of: caller,
            name: String::from_utf8_lossy(&self.source[name.byte_range()]).into_owned(),
            receiver,
            access,
            line: start_line(name),
        });
    }
}

pub(super) fn names_a_value(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "shorthand_property_identifier" | "shorthand_property_identifier_pattern"
    )
}

fn is_callee(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let field = match parent.kind() {
        "call_expression" => "function",
        "new_expression" => "constructor",
        "jsx_opening_element" | "jsx_self_closing_element" | "jsx_closing_element" => "name",
        _ => return false,
    };
    parent.child_by_field_name(field) == Some(node)
}

fn declares(identifier: Node<'_>) -> bool {
    let Some(parent) = identifier.parent() else {
        return false;
    };
    let in_field = |field| parent.child_by_field_name(field) == Some(identifier);
    match parent.kind() {
        "variable_declarator" => in_field("name"),
        "assignment_pattern" => in_field("left"),
        "required_parameter" | "optional_parameter" => in_field("pattern"),
        "import_specifier" | "import_clause" | "namespace_import" | "export_specifier" => true,
        _ => false,
    }
}

// constraint: only the outermost expression on the left of an assignment is assigned, a name a destructuring pattern
// there holds included; `a.b` in `a.b.c = v` is read
fn access_of(site: Node<'_>) -> Access {
    let mut at = site;
    while let Some(parent) = at.parent() {
        let left = |field| parent.child_by_field_name(field) == Some(at);
        return match parent.kind() {
            "parenthesized_expression"
            | "non_null_expression"
            | "array_pattern"
            | "object_pattern"
            | "rest_pattern"
            | "pair_pattern" => {
                at = parent;
                continue;
            }
            "assignment_expression" if left("left") => Access::Write,
            "augmented_assignment_expression" if left("left") => Access::Update,
            "update_expression" => Access::Update,
            _ => Access::Read,
        };
    }
    Access::Read
}

fn literal_type(object: Node<'_>) -> Option<Span> {
    let parent = object.parent()?;
    let annotation = match parent.kind() {
        "variable_declarator" | "public_field_definition" | "field_definition"
            if parent.child_by_field_name("value") == Some(object) =>
        {
            parent.child_by_field_name("type")?
        }
        "as_expression" | "satisfies_expression" => parent.named_child(1)?,
        _ => return None,
    };
    annotated_type(annotation)
}

pub(super) fn constructed_type(declarator: Node<'_>) -> Option<Span> {
    let constructor = declarator
        .child_by_field_name("value")
        .filter(|value| value.kind() == "new_expression")?
        .child_by_field_name("constructor")?;
    annotated_type(constructor)
}

// constraint: a type names an owner only where it is one name, qualified or not, its type arguments aside
pub(super) fn annotated_type(annotation: Node<'_>) -> Option<Span> {
    let written = if annotation.kind() == "type_annotation" {
        annotation.named_child(0)?
    } else {
        annotation
    };
    let name = match written.kind() {
        "type_identifier" | "nested_type_identifier" | "identifier" => written,
        "generic_type" => written.child_by_field_name("name")?,
        _ => return None,
    };
    Some((name.start_byte(), name.end_byte()))
}
