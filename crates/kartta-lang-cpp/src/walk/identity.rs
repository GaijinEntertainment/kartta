use std::borrow::Cow;

use kartta_lang::tree::text;
use tree_sitter::Node;

use super::declarator::{
    WrittenName, declarator_name, qualified_terminal, real_function_declarator, written_name,
};
use super::name::{is_simple_identifier, specializes};
use super::{Replayer, TemplateScope, child_nodes};

impl Replayer<'_, '_> {
    pub(super) fn callable_key(&self, node: Node<'_>) -> Option<String> {
        if !self.functions.is_empty() || has_unkeyed_ancestor(node) {
            return None;
        }
        let declarator = real_function_declarator(node, self.source)?;
        let written = declarator_name(declarator, self.source)?;
        let declared = written_name(written, self.source);
        let error_beside_the_name = child_nodes(declarator)
            .any(|child| child.start_byte() >= written.end_byte() && child.has_error());
        if error_beside_the_name || declared.name.has_error() {
            return None;
        }
        let own_head = node
            .parent()
            .is_some_and(|parent| parent.kind() == "template_declaration");
        let qualified = self.qualified_callable_name(declared, own_head)?;
        let parameters = declarator.child_by_field_name("parameters")?;
        let types = parameter_types(parameters, self.source, &self.templates)?;
        let qualifiers = member_qualifiers(declarator, written, parameters, self.source)?;

        Some(format!("{qualified}({}){qualifiers}", types.join(",")))
    }

    // constraint: a callable under a head of its own is one callable with its namesakes only as a class template's
    // member whose scope names the head's parameters, in order: a function template or a member template overloads on
    // its head, which the key does not hold, and any other argument names a specialization
    fn qualified_callable_name(&self, declared: WrittenName<'_>, own_head: bool) -> Option<String> {
        let WrittenName {
            name,
            stranded_scope,
        } = declared;
        let qualified = name.kind() == "qualified_identifier";
        let terminal = if qualified {
            qualified_terminal(name).0?
        } else {
            name
        };
        let qualified_by_scope = qualified || stranded_scope.is_some();
        let start = stranded_scope.unwrap_or(name).start_byte();
        let written = String::from_utf8_lossy(&self.source[start..name.end_byte()]);
        let rooted = written.starts_with("::");
        let unrooted = written.trim_start_matches("::");
        let (prefix, last) = unrooted.rsplit_once("::").unwrap_or(("", unrooted));
        let (prefix, arguments) = scope_without_arguments(prefix)?;
        let matches_its_head = if own_head {
            !arguments.is_empty()
                && arguments
                    .iter()
                    .copied()
                    .eq(parameter_names(&self.templates))
        } else {
            arguments.is_empty()
        };

        if !matches_its_head
            || (!prefix.is_empty() && !prefix.split("::").all(is_simple_identifier))
            || (!is_simple_identifier(last)
                && !matches!(terminal.kind(), "destructor_name" | "operator_name"))
            || (!rooted && self.names_an_enclosing_namespace(&prefix))
        {
            return None;
        }
        let name = if prefix.is_empty() {
            Cow::Borrowed(last)
        } else {
            Cow::Owned(format!("{prefix}::{last}"))
        };

        let mut scope: Vec<&str> = self
            .namespaces
            .iter()
            .map(|namespace| namespace.name.as_deref().unwrap_or("(anonymous)"))
            .collect();
        if !qualified_by_scope {
            scope.extend(
                self.classes
                    .iter()
                    .map(|class| self.raw.symbols[class.id.0 as usize].name.as_str()),
            );
        }

        Some(if rooted || scope.is_empty() {
            name.into_owned()
        } else {
            format!("{}::{name}", scope.join("::"))
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
        "lambda_expression" => true,
        "class_specifier" | "struct_specifier" | "union_specifier" => {
            ancestor.child_by_field_name("name").is_none() || specializes(ancestor)
        }
        _ => false,
    })
}

fn parameter_names(templates: &[TemplateScope]) -> impl Iterator<Item = &str> {
    templates
        .iter()
        .flat_map(|scope| scope.parameters.iter().map(String::as_str))
}

fn parameter_types(
    parameters: Node<'_>,
    source: &[u8],
    templates: &[TemplateScope],
) -> Option<Vec<String>> {
    let mut types = Vec::new();
    for parameter in child_nodes(parameters) {
        if parameter.kind() == "..." {
            types.push("...".to_owned());
        } else if parameter.is_named() && parameter.kind() != "comment" {
            types.push(parameter_identity(parameter, source, templates)?);
        }
    }
    if types == ["void"] {
        types.clear();
    }
    Some(types)
}

/// `scope` with the template arguments of its segments left out, and those arguments in order, a pack's without its
/// `...`: `A<T, Ts...>::B` is `A::B`, `T` and `Ts`. `None` where its angle brackets do not balance.
fn scope_without_arguments(scope: &str) -> Option<(Cow<'_, str>, Vec<&str>)> {
    if !scope.contains('<') {
        return Some((Cow::Borrowed(scope), Vec::new()));
    }
    let mut plain = String::with_capacity(scope.len());
    let mut arguments = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0;
    for (at, byte) in scope.bytes().enumerate() {
        match byte {
            b'<' => {
                depth += 1;
                if depth == 1 {
                    start = at + 1;
                }
            }
            b'>' | b',' if depth == 1 => {
                arguments.push(scope[start..at].trim().trim_end_matches("...").trim_end());
                start = at + 1;
                depth -= usize::from(byte == b'>');
            }
            b'>' => depth = depth.checked_sub(1)?,
            _ if depth == 0 => plain.push(char::from(byte)),
            _ => {}
        }
    }

    (depth == 0).then_some((Cow::Owned(plain), arguments))
}

fn member_qualifiers(
    declarator: Node<'_>,
    name: Node<'_>,
    parameters: Node<'_>,
    source: &[u8],
) -> Option<String> {
    let mut qualifiers = Vec::new();
    let mut reference = None;
    for child in child_nodes(declarator) {
        if child.end_byte() <= name.end_byte() || child == parameters || !child.is_named() {
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

fn parameter_identity(
    parameter: Node<'_>,
    source: &[u8],
    templates: &[TemplateScope],
) -> Option<String> {
    if !matches!(
        parameter.kind(),
        "parameter_declaration"
            | "optional_parameter_declaration"
            | "variadic_parameter_declaration"
    ) {
        return None;
    }
    let (binding, top_level_type) = binding_and_top_level_type(parameter)?;
    let identity = ParameterIdentity {
        source,
        templates,
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
            "variadic_declarator" => binding = node.named_child(0),
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
    child_nodes(node).find(|child| {
        matches!(
            child.kind(),
            "identifier" | "pointer_declarator" | "reference_declarator"
        )
    })
}

struct ParameterIdentity<'t, 's> {
    source: &'s [u8],
    templates: &'s [TemplateScope],
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
        let mut qualifiers: Vec<Node<'t>> = child_nodes(node)
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
                child_nodes(node)
                    .rev()
                    .filter(|child| child.kind() != "type_qualifier"),
            );
        } else {
            let mut sorted = qualifiers.into_iter().rev();
            pending.extend(child_nodes(node).rev().map(|child| match child.kind() {
                "type_qualifier" => sorted.next().unwrap_or(child),
                _ => child,
            }));
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
        let spelled = String::from_utf8_lossy(&self.source[node.byte_range()]);
        parameter_names(self.templates)
            .position(|parameter| parameter == spelled)
            .map_or(spelled, |position| Cow::Owned(format!("${position}")))
    }
}
