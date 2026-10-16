use kartta_lang::tree::{children, signature, text};
use tree_sitter::Node;

use super::name::type_base_name;

const DATA_SIGNATURE_MAX: usize = 160;

pub(super) fn has_storage_class(node: Node<'_>, class: &str, source: &[u8]) -> bool {
    has_child_spelled(node, "storage_class_specifier", class, source)
}

pub(super) fn has_qualifier(node: Node<'_>, qualifier: &str, source: &[u8]) -> bool {
    has_child_spelled(node, "type_qualifier", qualifier, source)
}

fn has_child_spelled(node: Node<'_>, kind: &str, spelling: &str, source: &[u8]) -> bool {
    children(node)
        .filter_map(|index| node.child(index))
        .any(|child| child.kind() == kind && &source[child.byte_range()] == spelling.as_bytes())
}

pub(super) fn alias_declarator_name(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "type_identifier" => return Some(node),
            "pointer_declarator" | "array_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            _ => return None,
        }
    }
}

pub(super) fn field_owner(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut member = node;
    loop {
        let class = member.parent()?.parent()?;
        if !matches!(
            class.kind(),
            "class_specifier" | "struct_specifier" | "union_specifier"
        ) {
            return None;
        }
        if let Some(name) = class.child_by_field_name("name") {
            return Some(type_base_name(name, source));
        }

        let holder = class.parent()?;
        match holder.kind() {
            "field_declaration" if holder.child_by_field_name("declarator").is_none() => {
                member = holder;
            }
            "type_definition" => {
                let mut cursor = holder.walk();
                let alias = holder
                    .children_by_field_name("declarator", &mut cursor)
                    .find_map(alias_declarator_name)?;
                return Some(text(alias, source));
            }
            _ => return None,
        }
    }
}

pub(super) fn field_declarator(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        match node.kind() {
            "field_identifier" => return Some(node),
            "pointer_declarator" | "array_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            "function_declarator" => node = parenthesized_declarator(node)?,
            "reference_declarator" | "parenthesized_declarator" => node = node.named_child(0)?,
            _ => return None,
        }
    }
}

pub(super) fn data_declarator<'t>(
    mut node: Node<'t>,
    source: &[u8],
) -> Option<(Node<'t>, Option<bool>)> {
    let mut const_pointer = None;
    loop {
        match node.kind() {
            "identifier" => return Some((node, const_pointer)),
            "pointer_declarator" => {
                const_pointer = Some(has_qualifier(node, "const", source));
                node = node.child_by_field_name("declarator")?;
            }
            "init_declarator" | "array_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            "function_declarator" => node = parenthesized_declarator(node)?,
            "reference_declarator" | "parenthesized_declarator" => node = node.named_child(0)?,
            _ => return None,
        }
    }
}

fn parenthesized_declarator(function: Node<'_>) -> Option<Node<'_>> {
    function
        .child_by_field_name("declarator")
        .filter(|declarator| declarator.kind() == "parenthesized_declarator")
}

pub(super) fn data_signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    if node.start_position().row == node.end_position().row
        && node.byte_range().len() <= DATA_SIGNATURE_MAX
    {
        return signature(node, source);
    }

    let mut cursor = node.walk();
    let Some(value) = node
        .children_by_field_name("declarator", &mut cursor)
        .find_map(|declarator| declarator.child_by_field_name("value"))
    else {
        return signature(node, source);
    };
    let text = String::from_utf8_lossy(&source[node.start_byte()..value.start_byte()]);
    let text = text.trim_end().trim_end_matches('=').trim_end();

    (!text.is_empty()).then(|| text.to_owned())
}

pub(super) fn at_namespace_scope(node: Node<'_>) -> bool {
    std::iter::successors(node.parent(), Node::parent).all(|ancestor| {
        matches!(
            ancestor.kind(),
            "translation_unit"
                | "namespace_definition"
                | "declaration_list"
                | "linkage_specification"
                | "template_declaration"
        )
    })
}
