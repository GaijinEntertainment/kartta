use std::collections::HashMap;

use kartta_lang::tree::named_child_nodes;
use tree_sitter::Node;

use super::Emitter;
use super::declared::LocalType;

// constraint: a region hides the names bound in it: a function and a closure their parameters, a match arm its
// pattern, a `for` its pattern in its body, an `if let` or a `while let` its pattern in the operands after it and the
// branch it runs on a match, and a block its `let`s; a function's region ends the lookup, as an item sees no local of
// the function around it; each name holds the type path its parameter or its `let` names, and a later `let` of the
// name replaces it
pub(super) struct Region<'a> {
    pub(super) end: usize,
    function: bool,
    pub(super) names: HashMap<&'a [u8], Option<LocalType>>,
}

// constraint: a binding hides its names from the end of what it reads on, as `let helper = helper` and
// `for item in [item]` read the item outside: a `let` from its end into its block, a `for` from its iterator's end
// into its own region, and a `let` condition from its end into a region to the end of its `if`'s consequence or its
// `while`'s body, so not in an `else`; where the walk reaches no node between a binding's end and its region's, the
// region is gone by then and the binding hides nothing
pub(super) struct PendingBindings<'a> {
    at: usize,
    into: Into,
    names: Vec<&'a [u8]>,
    declared: Option<LocalType>,
}

pub(super) enum Into {
    Region(usize),
    Opened(usize),
}

impl<'a> Emitter<'a> {
    pub(super) fn track_bindings(&mut self, node: Node<'_>) {
        let start = node.start_byte();
        while self.regions.pop_if(|region| region.end <= start).is_some() {}
        while let Some(pending) = self.pending_bindings.pop_if(|pending| pending.at <= start) {
            self.activate(pending, start);
        }
        while self.branches.pop_if(|branch| branch.0 <= start).is_some() {}

        match node.kind() {
            "function_item" => {
                let parameters = node.child_by_field_name("parameters");
                let names = self.parameter_names(parameters);
                self.open_region(node, true, names);
                self.parameter_types(node);
            }
            "closure_expression" => {
                let parameters = node.child_by_field_name("parameters");
                let names = self.parameter_names(parameters);
                self.open_region(node, false, names);
            }
            "match_arm" => {
                let pattern = node.child_by_field_name("pattern");
                let condition =
                    pattern.and_then(|pattern| pattern.child_by_field_name("condition"));
                let patterns = pattern
                    .into_iter()
                    .flat_map(named_child_nodes)
                    .filter(|inner| Some(*inner) != condition);
                let names = self.bound_names(patterns);
                self.open_region(node, false, names);
            }
            "for_expression" => {
                self.open_region(node, false, Vec::new());
                let names = self.bound_names(node.child_by_field_name("pattern").into_iter());
                let at = node
                    .child_by_field_name("value")
                    .map_or_else(|| node.start_byte(), |value| value.end_byte());
                self.bind_later(at, Into::Region(self.regions.len() - 1), (names, None));
            }
            "if_expression" | "while_expression" => {
                let branch = node
                    .child_by_field_name("consequence")
                    .or_else(|| node.child_by_field_name("body"));
                if let Some(branch) = branch {
                    self.branches.push((node.end_byte(), branch.end_byte()));
                }
            }
            "let_condition" => {
                let names = self.bound_names(node.child_by_field_name("pattern").into_iter());
                if let Some(&(_, branch_end)) = self.branches.last() {
                    self.bind_later(node.end_byte(), Into::Opened(branch_end), (names, None));
                }
            }
            "block" => self.open_region(node, false, Vec::new()),
            "let_declaration" => {
                let names = self.bound_names(node.child_by_field_name("pattern").into_iter());
                if let Some(region) = self.regions.len().checked_sub(1) {
                    let declared = self.local_type(node);
                    self.bind_later(node.end_byte(), Into::Region(region), (names, declared));
                }
            }
            _ => {}
        }
    }

    fn activate(&mut self, pending: PendingBindings<'a>, start: usize) {
        let PendingBindings {
            into,
            names,
            declared,
            ..
        } = pending;
        let named = names.into_iter().map(|name| (name, declared.clone()));
        match into {
            Into::Region(region) => {
                if let Some(region) = self.regions.get_mut(region) {
                    region.names.extend(named);
                }
            }
            Into::Opened(end) if end > start => self.regions.push(Region {
                end,
                function: false,
                names: named.collect(),
            }),
            Into::Opened(_) => {}
        }
    }

    fn bind_later(
        &mut self,
        at: usize,
        into: Into,
        (names, declared): (Vec<&'a [u8]>, Option<LocalType>),
    ) {
        if !names.is_empty() {
            self.pending_bindings.push(PendingBindings {
                at,
                into,
                names,
                declared,
            });
        }
    }

    fn open_region(&mut self, node: Node<'_>, function: bool, names: Vec<&'a [u8]>) {
        self.regions.push(Region {
            end: node.end_byte(),
            function,
            names: names.into_iter().map(|name| (name, None)).collect(),
        });
    }

    // constraint: a parameter list holds a typed `parameter` around its pattern, or, for a closure, the pattern alone
    fn parameter_names(&self, parameters: Option<Node<'_>>) -> Vec<&'a [u8]> {
        let patterns = parameters
            .into_iter()
            .flat_map(named_child_nodes)
            .map(|parameter| {
                if parameter.kind() == "parameter" {
                    parameter.child_by_field_name("pattern")
                } else {
                    Some(parameter)
                }
            });
        self.bound_names(patterns.flatten())
    }

    fn bound_names<'t>(&self, patterns: impl Iterator<Item = Node<'t>>) -> Vec<&'a [u8]> {
        let source = self.source;
        let mut names = Vec::new();
        for pattern in patterns {
            pattern_bindings(pattern, &mut |name| names.push(&source[name.byte_range()]));
        }
        names
    }

    pub(super) fn is_bound(&self, spelled: &[u8]) -> bool {
        for region in self.regions.iter().rev() {
            if region.names.contains_key(spelled) {
                return true;
            }
            if region.function {
                break;
            }
        }
        false
    }

    // constraint: the innermost binding of the name decides, so a `let`, a closure's parameter or a pattern that binds
    // the name again hides the type an earlier binding names; the bool tells a `let`'s type from a parameter's
    pub(super) fn declared_type(&self, spelled: &[u8]) -> Option<(LocalType, bool)> {
        for region in self.regions.iter().rev() {
            if let Some(declared) = region.names.get(spelled) {
                return declared.clone().map(|written| (written, !region.function));
            }
            if region.function {
                break;
            }
        }
        None
    }
}

// constraint: a pattern binds its identifiers, a shorthand field of `S { helper }` among them, and never the type a
// struct or tuple-struct pattern names
pub(super) fn pattern_bindings<'t>(node: Node<'t>, bind: &mut impl FnMut(Node<'t>)) {
    match node.kind() {
        "identifier" | "shorthand_field_identifier" => bind(node),
        "field_pattern" => {
            if let Some(bound) = node
                .child_by_field_name("pattern")
                .or_else(|| node.child_by_field_name("name"))
            {
                pattern_bindings(bound, bind);
            }
        }
        "mut_pattern"
        | "ref_pattern"
        | "reference_pattern"
        | "or_pattern"
        | "tuple_pattern"
        | "captured_pattern"
        | "slice_pattern"
        | "struct_pattern"
        | "tuple_struct_pattern" => {
            let named_type = node.child_by_field_name("type");
            named_child_nodes(node)
                .filter(|inner| Some(*inner) != named_type)
                .for_each(|inner| pattern_bindings(inner, bind));
        }
        _ => {}
    }
}
