use kartta_lang::SymbolId;
use kartta_lang::tree::named_child_nodes;
use tree_sitter::Node;

use super::Emitter;
use super::locals::{Local, LocalKind};
use super::syntax::first_operand;

// constraint: a path grows with the expression it names, and a long chain is rare, so a path past this length is
// left unnamed rather than carried into every call site and value type that repeats it
const MAX_PATH: usize = 160;

pub(super) const MAX_STEPS: usize = 2 * MAX_PATH;

const CONTAINER: &str = "[]";

const UNDERLYING: &str = "~";

const RESULTS: &str = "()";

impl Emitter<'_> {
    pub(super) fn underlying_path(&self, node: Node<'_>) -> Option<String> {
        let mut node = node;
        while node.kind() == "parenthesized_type" {
            node = first_operand(node)?;
        }
        match node.kind() {
            "type_identifier" | "qualified_identifier" | "generic_type" => {
                Some(format!("{UNDERLYING}{}", self.type_path(node)?))
            }
            _ => None,
        }
    }

    pub(super) fn type_path(&self, node: Node<'_>) -> Option<String> {
        let mut node = node;
        let mut containers = 0;
        loop {
            match node.kind() {
                "pointer_type" | "parenthesized_type" => node = first_operand(node)?,
                "generic_type" => node = node.child_by_field_name("type")?,
                _ if is_container_type(node) => {
                    containers += 1;
                    node = node.child_by_field_name("element")?;
                }
                _ => break,
            }
        }
        let named = self.named_type_path(node)?;
        if containers == 0 {
            return Some(named);
        }
        (containers * CONTAINER.len() + named.len() <= MAX_PATH)
            .then(|| format!("{}{named}", CONTAINER.repeat(containers)))
    }

    fn named_type_path(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "type_identifier" => {
                let name = self.borrowed(node);
                if let Some(Local {
                    kind: LocalKind::TypeSymbol(symbol),
                    ..
                }) = self.innermost_local(name, node.start_byte())
                {
                    return Some(symbol_path(*symbol));
                }
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
            "struct_type" => self.anonymous_name(node),
            _ => None,
        }
    }

    // constraint: a step lengthens a path but for an element, which can take off at most `MAX_PATH / 2` containers, so
    // more steps than `MAX_STEPS` always leave the path past `MAX_PATH`, whatever the operand under them reads as
    pub(super) fn value_path(&self, node: Node<'_>) -> Option<String> {
        let mut outer_steps = Vec::new();
        let mut node = node;
        let start = loop {
            match self.path_step(node)? {
                PathStep::Start(path) => break path,
                PathStep::Inside(inner) => node = inner,
                PathStep::Through(_, _) if outer_steps.len() == MAX_STEPS => return None,
                PathStep::Through(step, inner) => {
                    outer_steps.push(step);
                    node = inner;
                }
            }
        };
        outer_steps
            .iter()
            .rev()
            .try_fold(start, |path, step| self.stepped(path, *step))
    }

    fn path_step<'t>(&self, node: Node<'t>) -> Option<PathStep<'t>> {
        let operand = || node.child_by_field_name("operand");
        Some(match node.kind() {
            "identifier" => PathStep::Start(self.identifier_path(node)?),
            "selector_expression" => {
                let field = node.child_by_field_name("field")?;
                PathStep::Through(Step::Field(field), operand()?)
            }
            "call_expression" => return self.call_step(node),
            "index_expression" => PathStep::Through(Step::Element, operand()?),
            "parenthesized_expression" => PathStep::Inside(first_operand(node)?),
            "type_assertion_expression" | "composite_literal" => {
                PathStep::Start(self.type_path(node.child_by_field_name("type")?)?)
            }
            "unary_expression" => match node.child_by_field_name("operator")?.kind() {
                "&" | "*" => PathStep::Inside(operand()?),
                "<-" => PathStep::Through(Step::Element, operand()?),
                _ => return None,
            },
            _ => return None,
        })
    }

    // constraint: `new(T)` and `make(T, n)` yield a value of the type they name, which only their first argument spells
    fn call_step<'t>(&self, call: Node<'t>) -> Option<PathStep<'t>> {
        let mut function = call.child_by_field_name("function")?;
        if function.kind() == "index_expression" {
            function = function.child_by_field_name("operand")?;
        }
        if self.is_allocator(function) {
            let argument = first_operand(call.child_by_field_name("arguments")?)?;
            if let Some(path) = self.type_path(argument) {
                return Some(PathStep::Start(path));
            }
            return matches!(argument.kind(), "identifier" | "selector_expression")
                .then_some(PathStep::Inside(argument));
        }
        matches!(function.kind(), "identifier" | "selector_expression")
            .then_some(PathStep::Through(Step::Result(function), function))
    }

    fn stepped(&self, path: String, step: Step<'_>) -> Option<String> {
        match step {
            Step::Field(field) => bounded(format!("{path}.{}", self.borrowed(field))),
            Step::Element => element_of(&path),
            Step::Result(function) => {
                let local = self.innermost_local(self.borrowed(function), function.start_byte());
                match local.map(|local| local.kind) {
                    Some(LocalKind::TypeSymbol(_)) => Some(path),
                    Some(_) => bounded(format!("{path}{UNDERLYING}()")),
                    None => bounded(format!("{path}()")),
                }
            }
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
        for declaration in named_child_nodes(result) {
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

    pub(super) fn function_type_paths(&self, function: Node<'_>) -> Vec<String> {
        let mut paths = self.result_paths(function);
        if !paths.is_empty() {
            paths.insert(0, RESULTS.to_owned());
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
            LocalKind::TypeSymbol(symbol) => Some(symbol_path(symbol)),
        }
    }

    // constraint: a channel's path reads as a slice's, because the element a receive yields is what the graph reads, so
    // only the value's declaration or its expression tells the two apart
    pub(super) fn value_holds_channel(&self, value: Node<'_>) -> bool {
        let mut value = value;
        loop {
            let operand = match value.kind() {
                "parenthesized_expression" => first_operand(value),
                "index_expression" => value.child_by_field_name("operand"),
                "unary_expression"
                    if value
                        .child_by_field_name("operator")
                        .is_some_and(|operator| matches!(operator.kind(), "&" | "*")) =>
                {
                    value.child_by_field_name("operand")
                }
                _ => return self.names_channel(value),
            };
            let Some(operand) = operand else {
                return false;
            };
            value = operand;
        }
    }

    fn names_channel(&self, value: Node<'_>) -> bool {
        match value.kind() {
            "identifier" => self
                .innermost_local(self.borrowed(value), value.start_byte())
                .is_some_and(|local| local.holds_channel),
            "type_assertion_expression" | "composite_literal" => {
                value.child_by_field_name("type").is_some_and(holds_channel)
            }
            "call_expression" => {
                value
                    .child_by_field_name("function")
                    .is_some_and(|function| self.is_allocator(function))
                    && value
                        .child_by_field_name("arguments")
                        .and_then(first_operand)
                        .is_some_and(holds_channel)
            }
            _ => false,
        }
    }

    fn is_allocator(&self, function: Node<'_>) -> bool {
        function.kind() == "identifier"
            && matches!(self.borrowed(function), "new" | "make")
            && !self.is_local(self.borrowed(function), function.start_byte())
    }
}

/// What [`Emitter::value_path`] finds at one node of a value: the path it starts from, the operand that stands for it,
/// or a step to take from the path of an operand.
enum PathStep<'t> {
    Start(String),
    Inside(Node<'t>),
    Through(Step<'t>, Node<'t>),
}

#[derive(Clone, Copy)]
enum Step<'t> {
    Field(Node<'t>),
    Element,
    Result(Node<'t>),
}

// constraint: the graph reads a path's `{N}` as the symbol at index N of the path's file, whatever the scopes around it
// declare (ADR 3.170), and no Go identifier or path step holds a brace
pub(super) fn symbol_path(symbol: SymbolId) -> String {
    format!("{{{}}}", symbol.0)
}

pub(super) fn is_container_type(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "slice_type" | "array_type" | "implicit_length_array_type" | "map_type" | "channel_type"
    )
}

pub(super) fn holds_channel(written: Node<'_>) -> bool {
    let mut written = written;
    loop {
        let inner = match written.kind() {
            "channel_type" => return true,
            "pointer_type" | "parenthesized_type" => first_operand(written),
            _ if is_container_type(written) => written.child_by_field_name("element"),
            _ => return false,
        };
        let Some(inner) = inner else {
            return false;
        };
        written = inner;
    }
}

pub(super) fn is_container_path(path: &str) -> bool {
    path.starts_with(CONTAINER)
}

pub(super) fn is_one_container(path: &str) -> bool {
    path.strip_prefix(CONTAINER)
        .is_some_and(|element| !is_container_path(element))
}

fn inside_containers(path: &str) -> &str {
    path.trim_start_matches(CONTAINER)
}

pub(super) fn named_type(path: &str) -> &str {
    let name = inside_containers(path);
    name.strip_prefix(UNDERLYING).unwrap_or(name)
}

pub(super) fn element_of(path: &str) -> Option<String> {
    path.strip_prefix(CONTAINER).map_or_else(
        || bounded(format!("{path}{CONTAINER}")),
        |element| Some(element.to_owned()),
    )
}

pub(super) fn bounded(path: String) -> Option<String> {
    (path.len() <= MAX_PATH).then_some(path)
}
