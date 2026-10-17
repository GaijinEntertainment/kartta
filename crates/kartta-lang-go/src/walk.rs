use std::collections::{HashSet, VecDeque};

use kartta_lang::tree::{children, named_children, preorder, signature, text};
use kartta_lang::{
    ImplementsAssertion, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId,
    SymbolKind,
};
use tree_sitter::{Node, Tree};

use self::locals::{Block, Local, LocalKind, declares_left, header_statement};
use self::syntax::{
    assertion_value_type, default_name_candidates, is_exported, is_generated_source, line,
    parents_of, receiver_type, trim_quotes,
};
use self::types::{element_of, is_container_type};

mod calls;
mod locals;
mod syntax;
mod types;
mod uses;

pub(crate) const IMPORT_FORM: u32 = 0;

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let package = package_name(tree, source);
    let mut emitter = Emitter::new(source, package, package_values(tree, source));

    preorder(tree, |node| !node.is_named() || emitter.handle(node));

    emitter.apply_pending_parents();
    emitter.raw
}

fn package_name(tree: &Tree, source: &[u8]) -> String {
    let root = tree.root_node();

    children(root)
        .filter_map(|i| root.child(i))
        .find(|child| child.kind() == "package_clause")
        .and_then(|clause| clause.child_by_field_name("name"))
        .map(|name| text(name, source))
        .unwrap_or_default()
}

// constraint: a package-level name is in scope above its declaration, so the walk collects the file's package-level
// `var` and `const` names before it starts
fn package_values<'a>(tree: &Tree, source: &'a [u8]) -> HashSet<&'a str> {
    let mut names = HashSet::new();
    let root = tree.root_node();
    let specs = children(root)
        .filter_map(|i| root.child(i))
        .filter(|declaration| matches!(declaration.kind(), "var_declaration" | "const_declaration"))
        .flat_map(|declaration| children(declaration).filter_map(move |j| declaration.child(j)));
    for spec in specs {
        let mut cursor = spec.walk();
        let spec_names = spec
            .children_by_field_name("name", &mut cursor)
            .filter_map(|name| name.utf8_text(source).ok())
            .filter(|&name| name != "_");
        names.extend(spec_names);
    }

    names
}

#[derive(Clone, Copy)]
struct Scope {
    end: usize,
    id: SymbolId,
}

struct Emitter<'a> {
    source: &'a [u8],
    package: String,
    callers: Vec<Scope>,
    use_owners: Vec<Scope>,
    type_params: Vec<(usize, String)>,
    locals: Vec<Local>,
    package_values: HashSet<&'a str>,
    blocks: Vec<Block>,
    spec_owners: VecDeque<(usize, SymbolId)>,
    named_by_call: Option<usize>,
    import_names: Vec<String>,
    pending_parents: Vec<(String, String)>,
    file_is_generated: bool,
    symbol_count: u32,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn new(source: &'a [u8], package: String, package_values: HashSet<&'a str>) -> Self {
        let mut raw = RawExtraction::default();
        raw.package = (!package.is_empty()).then(|| package.clone());

        Self {
            source,
            package,
            callers: Vec::new(),
            use_owners: Vec::new(),
            type_params: Vec::new(),
            locals: Vec::new(),
            package_values,
            blocks: Vec::new(),
            spec_owners: VecDeque::new(),
            named_by_call: None,
            import_names: Vec::new(),
            pending_parents: Vec::new(),
            file_is_generated: is_generated_source(source),
            symbol_count: 0,
            raw,
        }
    }

    // constraint: an assertion may precede the type it names, so the pairs wait for the end of the walk; a pair whose
    // type the file does not declare goes out as an assertion, for the graph to apply where it sees the whole package
    fn apply_pending_parents(&mut self) {
        for (type_name, iface) in std::mem::take(&mut self.pending_parents) {
            let declared = self.raw.symbols.iter_mut().find(|symbol| {
                symbol.name == type_name
                    && matches!(
                        symbol.kind,
                        SymbolKind::Struct | SymbolKind::Interface | SymbolKind::TypeAlias
                    )
            });
            match declared {
                Some(symbol) if symbol.parents.contains(&iface) => {}
                Some(symbol) => symbol.parents.push(iface),
                None => self
                    .raw
                    .assertions
                    .push(ImplementsAssertion { type_name, iface }),
            }
        }
    }

    fn handle(&mut self, node: Node<'_>) -> bool {
        self.close_scopes(node.start_byte());
        let caller = self.callers.last().map_or(NO_CALLER_ID, |scope| scope.id);

        match node.kind() {
            "function_declaration" => self.enter_function(node),
            "method_declaration" => self.enter_method(node),
            "function_literal" => {
                self.push_params(node, "parameters", LocalKind::Value);
                self.push_params(node, "result", LocalKind::Value);
            }
            "type_switch_statement" => self.enter_type_switch(node),
            "block" | "expression_case_clause" | "communication_clause" => {
                self.blocks.push(Block::of(node));
            }
            "type_case_clause" => {
                self.blocks.push(Block::of(node));
                self.enter_type_case(node);
            }
            "short_var_declaration" | "range_clause" | "receive_statement" => {
                self.declare_short(node);
            }
            "type_declaration" => {
                self.type_declaration(node);
                return false;
            }
            "const_declaration" | "var_declaration" => self.var_const(node),
            "const_spec" | "var_spec" => self.enter_spec(node),
            "call_expression" => self.call(node, caller),
            "type_identifier" | "qualified_identifier" | "selector_expression" => {
                let owner = self.use_owner();
                return self.record_use(node, owner);
            }
            "import_declaration" => {
                self.import_declaration(node);
                return false;
            }
            _ => {}
        }

        true
    }

    // constraint: a node is inside a scope while it starts before the scope's end, which is exclusive
    fn close_scopes(&mut self, start: usize) {
        while self.callers.last().is_some_and(|scope| scope.end <= start) {
            self.callers.pop();
        }
        while self
            .use_owners
            .last()
            .is_some_and(|scope| scope.end <= start)
        {
            self.use_owners.pop();
        }
        while self
            .type_params
            .last()
            .is_some_and(|&(end, _)| end <= start)
        {
            self.type_params.pop();
        }
        while self.blocks.last().is_some_and(|block| block.end <= start) {
            self.blocks.pop();
        }
    }

    fn enter_function(&mut self, node: Node<'_>) {
        self.push_type_params(node.child_by_field_name("type_parameters"), node.end_byte());
        if let Some(id) = self.function(node) {
            self.enter_body(node, id);
        }
        self.locals.clear();
        self.push_params(node, "parameters", LocalKind::Value);
        self.push_params(node, "result", LocalKind::Value);
    }

    fn enter_method(&mut self, node: Node<'_>) {
        self.push_receiver_type_params(node);
        self.push_type_params(node.child_by_field_name("type_parameters"), node.end_byte());
        let method = self.method(node);
        if let Some(id) = method {
            self.enter_body(node, id);
        }
        self.locals.clear();
        let receiver = method.map_or(LocalKind::Value, LocalKind::ReceiverOf);
        self.push_params(node, "receiver", receiver);
        self.push_params(node, "parameters", LocalKind::Value);
        self.push_params(node, "result", LocalKind::Value);
    }

    fn enter_body(&mut self, node: Node<'_>, id: SymbolId) {
        let scope = Scope {
            end: node.end_byte(),
            id,
        };
        self.callers.push(scope);
        self.use_owners.push(scope);
    }

    // constraint: outside a clause that names one type, the switch's variable holds the guard's value
    fn enter_type_switch(&mut self, node: Node<'_>) {
        if let Some(name) = node.child_by_field_name("name") {
            let guard = node.child_by_field_name("operand");
            let from = guard.map_or_else(|| name.end_byte(), |guard| guard.end_byte());
            self.locals.push(Local {
                from,
                block: Block::of(node),
                name: text(name, self.source),
                kind: LocalKind::Value,
                type_path: guard.and_then(|guard| self.value_path(guard)),
            });
        }
    }

    // constraint: in a clause that names one type, the switch's variable holds a value of that type
    fn enter_type_case(&mut self, clause: Node<'_>) {
        let Some(name) = clause
            .parent()
            .and_then(|switch| switch.child_by_field_name("name"))
        else {
            return;
        };
        let mut cursor = clause.walk();
        let mut types = clause.children_by_field_name("type", &mut cursor);
        if let (Some(single), None) = (types.next(), types.next())
            && let Some(type_path) = self.type_path(single)
        {
            self.locals.push(Local {
                from: clause.start_byte(),
                block: Block::of(clause),
                name: text(name, self.source),
                kind: LocalKind::Value,
                type_path: Some(type_path),
            });
        }
    }

    fn declare_short(&mut self, node: Node<'_>) {
        if let Some(left) = node.child_by_field_name("left")
            && declares_left(node)
            && let Some(block) = header_statement(node)
                .map(Block::of)
                .or_else(|| self.blocks.last().copied())
        {
            let mut paths = self.declared_paths(node, left);
            self.push_locals(left, (node.end_byte(), block), &mut |position| {
                paths.get_mut(position).and_then(Option::take)
            });
        }
    }

    // constraint: a range yields an index or a key first, which no path names, and an element second
    fn declared_paths(&self, node: Node<'_>, left: Node<'_>) -> Vec<Option<String>> {
        let names = named_children(left).len();
        let right = node.child_by_field_name("right");
        if node.kind() == "range_clause" {
            let element = right
                .and_then(|ranged| self.value_path(ranged))
                .and_then(|ranged| element_of(&ranged));
            return vec![None, element];
        }
        let values: Vec<Node<'_>> = match right {
            Some(list) if list.kind() == "expression_list" => named_children(list)
                .filter_map(|i| list.named_child(i))
                .collect(),
            Some(value) => vec![value],
            None => Vec::new(),
        };
        (0..names)
            .map(|position| self.value_path_at(&values, names, position))
            .collect()
    }

    fn enter_spec(&mut self, node: Node<'_>) {
        if self
            .spec_owners
            .front()
            .is_some_and(|&(start, _)| start == node.start_byte())
            && let Some((_, id)) = self.spec_owners.pop_front()
        {
            let scope = Scope {
                end: node.end_byte(),
                id,
            };
            self.use_owners.push(scope);
            self.callers.push(scope);
            self.push_value_callers(node, id);
        }
    }

    // constraint: a spec's values follow one another in the source, so their scopes, stacked last to first, close in
    // turn; a blank name's value, and every value of a list that does not line up with the names, calls as the first
    fn push_value_callers(&mut self, spec: Node<'_>, first: SymbolId) {
        let Some(list) = spec.child_by_field_name("value") else {
            return;
        };
        let mut cursor = spec.walk();
        let names = spec.children_by_field_name("name", &mut cursor).count();
        if names < 2 || names != named_children(list).len() {
            return;
        }
        let stacked = self.callers.len();
        let mut next = first.0;
        let names = spec.children_by_field_name("name", &mut cursor);
        for (name, i) in names.zip(named_children(list)) {
            let id = if name.utf8_text(self.source) == Ok("_") {
                first
            } else {
                let id = SymbolId(next);
                next += 1;
                id
            };
            if let Some(value) = list.named_child(i) {
                self.callers.push(Scope {
                    end: value.end_byte(),
                    id,
                });
            }
        }
        if let Some(values) = self.callers.get_mut(stacked..) {
            values.reverse();
        }
    }

    fn import_declaration(&mut self, node: Node<'_>) {
        for spec in named_children(node).filter_map(|i| node.named_child(i)) {
            if spec.kind() == "import_spec" {
                self.import_spec(spec);
            }
        }
    }

    fn import_spec(&mut self, spec: Node<'_>) {
        let Some(path_node) = spec.child_by_field_name("path") else {
            return;
        };
        let path = trim_quotes(text(path_node, self.source));
        if path.is_empty() {
            return;
        }

        let alias = spec
            .child_by_field_name("name")
            .map(|name| match name.kind() {
                "dot" => ".".to_owned(),
                "blank_identifier" => "_".to_owned(),
                _ => text(name, self.source),
            })
            .filter(|alias| !alias.is_empty());

        match alias.as_deref() {
            Some("." | "_") => {}
            Some(alias) => self.import_names.push(alias.to_owned()),
            None => self.import_names.extend(default_name_candidates(&path)),
        }

        self.raw.directives.push(RawDirective {
            form: IMPORT_FORM,
            spec: path,
            names: alias.into_iter().collect(),
            line: line(spec.start_position().row),
        });
    }

    fn push_symbol(&mut self, symbol: Symbol) -> SymbolId {
        let id = SymbolId(self.symbol_count);
        self.symbol_count += 1;
        self.raw.symbols.push(symbol);
        if !self.blocks.is_empty() {
            self.raw.local_symbols.push(id);
        }

        id
    }

    // constraint: `Symbol::new` copies its name into the qualified name, so the symbol starts unnamed, which allocates
    // nothing, and then takes both names
    fn declared(
        &self,
        kind: SymbolKind,
        node: Node<'_>,
        (owner, name): (Option<String>, String),
    ) -> Symbol {
        let start = line(node.start_position().row);
        let mut symbol = Symbol::new(String::new(), kind, start, line(node.end_position().row));
        symbol.qualified_name = owner.as_ref().map_or_else(
            || format!("{}.{name}", self.package),
            |owner| format!("{}.{owner}.{name}", self.package),
        );
        symbol.exported = is_exported(&name);
        symbol.name = name;
        symbol.owner = owner;
        symbol.flags = SymbolFlags::empty().with_generated(self.file_is_generated);

        symbol
    }

    fn function(&mut self, node: Node<'_>) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let is_entry = name == "main" && self.package == "main";

        let mut symbol = self.declared(SymbolKind::Function, node, (None, name));
        symbol.signature = signature(node, self.source);
        symbol.flags = symbol.flags.with_entry(is_entry);
        symbol.value_types = self.result_paths(node);
        Some(self.push_symbol(symbol))
    }

    fn method(&mut self, node: Node<'_>) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let owner = node
            .child_by_field_name("receiver")
            .map(|receiver| receiver_type(receiver, self.source))
            .filter(|owner| !owner.is_empty());

        let mut symbol = self.declared(SymbolKind::Method, node, (owner, name));
        symbol.signature = signature(node, self.source);
        symbol.value_types = self.result_paths(node);
        Some(self.push_symbol(symbol))
    }

    fn type_declaration(&mut self, node: Node<'_>) {
        let specs = children(node)
            .filter_map(|i| node.child(i))
            .filter(|spec| matches!(spec.kind(), "type_definition" | "alias_declaration"));
        for spec in specs {
            let start = spec.start_byte();
            self.type_params.retain(|&(end, _)| end > start);
            if let Some(&block) = self.blocks.last()
                && let Some(name) = spec.child_by_field_name("name")
            {
                self.locals.push(Local {
                    from: start,
                    block,
                    name: text(name, self.source),
                    kind: LocalKind::TypeSymbol(SymbolId(self.symbol_count)),
                    type_path: None,
                });
            }
            self.type_spec(spec);
        }
    }

    // constraint: the walk never descends a type declaration, so the type parameters of a spec and every use inside
    // its type are recorded here, attributed to the member that holds them or else to the type
    fn type_spec(&mut self, spec: Node<'_>) {
        let type_node = spec.child_by_field_name("type");
        let (kind, parents) = match type_node {
            Some(node) if node.kind() == "struct_type" => {
                (SymbolKind::Struct, parents_of(node, self.source))
            }
            Some(node) if node.kind() == "interface_type" => {
                (SymbolKind::Interface, parents_of(node, self.source))
            }
            _ => (SymbolKind::TypeAlias, Vec::new()),
        };
        let type_params = spec.child_by_field_name("type_parameters");
        self.push_type_params(type_params, spec.end_byte());
        let value_types = match type_node {
            Some(written) if written.kind() == "function_type" => self.result_paths(written),
            Some(written) if spec.kind() == "alias_declaration" || is_container_type(written) => {
                self.type_path(written).into_iter().collect()
            }
            _ => Vec::new(),
        };
        let Some((owner, type_id)) = self.named_type(spec, kind, (parents, value_types)) else {
            return;
        };

        if let Some(list) = type_params {
            self.record_uses_in(list, type_id);
        }

        let Some(type_node) = type_node else {
            return;
        };
        match kind {
            SymbolKind::Interface => self.interface_methods(type_node, &owner, type_id),
            SymbolKind::Struct => self.struct_fields(type_node, &owner, type_id),
            _ => self.record_uses_in(type_node, type_id),
        }
    }

    fn named_type(
        &mut self,
        spec: Node<'_>,
        kind: SymbolKind,
        (parents, value_types): (Vec<String>, Vec<String>),
    ) -> Option<(String, SymbolId)> {
        let name = text(spec.child_by_field_name("name")?, self.source);

        let mut symbol = self.declared(kind, spec, (None, name.clone()));
        symbol.parents = parents;
        symbol.value_types = value_types;
        Some((name, self.push_symbol(symbol)))
    }

    fn interface_methods(&mut self, interface_type: Node<'_>, owner: &str, type_id: SymbolId) {
        for element in children(interface_type).filter_map(|i| interface_type.child(i)) {
            if element.kind() != "method_elem" {
                self.record_uses_in(element, type_id);
                continue;
            }
            let Some(name_node) = element.child_by_field_name("name") else {
                continue;
            };

            self.record_uses_in(element, SymbolId(self.symbol_count));
            let name = text(name_node, self.source);
            let mut symbol =
                self.declared(SymbolKind::Method, element, (Some(owner.to_owned()), name));
            symbol.signature = signature(element, self.source);
            symbol.value_types = self.result_paths(element);
            self.push_symbol(symbol);
        }
    }

    fn struct_fields(&mut self, struct_type: Node<'_>, owner: &str, type_id: SymbolId) {
        let fields = children(struct_type)
            .filter_map(|i| struct_type.child(i))
            .filter(|field| field.kind() == "field_declaration");
        for field in fields {
            let first_field = SymbolId(self.symbol_count);
            self.field_names(field, owner);
            let site_of = if self.symbol_count > first_field.0 {
                first_field
            } else {
                type_id
            };
            if let Some(type_node) = field.child_by_field_name("type") {
                self.record_uses_in(type_node, site_of);
            }
        }
    }

    fn field_names(&mut self, field: Node<'_>, owner: &str) {
        let type_path = field
            .child_by_field_name("type")
            .and_then(|written| self.type_path(written));
        let mut cursor = field.walk();
        for name_node in field.children_by_field_name("name", &mut cursor) {
            let name = text(name_node, self.source);
            if name != "_" {
                let mut symbol =
                    self.declared(SymbolKind::Field, field, (Some(owner.to_owned()), name));
                symbol.value_types = type_path.clone().into_iter().collect();
                self.push_symbol(symbol);
            }
        }
    }

    fn var_const(&mut self, node: Node<'_>) {
        let kind = if node.kind() == "const_declaration" {
            SymbolKind::Constant
        } else {
            SymbolKind::Variable
        };

        for spec in children(node).filter_map(|i| node.child(i)) {
            if matches!(spec.kind(), "const_spec" | "var_spec") {
                self.spec_names(spec, kind);
            }
        }
    }

    fn spec_names(&mut self, spec: Node<'_>, kind: SymbolKind) {
        let mut cursor = spec.walk();
        let names: Vec<Node<'_>> = spec.children_by_field_name("name", &mut cursor).collect();
        let written = spec.child_by_field_name("type");
        let values: Vec<Node<'_>> = match spec.child_by_field_name("value") {
            Some(list) if written.is_none() => named_children(list)
                .filter_map(|i| list.named_child(i))
                .collect(),
            _ => Vec::new(),
        };
        for (position, &name_node) in names.iter().enumerate() {
            let name = text(name_node, self.source);
            if name == "_" {
                self.assertion_implements(spec);
            } else {
                let type_path = written.map_or_else(
                    || self.value_path_at(&values, names.len(), position),
                    |written| self.type_path(written),
                );
                self.spec_name(spec, kind, (name, type_path));
            }
        }
    }

    fn spec_name(
        &mut self,
        spec: Node<'_>,
        kind: SymbolKind,
        (name, type_path): (String, Option<String>),
    ) {
        let id = SymbolId(self.symbol_count);
        if let Some(&block) = self.blocks.last() {
            self.locals.push(Local {
                from: spec.end_byte(),
                block,
                name: name.clone(),
                kind: LocalKind::ValueSymbol(id),
                type_path: type_path.clone(),
            });
        }
        let mut symbol = self.declared(kind, spec, (None, name));
        symbol.value_types = type_path.into_iter().collect();
        self.push_symbol(symbol);

        let first_of_spec = self
            .spec_owners
            .back()
            .is_none_or(|&(start, _)| start != spec.start_byte());
        if self.use_owners.is_empty() && first_of_spec {
            self.spec_owners.push_back((spec.start_byte(), id));
        }
    }

    fn assertion_implements(&mut self, spec: Node<'_>) {
        let Some(iface_node) = spec
            .child_by_field_name("type")
            .filter(|node| matches!(node.kind(), "type_identifier" | "qualified_identifier"))
        else {
            return;
        };
        let Some(value) = spec.child_by_field_name("value") else {
            return;
        };
        let Some(type_name) = children(value)
            .filter_map(|i| value.child(i))
            .find_map(|expression| assertion_value_type(expression, self.source))
        else {
            return;
        };

        self.pending_parents
            .push((type_name, text(iface_node, self.source)));
    }
}
