use std::cell::OnceCell;
use std::collections::{HashMap, HashSet, VecDeque};

use kartta_lang::tree::{child_nodes, line, named_child_nodes, preorder, signature, text};
use kartta_lang::{
    FILE_SCOPE, ImplementsAssertion, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId,
    SymbolKind,
};
use tree_sitter::{Node, Tree};

use self::anonymous::literal_type;
use self::locals::{Block, Local, LocalKind, declares_left, header_statement};
use self::syntax::{
    assertion_value_type, default_name_candidates, is_exported, is_generated_source, parents_of,
    receiver_type, tag_hands_over_its_field, trim_quotes,
};
use self::types::{element_of, inside_containers, is_container_type};

mod anonymous;
mod calls;
mod locals;
mod syntax;
mod types;
mod uses;

pub(crate) const IMPORT_FORM: u32 = 0;

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let package = package_name(tree, source);
    let root = tree.root_node();
    let mut emitter = Emitter::new((source, root), package, package_values(tree, source));

    preorder(root, |node| !node.is_named() || emitter.handle(node));

    emitter.apply_pending_parents();
    emitter.mark_reflected_fields();
    emitter.raw
}

fn package_name(tree: &Tree, source: &[u8]) -> String {
    let root = tree.root_node();

    child_nodes(root)
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
    let specs = child_nodes(root)
        .filter(|declaration| matches!(declaration.kind(), "var_declaration" | "const_declaration"))
        .flat_map(child_nodes);
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

// constraint: the go command builds a `package main` with no `func main` only as a plugin, and a plugin's host looks
// its exported functions and package-level variables up by name through `plugin.Lookup`
// shortcut: the walk reads one file, so a file of a program's `package main` that leaves `func main` to a sibling
// passes as a plugin's — read the package's files once a program's dead exports show among the reflected
fn is_plugin(package: &str, root: Node<'_>, source: &[u8]) -> bool {
    package == "main"
        && !child_nodes(root).any(|declaration| {
            declaration.kind() == "function_declaration"
                && (declaration.child_by_field_name("name"))
                    .is_some_and(|name| name.utf8_text(source) == Ok("main"))
        })
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
    root: Node<'a>,
    package_types: OnceCell<HashMap<&'a str, Node<'a>>>,
    blocks: Vec<Block>,
    spec_owners: VecDeque<(usize, SymbolId)>,
    named_by_call: Option<usize>,
    literal_key: Option<usize>,
    /// The nodes of a literal open at the walk's position, outermost first, a parse error inside one among them: each
    /// one's parent stands before it, so a key finds its literal's type without `Node::parent`, which searches from
    /// the root past every element before it.
    literals: Vec<Node<'a>>,
    /// The identifiers a conversion, an allocation or a type argument names as a type, which the walk reaches after
    /// the call that holds them.
    type_operands: Vec<usize>,
    import_names: Vec<String>,
    pending_parents: Vec<(String, String)>,
    /// The symbol that owns the fields of each anonymous struct, by the struct's node.
    anonymous: HashMap<usize, SymbolId>,
    /// The type paths of the values that a call which marshals, unmarshals or reflects on them receives, each with
    /// whether the call skips a field whose tag says so.
    reflected_values: Vec<(String, bool)>,
    /// The fields whose tag makes every serializing library skip them.
    skipped_fields: Vec<SymbolId>,
    file_is_generated: bool,
    is_plugin: bool,
    symbol_count: u32,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn new(
        (source, root): (&'a [u8], Node<'a>),
        package: String,
        package_values: HashSet<&'a str>,
    ) -> Self {
        let mut raw = RawExtraction::default();
        raw.package = (!package.is_empty()).then(|| package.clone());
        let is_plugin = is_plugin(&package, root, source);

        Self {
            source,
            package,
            callers: Vec::new(),
            use_owners: Vec::new(),
            type_params: Vec::new(),
            locals: Vec::new(),
            package_values,
            root,
            package_types: OnceCell::new(),
            blocks: Vec::new(),
            spec_owners: VecDeque::new(),
            named_by_call: None,
            literal_key: None,
            literals: Vec::new(),
            type_operands: Vec::new(),
            import_names: Vec::new(),
            pending_parents: Vec::new(),
            anonymous: HashMap::new(),
            reflected_values: Vec::new(),
            skipped_fields: Vec::new(),
            file_is_generated: is_generated_source(source),
            is_plugin,
            symbol_count: 0,
            raw,
        }
    }

    // constraint: an assertion may precede the type it names, so the pairs wait for the end of the walk; a pair whose
    // type the file does not declare goes out as an assertion, for the graph to apply where it sees the whole package
    fn apply_pending_parents(&mut self) {
        for (type_name, interface) in std::mem::take(&mut self.pending_parents) {
            let declared = self.raw.symbols.iter_mut().find(|symbol| {
                symbol.name == type_name
                    && matches!(
                        symbol.kind,
                        SymbolKind::Struct
                            | SymbolKind::Interface
                            | SymbolKind::TypeAlias
                            | SymbolKind::DefinedType
                    )
            });
            match declared {
                Some(symbol) if symbol.parents.contains(&interface) => {}
                Some(symbol) => symbol.parents.push(interface),
                None => self.raw.assertions.push(ImplementsAssertion {
                    type_name,
                    interface,
                }),
            }
        }
    }

    // constraint: a library that marshals, unmarshals or reflects on a struct's value reads or writes its exported
    // fields by their names, and those of the structs the fields hold, unless a tag skips the field for a library
    // that honours it
    // shortcut: the walk reads one file, so it follows only the package-level types the file declares and the values
    // whose path is a bare type — resolve the paths in the graph once a dead field of a struct another file marshals
    // shows
    fn mark_reflected_fields(&mut self) {
        let mut pending = std::mem::take(&mut self.reflected_values);
        let mut followed: Vec<(String, bool)> = Vec::new();
        while let Some((path, honors_skip_tags)) = pending.pop() {
            let name = inside_containers(&path);
            let seen = followed
                .iter()
                .any(|(held, honors)| held == name && *honors == honors_skip_tags);
            let Some(declared) = self.package_type_symbol(name).filter(|_| !seen) else {
                continue;
            };
            followed.push((name.to_owned(), honors_skip_tags));
            if declared.kind != SymbolKind::Struct {
                let named = declared.value_types.iter().cloned();
                pending.extend(named.map(|path| (path, honors_skip_tags)));
                continue;
            }
            let (local, skipped) = (&self.raw.local_symbols, &self.skipped_fields);
            let fields = ((0..).map(SymbolId).zip(self.raw.symbols.iter_mut()))
                .filter(|(id, field)| {
                    field.kind == SymbolKind::Field
                        && field.exported
                        && field.owner.as_deref() == Some(name)
                        && !(honors_skip_tags && skipped.contains(id))
                        && local.binary_search(id).is_err()
                })
                .map(|(_, field)| field);
            for field in fields {
                field.flags = field.flags.with_reflected(true);
                let held = field.value_types.iter().cloned();
                pending.extend(held.map(|path| (path, honors_skip_tags)));
            }
        }
    }

    // constraint: an alias and a defined container type record the type they name as their value type, which a
    // library reaches through them
    fn package_type_symbol(&self, name: &str) -> Option<&Symbol> {
        let mut declared = (0..).map(SymbolId).zip(&self.raw.symbols);
        declared
            .find(|(id, symbol)| {
                matches!(
                    symbol.kind,
                    SymbolKind::Struct | SymbolKind::TypeAlias | SymbolKind::DefinedType
                ) && symbol.owner.is_none()
                    && symbol.name == name
                    && self.raw.local_symbols.binary_search(id).is_err()
            })
            .map(|(_, symbol)| symbol)
    }

    fn handle(&mut self, node: Node<'a>) -> bool {
        self.close_scopes(node.start_byte());
        let caller = self.callers.last().map_or(FILE_SCOPE, |scope| scope.id);

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
            "index_expression" => self.record_type_arguments(node),
            "type_identifier" | "qualified_identifier" | "selector_expression" => {
                let owner = self.use_owner();
                return self.record_use(node, owner);
            }
            "identifier" => self.record_identifier(node, self.use_owner()),
            "keyed_element" => {
                self.record_literal_key(node, self.use_owner());
                self.literals.push(node);
            }
            "composite_literal" => {
                self.literals.push(node);
                self.record_positional_literal(node, self.use_owner());
            }
            "literal_value" | "ERROR" => self.literals.push(node),
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
        while self.callers.pop_if(|scope| scope.end <= start).is_some() {}
        while self.use_owners.pop_if(|scope| scope.end <= start).is_some() {}
        while self.type_params.pop_if(|(end, _)| *end <= start).is_some() {}
        while self.blocks.pop_if(|block| block.end <= start).is_some() {}
        while self
            .literals
            .pop_if(|open| open.end_byte() <= start)
            .is_some()
        {}
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
            let values = if node.kind() == "range_clause" {
                Vec::new()
            } else {
                right_values(node)
            };
            let has_literal = values.iter().any(|&value| literal_type(value).is_some());
            let names: Vec<Node<'_>> = if has_literal && node.kind() == "short_var_declaration" {
                named_child_nodes(left).collect()
            } else {
                Vec::new()
            };
            let literals = self.name_literal_structs((&names, &values), Some(block));
            let mut paths = self.declared_paths(node, left, &values);
            for &(_, _, position) in &literals {
                let name = text(names[position], self.source);
                let mut host = self.declared(SymbolKind::Variable, node, (None, name));
                host.value_types = paths.get(position).cloned().flatten().into_iter().collect();
                self.push_symbol(host);
            }
            self.push_locals(left, (node.end_byte(), block), &mut |position| {
                paths.get_mut(position).and_then(Option::take)
            });
            for (struct_type, scope, _) in literals {
                self.declare_anonymous(struct_type, &scope);
            }
        }
    }

    // constraint: a range yields an index or a key first, which no path names, and an element second
    fn declared_paths(
        &self,
        node: Node<'_>,
        left: Node<'_>,
        values: &[Node<'_>],
    ) -> Vec<Option<String>> {
        let names = left.named_child_count();
        if node.kind() == "range_clause" {
            let element = node
                .child_by_field_name("right")
                .and_then(|ranged| self.value_path(ranged))
                .and_then(|ranged| element_of(&ranged));
            return vec![None, element];
        }
        (0..names)
            .map(|position| self.value_path_at(values, names, position))
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
        if names < 2 || names != list.named_child_count() {
            return;
        }
        let stacked = self.callers.len();
        let mut next = first.0;
        let names = spec.children_by_field_name("name", &mut cursor);
        for (name, value) in names.zip(named_child_nodes(list)) {
            let id = if name.utf8_text(self.source) == Ok("_") {
                first
            } else {
                let id = SymbolId(next);
                next += 1;
                id
            };
            self.callers.push(Scope {
                end: value.end_byte(),
                id,
            });
        }
        if let Some(values) = self.callers.get_mut(stacked..) {
            values.reverse();
        }
    }

    fn import_declaration(&mut self, node: Node<'_>) {
        for spec in named_child_nodes(node) {
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
        let is_entry = match name.as_str() {
            "main" => self.package == "main",
            "init" => {
                node.child_by_field_name("result").is_none()
                    && node
                        .child_by_field_name("parameters")
                        .is_some_and(|list| list.named_child_count() == 0)
            }
            _ => false,
        };

        let mut symbol = self.declared(SymbolKind::Function, node, (None, name));
        symbol.signature = signature(node, self.source);
        let looked_up = self.is_plugin && symbol.exported;
        symbol.flags = symbol.flags.with_entry(is_entry).with_reflected(looked_up);
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
        let specs = child_nodes(node)
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
            _ if spec.kind() == "alias_declaration" => (SymbolKind::TypeAlias, Vec::new()),
            _ => (SymbolKind::DefinedType, Vec::new()),
        };
        let type_params = spec.child_by_field_name("type_parameters");
        self.push_type_params(type_params, spec.end_byte());
        let value_types = match type_node {
            Some(written) if written.kind() == "function_type" => self.result_paths(written),
            Some(written) if spec.kind() == "alias_declaration" || is_container_type(written) => {
                self.type_path(written).into_iter().collect()
            }
            Some(written) => self.underlying_path(written).into_iter().collect(),
            None => Vec::new(),
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
        for element in child_nodes(interface_type) {
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
        let fields = child_nodes(struct_type).filter(|field| field.kind() == "field_declaration");
        for field in fields {
            let first_field = SymbolId(self.symbol_count);
            self.field_names(field, (owner, owner));
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

    fn field_names(&mut self, field: Node<'_>, (owner, scope): (&str, &str)) {
        let nested = self.name_field_struct(field, scope);
        let type_path = field
            .child_by_field_name("type")
            .and_then(|written| self.type_path(written));
        let own_owner = (owner != scope).then(|| owner.to_owned());
        let tag = field.child_by_field_name("tag");
        let reflected = tag.is_some_and(|tag| tag_hands_over_its_field(&text(tag, self.source)));
        let first = self.symbol_count;
        let mut cursor = field.walk();
        for name_node in field.children_by_field_name("name", &mut cursor) {
            let name = text(name_node, self.source);
            if name != "_" {
                let mut symbol =
                    self.declared(SymbolKind::Field, field, (Some(scope.to_owned()), name));
                symbol.owner = own_owner.clone().or(symbol.owner);
                symbol.flags = symbol.flags.with_reflected(reflected);
                symbol.value_types = type_path.clone().into_iter().collect();
                self.push_symbol(symbol);
            }
        }
        if tag.is_some() && !reflected {
            self.skipped_fields
                .extend((first..self.symbol_count).map(SymbolId));
        }
        if let Some((struct_type, nested_scope)) = nested {
            self.declare_anonymous(struct_type, &nested_scope);
        }
    }

    fn var_const(&mut self, node: Node<'_>) {
        let kind = if node.kind() == "const_declaration" {
            SymbolKind::Constant
        } else {
            SymbolKind::Variable
        };

        for spec in child_nodes(node) {
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
            Some(list) if written.is_none() => named_child_nodes(list).collect(),
            _ => Vec::new(),
        };
        let host = SymbolId(self.symbol_count);
        let mut nested = self.name_literal_structs((&names, &values), None);
        nested.extend(
            written
                .zip(self.first_named(&names))
                .and_then(|(written, first)| self.name_value_struct(written, (first, host)))
                .map(|(struct_type, scope)| (struct_type, scope, 0)),
        );
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
        for (struct_type, scope, _) in nested {
            self.declare_anonymous(struct_type, &scope);
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
        let looked_up = self.is_plugin && kind == SymbolKind::Variable && self.blocks.is_empty();
        symbol.flags = symbol.flags.with_reflected(looked_up && symbol.exported);
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
        let Some(type_name) =
            child_nodes(value).find_map(|expression| assertion_value_type(expression, self.source))
        else {
            return;
        };

        self.pending_parents
            .push((type_name, text(iface_node, self.source)));
    }
}

fn right_values(statement: Node<'_>) -> Vec<Node<'_>> {
    match statement.child_by_field_name("right") {
        Some(list) if list.kind() == "expression_list" => named_child_nodes(list).collect(),
        Some(value) => vec![value],
        None => Vec::new(),
    }
}
