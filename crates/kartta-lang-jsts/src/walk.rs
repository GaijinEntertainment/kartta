mod calls;
mod exports;

use kartta_lang::tree::{preorder, signature, signature_from, text};
use kartta_lang::{
    Callable, CallableRole, NO_CALLER_ID, RawExtraction, Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use crate::generated::is_generated_source;
use crate::syntax::{
    declarator_kind, end_line, function_value, is_function_type, is_member_exported, member_name,
    parents, start_line, trim_quotes, type_name,
};

pub(crate) fn walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter::new(source);

    preorder(tree, |node| {
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

struct Emitter<'s> {
    source: &'s [u8],
    scopes: Vec<Scope>,
    types: Vec<TypeScope>,
    decls: Vec<DeclScope>,
    exported_ids: (Option<usize>, Option<usize>),
    exported_names: Vec<String>,
    file_is_generated: bool,
    overloads: OverloadRun,
    symbol_count: u32,
    raw: RawExtraction,
}

impl<'s> Emitter<'s> {
    fn new(source: &'s [u8]) -> Self {
        Self {
            source,
            scopes: Vec::new(),
            types: Vec::new(),
            decls: Vec::new(),
            exported_ids: (None, None),
            exported_names: Vec::new(),
            file_is_generated: is_generated_source(source),
            overloads: OverloadRun::default(),
            symbol_count: 0,
            raw: RawExtraction::default(),
        }
    }

    fn handle(&mut self, node: Node<'_>) {
        self.close_scopes(node.start_byte());
        let caller = self.scopes.last().map_or(NO_CALLER_ID, |scope| scope.id);

        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                self.function_declaration(node);
            }
            "method_definition" => self.method_definition(node),
            "function_expression" | "generator_function" | "arrow_function" => {
                self.anonymous_function(node, caller);
            }
            "class_declaration" | "abstract_class_declaration" => {
                self.type_with_members(node, SymbolKind::Class);
            }
            "interface_declaration" => self.type_with_members(node, SymbolKind::Interface),
            "internal_module" | "module" => self.type_with_members(node, SymbolKind::Namespace),
            "class" => self.push_type_scope(node),
            "type_alias_declaration" => {
                self.type_declaration(node, SymbolKind::TypeAlias, Vec::new());
            }
            "enum_declaration" => self.type_declaration(node, SymbolKind::Enum, Vec::new()),
            "lexical_declaration" | "variable_declaration" => self.decls.push(DeclScope {
                end: node.end_byte(),
                kind: declarator_kind(node),
                exported: self.is_export_target(node),
            }),
            "variable_declarator" => self.declarator(node),
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
            "export_statement" => self.export_statement(node),
            "import_statement" => self.import_statement(node),
            _ => {}
        }
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
        self.push_symbol(symbol);
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

        let mut symbol = self.unowned(
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
        let id = self.push_symbol(symbol);

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
        let id = self.push_symbol(symbol);
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
        let id = self.push_symbol(symbol);

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
        self.push_symbol(symbol);
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
        Some(self.push_symbol(symbol))
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
        let owner = node.child_by_field_name("name").map(|name| {
            let name = type_name(name, self.source);
            if node.kind() == "class" {
                name
            } else {
                self.qualified(&name)
            }
        });
        self.types.push(TypeScope {
            end: node.end_byte(),
            owner,
        });
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
        let mut symbol = Symbol::new(name, kind, start_line(node), end_line(node));
        symbol.flags = SymbolFlags::empty().with_generated(self.file_is_generated);
        symbol
    }

    fn push_symbol(&mut self, symbol: Symbol) -> SymbolId {
        let id = SymbolId(self.symbol_count);
        self.symbol_count += 1;
        self.raw.symbols.push(symbol);
        id
    }
}
