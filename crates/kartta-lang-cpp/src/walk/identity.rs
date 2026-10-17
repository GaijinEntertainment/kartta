use std::borrow::Cow;

use kartta_lang::tree::{children, text};
use tree_sitter::Node;

use super::Replayer;
use super::declarator::{joins_a_macro_as_scope, qualified_terminal, real_function_declarator};
use super::name::is_simple_identifier;

impl Replayer<'_> {
    pub(super) fn callable_key(&self, node: Node<'_>) -> Option<String> {
        if !self.functions.is_empty() || has_unkeyed_ancestor(node) {
            return None;
        }
        let declarator = real_function_declarator(node, self.source)?;
        let written = declarator.child_by_field_name("declarator")?;
        let name = if joins_a_macro_as_scope(written) {
            written.child_by_field_name("name")?
        } else {
            written
        };
        let error_beside_the_name = children(declarator)
            .filter_map(|index| declarator.child(index))
            .any(|child| child != written && child.has_error());
        if error_beside_the_name || name.has_error() {
            return None;
        }
        let qualified = self.qualified_callable_name(name)?;
        let parameters = declarator.child_by_field_name("parameters")?;
        let types = parameter_types(parameters, self.source)?;
        let qualifiers = member_qualifiers(declarator, written, parameters, self.source)?;

        Some(format!("{qualified}({}){qualifiers}", types.join(",")))
    }

    fn qualified_callable_name(&self, name: Node<'_>) -> Option<String> {
        let qualified_by_scope = name.kind() == "qualified_identifier";
        let terminal = if qualified_by_scope {
            qualified_terminal(name).0?
        } else {
            name
        };
        let written = text(name, self.source);
        let rooted = written.starts_with("::");
        let unrooted = written.trim_start_matches("::");
        let (prefix, last) = unrooted.rsplit_once("::").unwrap_or(("", unrooted));

        if (!prefix.is_empty() && !prefix.split("::").all(is_simple_identifier))
            || (!is_simple_identifier(last)
                && !matches!(terminal.kind(), "destructor_name" | "operator_name"))
            || (!rooted && self.names_an_enclosing_namespace(prefix))
        {
            return None;
        }

        let mut scope: Vec<&str> = self
            .namespaces
            .iter()
            .map(|namespace| namespace.name.as_deref().unwrap_or("(anonymous)"))
            .collect();
        if !qualified_by_scope {
            scope.extend(self.classes.iter().map(|class| class.name.as_str()));
        }

        Some(if rooted || scope.is_empty() {
            unrooted.to_owned()
        } else {
            format!("{}::{written}", scope.join("::"))
        })
    }

    fn names_an_enclosing_namespace(&self, prefix: &str) -> bool {
        let first = prefix.split("::").next();
        self.namespaces
            .iter()
            .filter_map(|namespace| namespace.name.as_deref())
            .flat_map(|name| name.split("::"))
            .any(|segment| first == Some(segment))
    }
}

fn has_unkeyed_ancestor(node: Node<'_>) -> bool {
    std::iter::successors(node.parent(), Node::parent).any(|ancestor| match ancestor.kind() {
        "template_declaration" | "lambda_expression" => true,
        "class_specifier" | "struct_specifier" | "union_specifier" => {
            ancestor.child_by_field_name("name").is_none()
        }
        _ => false,
    })
}

fn parameter_types(parameters: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let mut types = Vec::new();
    for parameter in children(parameters).filter_map(|index| parameters.child(index)) {
        if parameter.kind() == "..." {
            types.push("...".to_owned());
        } else if parameter.is_named() && parameter.kind() != "comment" {
            types.push(parameter_identity(parameter, source)?);
        }
    }
    if types == ["void"] {
        types.clear();
    }
    Some(types)
}

fn member_qualifiers(
    declarator: Node<'_>,
    name: Node<'_>,
    parameters: Node<'_>,
    source: &[u8],
) -> Option<String> {
    let mut qualifiers = Vec::new();
    let mut reference = None;
    for child in children(declarator).filter_map(|index| declarator.child(index)) {
        if child == name || child == parameters || !child.is_named() {
            continue;
        }
        match child.kind() {
            "type_qualifier" => qualifiers.push(text(child, source)),
            "ref_qualifier" => reference = Some(text(child, source)),
            "noexcept" | "virtual_specifier" | "comment" => {}
            _ => return None,
        }
    }
    qualifiers.sort_unstable();
    qualifiers.extend(reference);

    Some(qualifiers.join(" "))
}

fn parameter_identity(parameter: Node<'_>, source: &[u8]) -> Option<String> {
    if !matches!(
        parameter.kind(),
        "parameter_declaration" | "optional_parameter_declaration"
    ) {
        return None;
    }
    let (binding, top_level_type) = binding_and_top_level_type(parameter)?;
    let identity = ParameterIdentity {
        source,
        top_level_type,
        binding,
        default: parameter.child_by_field_name("default_value"),
    };

    Some(identity.tokens(parameter).join(" "))
}

fn binding_and_top_level_type(parameter: Node<'_>) -> Option<(Option<Node<'_>>, Node<'_>)> {
    let mut binding = parameter.child_by_field_name("declarator");
    let mut top_level_type = parameter;
    while let Some(node) = binding {
        match node.kind() {
            "identifier" => break,
            "pointer_declarator"
            | "reference_declarator"
            | "abstract_pointer_declarator"
            | "abstract_reference_declarator" => {
                top_level_type = node;
                binding = node
                    .child_by_field_name("declarator")
                    .or_else(|| inlined_declarator(node));
            }
            _ => return None,
        }
    }
    Some((binding, top_level_type))
}

fn inlined_declarator(node: Node<'_>) -> Option<Node<'_>> {
    children(node)
        .filter_map(|index| node.child(index))
        .find(|child| {
            matches!(
                child.kind(),
                "identifier" | "pointer_declarator" | "reference_declarator"
            )
        })
}

struct ParameterIdentity<'t, 's> {
    source: &'s [u8],
    top_level_type: Node<'t>,
    binding: Option<Node<'t>>,
    default: Option<Node<'t>>,
}

impl<'t, 's> ParameterIdentity<'t, 's> {
    fn tokens(&self, parameter: Node<'t>) -> Vec<Cow<'s, str>> {
        let mut tokens = Vec::new();
        let mut pending = vec![parameter];

        while let Some(node) = pending.pop() {
            if self.is_dropped(node) {
                continue;
            }
            if node.child_count() == 0 {
                tokens.push(self.spelling(node));
            } else {
                self.expand(node, &mut pending, &mut tokens);
            }
        }

        tokens
    }

    fn expand(&self, node: Node<'t>, pending: &mut Vec<Node<'t>>, tokens: &mut Vec<Cow<'s, str>>) {
        let mut qualifiers: Vec<Node<'t>> = children(node)
            .filter_map(|index| node.child(index))
            .filter(|child| child.kind() == "type_qualifier")
            .collect();
        qualifiers.sort_unstable_by_key(|qualifier| &self.source[qualifier.byte_range()]);

        if matches!(
            node.kind(),
            "parameter_declaration" | "optional_parameter_declaration" | "type_descriptor"
        ) {
            tokens.extend(
                qualifiers
                    .iter()
                    .filter(|qualifier| !self.is_top_level_cv(**qualifier))
                    .map(|qualifier| self.spelling(*qualifier)),
            );
            pending.extend(
                children(node)
                    .rev()
                    .filter_map(|index| node.child(index))
                    .filter(|child| child.kind() != "type_qualifier"),
            );
        } else {
            let mut sorted = qualifiers.into_iter().rev();
            pending.extend(
                children(node)
                    .rev()
                    .filter_map(|index| node.child(index))
                    .map(|child| match child.kind() {
                        "type_qualifier" => sorted.next().unwrap_or(child),
                        _ => child,
                    }),
            );
        }
    }

    fn is_dropped(&self, node: Node<'t>) -> bool {
        Some(node) == self.binding
            || Some(node) == self.default
            || matches!(node.kind(), "comment" | "=")
            || self.is_top_level_cv(node)
    }

    fn is_top_level_cv(&self, node: Node<'t>) -> bool {
        !matches!(
            self.top_level_type.kind(),
            "reference_declarator" | "abstract_reference_declarator"
        ) && node.kind() == "type_qualifier"
            && node.parent() == Some(self.top_level_type)
            && matches!(&self.source[node.byte_range()], b"const" | b"volatile")
    }

    fn spelling(&self, node: Node<'t>) -> Cow<'s, str> {
        String::from_utf8_lossy(&self.source[node.byte_range()])
    }
}
