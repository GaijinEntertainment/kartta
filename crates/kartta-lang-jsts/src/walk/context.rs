use tree_sitter::Node;

use super::uses::annotated_type;
use super::{Emitter, Span};
use crate::syntax::{named_child_nodes, type_name};

/// How many type names a context's lookup follows to the declaration they name, so that a cycle of aliases ends.
const NAMES_FOLLOWED: usize = 8;

impl Emitter<'_> {
    // constraint: a function written as a declaration's value, or as a property of an object literal whose type is
    // written beside it, takes its parameters' types from the function type that annotation spells, as the compiler
    // types them by their context: the annotation's own function type, the property's type in an object type, an
    // interface, an index signature or a mapped type, and a type alias or an interface the file declares at its top
    // level by its name
    pub(super) fn contextual_parameters<'t>(&self, function: Node<'t>) -> Option<Node<'t>> {
        let slot = self.slot_type(function)?;
        self.function_type(slot, NAMES_FOLLOWED)?
            .child_by_field_name("parameters")
    }

    /// The span of the one type the context gives a parameter, unless that type is a type parameter of a declaration
    /// around the context's own parameter.
    pub(super) fn contextual_type(&self, parameter: Node<'_>) -> Option<Span> {
        let span = annotated_type(parameter.child_by_field_name("type")?)?;
        let written = &self.source[span.0..span.1];
        let generic = std::iter::successors(parameter.parent(), Node::parent)
            .filter_map(|around| around.child_by_field_name("type_parameters"))
            .flat_map(named_child_nodes)
            .filter_map(|declared| declared.child_by_field_name("name"))
            .any(|name| &self.source[name.byte_range()] == written);
        (!generic).then_some(span)
    }

    fn slot_type<'t>(&self, function: Node<'t>) -> Option<Node<'t>> {
        let parent = function.parent()?;
        let is_value = parent.child_by_field_name("value") == Some(function);
        match parent.kind() {
            "variable_declarator" if is_value => written_type(parent.child_by_field_name("type")?),
            "pair" if is_value => {
                let object = parent.parent()?;
                self.member_type(
                    literal_annotation(object)?,
                    parent.child_by_field_name("key")?,
                )
            }
            "object" if function.kind() == "method_definition" => self.member_type(
                literal_annotation(parent)?,
                function.child_by_field_name("name")?,
            ),
            _ => None,
        }
    }

    fn member_type<'t>(&self, annotation: Node<'t>, key: Node<'_>) -> Option<Node<'t>> {
        let body = match annotation.kind() {
            "object_type" => annotation,
            _ => self.declared_type(annotation)?,
        };
        let key = type_name(key, self.source);
        named_child_nodes(body).find_map(|member| match member.kind() {
            "property_signature" if self.names(member, &key) => {
                written_type(member.child_by_field_name("type")?)
            }
            "method_signature" if self.names(member, &key) => Some(member),
            "index_signature" => written_type(member.child_by_field_name("type")?),
            _ => None,
        })
    }

    fn function_type<'t>(&self, slot: Node<'t>, names_left: usize) -> Option<Node<'t>> {
        match slot.kind() {
            "function_type" | "method_signature" => Some(slot),
            "parenthesized_type" => self.function_type(slot.named_child(0)?, names_left),
            _ if names_left > 0 => self.function_type(self.declared_type(slot)?, names_left - 1),
            _ => None,
        }
    }

    // constraint: a type name selects the type alias's value or the interface's body that the file declares at its
    // top level under that name, its type arguments aside
    fn declared_type<'t>(&self, written: Node<'t>) -> Option<Node<'t>> {
        let name = match written.kind() {
            "type_identifier" => written,
            "generic_type" => written.child_by_field_name("name")?,
            _ => return None,
        };
        let declared = self
            .type_declarations
            .get(&self.source[name.byte_range()])?;
        let program = std::iter::successors(Some(written), Node::parent).last()?;
        let declaration = program.descendant_for_byte_range(declared.start, declared.end)?;
        match declaration.kind() {
            "type_alias_declaration" => declaration.child_by_field_name("value"),
            "interface_declaration" => declaration.child_by_field_name("body"),
            _ => None,
        }
    }

    fn names(&self, member: Node<'_>, key: &str) -> bool {
        member
            .child_by_field_name("name")
            .is_some_and(|name| type_name(name, self.source) == key)
    }
}

/// The parameters a parameter list declares, in order.
pub(super) fn parameters_of(parameters: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named_child_nodes(parameters).filter(|parameter| parameter.kind() != "comment")
}

/// The name a parameter binds where it is one identifier and no annotation spells its type.
pub(super) fn untyped_name(parameter: Node<'_>) -> Option<Node<'_>> {
    match parameter.kind() {
        "identifier" => Some(parameter),
        "required_parameter" | "optional_parameter"
            if parameter.child_by_field_name("type").is_none() =>
        {
            parameter
                .child_by_field_name("pattern")
                .filter(|pattern| pattern.kind() == "identifier")
        }
        _ => None,
    }
}

/// The type an object literal's annotation, `as` or `satisfies` writes beside it.
pub(super) fn literal_annotation(object: Node<'_>) -> Option<Node<'_>> {
    let parent = object.parent()?;
    match parent.kind() {
        "variable_declarator" | "public_field_definition" | "field_definition"
            if parent.child_by_field_name("value") == Some(object) =>
        {
            written_type(parent.child_by_field_name("type")?)
        }
        "as_expression" | "satisfies_expression" => parent.named_child(1),
        _ => None,
    }
}

fn written_type(annotation: Node<'_>) -> Option<Node<'_>> {
    if annotation.kind() == "type_annotation" {
        annotation.named_child(0)
    } else {
        Some(annotation)
    }
}
