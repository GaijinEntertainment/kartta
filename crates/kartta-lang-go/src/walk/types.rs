use kartta_lang::tree::{named_children, text};
use tree_sitter::Node;

use super::Emitter;
use super::locals::LocalKind;
use super::syntax::first_operand;

// constraint: a path grows with the expression it names, and a long chain is rare, so a path past this length is
// left unnamed rather than carried into every call site and value type that repeats it
const MAX_PATH: usize = 160;

const CONTAINER: &str = "[]";

impl Emitter<'_> {
    pub(super) fn type_path(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "type_identifier" => {
                let name = self.borrowed(node);
                let is_parameter = self.type_params.iter().any(|(_, param)| param == name);
                (!is_parameter).then(|| name.to_owned())
            }
            "qualified_identifier" => {
                let package = node.child_by_field_name("package")?;
                let name = node.child_by_field_name("name")?;
                Some(format!(
                    "{}.{}",
                    self.borrowed(package),
                    self.borrowed(name)
                ))
            }
            "pointer_type" | "parenthesized_type" => self.type_path(first_operand(node)?),
            "generic_type" => self.type_path(node.child_by_field_name("type")?),
            _ if is_container_type(node) => {
                let element = self.type_path(node.child_by_field_name("element")?)?;
                bounded(format!("{CONTAINER}{element}"))
            }
            _ => None,
        }
    }

    pub(super) fn value_path(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "identifier" => self.identifier_path(node),
            "selector_expression" => {
                let operand = self.value_path(node.child_by_field_name("operand")?)?;
                let field = node.child_by_field_name("field")?;
                bounded(format!("{operand}.{}", self.borrowed(field)))
            }
            "call_expression" => self.result_path(node),
            "index_expression" => {
                element_of(&self.value_path(node.child_by_field_name("operand")?)?)
            }
            "parenthesized_expression" => self.value_path(first_operand(node)?),
            "type_assertion_expression" | "composite_literal" => {
                self.type_path(node.child_by_field_name("type")?)
            }
            "unary_expression" => {
                let operand = self.value_path(node.child_by_field_name("operand")?)?;
                match node.child_by_field_name("operator")?.kind() {
                    "&" | "*" => Some(operand),
                    "<-" => element_of(&operand),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn value_path_at(
        &self,
        values: &[Node<'_>],
        names: usize,
        position: usize,
    ) -> Option<String> {
        if values.len() == names {
            return self.value_path(*values.get(position)?);
        }
        let [single] = values else {
            return None;
        };
        let path = self.value_path(*single)?;
        match position {
            0 => Some(path),
            _ if single.kind() == "call_expression" && path.ends_with("()") => {
                bounded(format!("{path}#{position}"))
            }
            _ => None,
        }
    }

    pub(super) fn result_paths(&self, function: Node<'_>) -> Vec<String> {
        let Some(result) = function.child_by_field_name("result") else {
            return Vec::new();
        };
        if result.kind() != "parameters" {
            return self.type_path(result).into_iter().collect();
        }
        let mut paths = Vec::new();
        for declaration in named_children(result).filter_map(|i| result.named_child(i)) {
            let path = declaration
                .child_by_field_name("type")
                .and_then(|written| self.type_path(written))
                .unwrap_or_default();
            let mut cursor = declaration.walk();
            let names = declaration
                .children_by_field_name("name", &mut cursor)
                .count();
            paths.extend(std::iter::repeat_n(path, names.max(1)));
        }
        if paths.iter().all(String::is_empty) {
            paths.clear();
        }
        paths
    }

    fn identifier_path(&self, node: Node<'_>) -> Option<String> {
        let name = self.borrowed(node);
        let Some(local) = self.innermost_local(name, node.start_byte()) else {
            return Some(name.to_owned());
        };
        match local.kind {
            LocalKind::ReceiverOf(method) => self.raw.symbols[method.0 as usize].owner.clone(),
            LocalKind::Value | LocalKind::ValueSymbol(_) => local.type_path.clone(),
            LocalKind::TypeSymbol(_) => None,
        }
    }

    // constraint: `new(T)` and `make(T, n)` yield a value of the type they name, which only their first argument spells
    fn result_path(&self, call: Node<'_>) -> Option<String> {
        let mut function = call.child_by_field_name("function")?;
        if function.kind() == "index_expression" {
            function = function.child_by_field_name("operand")?;
        }
        if function.kind() == "identifier"
            && matches!(self.borrowed(function), "new" | "make")
            && !self.is_local(self.borrowed(function), function.start_byte())
        {
            let argument = first_operand(call.child_by_field_name("arguments")?)?;
            return self.type_path(argument).or_else(|| match argument.kind() {
                "identifier" => Some(text(argument, self.source)),
                "selector_expression" => self.value_path(argument),
                _ => None,
            });
        }
        if !matches!(function.kind(), "identifier" | "selector_expression") {
            return None;
        }
        bounded(format!("{}()", self.value_path(function)?))
    }
}

pub(super) fn is_container_type(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "slice_type" | "array_type" | "implicit_length_array_type" | "map_type" | "channel_type"
    )
}

pub(super) fn is_container_path(path: &str) -> bool {
    path.starts_with(CONTAINER)
}

pub(super) fn element_of(path: &str) -> Option<String> {
    path.strip_prefix(CONTAINER).map_or_else(
        || bounded(format!("{path}{CONTAINER}")),
        |element| Some(element.to_owned()),
    )
}

fn bounded(path: String) -> Option<String> {
    (path.len() <= MAX_PATH).then_some(path)
}
