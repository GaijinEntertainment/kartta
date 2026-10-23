mod calls;
mod exports;
mod uses;

use std::collections::HashMap;
use std::ops::Range;

use kartta_lang::tree::{preorder, signature, signature_from, text};
use kartta_lang::{
    Callable, CallableRole, FILE_SCOPE, RawExtraction, Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use self::uses::{annotated_type, constructed_type, names_a_value};
use crate::generated::is_generated_source;
use crate::syntax::{
    declaration_start_line, declarator_kind, end_line, function_value, is_function_type,
    is_member_exported, member_name, named_child_nodes, parents, trim_quotes, type_name,
};

pub(crate) fn walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter::new(source);
    emitter.hoist(tree.root_node());

    preorder(tree.root_node(), |node| {
        emitter.overloads.observe(node);
        if node.is_named() {
            emitter.handle(node);
        }
        true
    });

    // constraint: `export { helper }` may follow or precede the declaration it exports, so it applies once the walk has
    // seen every declaration
    let mut raw = emitter.raw;
    for symbol in &mut raw.symbols {
        if symbol.owner.is_none() && emitter.exported_names.contains(&symbol.name) {
            symbol.exported = true;
        }
    }
    raw
}

#[derive(Default)]
struct OverloadRun {
    signatures: Vec<SymbolId>,
    end: usize,
}

impl OverloadRun {
    fn covers(&self, node: Node<'_>) -> bool {
        !self.signatures.is_empty() && node.start_byte() < self.end
    }

    fn observe(&mut self, node: Node<'_>) {
        if self.signatures.is_empty() || self.covers(node) {
            return;
        }
        match node.kind() {
            "comment" | "decorator" => self.end = node.end_byte(),
            "export_statement"
            | "function_signature"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_signature"
            | "method_definition" => {}
            kind if node.is_named() || kind == "}" => self.signatures.clear(),
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
struct Scope {
    end: usize,
    id: SymbolId,
    function_start: usize,
    this_of: Option<SymbolId>,
}

impl Scope {
    fn spanning(
        span: Node<'_>,
        function: Node<'_>,
        id: SymbolId,
        this_of: Option<SymbolId>,
    ) -> Self {
        Self {
            end: span.end_byte(),
            id,
            function_start: function.start_byte(),
            this_of,
        }
    }
}

struct TypeScope {
    end: usize,
    owner: Option<String>,
}

struct DeclScope {
    end: usize,
    kind: SymbolKind,
    exported: bool,
}

type Span = (usize, usize);

/// The binding a name a declaration holds gets for the one type the declaration spells, if any.
type ValueBinding = fn(Option<Span>) -> Binding;

/// What a name the file declares stands for where a use spells it; a value carries the span of the one type its
/// declaration spells, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Binding {
    Value(Option<Span>),
    Module(Option<Span>),
    Imported,
    Type,
    /// A function or a class the module declares at its top level, which a name taken as a value uses.
    Declared,
}

// constraint: a binding lives until the innermost scope around its declaration ends — a function, a block, a catch
// clause, a loop, a `switch` — so bindings close in the order they opened and each restores the one it shadowed; a
// `var` is held to its block too, so a call after the block keeps the name, a miss rather than a wrong empty receiver;
// a binding of the module's top level never closes, so nothing records it
struct Declared {
    name: Range<usize>,
    until: usize,
    shadowed: Option<Binding>,
}

struct Emitter<'s> {
    source: &'s [u8],
    scopes: Vec<Scope>,
    types: Vec<TypeScope>,
    decls: Vec<DeclScope>,
    scope_ends: Vec<usize>,
    function_ends: Vec<usize>,
    declared: Vec<Declared>,
    bindings: HashMap<&'s [u8], Binding>,
    /// What each name the module declares or imports at its top level stands for, wherever in the file a use spells it.
    module_bindings: HashMap<&'s [u8], Binding>,
    exported_ids: (Option<usize>, Option<usize>),
    exported_names: Vec<String>,
    file_is_generated: bool,
    overloads: OverloadRun,
    raw: RawExtraction,
}

impl<'s> Emitter<'s> {
    fn new(source: &'s [u8]) -> Self {
        Self {
            source,
            scopes: Vec::new(),
            types: Vec::new(),
            decls: Vec::new(),
            scope_ends: Vec::new(),
            function_ends: Vec::new(),
            declared: Vec::new(),
            bindings: HashMap::new(),
            module_bindings: HashMap::new(),
            exported_ids: (None, None),
            exported_names: Vec::new(),
            file_is_generated: is_generated_source(source),
            overloads: OverloadRun::default(),
            raw: RawExtraction::default(),
        }
    }

    fn handle(&mut self, node: Node<'_>) {
        self.close_scopes(node.start_byte());
        let caller = self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id);

        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                self.declare_function(node);
                self.function_declaration(node);
            }
            "method_definition" => {
                self.declare_parameters(node);
                self.method_definition(node);
            }
            "function_expression" | "generator_function" | "arrow_function" => {
                self.declare_parameters(node);
                self.anonymous_function(node, caller);
            }
            "class_declaration" | "abstract_class_declaration" => self.class_declaration(node),
            "class_static_block" => self.push_type_barrier(node),
            "statement_block" | "switch_body" | "for_statement" | "catch_clause"
            | "for_in_statement" => {
                self.open_scope(node);
            }
            "interface_declaration" => self.type_with_members(node, SymbolKind::Interface),
            "internal_module" | "module" => self.type_with_members(node, SymbolKind::Namespace),
            "class" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.declare(name.byte_range(), node.end_byte(), Binding::Type);
                }
                self.push_type_scope(node);
            }
            "type_alias_declaration" => {
                self.type_declaration(node, SymbolKind::TypeAlias, Vec::new());
                self.push_type_scope(node);
            }
            "enum_declaration" => self.type_declaration(node, SymbolKind::Enum, Vec::new()),
            "lexical_declaration" | "variable_declaration" => self.decls.push(DeclScope {
                end: node.end_byte(),
                kind: declarator_kind(node),
                exported: self.is_export_target(node),
            }),
            "variable_declarator" => {
                self.declare_declarator(node);
                self.declarator(node);
            }
            "pair" => self.pair_function(node),
            "field_definition" | "public_field_definition" => self.class_field(node),
            "method_signature" => {
                let id = self.named_symbol(
                    node,
                    SymbolKind::Method,
                    is_member_exported(node, self.source),
                );
                self.overload_member(node, id, CallableRole::Declaration);
            }
            "abstract_method_signature" => {
                self.named_symbol(
                    node,
                    SymbolKind::Method,
                    is_member_exported(node, self.source),
                );
            }
            "property_signature" => self.property_signature(node),
            "function_signature" => {
                let id = self.named_symbol(node, SymbolKind::Function, self.is_export_target(node));
                self.overload_member(node, id, CallableRole::Declaration);
            }
            "jsx_opening_element" | "jsx_self_closing_element" => self.jsx_call(node, caller),
            "new_expression" => self.new_call(node, caller),
            "call_expression" => self.call(node, caller),
            "member_expression" => self.member_use(node, caller),
            "object" => self.literal_keys(node, caller),
            "export_statement" => self.export_statement(node),
            "import_statement" => self.import_statement(node),
            kind if names_a_value(kind) => self.value_use(node, caller),
            _ => {}
        }
    }

    // constraint: a body runs once the module has run its top level, so it reads a value the module declares or
    // imports after the body as well as before it
    fn hoist(&mut self, program: Node<'_>) {
        let declarations = named_child_nodes(program).filter_map(|statement| {
            if statement.kind() == "export_statement" {
                statement.child_by_field_name("declaration")
            } else {
                Some(statement)
            }
        });
        for declaration in declarations {
            match declaration.kind() {
                "lexical_declaration" | "variable_declaration" => named_child_nodes(declaration)
                    .for_each(|declarator| self.declare_declarator(declarator)),
                "import_statement" => self.declare_imported(declaration),
                "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "abstract_class_declaration" => self.declare_module_name(declaration),
                _ => {}
            }
        }
        self.module_bindings = std::mem::take(&mut self.bindings);
    }

    fn close_scopes(&mut self, start: usize) {
        while self.scopes.last().is_some_and(|scope| scope.end <= start) {
            self.scopes.pop();
        }
        while self.types.last().is_some_and(|scope| scope.end <= start) {
            self.types.pop();
        }
        while self.decls.last().is_some_and(|scope| scope.end <= start) {
            self.decls.pop();
        }
        while self.scope_ends.last().is_some_and(|&end| end <= start) {
            self.scope_ends.pop();
        }
        while self.function_ends.last().is_some_and(|&end| end <= start) {
            self.function_ends.pop();
        }
        while let Some(closed) = self.declared.pop_if(|declared| declared.until <= start) {
            let name = &self.source[closed.name];
            match closed.shadowed {
                Some(binding) => self.bindings.insert(name, binding),
                None => self.bindings.remove(name),
            };
        }
    }

    fn scope_end(&self) -> usize {
        self.scope_ends.last().copied().unwrap_or(usize::MAX)
    }

    // constraint: a catch clause binds its parameter and a `for … in` or `for … of` with a `let`, `const` or `var`
    // binds its left side, each for the statement alone
    fn open_scope(&mut self, node: Node<'_>) {
        self.scope_ends.push(node.end_byte());
        let bound = match node.kind() {
            "catch_clause" => node.child_by_field_name("parameter"),
            "for_in_statement" if node.child_by_field_name("kind").is_some() => {
                node.child_by_field_name("left")
            }
            _ => None,
        };
        if let Some(bound) = bound {
            self.declare_pattern(bound, node.end_byte(), Binding::Value);
        }
    }

    fn declare(&mut self, name: Range<usize>, until: usize, binding: Binding) {
        let binding = match binding {
            Binding::Value(typed) if until == usize::MAX => Binding::Module(typed),
            binding => binding,
        };
        let source = self.source;
        let shadowed = self.bindings.insert(&source[name.clone()], binding);
        if until != usize::MAX {
            self.declared.push(Declared {
                name,
                until,
                shadowed,
            });
        }
    }

    fn declare_module_name(&mut self, declaration: Node<'_>) {
        if let Some(name) = declaration.child_by_field_name("name") {
            self.declare(name.byte_range(), usize::MAX, Binding::Declared);
        }
    }

    // constraint: a class at the module's top level keeps the binding the hoist gave it, which a value taken of the
    // name uses; one in a nearer scope is a type binding of that scope
    fn class_declaration(&mut self, node: Node<'_>) {
        if let Some(name) = node.child_by_field_name("name")
            && self.scope_end() != usize::MAX
        {
            self.declare(name.byte_range(), self.scope_end(), Binding::Type);
        }
        self.type_with_members(node, SymbolKind::Class);
    }

    // constraint: a function declared inside a function body names a value of its scope, while one at module or
    // namespace level keeps its name, because TypeScript merges it with a namespace of that name
    fn declare_function(&mut self, function: Node<'_>) {
        if !self.function_ends.is_empty()
            && let Some(name) = function.child_by_field_name("name")
        {
            self.declare(name.byte_range(), self.scope_end(), Binding::Value(None));
        }
        self.declare_parameters(function);
    }

    // constraint: the name of a named function expression is a value of its own body
    fn declare_parameters(&mut self, function: Node<'_>) {
        if matches!(
            function.kind(),
            "function_expression" | "generator_function"
        ) && let Some(name) = function.child_by_field_name("name")
        {
            self.declare(name.byte_range(), function.end_byte(), Binding::Value(None));
        }
        self.function_ends.push(function.end_byte());
        self.scope_ends.push(function.end_byte());
        for field in ["parameter", "parameters"] {
            if let Some(parameters) = function.child_by_field_name(field) {
                self.declare_pattern(parameters, function.end_byte(), Binding::Value);
            }
        }
    }

    // constraint: a declarator whose value is a class expression binds the class, whose static members a call on the
    // name reaches, so it is a type binding that shadows a value of that name
    fn declare_declarator(&mut self, declarator: Node<'_>) {
        let Some(name) = declarator.child_by_field_name("name") else {
            return;
        };
        let binds_a_class = declarator
            .child_by_field_name("value")
            .is_some_and(|value| value.kind() == "class");
        if binds_a_class && name.kind() == "identifier" {
            self.declare(name.byte_range(), self.scope_end(), Binding::Type);
        } else if name.kind() == "identifier" {
            let typed = declarator
                .child_by_field_name("type")
                .and_then(annotated_type)
                .or_else(|| constructed_type(declarator));
            self.declare(name.byte_range(), self.scope_end(), Binding::Value(typed));
        } else {
            self.declare_pattern(name, self.scope_end(), Binding::Value);
        }
    }

    // constraint: a pattern binds its identifiers and its shorthand properties; a property key, a default value and a
    // type annotation bind nothing, though a parameter's name keeps the one type its annotation spells
    fn declare_pattern(&mut self, pattern: Node<'_>, until: usize, value: ValueBinding) {
        let inner = |field| pattern.child_by_field_name(field);
        match pattern.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => {
                self.declare(pattern.byte_range(), until, value(None));
            }
            "required_parameter" | "optional_parameter" => match inner("pattern") {
                Some(bound) if bound.kind() == "identifier" => {
                    let typed = inner("type").and_then(annotated_type);
                    self.declare(bound.byte_range(), until, value(typed));
                }
                Some(bound) => self.declare_pattern(bound, until, value),
                None => {}
            },
            "assignment_pattern" | "object_assignment_pattern" => {
                if let Some(bound) = inner("left") {
                    self.declare_pattern(bound, until, value);
                }
            }
            "pair_pattern" => {
                if let Some(bound) = inner("value") {
                    self.declare_pattern(bound, until, value);
                }
            }
            "formal_parameters" | "object_pattern" | "array_pattern" | "rest_pattern" => {
                for bound in named_child_nodes(pattern) {
                    self.declare_pattern(bound, until, value);
                }
            }
            _ => {}
        }
    }

    fn names_a_declared_value(&self, receiver: &str) -> bool {
        matches!(
            self.bindings.get(receiver.as_bytes()),
            Some(Binding::Value(_) | Binding::Module(_))
        )
    }

    fn function_declaration(&mut self, node: Node<'_>) {
        let id = self.named_symbol(node, SymbolKind::Function, self.is_export_target(node));
        self.overload_member(node, id, CallableRole::Definition);
        if let Some(id) = id {
            self.scopes.push(Scope::spanning(node, node, id, None));
        }
        self.push_type_barrier(node);
    }

    fn method_definition(&mut self, node: Node<'_>) {
        let (kind, exported) = if self.enclosing_type().is_some() {
            (SymbolKind::Method, is_member_exported(node, self.source))
        } else {
            (SymbolKind::Function, false)
        };
        let id = self.named_symbol(node, kind, exported);
        self.overload_member(node, id, CallableRole::Definition);
        if let Some(id) = id {
            self.scopes.push(Scope::spanning(node, node, id, Some(id)));
        }
        self.push_type_barrier(node);
    }

    fn anonymous_function(&mut self, node: Node<'_>, caller: SymbolId) {
        let bound =
            (self.scopes.last()).is_some_and(|scope| scope.function_start == node.start_byte());
        if node.kind() != "arrow_function" && !bound {
            self.scopes.push(Scope::spanning(node, node, caller, None));
        }
        self.push_type_barrier(node);
    }

    fn type_with_members(&mut self, node: Node<'_>, kind: SymbolKind) {
        self.type_declaration(node, kind, parents(node, self.source));
        self.push_type_scope(node);
    }

    fn type_declaration(&mut self, node: Node<'_>, kind: SymbolKind, parents: Vec<String>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };

        let mut symbol = self.member(type_name(name, self.source), kind, node);
        symbol.parents = parents;
        symbol.exported = self.is_export_target(node);
        self.raw.push_symbol(symbol);
    }

    fn declarator(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        if name.kind() != "identifier" {
            return;
        }

        let (exported, kind) = (self.decls.last()).map_or((false, SymbolKind::Variable), |decl| {
            (decl.exported, decl.kind)
        });
        let name = text(name, self.source);
        let value = node.child_by_field_name("value");
        let function = value.and_then(|value| function_value(value, self.source));

        let mut symbol = self.member(
            name,
            if function.is_some() {
                SymbolKind::Function
            } else {
                kind
            },
            node,
        );
        symbol.exported = exported;
        symbol.signature =
            function.and_then(|function| signature_from(node.start_byte(), function, self.source));
        let id = self.raw.push_symbol(symbol);

        if let (Some(value), Some(function)) = (value, function) {
            let this_of = self.lexical_this(function);
            self.scopes
                .push(Scope::spanning(value, function, id, this_of));
        }
    }

    fn pair_function(&mut self, node: Node<'_>) {
        let Some(value) = node.child_by_field_name("value") else {
            return;
        };
        let Some(function) = function_value(value, self.source) else {
            return;
        };
        let Some(key) = node.child_by_field_name("key") else {
            return;
        };
        let name = match key.kind() {
            "property_identifier" | "identifier" => text(key, self.source),
            "string" => trim_quotes(&text(key, self.source)).to_owned(),
            _ => return,
        };
        if name.is_empty() {
            return;
        }

        let mut symbol = self.unowned(name, SymbolKind::Function, node);
        symbol.signature = signature_from(node.start_byte(), function, self.source);
        let id = self.raw.push_symbol(symbol);
        let this_of = self.lexical_this(function);
        self.scopes
            .push(Scope::spanning(value, function, id, this_of));
    }

    fn class_field(&mut self, node: Node<'_>) {
        let Some(name) = member_name(node) else {
            return;
        };
        let name = text(name, self.source);
        if name.is_empty() {
            return;
        }

        let value = node.child_by_field_name("value");
        let function = value.and_then(|value| function_value(value, self.source));
        let kind = if function.is_some() {
            SymbolKind::Method
        } else {
            SymbolKind::Field
        };

        let mut symbol = self.member(name, kind, node);
        symbol.exported = is_member_exported(node, self.source);
        symbol.signature =
            function.and_then(|function| signature_from(node.start_byte(), function, self.source));
        let id = self.raw.push_symbol(symbol);

        if let (Some(value), Some(function)) = (value, function) {
            self.scopes
                .push(Scope::spanning(value, function, id, Some(id)));
        }
    }

    fn property_signature(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let is_callable = node
            .child_by_field_name("type")
            .is_some_and(is_function_type);
        let kind = if is_callable {
            SymbolKind::Method
        } else {
            SymbolKind::Field
        };

        let mut symbol = self.member(text(name, self.source), kind, node);
        symbol.exported = is_member_exported(node, self.source);
        symbol.signature = signature(node, self.source);
        self.raw.push_symbol(symbol);
    }

    fn named_symbol(
        &mut self,
        node: Node<'_>,
        kind: SymbolKind,
        exported: bool,
    ) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);

        let mut symbol = self.member(name, kind, node);
        symbol.exported = exported;
        symbol.signature = signature(node, self.source);
        Some(self.raw.push_symbol(symbol))
    }

    fn overload_member(&mut self, node: Node<'_>, symbol: Option<SymbolId>, role: CallableRole) {
        if self.overloads.covers(node) {
            return;
        }
        let name = |id: SymbolId| self.raw.symbols[id.0 as usize].name.as_str();
        let first = self.overloads.signatures.first().copied();
        let continues =
            (symbol.zip(first)).is_some_and(|(member, first)| name(member) == name(first));
        if !continues {
            self.overloads.signatures.clear();
        }
        let Some(symbol) = symbol else {
            return;
        };

        match role {
            CallableRole::Declaration => {
                self.overloads.signatures.push(symbol);
                self.overloads.end = node.end_byte();
            }
            CallableRole::Definition if continues => {
                let key = symbol.0.to_string();
                let declarations =
                    (self.overloads.signatures.drain(..)).map(|declaration| Callable {
                        symbol: declaration,
                        key: key.clone(),
                        role: CallableRole::Declaration,
                    });
                self.raw.callables.extend(declarations);
                self.raw.callables.push(Callable { symbol, key, role });
            }
            CallableRole::Definition => {}
        }
    }

    fn lexical_this(&self, function: Node<'_>) -> Option<SymbolId> {
        (function.kind() == "arrow_function")
            .then(|| self.scopes.last()?.this_of)
            .flatten()
    }

    fn push_type_scope(&mut self, node: Node<'_>) {
        let owner = node.child_by_field_name("name").map_or_else(
            || self.anonymous_class_name(node),
            |name| {
                let name = type_name(name, self.source);
                Some(if node.kind() == "class" {
                    name
                } else {
                    self.qualified(&name)
                })
            },
        );
        self.types.push(TypeScope {
            end: node.end_byte(),
            owner,
        });
    }

    fn anonymous_class_name(&self, class: Node<'_>) -> Option<String> {
        if self.is_export_target(class) {
            return Some("default".to_owned());
        }
        let declarator = class
            .parent()
            .filter(|parent| parent.kind() == "variable_declarator")?;
        let name = declarator.child_by_field_name("name")?;
        (name.kind() == "identifier" && declarator.child_by_field_name("value") == Some(class))
            .then(|| self.qualified(&text(name, self.source)))
    }

    fn push_type_barrier(&mut self, node: Node<'_>) {
        self.types.push(TypeScope {
            end: node.end_byte(),
            owner: None,
        });
    }

    fn enclosing_type(&self) -> Option<&str> {
        self.types.last().and_then(|scope| scope.owner.as_deref())
    }

    fn qualified(&self, name: &str) -> String {
        self.enclosing_type()
            .map_or_else(|| name.to_owned(), |owner| format!("{owner}.{name}"))
    }

    fn is_export_target(&self, node: Node<'_>) -> bool {
        let id = Some(node.id());
        self.exported_ids.0 == id || self.exported_ids.1 == id
    }

    fn member(&self, name: String, kind: SymbolKind, node: Node<'_>) -> Symbol {
        let mut symbol = self.unowned(name, kind, node);
        if let Some(owner) = self.enclosing_type() {
            symbol.qualified_name = format!("{owner}.{}", symbol.name);
            symbol.owner = Some(owner.to_owned());
        }
        symbol
    }

    fn unowned(&self, name: String, kind: SymbolKind, node: Node<'_>) -> Symbol {
        let mut symbol = Symbol::new(name, kind, declaration_start_line(node), end_line(node));
        symbol.flags = SymbolFlags::empty().with_generated(self.file_is_generated);
        symbol
    }
}
