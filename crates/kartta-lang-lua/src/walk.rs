use kartta_lang::tree::{children, named_children, preorder, signature, signature_from, text};
use kartta_lang::{
    CallRef, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use crate::banner::is_generated_source;

pub(crate) const REQUIRE_FORM: u32 = 0;
pub(crate) const DOFILE_FORM: u32 = 1;

const IDENTIFIER: &str = "identifier";
const FUNCTION_DEFINITION: &str = "function_definition";

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter {
        source,
        scopes: Vec::new(),
        pending_scopes: Vec::new(),
        flags: SymbolFlags::empty().with_generated(is_generated_source(source)),
        raw: RawExtraction::default(),
    };

    preorder(tree, |node| {
        if node.is_named() {
            emitter.handle(node);
        }
        true
    });

    emitter.raw
}

#[derive(Clone, Copy)]
struct Scope {
    end: usize,
    id: SymbolId,
}

struct Emitter<'s> {
    source: &'s [u8],
    scopes: Vec<Scope>,
    // constraint: a function bound by an assignment opens its scope when the walk reaches the function, not at the
    // assignment: a value before it, `f, g = mk(), function() end`, is no call of `g`. The walk reaches the bound
    // functions in the order they are written, and those an assignment inside one of them binds before the next, so
    // the next to open is the last one held
    pending_scopes: Vec<(usize, SymbolId)>,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl Emitter<'_> {
    fn handle(&mut self, node: Node<'_>) {
        let start = node.start_byte();
        while self.scopes.last().is_some_and(|scope| scope.end <= start) {
            self.scopes.pop();
        }

        match node.kind() {
            "function_declaration" => self.function_declaration(node),
            "assignment_statement" => self.assignment(node),
            FUNCTION_DEFINITION => self.open_pending_scope(node),
            "field" => self.table_field(node),
            "function_call" => self.call(node),
            _ => {}
        }
    }

    fn push(&mut self, mut symbol: Symbol) -> SymbolId {
        let id = SymbolId(u32::try_from(self.raw.symbols.len()).unwrap_or(u32::MAX));
        symbol.flags = self.flags;
        self.raw.symbols.push(symbol);
        id
    }

    fn function_declaration(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let (name, owner) = name_and_owner(name, self.source);
        if name.is_empty() {
            return;
        }

        let mut symbol = function(
            name,
            owner,
            line(node.start_position().row),
            line(node.end_position().row),
        );
        symbol.exported = !children(node)
            .filter_map(|index| node.child(index))
            .any(|child| child.kind() == "local");
        symbol.signature = signature(node, self.source);
        let id = self.push(symbol);
        self.scopes.push(Scope {
            end: node.end_byte(),
            id,
        });
    }

    // constraint: only a value that is itself a function binds a symbol; a value that holds one, `x = f() and
    // function() end`, binds none, and a table value hands its fields to `table_field`
    fn assignment(&mut self, node: Node<'_>) {
        let (Some(variables), Some(values)) = (
            named_child_of_kind(node, "variable_list"),
            named_child_of_kind(node, "expression_list"),
        ) else {
            return;
        };
        let exported = node
            .parent()
            .is_none_or(|parent| parent.kind() != "variable_declaration");

        let held = self.pending_scopes.len();
        for (variable, value) in bound_variables(variables).zip(named(values)) {
            if value.kind() != FUNCTION_DEFINITION {
                continue;
            }
            let (name, owner) = name_and_owner(variable, self.source);
            if name.is_empty() {
                continue;
            }

            let mut symbol = function(
                name,
                owner,
                line(variable.start_position().row),
                line(value.end_position().row),
            );
            symbol.exported = exported;
            symbol.signature = signature(value, self.source)
                .map(|header| format!("{} = {header}", text(variable, self.source)));
            let id = self.push(symbol);
            self.pending_scopes.push((value.id(), id));
        }
        self.pending_scopes[held..].reverse();
    }

    fn open_pending_scope(&mut self, node: Node<'_>) {
        if self
            .pending_scopes
            .last()
            .is_some_and(|&(function, _)| function == node.id())
            && let Some((_, id)) = self.pending_scopes.pop()
        {
            self.scopes.push(Scope {
                end: node.end_byte(),
                id,
            });
        }
    }

    // constraint: only an identifier key names a method; a string key, `["setup"]`, would carry its quotes, and a
    // computed key, `[k]`, still puts its identifier in the `name` field, so the opening bracket is the tell
    fn table_field(&mut self, node: Node<'_>) {
        let Some(value) = node
            .child_by_field_name("value")
            .filter(|value| value.kind() == FUNCTION_DEFINITION)
        else {
            return;
        };
        let Some(key) = node.child_by_field_name("name") else {
            return;
        };
        if key.kind() != IDENTIFIER || node.child(0).is_none_or(|first| first.id() != key.id()) {
            return;
        }
        let name = text(key, self.source);
        if name.is_empty() {
            return;
        }

        let owner = table_binding(node, self.source);
        let mut symbol = function(
            name,
            owner,
            line(node.start_position().row),
            line(value.end_position().row),
        );
        symbol.exported = true;
        symbol.signature = signature_from(node.start_byte(), value, self.source);
        let id = self.push(symbol);
        self.scopes.push(Scope {
            end: value.end_byte(),
            id,
        });
    }

    fn call(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let (callee, owner) = name_and_owner(name, self.source);

        // constraint: a loader call is recorded before the guard on calls ahead of the first symbol, since a module
        // loads its dependencies at its top; only a bare `require`, `dofile` or `loadfile` loads, `M.require` is a
        // method
        if name.kind() == IDENTIFIER
            && let Some(form) = loader_form(&callee)
            && let Some(literal) = loader_literal(node, self.source)
        {
            self.raw.directives.push(RawDirective::new(
                form,
                literal,
                line(node.start_position().row),
            ));
            return;
        }

        if callee.is_empty() || self.raw.symbols.is_empty() {
            return;
        }
        let receiver = matches!(
            name.kind(),
            "dot_index_expression" | "method_index_expression"
        )
        .then(|| owner.unwrap_or_default());
        self.raw.calls.push(CallRef {
            caller_id: self.scopes.last().map_or(NO_CALLER_ID, |scope| scope.id),
            callee,
            receiver,
            line: line(node.start_position().row),
        });
    }
}

fn function(name: String, owner: Option<String>, line: u32, end_line: u32) -> Symbol {
    let Some(owner) = owner else {
        return Symbol::new(name, SymbolKind::Function, line, end_line);
    };

    let qualified_name = format!("{owner}.{name}");
    let mut symbol = Symbol::new(name, SymbolKind::Method, line, end_line);
    symbol.qualified_name = qualified_name;
    symbol.owner = Some(owner);
    symbol
}

// constraint: only an identifier table is an owner; a chained head, `a.b.c()`, names none, and a call through it keeps
// the empty receiver
fn name_and_owner(node: Node<'_>, source: &[u8]) -> (String, Option<String>) {
    let field = match node.kind() {
        IDENTIFIER => return (text(node, source), None),
        "dot_index_expression" => "field",
        "method_index_expression" => "method",
        _ => return (String::new(), None),
    };

    let name = node
        .child_by_field_name(field)
        .map(|name| text(name, source))
        .unwrap_or_default();
    let owner = node
        .child_by_field_name("table")
        .filter(|table| table.kind() == IDENTIFIER)
        .map(|table| text(table, source))
        .filter(|owner| !owner.is_empty());
    (name, owner)
}

fn loader_form(callee: &str) -> Option<u32> {
    match callee {
        "require" => Some(REQUIRE_FORM),
        "dofile" | "loadfile" => Some(DOFILE_FORM),
        _ => None,
    }
}

// constraint: `require("a")`, `require "a"` and `require[[a]]` all parse to a `string` whose `content` is the inner
// text; an empty string has no `content`
fn loader_literal(call: Node<'_>, source: &[u8]) -> Option<String> {
    let first = call.child_by_field_name("arguments")?.named_child(0)?;
    if first.kind() != "string" {
        return None;
    }

    Some(
        first
            .child_by_field_name("content")
            .map_or_else(String::new, |content| text(content, source)),
    )
}

// constraint: only a constructor bound directly by an assignment has an owner, the variable in its own place,
// `U` for the second table of `local T, U = {...}, {...}`; a nested table's fields have none
fn table_binding(field: Node<'_>, source: &[u8]) -> Option<String> {
    let constructor = field
        .parent()
        .filter(|node| node.kind() == "table_constructor")?;
    let values = constructor
        .parent()
        .filter(|node| node.kind() == "expression_list")?;
    let assignment = values
        .parent()
        .filter(|node| node.kind() == "assignment_statement")?;

    let place = named(values).position(|value| value.id() == constructor.id())?;
    let variable = bound_variables(named_child_of_kind(assignment, "variable_list")?).nth(place)?;
    matches!(variable.kind(), IDENTIFIER | "dot_index_expression").then(|| text(variable, source))
}

// constraint: the `<const>` of `local a <const>, b = ...` is a named child between the variables
fn bound_variables(variables: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named(variables).filter(|variable| variable.kind() != "attribute")
}

fn named(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named_children(node).filter_map(move |index| node.named_child(index))
}

fn named_child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    named(node).find(|child| child.kind() == kind)
}

fn line(row: usize) -> u32 {
    u32::try_from(row).map_or(u32::MAX, |row| row.saturating_add(1))
}
