use std::collections::HashSet;

use kartta_lang::tree::{line, named_child_nodes, preorder, signature_from, text};
use kartta_lang::{
    Access, Banner, CallRef, FILE_SCOPE, RawDirective, RawExtraction, Receiver, Symbol,
    SymbolFlags, SymbolId, SymbolKind, UseRef,
};
use tree_sitter::{Node, Tree};

const IMPORT_FORM: u32 = 0;

const BANNER: Banner = Banner::new(&["@generated", "DO NOT EDIT", "Code generated"])
    .with_lines(&["//"])
    .with_block("/*", "*/");

// constraint: a node is judged as a value by its parent's kind and its field there, which only the cursor holds, as
// `Node::parent` walks down from the root
pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter::new(source);
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

struct Scope<'a> {
    end: usize,
    id: SymbolId,
    locals: HashSet<&'a [u8]>,
}

enum SlotOwner {
    Class(String),
    ModuleTable(String),
}

struct SlotContainer {
    end: usize,
    owner: Option<SlotOwner>,
}

struct Binding {
    name: String,
    owner: Option<String>,
    header_start: usize,
}

struct Emitter<'a> {
    source: &'a [u8],
    scopes: Vec<Scope<'a>>,
    containers: Vec<SlotContainer>,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn new(source: &'a [u8]) -> Self {
        Self {
            source,
            scopes: Vec::new(),
            containers: Vec::new(),
            flags: SymbolFlags::empty().with_generated(BANNER.opens(source)),
            raw: RawExtraction::default(),
        }
    }

    fn handle(&mut self, node: Node<'_>, parent: Option<(&str, Option<&str>)>) {
        let start = node.start_byte();
        while self.scopes.last().is_some_and(|scope| scope.end <= start) {
            self.scopes.pop();
        }
        while self
            .containers
            .last()
            .is_some_and(|container| container.end <= start)
        {
            self.containers.pop();
        }

        match node.kind() {
            "table" => {
                let owner = self.module_table_name(node).map(SlotOwner::ModuleTable);
                self.open_container(node, owner);
            }
            "class_body" => {
                let owner = node
                    .parent()
                    .and_then(|class| class_name(class, self.source))
                    .map(SlotOwner::Class);
                self.open_container(node, owner);
            }
            "slot" => self.data_field(node),
            "class_declaration" | "class_expression" => self.class(node),
            "function_declaration" | "method" => self.declared_function(node),
            "local_declaration" => self.local_declaration(node),
            "const_declaration" => self.const_declaration(node),
            "function_expression" | "lambda_expression" => self.function_value(node),
            "call_expression" => self.call(node),
            "import_statement" => self.import(node),
            "enum_declaration" => self.enum_declaration(node),
            _ => {}
        }
        if let Some((parent, field)) = parent {
            self.value_use(parent, field, node);
        }
    }

    // constraint: a name read where no call takes it, and that no parameter or local of an enclosing function
    // declares, is a value its file binds: a function passed, stored or returned without a call
    fn value_use(&mut self, parent: &str, field: Option<&str>, child: Node<'_>) {
        match child.kind() {
            "identifier" if is_value_position(parent, field) => self.push_value_use(child, false),
            "root_table_access" if is_value_position(parent, field) => {
                if let Some(name) = child.child_by_field_name("name") {
                    self.push_value_use(name, true);
                }
            }
            _ => {}
        }
    }

    fn push_value_use(&mut self, name: Node<'_>, from_root: bool) {
        let spelled = &self.source[name.byte_range()];
        if !from_root
            && self
                .scopes
                .iter()
                .any(|scope| scope.locals.contains(spelled))
        {
            return;
        }

        let site_of = self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id);
        self.raw.uses.push(UseRef {
            site_of,
            name: text(name, self.source),
            receiver: Receiver::Bare,
            access: Access::Read,
            line: start_line(name),
        });
    }

    fn class(&mut self, node: Node<'_>) {
        if let Some(name) = class_name(node, self.source) {
            let mut symbol = self.declared(SymbolKind::Class, node, (None, name));
            symbol.parents = field_text(node, "base", self.source).into_iter().collect();
            self.raw.push_symbol(symbol);
        }
    }

    // constraint: a slot whose value is a function, a lambda or a class has a symbol of its own; a field keeps the
    // slot as its signature only when the value is a literal, a name or a unary expression
    fn data_field(&mut self, slot: Node<'_>) {
        let Some(owner) = self.field_owner() else {
            return;
        };
        let Some(value) = slot.child_by_field_name("value").filter(|value| {
            !matches!(
                value.kind(),
                "function_expression" | "lambda_expression" | "class_expression"
            )
        }) else {
            return;
        };
        let Some(name) = field_text(slot, "key", self.source) else {
            return;
        };

        let owner = owner.to_owned();
        let mut symbol = self.declared(SymbolKind::Field, slot, (Some(owner), name));
        symbol.signature = is_simple_value(value)
            .then(|| signature_from(slot.start_byte(), value, self.source))
            .flatten();
        self.raw.push_symbol(symbol);
    }

    fn local_declaration(&mut self, node: Node<'_>) {
        match node.child_by_field_name("body").map(|body| body.kind()) {
            Some("class_body") => self.class(node),
            Some(_) => self.declared_function(node),
            None => {}
        }
    }

    fn const_declaration(&mut self, node: Node<'_>) {
        if node.child_by_field_name("body").is_some() {
            self.declared_function(node);
        } else if let Some(name) = field_text(node, "name", self.source) {
            let symbol = self.declared(SymbolKind::Constant, node, (None, name));
            self.raw.push_symbol(symbol);
        }
    }

    fn declared_function(&mut self, node: Node<'_>) {
        let id = field_text(node, "name", self.source).map_or(FILE_SCOPE, |name| {
            let owner = (node.kind() == "method")
                .then(|| self.member_owner())
                .flatten()
                .map(str::to_owned);
            let header_start = node.start_byte();
            self.bind_function(
                Binding {
                    name,
                    owner,
                    header_start,
                },
                node,
            )
        });
        self.open_scope(node, id);
    }

    // constraint: a function value is named by its binding, which is the name its callers use; a value with no
    // binding and no name of its own runs where it is passed, so its calls belong to no symbol
    fn function_value(&mut self, node: Node<'_>) {
        let binding = self.value_binding(node).or_else(|| {
            field_text(node, "name", self.source).map(|name| Binding {
                name,
                owner: None,
                header_start: node.start_byte(),
            })
        });
        let id = binding.map_or(FILE_SCOPE, |binding| self.bind_function(binding, node));
        self.open_scope(node, id);
    }

    fn enum_declaration(&mut self, node: Node<'_>) {
        let Some(name) = field_text(node, "name", self.source) else {
            return;
        };

        let symbol = self.declared(SymbolKind::Enum, node, (None, name.clone()));
        self.raw.push_symbol(symbol);

        let members = named_child_nodes(node);
        for member in members {
            if let Some(member_name) = field_text(member, "name", self.source) {
                let owner = Some(name.clone());
                let symbol = self.declared(SymbolKind::EnumVariant, member, (owner, member_name));
                self.raw.push_symbol(symbol);
            }
        }
    }

    fn bind_function(&mut self, binding: Binding, node: Node<'_>) -> SymbolId {
        let Binding {
            name,
            owner,
            header_start,
        } = binding;
        let kind = if owner.is_some() {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };

        let mut symbol = self.declared(kind, node, (owner, name));
        symbol.signature = signature_from(header_start, node, self.source);
        self.raw.push_symbol(symbol)
    }

    fn open_scope(&mut self, callable: Node<'_>, id: SymbolId) {
        self.scopes.push(Scope {
            end: callable.end_byte(),
            id,
            locals: declared_names(callable, self.source),
        });
    }

    fn open_container(&mut self, node: Node<'_>, owner: Option<SlotOwner>) {
        self.containers.push(SlotContainer {
            end: node.end_byte(),
            owner,
        });
    }

    fn member_owner(&self) -> Option<&str> {
        match self.containers.last()?.owner.as_ref()? {
            SlotOwner::Class(name) => Some(name),
            SlotOwner::ModuleTable(_) => None,
        }
    }

    fn field_owner(&self) -> Option<&str> {
        match self.containers.last()?.owner.as_ref()? {
            SlotOwner::Class(name) | SlotOwner::ModuleTable(name) => Some(name),
        }
    }

    // constraint: a module table is a table that a file-scope `let` or `local` binds; `Node::parent` walks down from
    // the root, so a table inside a scope or a container skips the lookup
    fn module_table_name(&self, table: Node<'_>) -> Option<String> {
        if !self.scopes.is_empty() || !self.containers.is_empty() {
            return None;
        }
        let variable = table
            .parent()
            .filter(|parent| parent.kind() == "variable_declaration")?;
        variable
            .parent()
            .filter(|parent| parent.kind() == "local_declaration")?
            .parent()
            .filter(|parent| parent.kind() == "source_file")?;

        field_text(variable, "name", self.source)
    }

    fn value_binding(&self, value: Node<'_>) -> Option<Binding> {
        let parent = value.parent()?;
        let (name, owner, header_start) = match parent.kind() {
            "slot" => (
                field_text(parent, "key", self.source)?,
                self.member_owner().map(str::to_owned),
                parent.start_byte(),
            ),
            "variable_declaration" => (
                field_text(parent, "name", self.source)?,
                None,
                declarator_header_start(parent),
            ),
            "assignment_expression" | "newslot_expression" => {
                let (name, owner) = assignment_target(parent, value, self.source)?;
                (name, owner, parent.start_byte())
            }
            _ => return None,
        };

        Some(Binding {
            name,
            owner,
            header_start,
        })
    }

    fn call(&mut self, node: Node<'_>) {
        let Some(callee) = node.child_by_field_name("callee") else {
            return;
        };

        match callee.kind() {
            "identifier" => self.bare_call(node, text(callee, self.source)),
            "root_table_access" => {
                if let Some(name) = field_text(callee, "name", self.source) {
                    self.push_call(name, None, node);
                }
            }
            "field_access_expression" => {
                if let Some(name) = field_text(callee, "field", self.source) {
                    let receiver = field_text(callee, "receiver", self.source);
                    self.push_call(name, receiver, node);
                }
            }
            _ => {}
        }
    }

    // constraint: the loader's `require("spec")` and `require_optional("spec")` load a module, so a call of either
    // with one string literal is an import, before any symbol and without a call of the loader
    fn bare_call(&mut self, node: Node<'_>, name: String) {
        let loaded = matches!(name.as_str(), "require" | "require_optional")
            .then(|| sole_string_argument(node))
            .flatten();
        let Some(spec) = loaded else {
            self.push_call(name, None, node);
            return;
        };

        let module = trim_quotes(&text(spec, self.source)).to_owned();
        if !module.is_empty() {
            self.record_import(module, Vec::new(), start_line(node));
        }
    }

    fn import(&mut self, node: Node<'_>) {
        let Some(module) = node.child_by_field_name("module") else {
            return;
        };
        let module = trim_quotes(&text(module, self.source)).to_owned();
        if module.is_empty() {
            return;
        }

        let names = named_child_nodes(node)
            .filter(|child| child.kind() == "import_slot")
            .filter_map(|slot| field_text(slot, "name", self.source))
            .collect();
        self.record_import(module, names, start_line(node));
    }

    fn record_import(&mut self, spec: String, names: Vec<String>, line: u32) {
        let mut directive = RawDirective::new(IMPORT_FORM, spec, line);
        directive.names = names;
        self.raw.directives.push(directive);
    }

    // constraint: `Symbol::new` copies its name into the qualified name, so the symbol starts unnamed, which allocates
    // nothing, and then takes both names
    fn declared(
        &self,
        kind: SymbolKind,
        node: Node<'_>,
        (owner, name): (Option<String>, String),
    ) -> Symbol {
        let mut symbol = Symbol::new(String::new(), kind, start_line(node), end_line(node));
        symbol.qualified_name = owner
            .as_ref()
            .map_or_else(|| name.clone(), |owner| format!("{owner}.{name}"));
        symbol.name = name;
        symbol.owner = owner;
        symbol.exported = true;
        symbol.flags = self.flags;
        symbol
    }

    fn push_call(&mut self, callee: String, receiver: Option<String>, node: Node<'_>) {
        if callee.is_empty() {
            return;
        }

        let site_of = self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id);
        self.raw.calls.push(CallRef {
            site_of,
            callee,
            receiver,
            line: start_line(node),
        });
    }
}

// constraint: a name a declaration, a slot's key, a member's field, an assignment's target or an increment holds is
// no value read; a shorthand slot's key is the value it stores
fn is_value_position(parent: &str, field: Option<&str>) -> bool {
    match parent {
        "shorthand_slot" => true,
        "call_expression" => field != Some("callee"),
        "assignment_expression" | "newslot_expression" | "compound_assignment_expression" => {
            field != Some("left")
        }
        "foreach_statement" => !matches!(field, Some("index" | "value")),
        "field_access_expression"
        | "increment_expression"
        | "root_table_access"
        | "directive"
        | "function_attributes"
        | "type_annotation" => false,
        _ => !matches!(field, Some("name" | "key" | "alias" | "index")),
    }
}

// constraint: a parameter, or a local a body declares anywhere, hides its name in the whole body, as no use before a
// local's declaration reads anything else; a nested function's name is a local, its own locals are not
fn declared_names<'a>(callable: Node<'_>, source: &'a [u8]) -> HashSet<&'a [u8]> {
    let mut names = HashSet::new();
    let mut declare = |declared: Option<Node<'_>>| {
        if let Some(declared) = declared.filter(|declared| declared.kind() == "identifier") {
            names.insert(&source[declared.byte_range()]);
        }
    };
    if let Some(parameters) = callable.child_by_field_name("parameters") {
        preorder(parameters, |node| {
            if matches!(node.kind(), "parameter" | "pattern_field") {
                declare(node.child_by_field_name("name"));
            }
            true
        });
    }
    let Some(body) = callable.child_by_field_name("body") else {
        return names;
    };
    preorder(body, |node| match node.kind() {
        "function_declaration"
        | "function_expression"
        | "lambda_expression"
        | "method"
        | "class_declaration"
        | "class_expression" => {
            declare(node.child_by_field_name("name"));
            false
        }
        "local_declaration" => {
            declare(node.child_by_field_name("name"));
            node.child_by_field_name("body").is_none()
        }
        "variable_declaration" | "pattern_field" | "catch_clause" | "const_declaration" => {
            declare(node.child_by_field_name("name"));
            true
        }
        "foreach_statement" => {
            declare(node.child_by_field_name("index"));
            declare(node.child_by_field_name("value"));
            true
        }
        _ => true,
    });
    names
}

// constraint: error recovery inserts MISSING nodes, which have a field but no text
fn field_text(node: Node<'_>, field: &str, source: &[u8]) -> Option<String> {
    node.child_by_field_name(field)
        .filter(|child| !child.is_missing())
        .map(|child| text(child, source))
}

// constraint: the first declarator of a `let` or `local` starts its header at the keyword, a later one at its name
fn declarator_header_start(variable: Node<'_>) -> usize {
    variable
        .parent()
        .filter(|declaration| declaration.named_child(0) == Some(variable))
        .map_or_else(
            || variable.start_byte(),
            |declaration| declaration.start_byte(),
        )
}

// constraint: a class expression has no name of its own; it takes the name of the slot, the declaration or the
// identifier it is bound to, and a class bound to a field access or passed as a value stays anonymous
fn class_name(class: Node<'_>, source: &[u8]) -> Option<String> {
    if class.kind() != "class_expression" {
        return field_text(class, "name", source);
    }

    let parent = class.parent()?;
    match parent.kind() {
        "slot" => field_text(parent, "key", source),
        "variable_declaration" => field_text(parent, "name", source),
        "assignment_expression" | "newslot_expression" => {
            match assignment_target(parent, class, source)? {
                (name, None) => Some(name),
                (_, Some(_)) => None,
            }
        }
        _ => None,
    }
}

// constraint: an assignment to a field access binds a member of the whole receiver expression, `gui.Widget.build`
// one of `gui.Widget`, and a computed target binds nothing
fn assignment_target(
    assignment: Node<'_>,
    value: Node<'_>,
    source: &[u8],
) -> Option<(String, Option<String>)> {
    if assignment.child_by_field_name("right") != Some(value) {
        return None;
    }

    let left = assignment.child_by_field_name("left")?;
    match left.kind() {
        "identifier" if !left.is_missing() => Some((text(left, source), None)),
        "root_table_access" => Some((field_text(left, "name", source)?, None)),
        "field_access_expression" => Some((
            field_text(left, "field", source)?,
            field_text(left, "receiver", source),
        )),
        _ => None,
    }
}

fn is_simple_value(value: Node<'_>) -> bool {
    matches!(
        value.kind(),
        "integer"
            | "float"
            | "string"
            | "verbatim_string"
            | "character"
            | "true"
            | "false"
            | "null"
            | "identifier"
            | "unary_expression"
    )
}

fn sole_string_argument(call: Node<'_>) -> Option<Node<'_>> {
    let arguments = call.child_by_field_name("arguments")?;
    if arguments.named_child_count() != 1 {
        return None;
    }

    arguments
        .named_child(0)
        .filter(|argument| matches!(argument.kind(), "string" | "verbatim_string"))
}

fn trim_quotes(literal: &str) -> &str {
    literal
        .strip_prefix('@')
        .unwrap_or(literal)
        .trim_matches('"')
}

fn start_line(node: Node<'_>) -> u32 {
    line(node.start_position().row)
}

fn end_line(node: Node<'_>) -> u32 {
    line(node.end_position().row)
}
