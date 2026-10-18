use kartta_lang::tree::{line, named_children, preorder, signature_from, text};
use kartta_lang::{
    Banner, CallRef, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId,
    SymbolKind,
};
use tree_sitter::{Node, Tree};

const IMPORT_FORM: u32 = 0;

const BANNER: Banner = Banner::new(&["@generated", "DO NOT EDIT", "Code generated"])
    .with_lines(&["//"])
    .with_block("/*", "*/");

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter::new(source);
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
    scopes: Vec<Scope>,
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

    fn handle(&mut self, node: Node<'_>) {
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
    }

    fn class(&mut self, node: Node<'_>) {
        if let Some(name) = class_name(node, self.source) {
            let mut symbol = self.declared(SymbolKind::Class, node, (None, name));
            symbol.parents = field_text(node, "base", self.source).into_iter().collect();
            self.push_symbol(symbol);
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
        self.push_symbol(symbol);
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
            self.push_symbol(symbol);
        }
    }

    fn declared_function(&mut self, node: Node<'_>) {
        let id = field_text(node, "name", self.source).map_or(NO_CALLER_ID, |name| {
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
        let id = binding.map_or(NO_CALLER_ID, |binding| self.bind_function(binding, node));
        self.open_scope(node, id);
    }

    fn enum_declaration(&mut self, node: Node<'_>) {
        let Some(name) = field_text(node, "name", self.source) else {
            return;
        };

        let symbol = self.declared(SymbolKind::Enum, node, (None, name.clone()));
        self.push_symbol(symbol);

        let members = named_children(node).filter_map(|i| node.named_child(i));
        for member in members {
            if let Some(member_name) = field_text(member, "name", self.source) {
                let owner = Some(name.clone());
                let symbol = self.declared(SymbolKind::EnumVariant, member, (owner, member_name));
                self.push_symbol(symbol);
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
        self.push_symbol(symbol)
    }

    fn open_scope(&mut self, callable: Node<'_>, id: SymbolId) {
        self.scopes.push(Scope {
            end: callable.end_byte(),
            id,
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

        let names = named_children(node)
            .filter_map(|i| node.named_child(i))
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

    fn push_symbol(&mut self, symbol: Symbol) -> SymbolId {
        let id = SymbolId(u32::try_from(self.raw.symbols.len()).unwrap_or(u32::MAX));
        self.raw.symbols.push(symbol);
        id
    }

    fn push_call(&mut self, callee: String, receiver: Option<String>, node: Node<'_>) {
        if callee.is_empty() || self.raw.symbols.is_empty() {
            return;
        }

        let caller_id = self.scopes.last().map_or(NO_CALLER_ID, |scope| scope.id);
        self.raw.calls.push(CallRef {
            caller_id,
            callee,
            receiver,
            line: start_line(node),
        });
    }
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
