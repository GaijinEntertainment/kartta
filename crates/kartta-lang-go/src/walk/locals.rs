use kartta_lang::SymbolId;
use kartta_lang::tree::{named_children, text};
use tree_sitter::Node;

use super::Emitter;
use super::syntax::preorder_within;

pub(super) struct Local {
    pub(super) from: usize,
    pub(super) block: Block,
    pub(super) name: String,
    pub(super) kind: LocalKind,
    pub(super) type_path: Option<String>,
}

// constraint: two blocks can end at the same byte, so a block is told apart by its node's id
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Block {
    pub(super) id: usize,
    pub(super) end: usize,
}

impl Block {
    pub(super) fn of(node: Node<'_>) -> Self {
        Self {
            id: node.id(),
            end: node.end_byte(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum LocalKind {
    Value,
    ReceiverOf(SymbolId),
    ValueSymbol(SymbolId),
    TypeSymbol(SymbolId),
}

impl Emitter<'_> {
    pub(super) fn push_params(&mut self, function: Node<'_>, field: &str, kind: LocalKind) {
        let Some(list) = function
            .child_by_field_name(field)
            .filter(|list| list.kind() == "parameters")
        else {
            return;
        };
        let body = function.child_by_field_name("body");
        let from = body.map_or_else(|| function.start_byte(), |body| body.start_byte());
        let block = Block::of(body.unwrap_or(function));
        for i in named_children(list) {
            let Some(param) = list.named_child(i) else {
                continue;
            };
            let type_path = param
                .child_by_field_name("type")
                .and_then(|written| self.type_path(written));
            let mut cursor = param.walk();
            for name in param.children_by_field_name("name", &mut cursor) {
                self.locals.push(Local {
                    from,
                    block,
                    name: text(name, self.source),
                    kind,
                    type_path: type_path.clone(),
                });
            }
        }
    }

    pub(super) fn push_locals(
        &mut self,
        left: Node<'_>,
        (from, block): (usize, Block),
        type_paths: &mut dyn FnMut(usize) -> Option<String>,
    ) {
        let names = named_children(left)
            .filter_map(|i| left.named_child(i))
            .enumerate()
            .filter(|(_, name)| name.kind() == "identifier");
        for (position, name) in names {
            let name = text(name, self.source);
            let reassigned =
                (self.locals.iter()).any(|local| local.block == block && local.name == name);
            if !reassigned {
                self.locals.push(Local {
                    from,
                    block,
                    name,
                    kind: LocalKind::Value,
                    type_path: type_paths(position),
                });
            }
        }
    }

    pub(super) fn is_local(&self, name: &str, at: usize) -> bool {
        self.innermost_local(name, at).is_some()
    }

    pub(super) fn is_value(&self, name: &str, at: usize) -> bool {
        self.innermost_local(name, at).map_or_else(
            || self.package_values.contains(name),
            |local| !matches!(local.kind, LocalKind::TypeSymbol(_)),
        )
    }

    pub(super) fn innermost_local(&self, name: &str, at: usize) -> Option<&Local> {
        (self.locals.iter().rev())
            .find(|local| local.from <= at && at < local.block.end && local.name == name)
    }

    pub(super) fn push_type_params(&mut self, list: Option<Node<'_>>, end: usize) {
        let Some(list) = list else {
            return;
        };
        for i in named_children(list) {
            let Some(declaration) = list.named_child(i) else {
                continue;
            };
            let mut cursor = declaration.walk();
            for name in declaration.children_by_field_name("name", &mut cursor) {
                self.type_params.push((end, text(name, self.source)));
            }
        }
    }

    pub(super) fn push_receiver_type_params(&mut self, method: Node<'_>) {
        let Some(receiver) = method.child_by_field_name("receiver") else {
            return;
        };
        let end = method.end_byte();
        preorder_within(receiver, |current| {
            let is_arguments = current.kind() == "type_arguments";
            if is_arguments {
                self.push_type_arguments(current, end);
            }
            !is_arguments
        });
    }

    fn push_type_arguments(&mut self, arguments: Node<'_>, end: usize) {
        for i in named_children(arguments) {
            if let Some(argument) = arguments.named_child(i)
                && argument.kind() == "type_identifier"
            {
                self.type_params.push((end, text(argument, self.source)));
            }
        }
    }
}

pub(super) fn declares_left(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == ":=")
}

pub(super) fn header_statement(node: Node<'_>) -> Option<Node<'_>> {
    let parent = node.parent()?;
    match parent.kind() {
        "for_clause" => parent.parent(),
        "if_statement"
        | "for_statement"
        | "expression_switch_statement"
        | "type_switch_statement" => Some(parent),
        _ => None,
    }
}
