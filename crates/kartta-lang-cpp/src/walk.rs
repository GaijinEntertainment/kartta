mod data;
mod declarator;
mod identity;
mod name;
mod sites;

use std::collections::{HashMap, HashSet};

use kartta_lang::tree::{
    child_nodes, line, preorder, signature, signature_from, text, uncommented,
};
use kartta_lang::{
    Banner, CallRef, Callable, CallableRole, FILE_SCOPE, FoldedDeclaration, RawDirective,
    RawExtraction, Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use self::data::{
    alias_declarator_name, at_namespace_scope, data_declarator, data_signature, defines_class_tag,
    field_declarator, field_owner, field_value_types, has_qualifier, has_storage_class,
};
use self::declarator::{
    function_symbol, is_macro_invocation, is_pure_virtual, line_gap_len, nested_scope,
    split_return_type, unterminated,
};
use self::name::{
    call_target, class_symbol, head_scope, include_spec, is_simple_identifier, specializes,
    template_parameters, type_base_name,
};
use self::sites::{Locals, body_start};
use crate::repair::is_macro_name;

const BANNER: Banner = Banner::new(&["@generated", "DO NOT EDIT", "Code generated"])
    .with_lines(&["//"])
    .with_block("/*", "*/");

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut replayer = Replayer::new(source);

    preorder(tree.root_node(), |node| {
        if node.is_named() {
            replayer.handle(node);
        }
        true
    });

    replayer.flush_prototypes();
    replayer.free_namespace_members();
    replayer.raw
}

struct FunctionScope {
    end: usize,
    id: SymbolId,
    body: usize,
}

struct ClassScope {
    end: usize,
    id: SymbolId,
}

struct NamespaceScope {
    end: usize,
    path: Option<usize>,
    name: Option<String>,
}

struct TemplateScope {
    end: usize,
    parameters: Vec<String>,
}

struct Prototype {
    symbol: Symbol,
    key: Option<String>,
    in_class: bool,
}

struct WrittenScope {
    nested: Option<String>,
    namespace: Option<usize>,
}

struct Replayer<'s, 't> {
    source: &'s [u8],
    ancestors: Vec<Node<'t>>,
    locals: Locals<'s>,
    functions: Vec<FunctionScope>,
    classes: Vec<ClassScope>,
    namespaces: Vec<NamespaceScope>,
    namespace_names: HashSet<String>,
    templates: Vec<TemplateScope>,
    prototypes: Vec<Prototype>,
    callable_keys: HashSet<String>,
    out_of_line: HashMap<String, Vec<SymbolId>>,
    namespace_paths: Vec<String>,
    used_namespaces: Vec<(Option<usize>, String)>,
    written_scopes: HashMap<usize, WrittenScope>,
    generated: bool,
    raw: RawExtraction,
}

impl<'s, 't> Replayer<'s, 't> {
    fn new(source: &'s [u8]) -> Self {
        Self {
            source,
            ancestors: Vec::new(),
            locals: Locals::default(),
            functions: Vec::new(),
            classes: Vec::new(),
            namespaces: Vec::new(),
            namespace_names: HashSet::new(),
            templates: Vec::new(),
            prototypes: Vec::new(),
            callable_keys: HashSet::new(),
            out_of_line: HashMap::new(),
            namespace_paths: Vec::new(),
            used_namespaces: Vec::new(),
            written_scopes: HashMap::new(),
            generated: BANNER.opens(source),
            raw: RawExtraction::default(),
        }
    }

    fn handle(&mut self, node: Node<'t>) {
        self.close_scopes(node.start_byte());
        self.handle_declaration(node);
        if self.functions.is_empty() {
            self.handle_at_file_scope(node);
        } else {
            self.handle_in_body(node);
        }
        self.ancestors.push(node);
    }

    fn handle_declaration(&mut self, node: Node<'_>) {
        match node.kind() {
            "namespace_definition" => self.open_namespace(node),
            "using_declaration" if is_using_namespace(node) => {
                let used = child_nodes(node).rfind(|child| {
                    matches!(
                        child.kind(),
                        "identifier" | "qualified_identifier" | "namespace_identifier"
                    )
                });
                if let Some(name) = used.map(|name| text(name, self.source)) {
                    self.note_namespace(&name);
                    let at = self.namespaces.last().and_then(|scope| scope.path);
                    self.used_namespaces.push((at, name));
                }
            }
            "function_definition" => self.define_function(node, None),
            "field_declaration" if is_pure_virtual(node) => self.declare_pure_virtual(node),
            "field_declaration" => {
                if let Some(body) = misread_definition_body(node, self.source) {
                    self.define_function(node, Some(body));
                } else {
                    self.queue_prototype(node);
                    self.push_fields(node);
                }
            }
            "declaration" if self.functions.is_empty() => {
                if let Some(body) = misread_definition_body(node, self.source) {
                    self.define_function(node, Some(body));
                } else {
                    self.queue_prototype(node);
                    self.push_data(node);
                }
            }
            "class_specifier" | "struct_specifier" | "union_specifier" => self.open_class(node),
            "enum_specifier" if node.child_by_field_name("body").is_some() => {
                self.define_enum(node);
            }
            "alias_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let aliased = node
                        .child_by_field_name("type")
                        .filter(|written| written.child_by_field_name("declarator").is_none())
                        .and_then(|written| written.child_by_field_name("type"));
                    self.push_type_alias(text(name, self.source), node, aliased);
                }
            }
            "type_definition" => {
                let aliased = node.child_by_field_name("type");
                let mut cursor = node.walk();
                let names = node
                    .children_by_field_name("declarator", &mut cursor)
                    .filter_map(|declarator| Some((alias_declarator_name(declarator)?, declarator)))
                    .filter(|&(name, plain)| !defines_class_tag(aliased, name, plain, self.source));
                for (name, declarator) in names {
                    let plain = aliased.filter(|_| name == declarator);
                    self.push_type_alias(text(name, self.source), node, plain);
                }
            }
            "template_declaration" => {
                let parameters = template_parameters(node, self.source);
                if !parameters.is_empty() {
                    self.templates.push(TemplateScope {
                        end: node.end_byte(),
                        parameters,
                    });
                }
            }
            "call_expression" | "new_expression" => self.record_call(node),
            "preproc_include" => {
                if let Some((spec, form)) = include_spec(node, self.source) {
                    let line = line(node.start_position().row);
                    self.raw
                        .directives
                        .push(RawDirective::new(form, spec, line));
                }
            }
            _ => {}
        }
    }

    fn close_scopes(&mut self, start: usize) {
        while self
            .ancestors
            .pop_if(|ancestor| ancestor.end_byte() <= start)
            .is_some()
        {}
        self.locals.close(start);
        while self
            .functions
            .last()
            .is_some_and(|scope| scope.end <= start)
        {
            self.functions.pop();
        }
        while self.classes.last().is_some_and(|scope| scope.end <= start) {
            self.classes.pop();
        }
        while self
            .templates
            .last()
            .is_some_and(|scope| scope.end <= start)
        {
            self.templates.pop();
        }
        while self
            .namespaces
            .last()
            .is_some_and(|scope| scope.end <= start)
        {
            self.namespaces.pop();
        }
    }

    fn open_namespace(&mut self, node: Node<'_>) {
        let name = node
            .child_by_field_name("name")
            .map(|name| text(name, self.source));
        let enclosing = self.namespaces.last().and_then(|scope| scope.path);
        let path = match &name {
            Some(name) => {
                self.note_namespace(name);
                let path = enclosing.map_or_else(
                    || name.clone(),
                    |outer| format!("{}::{name}", self.namespace_paths[outer]),
                );
                self.namespace_paths.push(path);
                Some(self.namespace_paths.len() - 1)
            }
            None => enclosing,
        };
        self.namespaces.push(NamespaceScope {
            end: node.end_byte(),
            path,
            name,
        });
    }

    fn define_function(&mut self, node: Node<'_>, misread_body: Option<Node<'_>>) {
        let Some(mut symbol) = self.callable_symbol(node) else {
            return;
        };
        let (end, body) = match misread_body {
            Some(body) => {
                symbol.end_line = line(body.end_position().row);
                let written = uncommented(node, node.start_byte()..body.start_byte(), self.source);
                symbol.signature = Some(written.trim_end().to_owned());
                (body.end_byte(), body.start_byte())
            }
            None => (node.end_byte(), body_start(node)),
        };
        let key = self.callable_key(node);
        let out_of_line = symbol.owner.is_some() && self.classes.is_empty();

        let id = self.push_symbol(symbol);
        if out_of_line && let Some(key) = &key {
            self.out_of_line.entry(key.clone()).or_default().push(id);
        }
        if out_of_line {
            let written = WrittenScope {
                nested: nested_scope(node, self.source),
                namespace: self.namespaces.last().and_then(|scope| scope.path),
            };
            self.written_scopes.insert(id.0 as usize, written);
        }
        self.record_callable(id, key, CallableRole::Definition);
        self.functions.push(FunctionScope { end, id, body });
    }

    fn declare_pure_virtual(&mut self, node: Node<'_>) {
        let Some(symbol) = self.callable_symbol(node) else {
            return;
        };
        let key = self.callable_key(node);

        let id = self.push_symbol(symbol);
        self.record_callable(id, key, CallableRole::Declaration);
    }

    fn queue_prototype(&mut self, node: Node<'_>) {
        let Some(mut symbol) = self.callable_symbol(node) else {
            return;
        };
        self.apply_namespace(&mut symbol);
        let key = self.callable_key(node);

        self.prototypes.push(Prototype {
            symbol,
            key,
            in_class: !self.classes.is_empty(),
        });
    }

    fn callable_symbol(&self, node: Node<'_>) -> Option<Symbol> {
        let mut symbol = function_symbol(node, self.source)?;
        self.attribute_to_class(&mut symbol);
        self.apply_linkage(&mut symbol, node);
        self.apply_flags(&mut symbol);

        if is_macro_invocation(node, &symbol) {
            let split = split_return_type(node, self.source)?;
            symbol.signature = signature_from(split.start_byte(), node, self.source);
        }
        Some(symbol)
    }

    fn open_class(&mut self, node: Node<'_>) {
        let Some(mut symbol) = class_symbol(node, self.source) else {
            return;
        };
        symbol
            .parents
            .retain(|parent| !self.is_template_parameter(parent));
        self.nest_in_class(&mut symbol, node);
        if symbol.owner.is_none()
            && let Some(scope) = head_scope(node, self.source)
        {
            symbol.qualified_name = format!("{}::{}", self.written_scope(&scope), symbol.name);
        }
        if specializes(node) {
            let head = self
                .ancestors
                .last()
                .filter(|parent| parent.kind() == "template_declaration")
                .map_or_else(|| node.start_byte(), Node::start_byte);
            symbol.signature = signature_from(head, node, self.source);
        }
        self.apply_flags(&mut symbol);

        let id = self.push_symbol(symbol);
        self.classes.push(ClassScope {
            end: node.end_byte(),
            id,
        });
    }

    fn define_enum(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let mut symbol = exported_symbol(text(name, self.source), SymbolKind::Enum, node);
        symbol.flags = self.base_flags();
        self.nest_in_class(&mut symbol, node);
        self.push_symbol(symbol);
    }

    /// Records the type alias `name`, whose value type is the class `aliased` names; an alias of a pointer or a
    /// reference records none.
    fn push_type_alias(&mut self, name: String, node: Node<'_>, aliased: Option<Node<'_>>) {
        let mut symbol = exported_symbol(name, SymbolKind::TypeAlias, node);
        symbol.value_types.extend(
            aliased
                .and_then(|written| self.aliased_class(written))
                .map(str::to_owned),
        );
        self.nest_in_class(&mut symbol, node);
        self.apply_linkage(&mut symbol, node);
        self.apply_flags(&mut symbol);
        self.push_symbol(symbol);
    }

    fn push_fields(&mut self, node: Node<'_>) {
        if node.child_by_field_name("type").is_none() || node.has_error() {
            return;
        }
        let mut cursor = node.walk();
        let mut names = node
            .children_by_field_name("declarator", &mut cursor)
            .filter_map(field_declarator)
            .peekable();
        if names.peek().is_none() {
            return;
        }
        let Some(owner) = field_owner(node, self.source) else {
            return;
        };
        let signature = signature(node, self.source);
        let written = node.child_by_field_name("type");
        let class = written.and_then(|written| self.owner_named(written));
        let reached = class
            .and(written)
            .and_then(|written| self.pointee_named(written));

        for (name, pointers) in names {
            let mut symbol = exported_symbol(text(name, self.source), SymbolKind::Field, node);
            symbol.value_types = field_value_types(class, pointers, reached);
            symbol.qualified_name = format!("{owner}::{}", symbol.name);
            symbol.owner = Some(owner.clone());
            symbol.signature.clone_from(&signature);
            symbol.flags = self.base_flags();
            self.apply_linkage(&mut symbol, node);
            self.push_symbol(symbol);
        }
    }

    fn push_data(&mut self, node: Node<'_>) {
        if node.child_by_field_name("type").is_none()
            || node.has_error()
            || !at_namespace_scope(node)
        {
            return;
        }
        let constexpr = has_qualifier(node, "constexpr", self.source);
        let declared_const = has_qualifier(node, "const", self.source);
        let declared_extern = has_storage_class(node, "extern", self.source);
        let external = declared_extern || has_storage_class(node, "inline", self.source);
        let signature = data_signature(node, self.source);

        let mut cursor = node.walk();
        for declarator in node.children_by_field_name("declarator", &mut cursor) {
            let Some((name, const_pointer)) = data_declarator(declarator, self.source) else {
                continue;
            };
            let kind = if constexpr || const_pointer.unwrap_or(declared_const) {
                SymbolKind::Constant
            } else {
                SymbolKind::Variable
            };
            let mut symbol = exported_symbol(text(name, self.source), kind, node);
            symbol.exported = external || kind != SymbolKind::Constant;
            symbol.signature.clone_from(&signature);
            symbol.flags = self.base_flags();
            self.apply_linkage(&mut symbol, node);
            let id = self.push_symbol(symbol);
            let role = if declared_extern && declarator.kind() != "init_declarator" {
                CallableRole::Declaration
            } else {
                CallableRole::Definition
            };
            let key = self.raw.symbols[id.0 as usize].qualified_name.clone();
            self.record_callable(id, Some(key), role);
        }
    }

    fn record_call(&mut self, node: Node<'_>) {
        let target = if node.kind() == "new_expression" {
            node.child_by_field_name("type")
                .map(|constructed| (type_base_name(constructed, self.source), None))
                .filter(|(callee, _)| is_simple_identifier(callee))
        } else {
            call_target(node, self.source)
        };
        let Some((callee, receiver)) = target else {
            return;
        };
        // constraint: a bare call of a name a body declares calls through the local or the parameter, a function
        // pointer or a callable, and binds no definition of the name
        if receiver.is_none()
            && (self.is_template_parameter(&callee) || self.locals.declares(&callee))
        {
            return;
        }
        let receiver = receiver.map(|receiver| match receiver.as_str() {
            "" => self.called_owner(node),
            "this" => self.callers_owner().map_or(receiver, str::to_owned),
            _ => receiver,
        });

        let mut call = CallRef::new(
            self.caller(),
            callee,
            receiver,
            line(node.start_position().row),
        );
        call.calls_a_method = call.receiver.is_some();
        self.raw.calls.push(call);
    }

    /// The class an alias of `written` names as its value type, spelled as a receiver spells it; a builtin, an unnamed
    /// type, a template parameter or a scope of one, `typename T::iterator`, and a scope after a template's arguments,
    /// `std::vector<Node>::iterator`, name none.
    // constraint: a scope holding a template's arguments, `std::vector<T>::iterator`, or naming a template parameter,
    // `T::iterator`, is no class a receiver's spelling can name, and its bare name would name another one
    fn aliased_class(&self, written: Node<'_>) -> Option<&'s str> {
        let written = match written.kind() {
            "dependent_type" => written.named_child(0)?,
            _ => written,
        };
        let class = self.owner_named(written)?;
        let head = class.trim_start_matches("::").split("::").next()?;
        let scoped = written.kind() != "qualified_identifier" || class.contains("::");
        (scoped && !self.is_template_parameter(head)).then_some(class)
    }

    // constraint: a template parameter names a type or a value the instantiation supplies, so `Compare()` and `F()`
    // inside the template call no definition of that name, and `: public Compare` derives from none
    fn is_template_parameter(&self, callee: &str) -> bool {
        self.templates
            .iter()
            .any(|scope| scope.parameters.iter().any(|parameter| parameter == callee))
    }

    fn caller(&self) -> SymbolId {
        self.functions.last().map_or(FILE_SCOPE, |scope| scope.id)
    }

    fn callers_owner(&self) -> Option<&str> {
        let scope = self.functions.last()?;
        self.raw.symbols[scope.id.0 as usize].owner.as_deref()
    }

    fn attribute_to_class(&self, symbol: &mut Symbol) {
        if symbol.owner.is_some() {
            return;
        }
        let Some(scope) = self.classes.last() else {
            return;
        };
        let class = &self.raw.symbols[scope.id.0 as usize];

        symbol.kind = SymbolKind::Method;
        symbol.qualified_name = format!("{}::{}", class.name, symbol.name);
        symbol.owner = Some(class.name.clone());
    }

    fn nest_in_class(&self, symbol: &mut Symbol, node: Node<'_>) {
        let Some(scope) = self.classes.last() else {
            return;
        };
        let in_body = std::iter::successors(node.parent(), Node::parent)
            .take_while(|ancestor| ancestor.end_byte() < scope.end)
            .any(|ancestor| matches!(ancestor.kind(), "function_definition" | "lambda_expression"));
        if in_body {
            return;
        }

        let class = &self.raw.symbols[scope.id.0 as usize];
        symbol.qualified_name = format!("{}::{}", class.qualified_name, symbol.name);
        symbol.owner = Some(class.name.clone());
    }

    fn push_symbol(&mut self, mut symbol: Symbol) -> SymbolId {
        self.apply_namespace(&mut symbol);

        self.raw.push_symbol(symbol)
    }

    fn apply_namespace(&self, symbol: &mut Symbol) {
        if symbol.owner.is_some() || symbol.qualified_name != symbol.name {
            return;
        }

        let prefix: Vec<&str> = self
            .namespaces
            .iter()
            .filter_map(|scope| scope.name.as_deref())
            .collect();
        if !prefix.is_empty() {
            symbol.qualified_name = format!("{}::{}", prefix.join("::"), symbol.name);
        }
    }

    fn written_scope(&self, written: &str) -> String {
        let enclosing: Vec<&str> = self
            .namespaces
            .iter()
            .filter_map(|scope| scope.name.as_deref())
            .flat_map(|name| name.split("::"))
            .collect();
        let first = written.split("::").next().unwrap_or(written);
        let kept = enclosing
            .iter()
            .rposition(|name| *name == first)
            .unwrap_or(enclosing.len());

        enclosing[..kept]
            .iter()
            .copied()
            .chain([written])
            .collect::<Vec<_>>()
            .join("::")
    }

    fn apply_linkage(&self, symbol: &mut Symbol, node: Node<'_>) {
        let in_anonymous_namespace = self.namespaces.iter().any(|scope| scope.name.is_none());
        if in_anonymous_namespace
            || (symbol.owner.is_none() && has_storage_class(node, "static", self.source))
        {
            symbol.exported = false;
        }
    }

    const fn base_flags(&self) -> SymbolFlags {
        SymbolFlags::empty().with_generated(self.generated)
    }

    fn apply_flags(&self, symbol: &mut Symbol) {
        let entry =
            (self.namespaces.is_empty() && is_entry_point(symbol)) || is_implicitly_called(symbol);
        symbol.flags = self.base_flags().with_entry(entry);
    }

    fn flush_prototypes(&mut self) {
        for Prototype {
            symbol,
            key,
            in_class,
        } in std::mem::take(&mut self.prototypes)
        {
            if !key
                .as_ref()
                .is_some_and(|key| self.callable_keys.contains(key))
            {
                let id = self.push_symbol(symbol);
                self.record_callable(id, key, CallableRole::Declaration);
                continue;
            }
            if in_class
                && let Some(definitions) = key.as_ref().and_then(|key| self.out_of_line.get(key))
            {
                let line = symbol.line;
                let folded = definitions
                    .iter()
                    .map(|&definition| FoldedDeclaration { definition, line });
                self.raw.folded_declarations.extend(folded);
            }
        }
    }

    fn note_namespace(&mut self, name: &str) {
        for segment in name
            .split("::")
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
        {
            if !self.namespace_names.contains(segment) {
                self.namespace_names.insert(segment.to_owned());
            }
        }
    }

    // constraint: an out-of-line definition names its scope as a class does, `bool d3d::clearview() {}`, and only the
    // file tells a namespace from a class: a scope the file opens as a namespace or uses as one, and declares no type
    // the scope can name, holds a free function
    fn free_namespace_members(&mut self) {
        if self.namespace_names.is_empty() {
            return;
        }
        let mut types: HashMap<&str, Vec<&str>> = HashMap::new();
        for symbol in self.raw.symbols.iter().filter(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::Class | SymbolKind::Struct | SymbolKind::Enum | SymbolKind::TypeAlias
            )
        }) {
            types
                .entry(symbol.name.as_str())
                .or_default()
                .push(symbol.qualified_name.as_str());
        }
        let freed: Vec<usize> = self
            .written_scopes
            .keys()
            .copied()
            .filter(|&index| {
                let symbol = &self.raw.symbols[index];
                symbol.kind == SymbolKind::Method
                    && symbol
                        .owner
                        .as_deref()
                        .is_some_and(|owner| self.is_namespace_member(index, owner, &types))
            })
            .collect();
        for index in freed {
            let symbol = &mut self.raw.symbols[index];
            symbol.kind = SymbolKind::Function;
            symbol.owner = None;
        }
    }

    fn is_namespace_member(
        &self,
        symbol: usize,
        owner: &str,
        types: &HashMap<&str, Vec<&str>>,
    ) -> bool {
        self.namespace_names.contains(owner)
            && !types.get(owner).is_some_and(|named| {
                named
                    .iter()
                    .any(|type_name| self.reaches(symbol, owner, type_name))
            })
    }

    // constraint: the scope an out-of-line definition writes names a type of that scope in an enclosing namespace or
    // in a namespace the file uses: `N::g` inside `namespace B` reaches `B::N` and a root `N`, and a root `N::g` no
    // `A::N`
    fn reaches(&self, symbol: usize, owner: &str, type_name: &str) -> bool {
        let written = self.written_scopes.get(&symbol);
        let scope = written
            .and_then(|written| written.nested.as_deref())
            .unwrap_or(owner);
        let Some(head) = type_name.strip_suffix(scope).and_then(|head| {
            head.strip_suffix("::")
                .or_else(|| head.is_empty().then_some(head))
        }) else {
            return false;
        };
        let enclosing = written
            .and_then(|written| written.namespace)
            .map(|path| self.namespace_paths[path].as_str());
        head.is_empty()
            || enclosing.is_some_and(|path| lies_in(path, head))
            || self.used_namespaces.iter().any(|(at, used)| {
                used == head
                    && at.is_none_or(|at| {
                        enclosing.is_some_and(|path| lies_in(path, &self.namespace_paths[at]))
                    })
            })
    }

    fn record_callable(&mut self, symbol: SymbolId, key: Option<String>, role: CallableRole) {
        let Some(key) = key else {
            return;
        };
        self.callable_keys.insert(key.clone());
        if self.raw.symbols[symbol.0 as usize].exported {
            self.raw.callables.push(Callable::new(symbol, key, role));
        }
    }
}

// constraint: the grammar can end a definition before its body and read the body apart: a macro alone on its line
// above an out-of-line constructor with an initializer list, `NS_BEGIN\n\nVec4::Vec4(float x) :\n\tmX(x)\n{`,
// reads as the type of a declaration that holds the name and the `:` in an error and the initializers as its
// declarators, its body as a block after it; a macro call after the parameters of a function that takes a scoped type,
// `void lock(ns::id_t token) ACQUIRE()\n{`, reads as a function of its own that owns the body
fn misread_definition_body<'t>(declaration: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    if !unterminated(declaration, source) {
        return None;
    }
    let after = &source[declaration.end_byte()..];
    let opens_a_body = after[after
        .iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count()..]
        .starts_with(b"{");
    let on_the_line = &after[line_gap_len(after)..];
    let word = on_the_line
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
        .count();
    if !opens_a_body && !is_macro_name(&on_the_line[..word]) {
        return None;
    }
    let next = declaration.next_sibling()?;
    if next.kind() == "function_definition" {
        let macro_call = next.child_by_field_name("type").is_none()
            && next.start_position().row == declaration.end_position().row
            && next
                .child_by_field_name("declarator")
                .filter(|declarator| declarator.kind() == "function_declarator")
                .and_then(|declarator| declarator.child_by_field_name("declarator"))
                .is_some_and(|name| is_macro_name(&source[name.byte_range()]));
        return macro_call
            .then(|| next.child_by_field_name("body"))
            .flatten();
    }
    let error = child_nodes(declaration).find(Node::is_error)?;
    let names_a_function = child_nodes(error).any(|child| child.kind() == "function_declarator");
    let opens_initializers = child_nodes(error)
        .next_back()
        .is_some_and(|last| last.kind() == ":");
    (names_a_function && opens_initializers && next.kind() == "compound_statement").then_some(next)
}

fn exported_symbol(name: String, kind: SymbolKind, node: Node<'_>) -> Symbol {
    let mut symbol = Symbol::new(
        name,
        kind,
        line(node.start_position().row),
        line(node.end_position().row),
    );
    symbol.exported = true;
    symbol
}

fn lies_in(path: &str, outer: &str) -> bool {
    path.strip_prefix(outer)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

fn is_using_namespace(node: Node<'_>) -> bool {
    child_nodes(node).any(|child| child.kind() == "namespace")
}

fn is_entry_point(symbol: &Symbol) -> bool {
    symbol.owner.is_none()
        && symbol.kind == SymbolKind::Function
        && matches!(
            symbol.name.as_str(),
            "main" | "wmain" | "WinMain" | "wWinMain" | "DagorWinMain"
        )
}

// constraint: the language calls a constructor for each object it makes, of which only a `new` expression is recorded
// as a call, a destructor for each object it ends and an operator for an expression
fn is_implicitly_called(symbol: &Symbol) -> bool {
    let is_operator = symbol
        .name
        .strip_prefix("operator")
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'));
    matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
        && (is_operator
            || symbol.name.starts_with('~')
            || symbol.owner.as_deref() == Some(symbol.name.as_str()))
}

#[cfg(test)]
mod tests {
    use kartta_lang::tree::parse;
    use kartta_lang::{Access, Language, Receiver};

    use super::*;
    use crate::{Cpp, grammar};

    const SAMPLE: &[u8] = br#"
#include "core/util.h"
#include <vector>

namespace ns {

class Base {
public:
    virtual int compute() = 0;
};

class Widget : public Base {
public:
    int compute() override {
        return helper(size_);
    }

    void render() {
        compute();
        painter_.draw(this);
    }

private:
    int size_;
    Painter painter_;
};

}

int ns::Widget::area() const {
    return compute();
}

int *Factory::make() {
    return new int(0);
}

static int helper(int v) {
    return v;
}

enum Color { Red, Green };

TEST(MySuite, DoesThing) {
    helper(1);
}
"#;

    const HEADERISH: &[u8] = br"
namespace {
void local_helper() {}
}

class Engine {
public:
    void start();
    void stop();
    void run() {
        auto w = new gfx::Window<int>();
        start();
    }
};

int free_proto(int x);

int defined_and_declared(int x);
int defined_and_declared(int x) { return x; }
";

    fn extract(source: &[u8]) -> RawExtraction {
        Cpp::new().extract(source)
    }

    fn names(raw: &RawExtraction) -> Vec<&str> {
        raw.symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect()
    }

    fn qualified_names(raw: &RawExtraction) -> Vec<&str> {
        raw.symbols
            .iter()
            .map(|symbol| symbol.qualified_name.as_str())
            .collect()
    }

    fn callees(raw: &RawExtraction) -> Vec<&str> {
        raw.calls.iter().map(|call| call.callee.as_str()).collect()
    }

    fn spans(raw: &RawExtraction) -> Vec<(&str, u32, u32)> {
        raw.symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.line, symbol.end_line))
            .collect()
    }

    fn symbol<'r>(raw: &'r RawExtraction, name: &str) -> &'r Symbol {
        raw.symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("no symbol {name} in {:?}", names(raw)))
    }

    fn qualified<'r>(raw: &'r RawExtraction, qualified_name: &str) -> &'r Symbol {
        raw.symbols
            .iter()
            .find(|symbol| symbol.qualified_name == qualified_name)
            .unwrap_or_else(|| panic!("no symbol {qualified_name} in {:?}", qualified_names(raw)))
    }

    fn id_of(raw: &RawExtraction, name: &str) -> SymbolId {
        let position = raw.symbols.iter().position(|symbol| symbol.name == name);
        SymbolId(
            u32::try_from(position.unwrap_or_else(|| panic!("no symbol {name}")))
                .expect("a small fixture"),
        )
    }

    #[test]
    fn extracts_qualified_names_and_kinds_in_preorder() {
        let raw = extract(SAMPLE);

        assert_eq!(
            qualified_names(&raw),
            [
                "ns::Base",
                "Base::compute",
                "ns::Widget",
                "Widget::compute",
                "Widget::render",
                "Widget::size_",
                "Widget::painter_",
                "Widget::area",
                "Factory::make",
                "helper",
                "Color",
            ]
        );
        assert_eq!(symbol(&raw, "Base").kind, SymbolKind::Class);
        assert_eq!(symbol(&raw, "Color").kind, SymbolKind::Enum);
    }

    #[test]
    fn inline_member_is_attributed_to_enclosing_class() {
        let raw = extract(SAMPLE);

        let render = symbol(&raw, "render");
        assert_eq!(render.kind, SymbolKind::Method);
        assert_eq!(render.owner.as_deref(), Some("Widget"));
        assert_eq!(render.qualified_name, "Widget::render");
    }

    #[test]
    fn out_of_line_method_takes_scope_qualifier_as_owner() {
        let raw = extract(SAMPLE);

        let area = symbol(&raw, "area");
        assert_eq!(area.kind, SymbolKind::Method);
        assert_eq!(area.owner.as_deref(), Some("Widget"));
        assert_eq!(area.qualified_name, "Widget::area");
    }

    #[test]
    fn a_pointer_return_keeps_its_owner_and_a_static_function_is_unexported() {
        let raw = extract(SAMPLE);

        let make = symbol(&raw, "make");
        assert_eq!(make.owner.as_deref(), Some("Factory"));
        assert!(make.exported);
        assert!(!symbol(&raw, "helper").exported);
    }

    #[test]
    fn calls_are_attributed_to_their_caller() {
        let raw = extract(SAMPLE);
        let render = id_of(&raw, "render");

        let draw = raw
            .calls
            .iter()
            .find(|call| call.callee == "draw")
            .expect("draw call");
        assert_eq!(draw.receiver.as_deref(), Some(".painter_"));
        assert_eq!(draw.site_of, render);

        let bare_compute = raw
            .calls
            .iter()
            .find(|call| call.callee == "compute" && call.receiver.is_none())
            .expect("compute call");
        assert_eq!(bare_compute.site_of, render);
    }

    #[test]
    fn a_call_through_a_dot_or_an_arrow_is_marked_apart_from_a_bare_or_a_qualified_call() {
        let raw = extract(
            b"void f(W &w) { w.draw(); (&w)->draw(); this->draw(); draw(); a::draw(); new W; }\n",
        );

        let marks: Vec<bool> = raw.calls.iter().map(|call| call.calls_a_method).collect();
        assert_eq!(marks, [true, true, true, false, false, false]);
    }

    #[test]
    fn a_member_call_on_a_local_or_a_parameter_of_a_class_names_the_class() {
        let raw = extract(
            b"void f(Widget &w, Scene *s, int n) { NpScene *scene = get(); scene->startWrite(); w.draw();\n\
              s->update(); Widget copy; copy.draw(); auto a = make(); a.go(); s.update(); scene.run(); }\n",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .filter(|call| call.receiver.is_some())
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(
            calls,
            [
                ("startWrite", Some("NpScene")),
                ("draw", Some("Widget")),
                ("update", Some("Scene")),
                ("draw", Some("Widget")),
                ("go", Some("")),
                ("update", Some("")),
                ("run", Some("")),
            ]
        );
    }

    #[test]
    fn a_member_call_on_an_operand_of_a_scoped_class_keeps_the_scopes_as_written() {
        let raw = extract(
            b"void f(b::Widget &w, ::a::Widget *p, ns::Box<int> box, Outer<T>::Inner in, b :: Spaced s,\n\
              Map<ns::Key, const char *> map, Less<(1 > 2)> less) {\n\
              w.draw(); p->draw(); box.fill(); in.go(); s.go(); map.find(); less.go(); }\n",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(
            calls,
            [
                ("draw", Some("b::Widget")),
                ("draw", Some("::a::Widget")),
                ("fill", Some("ns::Box<int>")),
                ("go", Some("Outer<T>::Inner")),
                ("go", Some("Spaced")),
                ("find", Some("Map<ns::Key, const char *>")),
                ("go", Some("Less<(1 > 2)>")),
            ],
            "a template's arguments are kept as written, and a spelling with spaces outside them leaves the bare name"
        );
    }

    #[test]
    fn a_member_call_on_this_names_the_caller_s_own_class() {
        let raw = extract(
            b"struct Panel { void show() { this->hide(); } };\n\
              void Widget::paint() { this->draw(); }\n",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(calls, [("hide", Some("Panel")), ("draw", Some("Widget"))]);
    }

    #[test]
    fn a_member_call_on_a_name_a_member_does_not_declare_names_the_field() {
        let raw = extract(
            b"struct Cache { void read() { lock.unlockRead(); held->unlockRead(); } Lock lock; Lock *held; };\n\
              void Cache::release(Lock &lock) { this->lock.unlockRead(); this->held->unlockRead(); lock.reset(); }\n\
              template <typename T> void Cache::clear() { T other; other.reset(); }\n\
              void free() { lock.unlockRead(); }\n",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(
            calls,
            [
                ("unlockRead", Some(".lock")),
                ("unlockRead", Some("->held")),
                ("unlockRead", Some(".lock")),
                ("unlockRead", Some("->held")),
                ("reset", Some("Lock")),
                ("reset", Some("")),
                ("unlockRead", Some("")),
            ],
            "a parameter shadows the field, and outside a member the name is a namespace's"
        );
    }

    #[test]
    fn a_member_call_on_an_operand_the_walk_saw_no_class_for_names_no_type() {
        let raw = extract(
            b"void f() { descs[i].init(); a.b.close(); make().run(); this->reset(); buffer.resize(1);\n\
              list->clear(); Widget::create(); (*ptr).run(); }\n",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(
            calls,
            [
                ("init", Some("")),
                ("close", Some("")),
                ("run", Some("")),
                ("make", None),
                ("reset", Some("this")),
                ("resize", Some("")),
                ("clear", Some("")),
                ("Widget::create", None),
                ("run", Some("")),
            ]
        );
    }

    #[test]
    fn root_qualified_calls_and_bases_keep_the_root() {
        let raw = extract(
            b"void f() {\n  ::dgs_get_settings();\n  ::ns::helper();\n  ns::other();\n  ::tmpl<int>();\n}\n\
              class W : public ::Base {};\n",
        );

        assert_eq!(
            callees(&raw),
            ["::dgs_get_settings", "::ns::helper", "ns::other", "::tmpl"]
        );
        assert_eq!(symbol(&raw, "W").parents, ["::Base"]);
    }

    #[test]
    fn a_class_head_that_writes_its_scope_qualifies_by_it() {
        let raw = extract(
            b"class Project::Action {};\nstruct ::Root {};\ntemplate <> struct std::hash<Key> {};\n\
              namespace outer {\nclass inner::Gadget : public inner::Base {};\n}\n\
              namespace ns {\nclass ns::Knob {};\n}\n\
              namespace a {\nnamespace b {\nclass a::Dial {};\n}\n}\n",
        );

        assert_eq!(symbol(&raw, "Action").qualified_name, "Project::Action");
        assert_eq!(symbol(&raw, "Action").owner, None);
        assert_eq!(symbol(&raw, "Root").qualified_name, "Root");
        assert_eq!(symbol(&raw, "hash").qualified_name, "std::hash");
        assert_eq!(
            symbol(&raw, "Gadget").qualified_name,
            "outer::inner::Gadget"
        );
        assert_eq!(symbol(&raw, "Gadget").parents, ["inner::Base"]);
        assert_eq!(symbol(&raw, "Knob").qualified_name, "ns::Knob");
        assert_eq!(symbol(&raw, "Dial").qualified_name, "a::Dial");
    }

    #[test]
    fn parents_capture_base_class_clause() {
        let raw = extract(SAMPLE);

        assert_eq!(symbol(&raw, "Widget").parents, ["Base"]);
    }

    #[test]
    fn trailing_annotation_macros_never_name_the_symbol() {
        let raw = extract(
            b"class FS {\n  FS();\n  ~FS();\n  void load() DAG_TS_REQUIRES(mtx) { doLoad(); }\n\
              int count_ DAG_TS_GUARDED_BY(mtx);\n  int tickNumber DAG_TS_GUARDED_BY(mtx) = 0;\n\
              int size() const DAG_TS_REQUIRES(mtx) { return 0; }\n};\n\
              void free_fn() DAG_TS_REQUIRES(mtx) { work(); }\nvoid ALL_CAPS_REAL() { work(); }\n",
        );
        let names = names(&raw);

        assert!(
            !names.iter().any(|name| name.starts_with("DAG_TS_")),
            "{names:?}"
        );
        let load = symbol(&raw, "load");
        assert_eq!(load.kind, SymbolKind::Method);
        assert_eq!(load.owner.as_deref(), Some("FS"));
        assert!(names.contains(&"FS"), "{names:?}");
        assert!(names.contains(&"~FS"), "{names:?}");
        assert!(names.contains(&"ALL_CAPS_REAL"), "{names:?}");
        assert_eq!(symbol(&raw, "free_fn").kind, SymbolKind::Function);
        assert!(
            raw.callables
                .iter()
                .any(|callable| callable.key == "free_fn()"
                    && callable.role == CallableRole::Definition),
            "{:?}",
            raw.callables
        );
    }

    #[test]
    fn a_chain_of_trailing_macros_keeps_the_member() {
        let raw = extract(
            b"class T {\npublic:\n  void lock() DAG_TS_ACQUIRE() DAG_TS_NO_THREAD_SAFETY_ANALYSIS { impl.lock(); }\n\
              };\nclass U {\npublic:\n  void lock()\n    DAG_TS_ACQUIRE() DAG_TS_NO_THREAD_SAFETY_ANALYSIS\n  {\n\
              impl.lock();\n  }\n};\n\
              void unlock() DAG_TS_RELEASE() DAG_TS_NO_THREAD_SAFETY_ANALYSIS { go(); }\n",
        );
        let names = names(&raw);

        assert!(
            !names.iter().any(|name| name.starts_with("DAG_TS_")),
            "{names:?}"
        );
        assert_eq!(qualified(&raw, "T::lock").kind, SymbolKind::Method);
        assert_eq!(qualified(&raw, "U::lock").kind, SymbolKind::Method);
        assert_eq!(symbol(&raw, "unlock").kind, SymbolKind::Function);
        assert!(
            raw.callables
                .iter()
                .any(|callable| callable.key == "unlock()"
                    && callable.role == CallableRole::Definition),
            "{:?}",
            raw.callables
        );
    }

    #[test]
    fn a_macro_line_between_misparsed_members_names_no_member() {
        let raw = extract(
            b"struct FS {\n  FS() DAG_TS_ACQUIRE(mtx) { init(); }\n  MACRO_LINE()\n\
              ~FS() DAG_TS_RELEASE(mtx) { drop(); }\n};\n",
        );
        let names = names(&raw);

        assert!(!names.contains(&"MACRO_LINE"), "{names:?}");
        assert_eq!(symbol(&raw, "~FS").kind, SymbolKind::Method);
    }

    #[test]
    fn a_macro_before_the_return_type_keeps_the_function() {
        let raw = extract(
            b"JPH_INLINE float Tan(float x) { return x; }\nstatic JPH_INLINE Vec3 sMake() { return Vec3(); }\n\
              JPH_EXPORT void RegisterTypes();\nBOOL WINAPI DllMain(HINSTANCE h) { return TRUE; }\n\
              class FS {\n  int size_ DAG_TS_GUARDED_BY(mtx);\n  UINT32 count_ DAG_TS_GUARDED_BY(mtx);\n};\n",
        );

        assert_eq!(
            qualified_names(&raw),
            ["Tan", "sMake", "DllMain", "FS", "RegisterTypes"]
        );
        assert_eq!(symbol(&raw, "Tan").kind, SymbolKind::Function);
        assert!(!symbol(&raw, "sMake").exported);
        assert_eq!(
            symbol(&raw, "Tan").signature.as_deref(),
            Some("JPH_INLINE float Tan(float x)")
        );
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert!(keys.contains(&"Tan(float)"), "{keys:?}");
        assert!(keys.contains(&"RegisterTypes()"), "{keys:?}");
    }

    #[test]
    fn a_macro_between_the_return_type_and_the_name_names_no_scope() {
        let raw = extract(
            b"inline XMMATRIX XM_CALLCONV XMMatrixRotationZ(float Angle) noexcept { return M; }\n\
              GLM_FUNC_QUALIFIER GLM_CONSTEXPR vec<L, T, Q> mix(vec<L, T, Q> const& x) { return x; }\n\
              namespace ns {\nXMVECTOR XM_CALLCONV Rotate(XMVECTOR v) { return v; }\n}\n\
              int Widget::area() const { return 0; }\n\
              PUGI__FN xpath_node_set xpath_query::evaluate_node_set(xpath_node n) const { return x; }\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "XMMatrixRotationZ",
                "mix",
                "ns::Rotate",
                "Widget::area",
                "xpath_query::evaluate_node_set"
            ]
        );
        assert_eq!(
            symbol(&raw, "evaluate_node_set").owner.as_deref(),
            Some("xpath_query")
        );
        for name in ["XMMatrixRotationZ", "mix", "Rotate"] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        assert_eq!(symbol(&raw, "area").owner.as_deref(), Some("Widget"));
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert!(keys.contains(&"XMMatrixRotationZ(float)"), "{keys:?}");
    }

    #[test]
    fn a_calling_convention_between_the_return_type_and_the_name_keeps_the_function() {
        let raw = extract(
            b"class IBuild\n{\npublic:\n\tvirtual void CALLBACK release() = 0;\n\tvirtual int _stdcall build(int a);\n\
              \tvirtual HRESULT STDMETHODCALLTYPE find(int a);\n\
              \tvirtual HRESULT STDMETHODCALLTYPE attach(int a) = 0;\n\tstatic void CALLBACK cb(int a);\n\
              \tvirtual void ok() = 0;\n};\n\
              void WINAPI proto(int a);\nint WINAPI WinMain(int a, int b)\n{\n\treturn 0;\n}\n\
              static void CALLBACK waveOutCb(int a)\n{\n}\nvoid plain(int a);\n",
        );

        for name in ["release", "build", "find", "attach", "cb", "ok"] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Method, "{name}");
            assert_eq!(
                symbol(&raw, name).owner.as_deref(),
                Some("IBuild"),
                "{name}"
            );
        }
        for name in ["proto", "WinMain", "waveOutCb", "plain"] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        assert_eq!(raw.symbols.len(), 11, "{:?}", qualified_names(&raw));
        assert_eq!(
            symbol(&raw, "find").signature.as_deref(),
            Some("virtual HRESULT STDMETHODCALLTYPE find(int a)")
        );
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        for key in [
            "IBuild::find(int)",
            "IBuild::release()",
            "WinMain(int,int)",
            "proto(int)",
        ] {
            assert!(keys.contains(&key), "{key} in {keys:?}");
        }
    }

    #[test]
    fn a_calling_convention_after_a_scoped_return_type_names_no_owner() {
        let raw = extract(
            b"namespace rtm {\n\
              RTM_FORCE_INLINE constexpr rtm_impl::vector4f_get_x RTM_SIMD_CALL vector_get_x(vector4f input) \
              RTM_NO_EXCEPT\n{\n\treturn rtm_impl::vector4f_get_x{ input };\n}\n}\n",
        );

        assert_eq!(qualified_names(&raw), ["rtm::vector_get_x"]);
        assert_eq!(symbol(&raw, "vector_get_x").kind, SymbolKind::Function);
        assert_eq!(symbol(&raw, "vector_get_x").owner, None);
    }

    #[test]
    fn a_macro_before_a_scoped_return_type_keeps_the_namespace() {
        let raw = extract(
            b"#define VULKAN_HPP_INLINE inline\nstruct Widget { std::string label() const; };\n\
              struct Window { LRESULT wndProc(HWND wnd); };\nnamespace vk {\n\
              VULKAN_HPP_INLINE std::string to_string(int value) { return \"x\"; }\n\
              VULKAN_HPP_INLINE std::vector<int> template_type(int value) { return {}; }\n\
              VULKAN_HPP_INLINE Result plain_type(int value) { return {}; }\n\
              VULKAN_HPP_INLINE const std::string& const_qualified(int value) { return s; }\n}\n\
              VULKAN_HPP_INLINE std::string Widget::label() const { return n; }\n\
              LRESULT CALLBACK Window::wndProc(HWND wnd) { return 0; }\n\
              PX_INLINE void Scb::Scene::reset(PxF32 distance) {}\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "Widget",
                "Window",
                "vk::to_string",
                "vk::template_type",
                "vk::plain_type",
                "vk::const_qualified",
                "Widget::label",
                "Window::wndProc",
                "Scene::reset"
            ]
        );
        for name in ["to_string", "template_type"] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        for (name, owner) in [
            ("label", "Widget"),
            ("wndProc", "Window"),
            ("reset", "Scene"),
        ] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Method, "{name}");
            assert_eq!(symbol(&raw, name).owner.as_deref(), Some(owner), "{name}");
        }
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert!(keys.contains(&"vk::to_string(int)"), "{keys:?}");
        assert!(keys.contains(&"Widget::label()const"), "{keys:?}");
        assert!(keys.contains(&"Window::wndProc(HWND)"), "{keys:?}");
        assert_eq!(
            raw.folded_declarations,
            [
                FoldedDeclaration {
                    definition: id_of(&raw, "label"),
                    line: 2
                },
                FoldedDeclaration {
                    definition: id_of(&raw, "wndProc"),
                    line: 3
                }
            ]
        );
    }

    #[test]
    fn a_macro_before_a_pointer_return_type_keeps_the_function() {
        let raw = extract(
            b"namespace ImGui {\n\
              IMGUI_API ImGuiTabItem* TabBarFindTabByID(ImGuiTabBar* tab_bar, ImGuiID tab_id);\n\
              IMGUI_API const ImGuiTabItem** TabBarTabs(ImGuiTabBar* tab_bar) { return tab_bar->Tabs; }\n\
              static IMGUI_API ImVector<int>* const Indices(int order) { return nullptr; }\n}\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "ImGui::TabBarTabs",
                "ImGui::Indices",
                "ImGui::TabBarFindTabByID"
            ]
        );
        for name in ["TabBarTabs", "Indices", "TabBarFindTabByID"] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "ImGui::TabBarTabs(ImGuiTabBar *)",
                "ImGui::TabBarFindTabByID(ImGuiTabBar *,ImGuiID)"
            ]
        );
    }

    #[test]
    fn a_macro_before_a_multi_word_return_type_keeps_the_function() {
        let raw = extract(
            b"STBIWDEF unsigned char *write_png_to_mem(const unsigned char *pixels, int stride_bytes)\n\
              {\n\treturn 0;\n}\n\n\
              MY_API unsigned int *make_uints(int n);\n\n\
              OSSL_DEPRECATEDIN_3_0 unsigned char *MD5(const unsigned char *d, size_t n, unsigned char *md);\n\n\
              static MY_API unsigned char *read_bytes(int n)\n{\n\treturn 0;\n}\n\n\
              MY_API unsigned long long *read_longs(int n);\n\n\
              MY_API unsigned int count(int n);\n\n\
              MY_API const unsigned char *keep_const(int n);\n\n\
              MY_API unsigned char *guarded_ GUARDED_BY(mtx);\n\n\
              MY_API unsigned char *Buffer::data(int n)\n{\n\treturn 0;\n}\n\n\
              namespace pack {\nMY_API unsigned char *encode(int n)\n{\n\treturn 0;\n}\n}\n\n\
              void after(int n);\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "write_png_to_mem",
                "read_bytes",
                "Buffer::data",
                "pack::encode",
                "make_uints",
                "MD5",
                "read_longs",
                "count",
                "keep_const",
                "after"
            ]
        );
        for name in [
            "write_png_to_mem",
            "make_uints",
            "MD5",
            "read_bytes",
            "read_longs",
            "count",
            "keep_const",
            "encode",
            "after",
        ] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        assert_eq!(symbol(&raw, "data").owner.as_deref(), Some("Buffer"));
        assert!(!symbol(&raw, "read_bytes").exported);
        assert_eq!(
            symbol(&raw, "make_uints").signature.as_deref(),
            Some("MY_API unsigned int *make_uints(int n)")
        );
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert!(keys.contains(&"make_uints(int)"), "{keys:?}");
        assert!(keys.contains(&"pack::encode(int)"), "{keys:?}");
    }

    #[test]
    fn a_macro_before_a_scoped_pointer_return_type_keeps_the_function() {
        let raw = extract(
            b"PX_INLINE physx::PxPhysics* PxCreatePhysics(physx::PxU32 version, int x)\n{\n\treturn 0;\n}\n\n\
              DAS_API das::Context * get_context( int stackSize = 0 );\n\n\
              namespace das {\n\
              DAS_API das::Program* compile(int stackSize);\n\
              DAS_API das::detail::Heap* make_heap(int size)\n{\n\treturn 0;\n}\n}\n\n\
              DAS_API das::Pool<int>* make_pool(int size);\n\n\
              PX_INLINE physx::PxScene* Engine::scene(int id)\n{\n\treturn 0;\n}\n\n\
              DAS_API das::Context * MD5(int n);\n\n\
              IMGUI_API ImGuiTabItem* FindTab(int id);\n\n\
              void after(int n);\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "PxCreatePhysics",
                "das::make_heap",
                "Engine::scene",
                "get_context",
                "das::compile",
                "make_pool",
                "MD5",
                "FindTab",
                "after"
            ]
        );
        for name in [
            "PxCreatePhysics",
            "get_context",
            "compile",
            "make_heap",
            "make_pool",
            "MD5",
            "FindTab",
            "after",
        ] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        assert_eq!(symbol(&raw, "scene").owner.as_deref(), Some("Engine"));
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        for key in [
            "PxCreatePhysics(physx :: PxU32,int)",
            "get_context(int)",
            "das::make_heap(int)",
        ] {
            assert!(keys.contains(&key), "{key} in {keys:?}");
        }
    }

    #[test]
    fn a_macro_before_a_name_set_apart_from_its_parameters_keeps_the_name() {
        let raw = extract(
            b"namespace glm {\n\
              template<typename genType>\n\
              GLM_FUNC_QUALIFIER typename genType::row_type row\n\
              (genType const& m, length_t index) { return m[index]; }\n\
              template<typename genType>\n\
              GLM_FUNC_DECL typename genType::col_type column(genType const& m, length_t index);\n\
              INLINE std::string label (int value) { return {}; }\n}\n\
              struct Sound { FMOD_RESULT F_API stop (); };\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "glm::row",
                "glm::label",
                "Sound",
                "glm::column",
                "Sound::stop"
            ]
        );
        for name in ["row", "column", "label"] {
            assert_eq!(symbol(&raw, name).kind, SymbolKind::Function, "{name}");
            assert_eq!(symbol(&raw, name).owner, None, "{name}");
        }
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert_eq!(keys, ["glm::label(int)", "Sound::stop()"]);
    }

    #[test]
    fn a_macro_before_a_conversion_operator_keeps_the_operator() {
        let raw = extract(
            b"ML_INLINE double4::operator int4() const { return int4(x, y, z, w); }\n\
              ML_INLINE double4::operator bool() const { return x != 0; }\n\
              EA_CONSTEXPR inline ns::int128_t::operator unsigned __int128() const { return Low(); }\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "double4::operator int4",
                "double4::operator bool",
                "int128_t::operator unsigned __int128"
            ]
        );
        assert!(
            raw.symbols
                .iter()
                .all(|symbol| symbol.kind == SymbolKind::Method)
        );
        assert_eq!(callees(&raw), ["int4", "Low"]);
    }

    #[test]
    fn a_specifier_before_a_conversion_operator_keeps_the_operator() {
        let raw = extract(
            b"struct C\n{\n\toperator bool() const;\n\toperator int() const;\n\toperator signed char() const;\n\
              \toperator float() const;\n\tint Low() const;\n};\n\n\
              constexpr C::operator bool() const { return true; }\n\n\
              inline C::operator int() const { return 1; }\n\n\
              constexpr C::operator signed char() const { return 1; }\n\n\
              ML_INLINE C::operator float() const { return 1; }\n\n\
              template <typename T> constexpr inline ns::Wrap<T>::operator bool() const\n{\n\treturn true;\n}\n",
        );

        let defined: Vec<(&str, SymbolKind, u32, u32)> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.line > 9)
            .map(|symbol| {
                (
                    symbol.qualified_name.as_str(),
                    symbol.kind,
                    symbol.line,
                    symbol.end_line,
                )
            })
            .collect();
        assert_eq!(
            defined,
            [
                ("C::operator bool", SymbolKind::Method, 10, 10),
                ("C::operator int", SymbolKind::Method, 12, 12),
                ("C::operator signed char", SymbolKind::Method, 14, 14),
                ("C::operator float", SymbolKind::Method, 16, 16),
                ("Wrap::operator bool", SymbolKind::Method, 18, 21),
            ]
        );
        let keys: Vec<&str> = raw
            .callables
            .iter()
            .map(|callable| callable.key.as_str())
            .collect();
        assert_eq!(keys, ["C::Low()const"]);
    }

    #[test]
    fn trailing_macros_on_ctor_dtor_keep_the_member() {
        let raw = extract(
            b"class FS {\n  FS() DAG_TS_ACQUIRE(mtx) { init(); }\n  ~FS() DAG_TS_RELEASE(mtx) { drop(); }\n};\n\
              class LG {\n  ~LG() DAG_TS_RELEASE();\n};\n",
        );
        let names = names(&raw);

        assert!(
            !names.iter().any(|name| name.starts_with("DAG_TS_")),
            "{names:?}"
        );
        let constructor = qualified(&raw, "FS::FS");
        let destructor = qualified(&raw, "FS::~FS");
        let declared_destructor = qualified(&raw, "LG::~LG");
        assert_eq!(constructor.kind, SymbolKind::Method);
        assert_eq!(destructor.kind, SymbolKind::Method);
        assert_eq!(declared_destructor.kind, SymbolKind::Method);
    }

    #[test]
    fn a_lone_macro_line_above_a_class_keeps_the_class_and_its_members() {
        let raw = extract(
            b"#pragma once\n\nJPH_NAMESPACE_BEGIN\n\n/// Either a result or an error\n\
              template <class Type>\nclass [[nodiscard]] Result\n{\npublic:\n\
              \t\t\t\t\t\tResult() { }\n\t\t\t\t\t\tResult(const Result<Type> &inRHS) :\n\
              \t\tmState(inRHS.mState)\n\t{\n\t}\n\
              \tbool HasError() const { return mState == EState::Error; }\n\
              private:\n\tint mState;\n};\n\nJPH_NAMESPACE_END\n",
        );

        assert_eq!(qualified(&raw, "Result::HasError").kind, SymbolKind::Method);
        assert_eq!(symbol(&raw, "Result").kind, SymbolKind::Class);
    }

    #[test]
    fn a_lone_macro_line_above_a_template_head_keeps_the_class_and_its_members() {
        let raw = extract(
            br"FMT_BEGIN_EXPORT

// An output iterator that appends to a buffer.
template <typename T> class basic_appender {
 protected:
  detail::buffer<T>* container;

 public:
  using value_type = T;
  basic_appender(detail::buffer<T>& buf) : container(&buf) {}
  auto operator=(T c) -> basic_appender& {
    container->push_back(c);
    return *this;
  }
  auto operator*() -> basic_appender& { return *this; }
};

class basic_format_arg {
 public:
  int size() const { return 1; }
};

FMT_END_EXPORT
",
        );

        assert_eq!(
            spans(&raw),
            [
                ("basic_appender", 4, 16),
                ("basic_appender::container", 6, 6),
                ("basic_appender::value_type", 9, 9),
                ("basic_appender::basic_appender", 10, 10),
                ("basic_appender::operator=", 11, 14),
                ("basic_appender::operator*", 15, 15),
                ("basic_format_arg", 18, 21),
                ("basic_format_arg::size", 20, 20)
            ]
        );
    }

    #[test]
    fn a_lone_macro_line_above_a_class_head_with_its_own_macro_keeps_the_class_and_its_members() {
        let raw = extract(
            b"OPENMAYA_MAJOR_NAMESPACE_OPEN\n\nclass OPENMAYA_EXPORT MUniqueString\n{\npublic:\n\
              \tstatic MUniqueString intern(const char *name);\n\
              \tMUniqueString &operator=(const MUniqueString &other);\n\tint length() const;\n};\n\n\
              OPENMAYA_NAMESPACE_CLOSE\n",
        );

        assert_eq!(
            spans(&raw),
            [
                ("MUniqueString", 3, 9),
                ("MUniqueString::intern", 6, 6),
                ("MUniqueString::operator=", 7, 7),
                ("MUniqueString::length", 8, 8)
            ]
        );
    }

    #[test]
    fn a_lone_macro_line_above_a_class_past_a_block_comment_keeps_the_class_and_its_members() {
        let raw = extract(
            b"namespace physx\n{\nnamespace apex\n{\n\nPX_PUSH_PACK_DEFAULT\n\n\
              /**\n\\brief Base class for all interface classes\n*/\nclass NxApexInterface\n{\npublic:\n\
              \tNxApexInterface() : userData(NULL) {}\n\tvirtual void release() = 0;\n\tvoid* userData;\n};\n\n\
              PX_POP_PACK\n\n}\n} // namespace physx\n",
        );

        assert_eq!(
            spans(&raw),
            [
                ("physx::apex::NxApexInterface", 11, 17),
                ("NxApexInterface::NxApexInterface", 14, 14),
                ("NxApexInterface::release", 15, 15),
                ("NxApexInterface::userData", 16, 16)
            ]
        );
    }

    #[test]
    fn a_namespace_under_a_lone_macro_line_or_around_macros_keeps_its_name() {
        let under = extract(
            b"RTM_IMPL_FILE_PRAGMA_PUSH\n\nnamespace rtm\n{\n\
              \tinline float vector_get_x(float input) { return input; }\n\
              \tinline float vector_get_y(float input) { return input; }\n}\n\nRTM_IMPL_FILE_PRAGMA_POP\n",
        );
        let around = extract(b"namespace rtm { MACRO_BEGIN void f() {} MACRO_END }\n");

        assert_eq!(
            qualified_names(&under),
            ["rtm::vector_get_x", "rtm::vector_get_y"]
        );
        assert_eq!(qualified_names(&around), ["rtm::f"]);
    }

    #[test]
    fn a_namespace_a_macro_opens_inside_another_keeps_its_definitions() {
        let enumeration = extract(
            b"namespace absl {\nABSL_NAMESPACE_BEGIN\n\nenum ConstInitType {\n  kConstInit,\n};\n\n\
              ABSL_NAMESPACE_END\n}  // namespace absl\n",
        );
        let function = extract(
            b"namespace absl {\nABSL_NAMESPACE_BEGIN\n\n// Compares.\nbool Equal(int a, int b) noexcept {\n\
              \x20 return a == b;\n}\n\nABSL_NAMESPACE_END\n}  // namespace absl\n",
        );

        assert_eq!(qualified_names(&enumeration), ["absl::ConstInitType"]);
        let equal = qualified(&function, "absl::Equal");
        assert_eq!(
            (equal.line, equal.signature.as_deref()),
            (5, Some("bool Equal(int a, int b) noexcept"))
        );
    }

    #[test]
    fn a_macro_call_that_closes_a_table_above_an_error_keeps_the_function_after_it() {
        let raw = extract(
            b"int get_params(void) { return 1; }\n\n\
              TABLE_START(generic)\n{ PARAM_MAC, PARAM_PTR, NULL, 0, UNMODIFIED },\nTABLE_END(generic)\n\n\
              SETTABLE_START(generic)\nPARAM_uint(USE_BITS, NULL),\nPARAM_uint(TLS_VERSION, NULL),\n\
              PARAM_size_t(MAC_SIZE, NULL),\nSETTABLE_END(generic)\n\n\
              /*\n * Variable key length\n */\n\
              int var_keylen_set_ctx_params(void *vctx, const PARAM params[])\n{\n\
              \x20   const PARAM *p;\n    if (params == NULL)\n        return 1;\n\
              \x20   p = locate_const(params, KEYLEN);\n    return p != NULL;\n}\n",
        );

        assert_eq!(
            symbol(&raw, "var_keylen_set_ctx_params").kind,
            SymbolKind::Function
        );
    }

    #[test]
    fn a_macro_line_above_a_function_an_error_starts_at_keeps_the_function() {
        let raw = extract(
            b"template<typename TWork>\nHRESULT ApiImpl(TWork&& work) noexcept\n{\n    return work();\n}\n\n\
              NAMESPACE_SOCIAL_CPP_END\n\n\
              STDAPI_(bool) IsUserPlayingTitle(\n    _In_ const Record* record,\n    _In_ uint32_t titleId\n\
              ) XBL_NOEXCEPT\ntry\n{\n    if (record == nullptr)\n    {\n        return false;\n    }\n\
              \x20   return record->titleId == titleId;\n}\nCATCH_RETURN_WITH(false);\n\n\
              STDAPI GetType(\n    _In_ Handle handle,\n    _Out_ Type* type\n) XBL_NOEXCEPT\ntry\n{\n\
              \x20   return ApiImpl([&]() { return S_OK; });\n}\nCATCH_RETURN()\n",
        );

        assert_eq!(
            symbol(&raw, "IsUserPlayingTitle").kind,
            SymbolKind::Function
        );
        assert_eq!(symbol(&raw, "GetType").kind, SymbolKind::Function);
    }

    #[test]
    fn an_attribute_or_an_alignment_macro_in_a_class_head_keeps_its_members() {
        let raw = extract(
            b"class [[nodiscard]] AABox { void f(); };\nstruct alignas(16) Vec4 { void g(); };\n\
              struct __declspec(align(16)) M { void h(); };\nstruct DAGOR_ALIGN(16) N { void i(); };\n\
              class __attribute__((aligned(16))) P { void j(); };\n\
              class alignas(32) DLL_API E final { void l(); };\n",
        );

        for member in ["AABox::f", "Vec4::g", "M::h", "N::i", "P::j", "E::l"] {
            assert_eq!(qualified(&raw, member).kind, SymbolKind::Method, "{member}");
        }
    }

    #[test]
    fn a_macro_in_a_class_head_keeps_the_class_and_its_members() {
        let raw = extract(
            b"namespace libtorrent {\n\
              class TORRENT_EXTRA_EXPORT torrent : public request_callback, public peer_class_set\n\
              {\npublic:\n  void start();\n  int size() const { return 1; }\n};\n}\n\
              struct DLL_API Foo { void run(); };\nclass JPH_EXPORT Bar final { void go(); };\n\
              struct STAT st;\n",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "libtorrent::torrent",
                "torrent::size",
                "Foo",
                "Bar",
                "st",
                "torrent::start",
                "Foo::run",
                "Bar::go"
            ]
        );
        assert_eq!(
            symbol(&raw, "torrent").parents,
            ["request_callback", "peer_class_set"]
        );
        assert_eq!(symbol(&raw, "size").kind, SymbolKind::Method);
        assert_eq!(symbol(&raw, "st").kind, SymbolKind::Variable);
    }

    #[test]
    fn a_specialization_whose_head_holds_a_macro_keeps_its_members() {
        let raw = extract(
            b"namespace rtm {\ntemplate<>\nstruct RTM_DEPRECATED(\"Use 'related_types'\") float_traits<float>\n\
              {\n\tusing mask4 = mask4f;\n\tstatic float zero() { return 0.0f; }\n};\n}\n\
              namespace ozz {\ntemplate <>\nstruct OZZ_BASE_DLL Extern<math::Float2> {\n\
              \tstatic void Save(OArchive& _archive, const math::Float2* _values, size_t _count);\n};\n}\n",
        );

        assert_eq!(
            spans(&raw),
            [
                ("rtm::float_traits", 3, 7),
                ("rtm::float_traits::mask4", 5, 5),
                ("float_traits::zero", 6, 6),
                ("ozz::Extern", 11, 13),
                ("Extern::Save", 12, 12)
            ]
        );
    }

    #[test]
    fn a_specialization_keeps_its_head_as_its_signature_and_its_primary_none() {
        let raw = extract(
            b"template <typename T> struct Box { T size; };\n\
              template <> struct Box<bool> : Bits { int size; };\n\
              template <typename T> struct Box<T *> { int size; };\n\
              template <> struct ns::Box<bool> { int size; };\n\
              struct Plain { int size; };\n",
        );

        let classes: Vec<(&str, Option<&str>)> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Struct)
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.signature.as_deref()))
            .collect();
        assert_eq!(
            classes,
            [
                ("Box", None),
                ("Box", Some("template <> struct Box<bool> : Bits")),
                ("Box", Some("template <typename T> struct Box<T *>")),
                ("ns::Box", Some("template <> struct ns::Box<bool>")),
                ("Plain", None),
            ]
        );
    }

    #[test]
    fn a_macro_that_takes_the_class_key_keeps_the_class_and_its_members() {
        let raw = extract(
            b"ATTRIBUTE_ALIGNED16(class)\nbtCollisionObject\n{\npublic:\n\
              \tconst btBroadphaseProxy *getBroadphaseHandle() const { return m_broadphaseHandle; }\n\n\
              protected:\n\tbtBroadphaseProxy *m_broadphaseHandle;\n};\n\n\
              ATTRIBUTE_ALIGNED16(struct) b3Contact4\n{\n\tint m_x;\n\tint get() const { return m_x; }\n};\n\n\
              class btPlain\n{\npublic:\n\tint get() const { return 1; }\n};\n",
        );

        assert_eq!(
            spans(&raw),
            [
                ("btCollisionObject", 1, 9),
                ("btCollisionObject::getBroadphaseHandle", 5, 5),
                ("btCollisionObject::m_broadphaseHandle", 8, 8),
                ("b3Contact4", 11, 15),
                ("b3Contact4::m_x", 13, 13),
                ("b3Contact4::get", 14, 14),
                ("btPlain", 17, 21),
                ("btPlain::get", 20, 20)
            ]
        );
    }

    #[test]
    fn a_macro_between_a_class_name_and_its_body_keeps_the_class_and_its_members() {
        let raw = extract(
            br"namespace flexbuffers {
class Vector {
 public:
  int size() const { return 0; }
};

class Builder FLATBUFFERS_FINAL_CLASS {
 public:
  Builder(int initial_size = 256) : size_(initial_size) {}
  int StartVector() { return size_; }
  int Vector() { return 1; }

 private:
  int size_;
};
}

namespace libtorrent {
struct TORRENT_EXPORT torrent_removed_alert TORRENT_FINAL : torrent_alert
{
	int type() const { return 1; }
};
}
",
        );

        assert_eq!(
            spans(&raw),
            [
                ("flexbuffers::Vector", 2, 5),
                ("Vector::size", 4, 4),
                ("flexbuffers::Builder", 7, 15),
                ("Builder::Builder", 9, 9),
                ("Builder::StartVector", 10, 10),
                ("Builder::Vector", 11, 11),
                ("Builder::size_", 14, 14),
                ("libtorrent::torrent_removed_alert", 19, 22),
                ("torrent_removed_alert::type", 21, 21)
            ]
        );
        assert_eq!(
            symbol(&raw, "torrent_removed_alert").parents,
            ["torrent_alert"]
        );
    }

    #[test]
    fn an_out_of_line_definition_through_a_namespace_is_a_free_function() {
        let raw = extract(
            b"namespace ImGui { static void NavUpdate(); void EndGroup(); }\nvoid ImGui::EndGroup() {}\n\
              using namespace physx::shdfnd;\nint shdfnd::readChunk() { return 0; }\n\
              class Widget { int area(); };\nint Widget::area() { return 0; }\nint Other::size() { return 0; }\n\
              namespace Twin {}\nclass Twin { void run(); };\nvoid Twin::run() {}\n\
              namespace A::N {}\nclass N {};\nvoid A::N::f() {}\nnamespace B { void N::g() {} }\n\
              namespace C { struct M {}; }\nnamespace M {}\nnamespace D { using namespace C; }\nvoid M::h() {}\n\
              namespace Outer { class ImGui { void Begin() {} }; }\nnamespace Kind {}\nenum Kind { K };\n\
              void Kind::m() {}\nnamespace Alias {}\nusing Alias = Widget;\nvoid Alias::n() {}\n",
        );

        let shapes: Vec<(&str, SymbolKind, Option<&str>)> = raw
            .symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method))
            .map(|symbol| {
                (
                    symbol.qualified_name.as_str(),
                    symbol.kind,
                    symbol.owner.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            shapes,
            [
                ("ImGui::EndGroup", SymbolKind::Function, None),
                ("shdfnd::readChunk", SymbolKind::Function, None),
                ("Widget::area", SymbolKind::Method, Some("Widget")),
                ("Other::size", SymbolKind::Method, Some("Other")),
                ("Twin::run", SymbolKind::Method, Some("Twin")),
                ("N::f", SymbolKind::Function, None),
                ("N::g", SymbolKind::Method, Some("N")),
                ("M::h", SymbolKind::Function, None),
                ("ImGui::Begin", SymbolKind::Method, Some("ImGui")),
                ("Kind::m", SymbolKind::Method, Some("Kind")),
                ("Alias::n", SymbolKind::Method, Some("Alias")),
                ("ImGui::NavUpdate", SymbolKind::Function, None),
            ]
        );
    }

    #[test]
    fn a_conversion_operator_is_a_member() {
        let raw = extract(
            b"struct Conv { operator bool() const { return true; } explicit operator int() const;\n\
              operator const char *() const; };\nConv::operator int() const { return 0; }\n\
              template <class T> Wrap<T>::operator T&() { return v; }\n",
        );

        let members: Vec<(&str, Option<&str>, bool)> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Method)
            .map(|symbol| {
                (
                    symbol.qualified_name.as_str(),
                    symbol.owner.as_deref(),
                    symbol.flags.is_entry(),
                )
            })
            .collect();
        assert_eq!(
            members,
            [
                ("Conv::operator bool", Some("Conv"), true),
                ("Conv::operator int", Some("Conv"), true),
                ("Wrap::operator T&", Some("Wrap"), true),
                ("Conv::operator int", Some("Conv"), true),
                ("Conv::operator const char *", Some("Conv"), true),
            ]
        );
    }

    #[test]
    fn a_template_parameter_is_not_a_callee() {
        let raw = extract(
            b"template <class Compare, typename T = int, typename... Rest, auto F>\n\
              void sort3(Compare c) { Compare()(1, 2); T(); F(); auto *p = new T(); c(1); helper(); }\n\
              template <class Item> struct Box { void fill() { Item(); refill(); } };\n\
              template <template <class Elem> class Seq, void (*Hook)(), int &Ref, auto... Steps>\n\
              void build() { Seq<int>(); Hook(); Elem(); (Steps(), ...); }\n\
              void plain() { Compare(); T(); }\n",
        );

        let calls: Vec<(&str, SymbolId)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.site_of))
            .collect();
        assert_eq!(
            calls,
            [
                ("helper", id_of(&raw, "sort3")),
                ("refill", id_of(&raw, "fill")),
                ("Elem", id_of(&raw, "build")),
                ("Compare", id_of(&raw, "plain")),
                ("T", id_of(&raw, "plain")),
            ]
        );
    }

    #[test]
    fn a_call_through_a_parameter_or_a_local_is_not_a_callee() {
        let raw = extract(
            b"struct Node { template <typename T> void visitRefs(T visitor) { visitor(type); refs(visitor); } };\n\
              void each(void (*visit)(int), int n) { visit(n); auto step = [](int) {}; step(n); }\n\
              void scoped() { { auto inner = [] {}; inner(); } inner(); }\n\
              void captured() { run([hook = make()] { hook(); }); hook(); }\n\
              void after() { visitor(); visit(1); step(2); }\n",
        );

        let calls: Vec<(&str, SymbolId)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.site_of))
            .collect();
        assert_eq!(
            calls,
            [
                ("refs", id_of(&raw, "visitRefs")),
                ("inner", id_of(&raw, "scoped")),
                ("run", id_of(&raw, "captured")),
                ("make", id_of(&raw, "captured")),
                ("hook", id_of(&raw, "captured")),
                ("visitor", id_of(&raw, "after")),
                ("visit", id_of(&raw, "after")),
                ("step", id_of(&raw, "after")),
            ]
        );
    }

    #[test]
    fn a_template_parameter_is_not_a_parent() {
        let raw = extract(
            b"template <typename Compare> struct rb_base : public Compare {};\n\
              template <class T> class Holder : public Base<T>, public T {};\nclass Plain : public Compare {};\n\
              template <template <class Elem> class Policy> struct Adapter : public Policy<int>, public Elem {};\n\
              template <template <class> class Store = Vec> struct Cache : public Store<int> {};\n",
        );

        assert_eq!(symbol(&raw, "rb_base").parents, Vec::<String>::new());
        assert_eq!(symbol(&raw, "Holder").parents, ["Base"]);
        assert_eq!(symbol(&raw, "Adapter").parents, ["Elem"]);
        assert_eq!(symbol(&raw, "Cache").parents, Vec::<String>::new());
        assert_eq!(symbol(&raw, "Plain").parents, ["Compare"]);
    }

    #[test]
    fn templated_bases_record_their_name_without_arguments() {
        let raw = extract(
            b"class Foo : public Base<Foo> {};\n\
              class Bar : public ns::Base, protected Mixin<int>, public Plain {};\n\
              class Q : public ns::Tmpl<int> {};\n",
        );

        assert_eq!(symbol(&raw, "Foo").parents, ["Base"]);
        assert_eq!(symbol(&raw, "Bar").parents, ["ns::Base", "Mixin", "Plain"]);
        assert_eq!(symbol(&raw, "Q").parents, ["ns::Tmpl"]);
    }

    #[test]
    fn anonymous_namespace_members_are_unexported() {
        let raw = extract(HEADERISH);

        assert!(!symbol(&raw, "local_helper").exported);
    }

    #[test]
    fn a_base_initializer_records_a_call_of_the_base_beside_its_write() {
        let raw = extract(b"struct D : B { D(int x) : B(x) {} };");

        assert_eq!(callees(&raw), ["B"]);
        let caller = &raw.symbols[raw.calls[0].site_of.0 as usize];
        assert_eq!(caller.qualified_name, "D::D");
        assert!(!raw.calls[0].calls_a_method);
        assert!(raw.calls[0].receiver.is_none());
        assert!(
            raw.uses
                .iter()
                .any(|site| site.name == "B" && site.access == Access::Write)
        );
    }

    #[test]
    fn a_bare_initializer_of_a_base_declared_with_a_scope_records_a_call() {
        let raw = extract(b"struct D : ns::B { D(int x) : B(x) {} };");

        assert_eq!(callees(&raw), ["B"]);
    }

    #[test]
    fn an_initializer_of_the_caller_s_own_name_records_no_call() {
        let raw = extract(b"template <> struct D<int> : D<char> { D(int x) : D<char>(x) {} };");

        assert_eq!(callees(&raw), Vec::<&str>::new());
    }

    #[test]
    fn a_scoped_base_of_the_caller_s_name_records_its_constructor_in_full() {
        let raw = extract(b"struct B : a::B { B(int x) : a::B(x) {} };");

        assert_eq!(callees(&raw), ["a::B::B"]);
    }

    #[test]
    fn a_scoped_template_base_of_the_caller_s_name_records_its_constructor_with_no_arguments() {
        let raw = extract(b"struct B : a::B<int> { B(int x) : a::B<int>(x) {} };");

        assert_eq!(callees(&raw), ["a::B::B"]);
    }

    #[test]
    fn a_base_written_with_a_scope_records_its_bare_name() {
        let raw = extract(b"struct D : ns::B { D(int x) : ns::B(x) {} };");

        assert_eq!(callees(&raw), ["B"]);
    }

    #[test]
    fn a_base_written_with_a_scope_and_arguments_records_its_bare_name() {
        let raw = extract(b"struct D : ns::B<int> { D(int x) : ns::B<int>(x) {} };");

        assert_eq!(callees(&raw), ["B"]);
    }

    #[test]
    fn a_base_written_with_arguments_records_its_bare_name_outside_its_class() {
        let raw = extract(b"D::D(int x) : B<int>(x) {}");

        assert_eq!(callees(&raw), ["B"]);
    }

    #[test]
    fn a_member_initializer_records_a_write_and_no_call() {
        let raw = extract(b"struct D : B { int m; D(int x) : B(x), m(x) {} };");

        assert_eq!(callees(&raw), ["B"]);
        assert!(raw.uses.iter().any(|site| {
            site.name == "m" && site.access == Access::Write && site.receiver == Receiver::Own
        }));
    }

    #[test]
    fn a_name_of_a_base_and_a_field_records_a_call_and_a_write() {
        let raw = extract(b"struct D : B { int B; D(int x) : B(x) {} };");

        assert_eq!(callees(&raw), ["B"]);
        assert!(
            raw.uses
                .iter()
                .any(|site| site.name == "B" && site.access == Access::Write)
        );
    }

    #[test]
    fn a_constructor_outside_its_class_body_writes_a_name_it_cannot_tell_for_a_base() {
        let raw = extract(b"struct D : B { D(int x); };\nD::D(int x) : B(x) {}");

        assert_eq!(callees(&raw), Vec::<&str>::new());
        assert!(
            raw.uses
                .iter()
                .any(|site| site.name == "B" && site.access == Access::Write)
        );
    }

    #[test]
    fn a_template_parameter_initialized_as_a_base_records_no_call() {
        let raw = extract(b"template <class T> struct D : T { D(int x) : T(x), T::U<int>(x) {} };");

        assert_eq!(callees(&raw), ["U"]);
    }

    #[test]
    fn a_template_template_parameter_initialized_as_a_base_records_no_call() {
        let raw = extract(
            b"template <template <class> class T> struct D : T<int> { D(int x) : T<int>(x) {} };",
        );

        assert_eq!(callees(&raw), Vec::<&str>::new());
    }

    #[test]
    fn new_expressions_become_constructor_calls_by_base_name() {
        let raw = extract(HEADERISH);

        let callees = callees(&raw);
        assert!(callees.contains(&"Window"), "{callees:?}");
    }

    #[test]
    fn prototypes_emit_only_without_a_same_file_definition() {
        let raw = extract(HEADERISH);

        let start = qualified(&raw, "Engine::start");
        assert_eq!(start.kind, SymbolKind::Method);
        assert_eq!(start.signature.as_deref(), Some("void start()"));
        assert!(names(&raw).contains(&"free_proto"));
        let declared_and_defined = names(&raw)
            .iter()
            .filter(|name| **name == "defined_and_declared")
            .count();
        assert_eq!(declared_and_defined, 1);
    }

    fn spans_of(raw: &RawExtraction, qualified_name: &str) -> Vec<(u32, u32)> {
        raw.symbols
            .iter()
            .filter(|symbol| symbol.qualified_name == qualified_name)
            .map(|symbol| (symbol.line, symbol.end_line))
            .collect()
    }

    #[test]
    fn a_class_template_member_folds_into_its_out_of_line_definition() {
        let raw = extract(
            b"template <typename T>\nstruct Temp {\n    int eval(int c);\n};\n\n\
              template <typename T>\nint Temp<T>::eval(int c)\n{\n    return c;\n}\n",
        );

        assert_eq!(spans_of(&raw, "Temp::eval"), [(7, 10)]);
        assert_eq!(
            raw.folded_declarations,
            [FoldedDeclaration {
                definition: id_of(&raw, "eval"),
                line: 3
            }]
        );
    }

    #[test]
    fn a_class_template_member_folds_whatever_the_definition_names_its_parameters() {
        let raw = extract(
            b"template <class T, int N>\nstruct Arr {\n    void set(const T &value);\n    struct Cell {\n\
              void put(T value, int at);\n    };\n};\n\
              template <class U, int M>\nvoid Arr<U, M>::set(const U &v) {}\n\
              template <class U, int M>\nvoid Arr<U, M>::Cell::put(U v, int i) {}\n\
              template <class... Ts>\nstruct Tuple {\n    void take(Ts... all);\n};\n\
              template <class... Us>\nvoid Tuple<Us...>::take(Us... all) {}\n",
        );

        assert_eq!(spans_of(&raw, "Arr::set"), [(9, 9)]);
        assert_eq!(spans_of(&raw, "Cell::put"), [(11, 11)]);
        assert_eq!(spans_of(&raw, "Tuple::take"), [(17, 17)]);
        assert_eq!(raw.folded_declarations.len(), 3);
    }

    #[test]
    fn a_class_template_member_keys_alike_in_the_header_and_the_file_that_defines_it() {
        let header = extract(b"template <class T>\nstruct Temp {\n    void set(T value);\n};\n");
        let definition = extract(b"template <class U>\nvoid Temp<U>::set(U value) {}\n");

        let keys = |raw: &RawExtraction| -> Vec<(String, CallableRole)> {
            raw.callables
                .iter()
                .map(|callable| (callable.key.clone(), callable.role))
                .collect()
        };
        assert_eq!(
            keys(&header),
            [("Temp::set($0)".to_owned(), CallableRole::Declaration)]
        );
        assert_eq!(
            keys(&definition),
            [("Temp::set($0)".to_owned(), CallableRole::Definition)]
        );
    }

    #[test]
    fn the_overloads_of_a_class_template_member_stay_apart() {
        let raw = extract(
            b"template <typename T>\nstruct Temp {\n    int eval(int c);\n    int eval(T c);\n};\n\
              template <typename T>\nint Temp<T>::eval(int c) { return one(c); }\n\
              template <typename T>\nint Temp<T>::eval(T c) { return two(c); }\n",
        );

        assert_eq!(spans_of(&raw, "Temp::eval"), [(7, 7), (9, 9)]);
        let called: Vec<(&str, u32)> = raw
            .calls
            .iter()
            .map(|call| {
                (
                    call.callee.as_str(),
                    raw.symbols[call.site_of.0 as usize].line,
                )
            })
            .collect();
        assert_eq!(called, [("one", 7), ("two", 9)]);
    }

    #[test]
    fn a_class_template_member_never_folds_a_specialization() {
        let raw = extract(
            b"template <typename T>\nstruct Temp {\n    int eval(int c);\n};\n\
              template <>\nstruct Temp<char> {\n    int eval(int c);\n};\n\
              template <typename T>\nstruct Temp<T *> {\n    int eval(int c);\n};\n\
              template <>\nint Temp<int>::eval(int c) { return 1; }\n\
              int Temp<char>::eval(int c) { return 2; }\n\
              template <typename T>\nint Temp<T *>::eval(int c) { return 3; }\n",
        );

        assert_eq!(
            spans_of(&raw, "Temp::eval"),
            [(14, 14), (15, 15), (17, 17), (3, 3), (7, 7), (11, 11)]
        );
        assert_eq!(raw.folded_declarations.len(), 0);
    }

    #[test]
    fn an_empty_head_folds_no_plain_member_into_a_specialization() {
        let raw = extract(
            b"struct Plain {\n    template <typename U>\n    void member(U value);\n    void member(int value);\n};\n\
              template <>\nvoid Plain::member(int value) {}\n",
        );

        assert_eq!(spans_of(&raw, "Plain::member"), [(7, 7), (3, 3), (4, 4)]);
    }

    #[test]
    fn a_template_of_its_own_keeps_its_declaration() {
        let raw = extract(
            b"template <typename T>\nvoid free_fn(T value);\n\
              template <typename T>\nvoid free_fn(T value) {}\n\
              struct Plain {\n    template <typename U>\n    void member(U value);\n};\n\
              template <typename U>\nvoid Plain::member(U value) {}\n\
              template <typename T>\nstruct Temp {\n    template <typename U>\n    void member(U value);\n};\n\
              template <typename T>\ntemplate <typename U>\nvoid Temp<T>::member(U value) {}\n",
        );

        assert_eq!(spans_of(&raw, "free_fn").len(), 2);
        assert_eq!(spans_of(&raw, "Plain::member").len(), 2);
        assert_eq!(spans_of(&raw, "Temp::member").len(), 2);
    }

    #[test]
    fn namespace_members_store_qualified_names() {
        let raw = extract(
            br"
namespace d3d {
void clearview() {}
}
namespace a {
namespace b {
void g() {}
}
}
namespace c::d {
void h() {}
}
namespace ns {
class Widget {
public:
    void area() {}
};
}
namespace {
void hidden() {}
}
void global_fn() {}
",
        );

        assert_eq!(symbol(&raw, "clearview").qualified_name, "d3d::clearview");
        assert_eq!(symbol(&raw, "clearview").name, "clearview");
        assert_eq!(symbol(&raw, "g").qualified_name, "a::b::g");
        assert_eq!(symbol(&raw, "h").qualified_name, "c::d::h");
        assert_eq!(symbol(&raw, "Widget").qualified_name, "ns::Widget");
        assert_eq!(symbol(&raw, "area").qualified_name, "Widget::area");
        assert_eq!(symbol(&raw, "area").owner.as_deref(), Some("Widget"));
        assert_eq!(symbol(&raw, "hidden").qualified_name, "hidden");
        assert_eq!(symbol(&raw, "global_fn").qualified_name, "global_fn");
    }

    #[test]
    fn namespace_prototypes_qualify_at_queue_time() {
        let raw = extract(
            br"
namespace ns {
void f();
class C {
public:
    void m(int);
};
}
void free_proto(int);
",
        );

        assert_eq!(symbol(&raw, "f").qualified_name, "ns::f");
        assert_eq!(symbol(&raw, "f").name, "f");
        assert_eq!(symbol(&raw, "m").qualified_name, "C::m");
        assert_eq!(symbol(&raw, "m").owner.as_deref(), Some("C"));
        assert_eq!(symbol(&raw, "free_proto").qualified_name, "free_proto");
    }

    #[test]
    fn entry_points_are_free_main_like_functions() {
        let raw = extract(
            br"
int main() { return 0; }

void DagorWinMain() {}

int wmain() { return 0; }

int WinMain() { return 0; }

int wWinMain() { return 0; }

void helper() {}

struct App {
    void main() {}
};
",
        );

        for name in ["main", "wmain", "WinMain", "wWinMain", "DagorWinMain"] {
            assert!(qualified(&raw, name).flags.is_entry(), "{name}");
        }
        assert!(!qualified(&raw, "helper").flags.is_entry());
        assert!(!qualified(&raw, "App::main").flags.is_entry());
        let main = qualified(&raw, "main");
        assert!(!main.flags.is_test());
        assert!(!main.flags.is_generated());
    }

    #[test]
    fn entry_requires_global_namespace_scope() {
        let raw = extract(
            br"
int main() { return 0; }

namespace app {

int WinMain() { return 0; }

namespace nested {
int wmain() { return 0; }
}

}
",
        );

        assert!(qualified(&raw, "main").flags.is_entry());
        assert!(!qualified(&raw, "app::WinMain").flags.is_entry());
        assert!(!qualified(&raw, "app::nested::wmain").flags.is_entry());
    }

    #[test]
    fn a_constructor_a_destructor_or_an_operator_is_an_entry_point() {
        let raw = extract(
            br"
struct Box {
    Box() {}
    ~Box() {}
    bool operator==(const Box &) const { return true; }
    void operatorName() {}
    void make() {}
};

Box::Box(int) {}

Box operator+(const Box &, const Box &) { return Box(); }
",
        );

        for name in ["Box::Box", "Box::~Box", "Box::operator==", "operator+"] {
            let entries: Vec<bool> = raw
                .symbols
                .iter()
                .filter(|symbol| symbol.qualified_name == name)
                .map(|symbol| symbol.flags.is_entry())
                .collect();
            assert!(
                !entries.is_empty() && !entries.contains(&false),
                "{name}: {entries:?}"
            );
        }
        assert!(!qualified(&raw, "Box::operatorName").flags.is_entry());
        assert!(!qualified(&raw, "Box::make").flags.is_entry());
        assert!(!qualified(&raw, "Box").flags.is_entry());
    }

    #[test]
    fn a_generated_banner_flags_every_symbol_of_the_file() {
        let banner =
            extract(b"// @generated by tool\nclass Emitted { int field; };\nvoid emitted() {}\n");
        let plain = extract(b"class Emitted { int field; };\nvoid emitted() {}\n");

        assert_eq!(banner.symbols.len(), 3);
        assert!(
            banner
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_generated()),
            "{:?}",
            banner.symbols
        );
        assert!(
            !plain
                .symbols
                .iter()
                .any(|symbol| symbol.flags.is_generated()),
            "{:?}",
            plain.symbols
        );
    }

    #[test]
    fn each_marker_in_a_line_or_a_block_comment_announces_the_banner() {
        for source in [
            &b"// DO NOT EDIT\nvoid emitted() {}\n"[..],
            b"// Code generated by tool.\nvoid emitted() {}\n",
            b"/*\n * @generated\n */\nvoid emitted() {}\n",
        ] {
            assert!(BANNER.opens(source), "{}", String::from_utf8_lossy(source));
        }
    }

    #[test]
    fn forward_declarations_and_elaborated_refs_emit_no_symbols() {
        let raw = extract(
            br"
struct Fwd;
struct Fwd { int x; };
class Widget;
template<typename T> class Vec;
class Other* ptr = nullptr;
void anchor() {}
",
        );

        assert_eq!(qualified_names(&raw), ["Fwd", "Fwd::x", "ptr", "anchor"]);
    }

    #[test]
    fn forward_declared_enums_emit_no_symbols() {
        let raw = extract(
            br"
enum class Status : int;
enum class Status : int { Ok, Err };
enum Plain : int;
void anchor() {}
",
        );

        assert_eq!(qualified_names(&raw), ["Status", "anchor"]);
        assert_eq!(symbol(&raw, "Status").kind, SymbolKind::Enum);
    }

    #[test]
    fn unions_emit_struct_symbols_with_member_attribution() {
        let raw = extract(
            br"
union U {
    int a;
    float b;
    int as_int() { return a; }
};
union Fwd;
class C {
    union { int x; };
};
",
        );

        assert_eq!(
            qualified_names(&raw),
            ["U", "U::a", "U::b", "U::as_int", "C", "C::x"]
        );
        assert_eq!(symbol(&raw, "U").kind, SymbolKind::Struct);
        assert_eq!(symbol(&raw, "as_int").owner.as_deref(), Some("U"));
    }

    #[test]
    fn type_aliases_emit_symbols() {
        let raw = extract(
            br"
typedef struct { int x; } TD;
typedef unsigned long ulong_t;
typedef int *Ptr;
typedef int Arr[10];
typedef int Multi1, Multi2;
typedef void (*Cb)(int);
using Alias = TD;
template<class T> using VecT = T;
class K {
    using value_type = int;
};
",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "TD",
                "TD::x",
                "ulong_t",
                "Ptr",
                "Arr",
                "Multi1",
                "Multi2",
                "Alias",
                "VecT",
                "K",
                "K::value_type"
            ]
        );
        assert_eq!(symbol(&raw, "TD").kind, SymbolKind::TypeAlias);
        assert_eq!(symbol(&raw, "Alias").kind, SymbolKind::TypeAlias);
        assert_eq!(symbol(&raw, "VecT").kind, SymbolKind::TypeAlias);
        let in_class_alias = symbol(&raw, "value_type");
        assert_eq!(in_class_alias.kind, SymbolKind::TypeAlias);
        assert_eq!(in_class_alias.owner.as_deref(), Some("K"));
    }

    #[test]
    fn a_typedef_that_defines_the_tag_it_names_records_the_definition_alone() {
        let raw = extract(
            br"
typedef struct S { int x; } S, *SP;
typedef union U { int x; } U;
class K {
    typedef class N { int y; } N;
};
typedef enum E { A } E;
",
        );

        let kinds: Vec<_> = raw
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.kind != SymbolKind::Field && symbol.kind != SymbolKind::EnumVariant
            })
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("SP", SymbolKind::TypeAlias),
                ("S", SymbolKind::Struct),
                ("U", SymbolKind::Struct),
                ("K", SymbolKind::Class),
                ("K::N", SymbolKind::Class),
                ("E", SymbolKind::Enum),
            ]
        );
    }

    #[test]
    fn a_typedef_of_another_name_or_shape_keeps_its_alias() {
        let raw = extract(
            br"
typedef struct Tag { int x; } Name;
typedef struct Same { int x; } *Same;
struct Forward;
typedef struct Forward Forward;
typedef enum Kind { A } Other;
typedef enum Pointed { B } *Pointed;
enum Opaque : int;
typedef enum Opaque Opaque;
",
        );

        let aliases: Vec<_> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::TypeAlias)
            .map(|symbol| symbol.name.as_str())
            .collect();
        assert_eq!(
            aliases,
            ["Name", "Same", "Forward", "Other", "Pointed", "Opaque"]
        );
    }

    #[test]
    fn nested_types_qualify_by_their_enclosing_class() {
        let raw = extract(
            br"
namespace host_ns {
class Host {
public:
    struct Params { int size; };
    enum class Mode { A, B };
    void run() {
        struct Local { int x; };
        using LocalAlias = int;
    }
};
class Guest {
    struct Params { int count; };
    int x = [] { struct InLambda { int a; }; return 0; }();
};
struct Params { int other; };
}
namespace guest_ns {
class Host {
    struct Params { int depth; };
    struct Inner { enum Kind { X }; };
};
}
",
        );

        assert_eq!(
            qualified_names(&raw),
            [
                "host_ns::Host",
                "host_ns::Host::Params",
                "Params::size",
                "host_ns::Host::Mode",
                "Host::run",
                "host_ns::Local",
                "Local::x",
                "host_ns::LocalAlias",
                "host_ns::Guest",
                "host_ns::Guest::Params",
                "Params::count",
                "Guest::x",
                "host_ns::InLambda",
                "InLambda::a",
                "host_ns::Params",
                "Params::other",
                "guest_ns::Host",
                "guest_ns::Host::Params",
                "Params::depth",
                "guest_ns::Host::Inner",
                "guest_ns::Host::Inner::Kind",
            ]
        );
        assert_eq!(
            qualified(&raw, "host_ns::Host::Params").owner.as_deref(),
            Some("Host")
        );
        assert_eq!(
            qualified(&raw, "host_ns::Host::Mode").owner.as_deref(),
            Some("Host")
        );
        assert_eq!(
            qualified(&raw, "host_ns::Guest::Params").owner.as_deref(),
            Some("Guest")
        );
        assert_eq!(
            qualified(&raw, "guest_ns::Host::Params").owner.as_deref(),
            Some("Host")
        );
        assert_eq!(
            qualified(&raw, "guest_ns::Host::Inner::Kind")
                .owner
                .as_deref(),
            Some("Inner")
        );
        assert_eq!(qualified(&raw, "host_ns::Local").owner, None);
        assert_eq!(qualified(&raw, "host_ns::LocalAlias").owner, None);
        assert_eq!(qualified(&raw, "host_ns::InLambda").owner, None);
        assert_eq!(qualified(&raw, "host_ns::Params").owner, None);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the fixture and the table of the data it declares are one fact, and split neither reads alone"
    )]
    fn namespace_scope_declarations_emit_data_symbols() {
        let raw = extract(
            br#"
namespace snd {
static constexpr int BANDS = 8;
const float kScale = 0.5f, kBias = 1.0f;
int g_count;
static int g_hidden = 0;
void (*g_hook)(int) = nullptr;
static const int kTable[] = {
    1, 2,
    3, 4,
};
int &g_ref = g_count;
extern const int
    kLimits[4];
const char *g_label = "", kSep = ',';
char *const kBuffer = nullptr;
inline constexpr int kShared = 2;
template <typename T> constexpr T kUnit = T(1);
int prototype(int);
int Holder::s_total = 0;
}
extern "C" { int c_flag; }
void body() { int local = 0; PX_ASSERT(local); }
TEST(Suite, Case) { int in_macro = 0; }
"#,
        );

        let data: Vec<(&str, SymbolKind, bool, Option<&str>)> = raw
            .symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Constant))
            .map(|symbol| {
                (
                    symbol.qualified_name.as_str(),
                    symbol.kind,
                    symbol.exported,
                    symbol.signature.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            data,
            [
                (
                    "snd::BANDS",
                    SymbolKind::Constant,
                    false,
                    Some("static constexpr int BANDS = 8")
                ),
                (
                    "snd::kScale",
                    SymbolKind::Constant,
                    false,
                    Some("const float kScale = 0.5f, kBias = 1.0f")
                ),
                (
                    "snd::kBias",
                    SymbolKind::Constant,
                    false,
                    Some("const float kScale = 0.5f, kBias = 1.0f")
                ),
                (
                    "snd::g_count",
                    SymbolKind::Variable,
                    true,
                    Some("int g_count")
                ),
                (
                    "snd::g_hidden",
                    SymbolKind::Variable,
                    false,
                    Some("static int g_hidden = 0")
                ),
                (
                    "snd::g_hook",
                    SymbolKind::Variable,
                    true,
                    Some("void (*g_hook)(int) = nullptr")
                ),
                (
                    "snd::kTable",
                    SymbolKind::Constant,
                    false,
                    Some("static const int kTable[]")
                ),
                (
                    "snd::g_ref",
                    SymbolKind::Variable,
                    true,
                    Some("int &g_ref = g_count")
                ),
                (
                    "snd::kLimits",
                    SymbolKind::Constant,
                    true,
                    Some("extern const int\n    kLimits[4]")
                ),
                (
                    "snd::g_label",
                    SymbolKind::Variable,
                    true,
                    Some("const char *g_label = \"\", kSep = ','")
                ),
                (
                    "snd::kSep",
                    SymbolKind::Constant,
                    false,
                    Some("const char *g_label = \"\", kSep = ','")
                ),
                (
                    "snd::kBuffer",
                    SymbolKind::Constant,
                    false,
                    Some("char *const kBuffer = nullptr")
                ),
                (
                    "snd::kShared",
                    SymbolKind::Constant,
                    true,
                    Some("inline constexpr int kShared = 2")
                ),
                (
                    "snd::kUnit",
                    SymbolKind::Constant,
                    false,
                    Some("constexpr T kUnit = T(1)")
                ),
                ("c_flag", SymbolKind::Variable, true, Some("int c_flag")),
            ]
        );
    }

    #[test]
    fn a_declaration_whose_error_outlives_the_macro_repair_emits_no_data() {
        const SOURCE: &[u8] = b"namespace n {\nMACRO_BEGIN int x = 1;\nint y = (1;\n}\n";
        let tree = parse(&grammar(), SOURCE).expect("parse");
        assert!(tree.root_node().has_error());

        let raw = extract(SOURCE);

        let data: Vec<&str> = raw
            .symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Constant))
            .map(|symbol| symbol.qualified_name.as_str())
            .collect();
        assert_eq!(data, ["n::x"]);
    }

    #[test]
    fn an_export_macro_before_a_variables_type_keeps_the_variable() {
        let raw = extract(
            b"extern KRNLIMP OcclusionMap *occlusion_map;\nextern DAS_CC_API uint32_t SIDEEFFECTS_none;\n\
              DAS_CC_API uint32_t SIDEEFFECTS_unsafe = 1;\nEXPORT_API void *gpContext = NULL;\n\
              EXPORT_API extern int u;\nextern int plain_extern;\nint plain_data = 2;\n",
        );

        let data: Vec<(&str, u32, Option<&str>)> = raw
            .symbols
            .iter()
            .map(|symbol| {
                assert_eq!(symbol.kind, SymbolKind::Variable, "{}", symbol.name);
                (
                    symbol.name.as_str(),
                    symbol.line,
                    symbol.signature.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            data,
            [
                (
                    "occlusion_map",
                    1,
                    Some("extern KRNLIMP OcclusionMap *occlusion_map")
                ),
                (
                    "SIDEEFFECTS_none",
                    2,
                    Some("extern DAS_CC_API uint32_t SIDEEFFECTS_none")
                ),
                (
                    "SIDEEFFECTS_unsafe",
                    3,
                    Some("uint32_t SIDEEFFECTS_unsafe = 1")
                ),
                ("gpContext", 4, Some("void *gpContext = NULL")),
                ("u", 5, Some("extern int u")),
                ("plain_extern", 6, Some("extern int plain_extern")),
                ("plain_data", 7, Some("int plain_data = 2")),
            ]
        );
    }

    #[test]
    fn an_export_macro_before_a_variables_type_keeps_it_inside_conditionals_and_blocks() {
        let raw = extract(
            b"#ifndef G_H\n#define G_H\nextern \"C\" {\nKRNLIMP int in_block;\n}\nnamespace ns {\n#if HAS_API\n\
              DAS_API uint32_t in_branch = 1;\n#endif\n}\n#endif\n",
        );

        let data: Vec<(&str, SymbolKind)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
            .collect();
        assert_eq!(
            data,
            [
                ("in_block", SymbolKind::Variable),
                ("ns::in_branch", SymbolKind::Variable)
            ]
        );
    }

    #[test]
    fn data_a_preprocessor_conditional_holds_at_namespace_scope_is_data_of_each_branch() {
        let raw = extract(
            b"#ifndef G_H\n#define G_H\n\nextern float rc_fov_mult;\nstatic const int k_guarded = 3;\n\n\
              #ifdef BUILD_NUMBER\nstatic const int g_build = BUILD_NUMBER;\n#else\nstatic const int g_build = -1;\n\
              #endif\n\n#endif\n\nextern float outside_guard;\nstatic const int k_outside = 4;\n\
              int get() { return k_guarded; }\n\nnamespace ns {\n#if A\nint a_if;\n#elif B\nint b_elif;\n#endif\n}\n\
              void f() {\n#ifdef X\n  int local_in_cond = 1;\n#endif\n}\n\
              struct S {\n#ifdef X\n  int field_in_cond;\n#endif\n};\n",
        );

        let data: Vec<(&str, SymbolKind, u32)> = raw
            .symbols
            .iter()
            .filter(|symbol| {
                matches!(
                    symbol.kind,
                    SymbolKind::Variable | SymbolKind::Constant | SymbolKind::Field
                )
            })
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind, symbol.line))
            .collect();
        assert_eq!(
            data,
            [
                ("rc_fov_mult", SymbolKind::Variable, 4),
                ("k_guarded", SymbolKind::Constant, 5),
                ("g_build", SymbolKind::Constant, 8),
                ("g_build", SymbolKind::Constant, 10),
                ("outside_guard", SymbolKind::Variable, 15),
                ("k_outside", SymbolKind::Constant, 16),
                ("ns::a_if", SymbolKind::Variable, 21),
                ("ns::b_elif", SymbolKind::Variable, 23),
                ("S::field_in_cond", SymbolKind::Field, 33),
            ]
        );
    }

    #[test]
    fn a_conditional_holding_a_parse_error_holds_no_data() {
        let raw = extract(
            b"#ifdef D\nint cut_beside_error;\n}\n#endif\n#ifdef D\nint kept_beside_valid;\n#endif\n",
        );

        let data: Vec<&str> = raw
            .symbols
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Constant))
            .map(|symbol| symbol.qualified_name.as_str())
            .collect();
        assert_eq!(data, ["kept_beside_valid"]);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the fixture and the table of the members it declares are one fact, and split neither reads alone"
    )]
    fn field_declarations_emit_fields_of_their_class() {
        const SOURCE: &[u8] = br"
namespace ns {
int counter;
struct Config {
    int maxSources = 512;
    static constexpr int kMax = 8;
    float *weights, bias;
    int buf[8];
    const Config &parent;
    void (*callback)(int);
    void f();
    png_byte bit_depth PNG_DEPSTRUCT;
    union { int asInt; float asFloat; };
    unsigned flags : 3;
    struct { int hidden; } named;
};
}
typedef struct { int x; } TD, TD2;
void g() { int local = 0; struct Local { int y; }; }
template<> struct Traits<int> { int size; };
";
        let tree = parse(&grammar(), SOURCE).expect("parse");
        assert!(tree.root_node().has_error());

        let raw = extract(SOURCE);

        let rows: Vec<(&str, SymbolKind, Option<&str>)> = raw
            .symbols
            .iter()
            .map(|symbol| {
                (
                    symbol.qualified_name.as_str(),
                    symbol.kind,
                    symbol.signature.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("ns::counter", SymbolKind::Variable, Some("int counter")),
                ("ns::Config", SymbolKind::Struct, None),
                (
                    "Config::maxSources",
                    SymbolKind::Field,
                    Some("int maxSources = 512")
                ),
                (
                    "Config::kMax",
                    SymbolKind::Field,
                    Some("static constexpr int kMax = 8")
                ),
                (
                    "Config::weights",
                    SymbolKind::Field,
                    Some("float *weights, bias")
                ),
                (
                    "Config::bias",
                    SymbolKind::Field,
                    Some("float *weights, bias")
                ),
                ("Config::buf", SymbolKind::Field, Some("int buf[8]")),
                (
                    "Config::parent",
                    SymbolKind::Field,
                    Some("const Config &parent")
                ),
                (
                    "Config::callback",
                    SymbolKind::Field,
                    Some("void (*callback)(int)")
                ),
                ("Config::asInt", SymbolKind::Field, Some("int asInt")),
                ("Config::asFloat", SymbolKind::Field, Some("float asFloat")),
                (
                    "Config::flags",
                    SymbolKind::Field,
                    Some("unsigned flags : 3")
                ),
                (
                    "Config::named",
                    SymbolKind::Field,
                    Some("struct { int hidden; } named")
                ),
                ("TD", SymbolKind::TypeAlias, None),
                ("TD2", SymbolKind::TypeAlias, None),
                ("TD::x", SymbolKind::Field, Some("int x")),
                ("g", SymbolKind::Function, Some("void g()")),
                ("Local", SymbolKind::Struct, None),
                ("Local::y", SymbolKind::Field, Some("int y")),
                (
                    "Traits",
                    SymbolKind::Struct,
                    Some("template<> struct Traits<int>"),
                ),
                ("Traits::size", SymbolKind::Field, Some("int size")),
                ("Config::f", SymbolKind::Method, Some("void f()")),
            ]
        );
        assert_eq!(
            qualified(&raw, "Config::maxSources").owner.as_deref(),
            Some("Config")
        );
    }

    #[test]
    fn a_field_of_a_class_names_the_class_as_its_value_type() {
        let raw = extract(
            b"template <typename T> struct Cache {\n  Lock lock;\n  Lock *held, spare;\n  ns::Box<int> box;\n\
              const ::ns::Lock &shared;\n  int count;\n  T item;\n  Lock **chain;\n  Lock locks[2];\n\
              Lock *slots[2];\n  Lock ***deep;\n  Lock grid[2][2];\n  Lock (*on_lock)();\n};\n",
        );

        let types: Vec<(&str, &[String])> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Field)
            .map(|symbol| (symbol.name.as_str(), symbol.value_types.as_slice()))
            .collect();
        let named = |spelled: &str| vec![spelled.to_owned()];
        assert_eq!(
            types,
            [
                ("lock", named("Lock").as_slice()),
                ("held", named("*Lock").as_slice()),
                ("spare", named("Lock").as_slice()),
                ("box", named("ns::Box").as_slice()),
                ("shared", named("::ns::Lock").as_slice()),
                ("count", &[][..]),
                ("item", &[][..]),
                ("chain", named("**Lock").as_slice()),
                ("locks", named("*Lock").as_slice()),
                ("slots", named("**Lock").as_slice()),
                ("deep", &[][..]),
                ("grid", named("**Lock").as_slice()),
                ("on_lock", &[][..]),
            ]
        );
    }

    #[test]
    fn a_member_call_on_an_element_spells_the_field_it_indexes() {
        let raw = extract(
            b"struct Cache {\n  void read(int i, Tab<Lock> local) {\n    objs[i]->tick();\n    vals[i].tick();\n\
              this->objs[i]->tick();\n    local[i].tick();\n    other.objs[i].tick();\n    grid[i][0].tick();\n\
              }\n};\nvoid outside(int i, Lock held) { objs[i]->tick(); held[i].tick(); }\n",
        );

        let receivers: Vec<(u32, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.line, call.receiver.as_deref()))
            .collect();
        assert_eq!(
            receivers,
            [
                (3, Some("->objs[]")),
                (4, Some(".vals[]")),
                (5, Some("->objs[]")),
                (6, Some("")),
                (7, Some("")),
                (8, Some("")),
                (11, Some("")),
                (11, Some("")),
            ],
            "a local's element, a field of another value and a second index name no field of the caller's class"
        );
    }

    #[test]
    fn a_field_of_a_class_template_names_the_class_an_element_holds() {
        let raw = extract(
            b"struct Cache {\n  Tab<Lock> items;\n  Tab<const ns::Lock *> pointers;\n  Tab<Lock **> deep;\n\
              Tab<Lock> rows[2];\n  Tab<Lock *> tables[2];\n  Tab<Lock(*)()> calls;\n};\n",
        );

        let types: Vec<(&str, Vec<&str>)> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Field)
            .map(|symbol| {
                let types = symbol.value_types.iter().map(String::as_str).collect();
                (symbol.name.as_str(), types)
            })
            .collect();
        assert_eq!(
            types,
            [
                ("items", vec!["Tab", "*Lock"]),
                ("pointers", vec!["Tab", "**ns::Lock"]),
                ("deep", vec!["Tab"]),
                ("rows", vec!["*Tab", "**Lock"]),
                ("tables", vec!["*Tab"]),
                ("calls", vec!["Tab"]),
            ]
        );
    }

    #[test]
    fn a_field_a_preprocessor_conditional_holds_is_a_field_of_its_class() {
        let raw = extract(
            b"struct Peer {\n#ifdef __clang__\n#pragma clang diagnostic push\n#endif\n  Buffer recv;\n\
              #if defined(LOGGING)\n  Log log;\n#else\n  int quiet;\n#endif\n};\n",
        );

        let fields: Vec<String> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Field)
            .map(|symbol| format!("{} {:?}", symbol.qualified_name, symbol.value_types))
            .collect();
        assert_eq!(
            fields,
            [
                r#"Peer::recv ["Buffer"]"#,
                r#"Peer::log ["Log"]"#,
                "Peer::quiet []"
            ]
        );
    }

    #[test]
    fn a_field_of_a_class_template_names_the_class_its_arrow_reaches() {
        let raw = extract(
            b"struct Cache {\n  eastl::unique_ptr<Lock> owned;\n  Ptr<const ns::Lock> shared;\n  Box<int> box;\n\
              eastl::unique_ptr<Lock> *slot;\n  eastl::unique_ptr<Lock[]> many;\n};\n",
        );

        let types: Vec<(&str, Vec<&str>)> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::Field)
            .map(|symbol| {
                let types = symbol.value_types.iter().map(String::as_str).collect();
                (symbol.name.as_str(), types)
            })
            .collect();
        assert_eq!(
            types,
            [
                ("owned", vec!["eastl::unique_ptr", "*Lock"]),
                ("shared", vec!["Ptr", "*ns::Lock"]),
                ("box", vec!["Box"]),
                ("slot", vec!["*eastl::unique_ptr", "**Lock"]),
                ("many", vec!["eastl::unique_ptr"]),
            ]
        );
    }

    #[test]
    fn a_type_alias_names_the_class_it_aliases_as_its_value_type() {
        let raw = extract(
            b"template <class T> struct List {\n  typedef Impl<T, List> iterator;\n  typedef T value_type;\n\
              typedef typename T::iterator inner;\n  typedef typename Base::iterator base_iterator;\n};\n\
              typedef List<Node> NodeList;\ntypedef ns::Box<int> IntBox;\ntypedef std::vector<Node>::iterator It;\n\
              typedef Node *NodePtr, Plain;\nusing Alias = ::ns::Lock;\nusing Ref = Lock &;\n\
              typedef struct { int x; } Anon;\ntypedef unsigned long ulong_t;\n",
        );

        let types: Vec<(&str, &[String])> = raw
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == SymbolKind::TypeAlias)
            .map(|symbol| (symbol.name.as_str(), symbol.value_types.as_slice()))
            .collect();
        let named = |spelled: &str| vec![spelled.to_owned()];
        assert_eq!(
            types,
            [
                ("iterator", named("Impl").as_slice()),
                ("value_type", &[][..]),
                ("inner", &[][..]),
                ("base_iterator", named("Base::iterator").as_slice()),
                ("NodeList", named("List").as_slice()),
                ("IntBox", named("ns::Box").as_slice()),
                ("It", &[][..]),
                ("NodePtr", &[][..]),
                ("Plain", named("Node").as_slice()),
                ("Alias", named("::ns::Lock").as_slice()),
                ("Ref", &[][..]),
                ("Anon", &[][..]),
                ("ulong_t", &[][..]),
            ]
        );
    }

    #[test]
    fn member_template_calls_strip_template_arguments() {
        let raw = extract(
            br"
class Widget {
public:
    template<typename T> void method(T x) {}
};
void F() {
    Widget w;
    w.method<int>(42);
}
",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(calls, [("method", Some("Widget"))]);
    }

    #[test]
    fn templated_casts_emit_no_calls() {
        let raw = extract(
            br"
void F() {
    int x = 0;
    double d = static_cast<double>(x);
    void* p = reinterpret_cast<void*>(&x);
    const int& r = const_cast<const int&>(x);
    F* f = dynamic_cast<F*>(p);
    doThing<int>(4);
}
",
        );

        assert_eq!(callees(&raw), ["doThing"]);
    }

    #[test]
    fn template_disambiguator_member_calls_reduce_to_bare_name() {
        let raw = extract(
            br"
void F() {
    Widget w;
    w.template get<int>();
}
",
        );

        let calls: Vec<(&str, Option<&str>)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
            .collect();
        assert_eq!(calls, [("get", Some("Widget"))]);
    }

    #[test]
    fn static_member_functions_stay_exported() {
        let raw = extract(
            br"
class Foo {
public:
    static int inline_def() { return 1; }
    static int proto_only();
};

static int free_helper() { return 2; }
",
        );

        assert!(qualified(&raw, "Foo::inline_def").exported);
        assert!(qualified(&raw, "Foo::proto_only").exported);
        assert!(!qualified(&raw, "free_helper").exported);
    }

    #[test]
    fn a_call_before_the_first_symbol_is_a_call_from_file_scope() {
        let raw = extract(b"TEST(Suite, Case) { helper(); }\nvoid run() { go(); }\n");
        let file_scope: Vec<&str> = (raw.calls.iter())
            .filter(|call| call.site_of == FILE_SCOPE)
            .map(|call| call.callee.as_str())
            .collect();

        assert_eq!(callees(&raw), ["helper", "go"]);
        assert_eq!(file_scope, ["helper"]);
    }

    #[test]
    fn includes_record_their_specifier_and_form() {
        let raw = extract(b"#include \"core/util.h\"\n#include <vector>\n#include SOME_MACRO\n");

        let directives: Vec<(u32, &str, u32)> = raw
            .directives
            .iter()
            .map(|directive| (directive.form, directive.spec.as_str(), directive.line))
            .collect();
        assert_eq!(
            directives,
            [
                (crate::QUOTED_INCLUDE, "core/util.h", 1),
                (crate::BRACKET_INCLUDE, "vector", 2)
            ]
        );
    }

    #[test]
    fn a_declaration_keeps_the_macro_after_its_parameters() {
        let raw = extract(
            b"class CompileError\n{\npublic:\n\
              \tPX_FORCE_INLINE const Gu::GeometryUnion& geometry() const { return mShape; }\n\
              \tstatic LUAU_NORETURN void raise(const char* format, ...) LUAU_PRINTF_ATTR(2, 3);\n\
              \ttemplate <typename Dispatch = DISPATCHER_TYPE>\n\
              \tVULKAN_HPP_NODISCARD Result end(Dispatch const & d DISPATCHER_ASSIGNMENT) const VULKAN_HPP_NOEXCEPT;\n\
              };\nvoid plain(int x) VULKAN_HPP_NOEXCEPT;\n",
        );

        let signatures: Vec<(&str, Option<&str>)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.signature.as_deref()))
            .collect();
        assert_eq!(
            signatures,
            [
                ("CompileError", None),
                (
                    "CompileError::geometry",
                    Some("const Gu::GeometryUnion& geometry() const")
                ),
                (
                    "CompileError::raise",
                    Some(
                        "static LUAU_NORETURN void raise(const char* format, ...) LUAU_PRINTF_ATTR(2, 3)"
                    )
                ),
                (
                    "CompileError::end",
                    Some(
                        "Result end(Dispatch const & d DISPATCHER_ASSIGNMENT) const VULKAN_HPP_NOEXCEPT"
                    )
                ),
                ("plain", Some("void plain(int x) VULKAN_HPP_NOEXCEPT"))
            ]
        );
    }

    #[test]
    fn a_definition_whose_body_the_grammar_reads_apart_keeps_it() {
        let raw = extract(
            b"namespace physx\n{\nnamespace Sc\n{\n\tclass CoreInteraction\n\t{\n\tpublic:\n\
              \t\tPX_INLINE CoreInteraction(bool element);\n\
              \t\tPX_FORCE_INLINE const Gu::GeometryUnion& geometry() const { return mShape; }\n\
              \t\tDAGOR_NOINLINE void lockRead(::da_profiler::desc_id_t token) DAG_TS_ACQUIRE_SHARED()\n\
              \t\t{\n\t\t\twait(token);\n\t\t}\n\t\tPxU16 mFlags;\n\t};\n} // namespace Sc\n\n\
              PX_INLINE\nSc::CoreInteraction::CoreInteraction(bool element)\n\t: mDirty(ALL), mFlags(0)\n{\n\
              \tif (element)\n\t\tsetup(ELEMENT);\n}\n}\n",
        );

        let spans: Vec<(&str, u32, u32)> = raw
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.as_str(), symbol.line, symbol.end_line))
            .collect();
        assert_eq!(
            spans,
            [
                ("physx::Sc::CoreInteraction", 5, 15),
                ("CoreInteraction::geometry", 9, 9),
                ("CoreInteraction::lockRead", 10, 13),
                ("CoreInteraction::mFlags", 14, 14),
                ("CoreInteraction::CoreInteraction", 18, 24)
            ]
        );
        let constructor = raw
            .symbols
            .iter()
            .position(|symbol| symbol.qualified_name == "CoreInteraction::CoreInteraction")
            .and_then(|index| u32::try_from(index).ok())
            .map(SymbolId)
            .expect("the constructor");
        assert!(
            raw.callables
                .iter()
                .any(|callable| callable.symbol == constructor
                    && callable.key == "physx::Sc::CoreInteraction::CoreInteraction(bool)"
                    && callable.role == CallableRole::Definition),
            "{:?}",
            raw.callables
        );
        assert_eq!(
            qualified(&raw, "CoreInteraction::CoreInteraction")
                .signature
                .as_deref(),
            Some(
                "PX_INLINE\nSc::CoreInteraction::CoreInteraction(bool element)\n\t: mDirty(ALL), mFlags(0)"
            )
        );
        let sites: Vec<(&str, SymbolId)> = raw
            .calls
            .iter()
            .map(|call| (call.callee.as_str(), call.site_of))
            .collect();
        assert_eq!(
            sites,
            [("wait", id_of(&raw, "lockRead")), ("setup", constructor)]
        );
    }

    #[test]
    fn a_macro_before_a_constructors_initializer_list_keeps_the_method() {
        let raw = extract(
            b"class Plane : public Vec\n{\npublic:\n\tPlane();\n\tPlane(float x);\n};\n\n\
              HACD_INLINE Plane::Plane()\n\t: Vec()\n{\n}\n\n\
              HACD_INLINE Plane::Plane(float x)\n\t: Vec(x)\n{\n\tm_w = x;\n}\n\n\
              HACD_INLINE Widget::Widget(int x) : mX(x) {}\n\nvoid after() {}\n",
        );

        assert_eq!(
            spans(&raw),
            [
                ("Plane", 1, 6),
                ("Plane::Plane", 8, 11),
                ("Plane::Plane", 13, 17),
                ("Widget::Widget", 19, 19),
                ("after", 21, 21)
            ]
        );
        assert!(
            raw.symbols[1..4]
                .iter()
                .all(|symbol| symbol.kind == SymbolKind::Method)
        );
    }

    #[test]
    fn a_macro_before_a_constructor_keeps_a_body_the_error_would_take() {
        let raw = extract(
            b"PX_INLINE Sc::CoreInteraction::CoreInteraction(bool element)\n\t: mFlags(0)\n{\n\
              \tsetup(element);\n}\n\nPX_INLINE Widget::Widget(int x) : mX(x) {}\n\nvoid after() {}\n",
        );

        assert_eq!(
            spans(&raw),
            [
                ("CoreInteraction::CoreInteraction", 1, 5),
                ("Widget::Widget", 7, 7),
                ("after", 9, 9)
            ]
        );
        assert_eq!(
            qualified(&raw, "CoreInteraction::CoreInteraction").kind,
            SymbolKind::Method
        );
        assert_eq!(qualified(&raw, "Widget::Widget").kind, SymbolKind::Method);
        assert_eq!(symbol(&raw, "after").kind, SymbolKind::Function);
        assert!(
            raw.callables
                .iter()
                .any(|callable| callable.key == "Sc::CoreInteraction::CoreInteraction(bool)")
        );
        assert_eq!(callees(&raw), ["setup"]);
    }

    #[test]
    fn a_macro_stays_before_a_constructor_without_an_initializer_list() {
        let raw = extract(
            b"HACD_INLINE Plane::Plane(float x)\n{\n\tm_w = x;\n}\n\n\
              Plane::Plane() : Vec() {}\n",
        );

        assert_eq!(
            spans(&raw),
            [("Plane::Plane", 1, 4), ("Plane::Plane", 6, 6)]
        );
        assert_eq!(
            raw.symbols[0].signature.as_deref(),
            Some("HACD_INLINE Plane::Plane(float x)")
        );
        assert_eq!(
            raw.symbols[1].signature.as_deref(),
            Some("Plane::Plane() : Vec()")
        );
        assert!(
            raw.symbols
                .iter()
                .all(|symbol| symbol.kind == SymbolKind::Method)
        );
    }

    #[test]
    fn a_signature_leaves_out_its_comments() {
        let raw = extract(
            b"class task\n{\npublic:\n    template<typename _Ty>\n    __declspec(noinline) // Ask for no inlining\n\
              \x20   explicit task(_Ty _Param)\n    {\n    }\n\n    int run() // runs it\n    {\n        return 0;\n\
              \x20   }\n};\n\nint wide(int/*w*/x);\nvoid tick(float /*dt*/);\n\
              int table[] = // the table\n{\n    1,\n};\n",
        );

        assert_eq!(
            qualified(&raw, "task::task").signature.as_deref(),
            Some("__declspec(noinline)\n    explicit task(_Ty _Param)")
        );
        assert_eq!(
            qualified(&raw, "task::run").signature.as_deref(),
            Some("int run()")
        );
        assert_eq!(
            qualified(&raw, "wide").signature.as_deref(),
            Some("int wide(int x)")
        );
        assert_eq!(
            qualified(&raw, "tick").signature.as_deref(),
            Some("void tick(float)")
        );
        assert_eq!(
            qualified(&raw, "table").signature.as_deref(),
            Some("int table[]")
        );
    }
}
