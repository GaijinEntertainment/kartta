use std::collections::HashSet;

use kartta_lang::tree::{child_nodes, line, named_child_nodes, signature, signature_from, text};
use kartta_lang::{
    Access, CallRef, FILE_SCOPE, RawDirective, RawExtraction, Receiver, Symbol, SymbolFlags,
    SymbolId, SymbolKind, UseRef,
};
use tree_sitter::{Node, Tree};

use crate::banner::is_generated_source;

pub(crate) const REQUIRE_FORM: u32 = 0;
pub(crate) const DOFILE_FORM: u32 = 1;

const IDENTIFIER: &str = "identifier";
const FUNCTION_DEFINITION: &str = "function_definition";
const FUNCTION_DECLARATION: &str = "function_declaration";

// constraint: a node is judged as a value by its parent's kind and its field there, which only the cursor holds, as
// `Node::parent` walks down from the root
pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter {
        source,
        scopes: Vec::new(),
        regions: Vec::new(),
        pending_locals: Vec::new(),
        pending_scopes: Vec::new(),
        flags: SymbolFlags::empty().with_generated(is_generated_source(source)),
        raw: RawExtraction::default(),
    };
    let mut cursor = tree.walk();
    let mut parents = Vec::new();
    loop {
        let node = cursor.node();
        if node.is_named() {
            emitter.handle(
                node,
                parents.last().map(|&parent| (parent, cursor.field_name())),
            );
        }
        if cursor.goto_first_child() {
            parents.push(node.kind());
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return emitter.raw;
            }
            parents.pop();
        }
    }
}

#[derive(Clone, Copy)]
struct Scope {
    end: usize,
    id: SymbolId,
}

// constraint: a region hides the names declared in it: a function, an anonymous one too, which opens no scope since
// its calls belong to no symbol, its parameters and a local function its own name, a loop its variables, and a block
// or the chunk its locals; outside every function, a local function, a local bound to a function and one declared
// without a value, which `function f()` or `f = function() end` binds later, are definitions the file binds, and hide
// nothing
struct Region<'s> {
    end: usize,
    function: bool,
    names: HashSet<&'s [u8]>,
}

// constraint: a local hides its name from the end of its declaration on, as the initializer of `local f = f` reads
// the `f` outside; the declaration lies inside the region it declares into, so that region is still open when the
// walk passes the declaration's end
struct PendingLocals<'s> {
    at: usize,
    region: usize,
    names: Vec<&'s [u8]>,
}

struct Emitter<'s> {
    source: &'s [u8],
    scopes: Vec<Scope>,
    regions: Vec<Region<'s>>,
    pending_locals: Vec<PendingLocals<'s>>,
    // constraint: a function bound by an assignment opens its scope when the walk reaches the function, not at the
    // assignment: a value before it, `f, g = mk(), function() end`, is no call of `g`. The walk reaches the bound
    // functions in the order they are written, and those an assignment inside one of them binds before the next, so
    // the next to open is the last one held
    pending_scopes: Vec<(usize, SymbolId)>,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl<'s> Emitter<'s> {
    fn handle(&mut self, node: Node<'_>, parent: Option<(&str, Option<&str>)>) {
        let start = node.start_byte();
        while self.scopes.last().is_some_and(|scope| scope.end <= start) {
            self.scopes.pop();
        }
        while let Some(pending) = self.pending_locals.pop_if(|pending| pending.at <= start) {
            self.regions[pending.region].names.extend(pending.names);
        }
        while self
            .regions
            .last()
            .is_some_and(|region| region.end <= start)
        {
            self.regions.pop();
        }

        match node.kind() {
            FUNCTION_DECLARATION => {
                let source = self.source;
                let local = child_nodes(node)
                    .any(|child| child.kind() == "local")
                    .then(|| node.child_by_field_name("name"))
                    .flatten()
                    .filter(|name| name.kind() == IDENTIFIER)
                    .map(|name| &source[name.byte_range()]);
                if self.in_function()
                    && let (Some(name), Some(region)) = (local, self.regions.last_mut())
                {
                    region.names.insert(name);
                }
                self.function_declaration(node);
                self.open_function(node, local);
            }
            "variable_declaration" => self.declare_locals(node),
            "assignment_statement" => self.assignment(node),
            FUNCTION_DEFINITION => {
                self.open_pending_scope(node);
                self.open_function(node, None);
            }
            "field" => self.table_field(node),
            "function_call" => self.call(node),
            "for_statement" => self.open_loop(node),
            "chunk" | "block" => self.regions.push(Region {
                end: node.end_byte(),
                function: false,
                names: HashSet::new(),
            }),
            IDENTIFIER
                if parent.is_some_and(|(parent, field)| is_value_position(parent, field)) =>
            {
                self.value_use(node);
            }
            _ => {}
        }
    }

    fn open_function(&mut self, function: Node<'_>, own_name: Option<&'s [u8]>) {
        let mut names = parameter_names(function, self.source);
        names.extend(own_name);
        self.regions.push(Region {
            end: function.end_byte(),
            function: true,
            names,
        });
    }

    fn in_function(&self) -> bool {
        self.regions.iter().any(|region| region.function)
    }

    fn declare_locals(&mut self, declaration: Node<'_>) {
        let (Some(region), Some(variables)) = (
            self.regions.len().checked_sub(1),
            declared_variables(declaration),
        ) else {
            return;
        };
        let in_function = self.in_function();
        let mut values = declared_values(declaration);
        let names = bound_variables(variables)
            .filter(|_| {
                in_function
                    || values
                        .next()
                        .is_some_and(|value| value.kind() != FUNCTION_DEFINITION)
            })
            .filter(|name| name.kind() == IDENTIFIER)
            .map(|name| &self.source[name.byte_range()])
            .collect();
        self.pending_locals.push(PendingLocals {
            at: declaration.end_byte(),
            region,
            names,
        });
    }

    fn open_loop(&mut self, statement: Node<'_>) {
        let mut names = HashSet::new();
        if let Some(clause) = statement.child_by_field_name("clause") {
            let listed = named_child_of_kind(clause, "variable_list").into_iter();
            let variables = (clause.child_by_field_name("name").into_iter())
                .chain(listed.flat_map(bound_variables))
                .filter(|variable| variable.kind() == IDENTIFIER);
            names.extend(variables.map(|variable| &self.source[variable.byte_range()]));
        }
        self.regions.push(Region {
            end: statement.end_byte(),
            function: false,
            names,
        });
    }

    // constraint: a name read where no call takes it, and that no region around it declares, is a value its file
    // binds: a function passed, stored or returned without a call
    fn value_use(&mut self, name: Node<'_>) {
        let spelled = &self.source[name.byte_range()];
        if self
            .regions
            .iter()
            .any(|region| region.names.contains(spelled))
        {
            return;
        }
        self.raw.uses.push(UseRef {
            site_of: self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id),
            name: text(name, self.source),
            receiver: Receiver::Bare,
            access: Access::Read,
            line: line(name.start_position().row),
        });
    }

    fn push(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.flags = self.flags;
        self.raw.push_symbol(symbol)
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
        symbol.exported = !child_nodes(node).any(|child| child.kind() == "local");
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
        for (variable, value) in bound_variables(variables).zip(named_child_nodes(values)) {
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

        // constraint: only a bare `require`, `dofile` or `loadfile` loads, `M.require` is a method
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

        if callee.is_empty() {
            return;
        }
        let receiver = matches!(
            name.kind(),
            "dot_index_expression" | "method_index_expression"
        )
        .then(|| owner.unwrap_or_default());
        self.raw.calls.push(CallRef {
            site_of: self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id),
            callee,
            receiver,
            line: line(node.start_position().row),
            invokes_a_macro: false,
            calls_a_method: false,
            arguments: 0,
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

    let place = named_child_nodes(values).position(|value| value.id() == constructor.id())?;
    let variable = bound_variables(named_child_of_kind(assignment, "variable_list")?).nth(place)?;
    matches!(variable.kind(), IDENTIFIER | "dot_index_expression").then(|| text(variable, source))
}

// constraint: the `<const>` of `local a <const>, b = ...` is a named child between the variables
fn bound_variables(variables: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named_child_nodes(variables).filter(|variable| variable.kind() != "attribute")
}

fn named_child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    named_child_nodes(node).find(|child| child.kind() == kind)
}

// constraint: a name is read as a value as an argument, a value an assignment, a `return` or a loop takes, a field's
// value, a key, an operand or a condition; a callee, an assignment's target, a table indexed through `.` or `:` and a
// name being declared are not
fn is_value_position(parent: &str, field: Option<&str>) -> bool {
    match parent {
        "arguments"
        | "expression_list"
        | "binary_expression"
        | "unary_expression"
        | "parenthesized_expression" => true,
        "field" => field == Some("value"),
        "bracket_index_expression" => field == Some("field"),
        "if_statement" | "elseif_statement" | "while_statement" | "repeat_statement" => {
            field == Some("condition")
        }
        "for_numeric_clause" => field != Some("name"),
        _ => false,
    }
}

// constraint: a method declared with `:` holds `self` beside its parameters
fn parameter_names<'s>(function: Node<'_>, source: &'s [u8]) -> HashSet<&'s [u8]> {
    let mut names = HashSet::new();
    if function
        .child_by_field_name("name")
        .is_some_and(|name| name.kind() == "method_index_expression")
    {
        names.insert(b"self".as_slice());
    }
    if let Some(parameters) = function.child_by_field_name("parameters") {
        let parameters = named_child_nodes(parameters).filter(|name| name.kind() == IDENTIFIER);
        names.extend(parameters.map(|name| &source[name.byte_range()]));
    }
    names
}

// constraint: `local a` holds its names in a `variable_list`, and `local a = ...` in the one of its assignment
fn declared_variables(declaration: Node<'_>) -> Option<Node<'_>> {
    named_child_of_kind(declaration, "variable_list").or_else(|| {
        named_child_of_kind(declaration, "assignment_statement")
            .and_then(|assignment| named_child_of_kind(assignment, "variable_list"))
    })
}

// constraint: the values of `local a, b = x, y` in their places, and none for `local a`
fn declared_values(declaration: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    named_child_of_kind(declaration, "assignment_statement")
        .and_then(|assignment| named_child_of_kind(assignment, "expression_list"))
        .into_iter()
        .flat_map(named_child_nodes)
}
