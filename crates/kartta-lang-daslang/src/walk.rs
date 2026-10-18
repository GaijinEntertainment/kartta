use kartta_lang::tree::{child_nodes, line, preorder, signature, text};
use kartta_lang::{
    Banner, CallRef, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId,
    SymbolKind,
};
use tree_sitter::{Node, Tree};

const REQUIRE_FORM: u32 = 0;

const BANNER: Banner = Banner::new(&["@generated", "DO NOT EDIT", "Code generated"])
    .with_lines(&["//"])
    .with_block("/*", "*/");

// constraint: the function annotations daslang's runtime defines that only tune how a function compiles or what it
// may do; any other annotation, its own `[export]`, `[init]` or `[run]` or a project's `[es]` or `[console_cmd]`,
// registers the function with what calls it
const COMPILE_HINTS: [&str; 28] = [
    "alias_cmres",
    "arch",
    "cold_path",
    "deprecated",
    "generic",
    "hint",
    "hot_path",
    "hybrid",
    "inline",
    "jit",
    "local_only",
    "never_alias_cmres",
    "never_inline",
    "no_alloc",
    "no_aot",
    "no_env",
    "no_io",
    "no_jit",
    "no_lint",
    "nodiscard",
    "sideeffects",
    "temp_string_result",
    "unsafe_deref",
    "unsafe_heap_collect",
    "unsafe_operation",
    "unsafe_outside_of_for",
    "unsafe_when_not_clone_array",
    "unused_argument",
];

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    if !opts_into_gen2(tree.root_node(), source) {
        return RawExtraction::default();
    }

    let mut emitter = Emitter::new(source);
    preorder(tree.root_node(), |node| {
        if node.is_named() {
            emitter.handle(node);
        }
        true
    });

    emitter.raw
}

// constraint: the grammar reads only the second-generation syntax, which a file opts into with a top-level
// `options gen2`; reading the option from the tree rather than the text keeps a commented-out or quoted `options gen2`
// from passing a first-generation file, whose tree would be errors
fn opts_into_gen2(root: Node<'_>, source: &[u8]) -> bool {
    let mut opted_in = false;
    let options = child_nodes(root).filter(|node| node.kind() == "options_declaration");
    for declaration in options {
        match gen2_verdict(declaration, source) {
            Some(false) => return false,
            Some(true) => opted_in = true,
            None => {}
        }
    }

    opted_in
}

fn gen2_verdict(declaration: Node<'_>, source: &[u8]) -> Option<bool> {
    let mut verdict = None;
    let arguments =
        child_nodes(declaration).filter(|argument| argument.kind() == "annotation_argument");
    for argument in arguments {
        let names_gen2 = argument
            .child_by_field_name("name")
            .is_some_and(|name| name.utf8_text(source) == Ok("gen2"));
        if !names_gen2 {
            continue;
        }
        let opted_out = argument.child_by_field_name("value").is_some_and(|value| {
            value.kind() == "constant_boolean" && value.utf8_text(source) == Ok("false")
        });
        if opted_out {
            return Some(false);
        }
        verdict = Some(true);
    }

    verdict
}

#[derive(Clone, Copy)]
struct Scope {
    end: usize,
    id: SymbolId,
}

struct TypeScope {
    end: usize,
    name: String,
}

struct Emitter<'a> {
    source: &'a [u8],
    scopes: Vec<Scope>,
    types: Vec<TypeScope>,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn new(source: &'a [u8]) -> Self {
        Self {
            source,
            scopes: Vec::new(),
            types: Vec::new(),
            flags: SymbolFlags::empty().with_generated(BANNER.opens(source)),
            raw: RawExtraction::default(),
        }
    }

    fn handle(&mut self, node: Node<'_>) {
        let start = node.start_byte();
        while self.scopes.last().is_some_and(|scope| scope.end <= start) {
            self.scopes.pop();
        }
        while self.types.last().is_some_and(|scope| scope.end <= start) {
            self.types.pop();
        }

        match node.kind() {
            "module_declaration" => self.module(node),
            "require_declaration" => self.require(node),
            "include_directive" => self.include(node),
            "function_declaration" => self.function(node),
            "struct_declaration" => self.structure(node, SymbolKind::Struct),
            "class_declaration" => self.structure(node, SymbolKind::Class),
            "field_declaration" => self.field(node),
            "typedef_declaration" if is_member(node) => self.member_alias(node),
            "typedef_declaration" => self.alias(node),
            "enumeration_declaration" | "bitfield_alias_declaration" => {
                self.type_scope(node, SymbolKind::Enum);
            }
            "enumeration_entry" | "bitfield_entry" => self.enumerator(node),
            "variant_alias_declaration" => {
                self.type_scope(node, SymbolKind::Enum);
                self.named_entries(node, SymbolKind::EnumVariant);
            }
            "tuple_alias_declaration" => {
                self.type_scope(node, SymbolKind::Struct);
                self.named_entries(node, SymbolKind::Field);
            }
            "global_let" => self.globals(node),
            "named_call" if node.child_by_field_name("value").is_some() => self.member_call(node),
            "call" | "named_call" => self.plain_call(node),
            "make_struct" => self.constructor(node),
            "new_expression" => self.new_call(node),
            "invoke" => self.member_call(node),
            "pipe" => self.block_call(node),
            "make_block" => self.anonymous_scope(node),
            _ => {}
        }
    }

    // constraint: `module $` names the compiler's builtin module, which no qualified name can spell, so it declares
    // no namespace
    fn module(&mut self, node: Node<'_>) {
        let Some(name) = self.field_text(node, "name").filter(|name| name != "$") else {
            return;
        };

        let mut symbol = Symbol::new(
            name.clone(),
            SymbolKind::Namespace,
            start_line(node),
            end_line(node),
        );
        symbol.exported = true;
        symbol.flags = self.flags;
        self.raw.package = Some(name);
        self.raw.push_symbol(symbol);
    }

    // constraint: `require ?guard target` keeps its guard in its own field, so only `name` is the import
    fn require(&mut self, node: Node<'_>) {
        if let Some(module) = self.field_text(node, "name") {
            self.directive(module, node);
        }
    }

    // constraint: the compiler's lexer takes the run of non-blank characters after `include` as the file name, so a
    // quoted name keeps its quotes
    fn include(&mut self, node: Node<'_>) {
        let written = text(node, self.source);
        if let Some(file) = written
            .strip_prefix("include")
            .and_then(|rest| rest.split_whitespace().next())
        {
            self.directive(file.to_owned(), node);
        }
    }

    fn directive(&mut self, spec: String, node: Node<'_>) {
        if !spec.is_empty() {
            self.raw
                .directives
                .push(RawDirective::new(REQUIRE_FORM, spec, start_line(node)));
        }
    }

    fn function(&mut self, node: Node<'_>) {
        let Some(name) = self
            .field_text(node, "name")
            .filter(|name| !name.is_empty())
        else {
            return;
        };
        let (kind, owner) = if is_member(node) {
            (SymbolKind::Method, self.enclosing_type())
        } else {
            (SymbolKind::Function, None)
        };

        let mut symbol = self.declared(kind, node, (owner, name));
        symbol.exported = !is_private(node);
        symbol.signature = arrow_trimmed_signature(node, self.source);
        symbol.flags = symbol.flags.with_entry(is_registered(node, self.source));
        let id = self.raw.push_symbol(symbol);
        self.scopes.push(Scope {
            end: node.end_byte(),
            id,
        });
    }

    fn structure(&mut self, node: Node<'_>, kind: SymbolKind) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };

        let mut symbol = self.declared(kind, node, (None, name.clone()));
        symbol.parents = self.field_text(node, "parent").into_iter().collect();
        symbol.exported = !is_private(node);
        self.raw.push_symbol(symbol);
        self.types.push(TypeScope {
            end: node.end_byte(),
            name,
        });
    }

    fn field(&mut self, node: Node<'_>) {
        let owner = self.enclosing_type();
        let exported = !is_private(node);

        let mut cursor = node.walk();
        let names = node
            .children_by_field_name("name", &mut cursor)
            .filter(|name| is_member_name(*name));
        for name in names {
            let name = text(name, self.source);
            let mut symbol = self.declared(SymbolKind::Field, node, (owner.clone(), name));
            symbol.exported = exported;
            self.raw.push_symbol(symbol);
        }
    }

    fn named_entries(&mut self, node: Node<'_>, kind: SymbolKind) {
        let Some(owner) = self.field_text(node, "name") else {
            return;
        };

        let mut cursor = node.walk();
        let entries = node
            .children_by_field_name("argument_names", &mut cursor)
            .filter(|name| is_member_name(*name));
        for entry in entries {
            let name = text(entry, self.source);
            self.push_exported(self.declared(kind, entry, (Some(owner.clone()), name)));
        }
    }

    fn member_alias(&mut self, node: Node<'_>) {
        if let Some(name) = self.field_text(node, "name") {
            let owner = self.enclosing_type();
            self.push_exported(self.declared(SymbolKind::TypeAlias, node, (owner, name)));
        }
    }

    fn type_scope(&mut self, node: Node<'_>, kind: SymbolKind) {
        let Some(name) = self.field_text(node, "name") else {
            return;
        };

        let mut symbol = self.declared(kind, node, (None, name.clone()));
        symbol.exported = !is_private(node);
        self.raw.push_symbol(symbol);
        self.types.push(TypeScope {
            end: node.end_byte(),
            name,
        });
    }

    fn enumerator(&mut self, node: Node<'_>) {
        if let Some(name) = self.field_text(node, "name") {
            let owner = self.enclosing_type();
            self.push_exported(self.declared(SymbolKind::EnumVariant, node, (owner, name)));
        }
    }

    fn globals(&mut self, node: Node<'_>) {
        let kind = if has_child_token(node, "var") {
            SymbolKind::Variable
        } else {
            SymbolKind::Constant
        };
        let exported = !is_private(node);

        let mut bindings = node.walk();
        for binding in node.children_by_field_name("variables", &mut bindings) {
            let mut cursor = binding.walk();
            let names = binding
                .children_by_field_name("name", &mut cursor)
                .filter(|name| name.kind() == "identifier");
            for name in names {
                let name = text(name, self.source);
                let mut symbol = self.declared(kind, binding, (None, name));
                symbol.exported = exported;
                self.raw.push_symbol(symbol);
            }
        }
    }

    fn alias(&mut self, node: Node<'_>) {
        if let Some(name) = self.field_text(node, "name") {
            let mut symbol = self.declared(SymbolKind::TypeAlias, node, (None, name));
            symbol.exported = !is_private(node);
            self.raw.push_symbol(symbol);
        }
    }

    fn plain_call(&mut self, node: Node<'_>) {
        if let Some(callee) = node.child_by_field_name("name") {
            let (callee, receiver) = callee_parts(callee, self.source);
            self.push_call(callee, receiver, node);
        }
    }

    // constraint: `default<T>` is a make-expression too and constructs nothing a call names; only the form with its
    // own parentheses calls the type
    fn constructor(&mut self, node: Node<'_>) {
        if !has_child_token(node, "(") {
            return;
        }
        if let Some(type_name) = node.child_by_field_name("make_type").and_then(nominal_type) {
            let (callee, receiver) = callee_parts(type_name, self.source);
            self.push_call(callee, receiver, node);
        }
    }

    // constraint: a `new` over a make-expression, `new array<int>()` or `new { 1 => 2 }`, is an `ascend` the grammar
    // names no type for, and the new-expression holds its own arguments, so no `call` node records the site twice
    fn new_call(&mut self, node: Node<'_>) {
        let type_name = node
            .child_by_field_name("type_expression")
            .and_then(nominal_type);
        if let Some(type_name) = type_name {
            let (callee, receiver) = callee_parts(type_name, self.source);
            self.push_call(callee, receiver, node);
        }
    }

    fn member_call(&mut self, node: Node<'_>) {
        if let Some(method) = self.field_text(node, "name") {
            let receiver = node
                .child_by_field_name("value")
                .map(|object| member_receiver(object, self.source));
            self.push_call(method, receiver, node);
        }
    }

    // constraint: `obj.handler { ... }` and `obj.handler <| ${ ... }` are a `pipe` whose head is a `field` no call
    // node covers; a data pipe `obj.field |> f()` holds the field as its argument and calls nothing through it
    fn block_call(&mut self, node: Node<'_>) {
        let applies_a_block = node
            .child_by_field_name("argument")
            .is_some_and(|argument| argument.kind() == "make_block");
        let Some(head) = node
            .child_by_field_name("function_call")
            .filter(|head| applies_a_block && head.kind() == "field")
        else {
            return;
        };
        let Some(callee) = head
            .child_by_field_name("name")
            .filter(|name| is_member_name(*name))
            .map(|name| text(name, self.source))
        else {
            return;
        };

        let receiver = head
            .child_by_field_name("value")
            .map(|object| member_receiver(object, self.source));
        self.push_call(callee, receiver, node);
    }

    // constraint: a block bound to a name runs when the value is called later, so its body's calls are no call of
    // the def that defines it; a block a pipe or a call applies in place keeps its calls in the enclosing def
    fn anonymous_scope(&mut self, node: Node<'_>) {
        let applied = matches!(
            node.parent().map(|parent| parent.kind()),
            Some("pipe" | "call" | "invoke" | "named_call" | "new_expression")
        );
        if !applied {
            self.scopes.push(Scope {
                end: node.end_byte(),
                id: NO_CALLER_ID,
            });
        }
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
        symbol.qualified_name = self.qualify(owner.as_deref(), &name);
        symbol.name = name;
        symbol.owner = owner;
        symbol.flags = self.flags;
        symbol
    }

    fn enclosing_type(&self) -> Option<String> {
        self.types.last().map(|scope| scope.name.clone())
    }

    fn push_exported(&mut self, mut symbol: Symbol) {
        symbol.exported = true;
        self.raw.push_symbol(symbol);
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

    fn qualify(&self, owner: Option<&str>, name: &str) -> String {
        match (&self.raw.package, owner) {
            (Some(module), Some(owner)) => format!("{module}.{owner}.{name}"),
            (Some(module), None) => format!("{module}.{name}"),
            (None, Some(owner)) => format!("{owner}.{name}"),
            (None, None) => name.to_owned(),
        }
    }

    fn field_text(&self, node: Node<'_>, field: &str) -> Option<String> {
        node.child_by_field_name(field)
            .map(|child| text(child, self.source))
    }
}

// constraint: `def f() => expr` keeps the expression as its body, so the signature would end in a dangling `=>`
fn arrow_trimmed_signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    signature(node, source).map(|header| header.trim_end_matches("=>").trim_end().to_owned())
}

fn member_receiver(object: Node<'_>, source: &[u8]) -> String {
    object
        .child_by_field_name("name")
        .filter(|name| object.kind() == "variable" && name.kind() == "identifier")
        .map_or_else(String::new, |name| text(name, source))
}

// constraint: `mod::f` keeps `mod` as the receiver, so same-named functions of two modules stay apart; a root-scoped
// `::f` has no module and no receiver
fn callee_parts(node: Node<'_>, source: &[u8]) -> (String, Option<String>) {
    if node.kind() != "qualified_name" {
        return (text(node, source), None);
    }

    let callee = node
        .child_by_field_name("name")
        .map_or_else(String::new, |name| text(name, source));
    let receiver = node
        .child_by_field_name("module")
        .map(|module| text(module, source));
    (callee, receiver)
}

fn nominal_type(type_node: Node<'_>) -> Option<Node<'_>> {
    if type_node.kind() != "structure_type" {
        return None;
    }

    type_node
        .named_child(0)
        .filter(|name| matches!(name.kind(), "identifier" | "qualified_name"))
}

// constraint: a macro splice such as `$i(x)` names a member as a `tag`, kept as written
fn is_member_name(node: Node<'_>) -> bool {
    matches!(node.kind(), "identifier" | "tag")
}

fn is_member(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| matches!(parent.kind(), "struct_declaration" | "class_declaration"))
}

fn has_child_token(node: Node<'_>, token: &str) -> bool {
    child_nodes(node).any(|child| child.kind() == token)
}

fn is_private(node: Node<'_>) -> bool {
    has_child_token(node, "private")
}

fn is_registered(function: Node<'_>, source: &[u8]) -> bool {
    function
        .child_by_field_name("annotations")
        .is_some_and(|list| {
            child_nodes(list)
                .filter(|annotation| annotation.kind() == "annotation_declaration")
                .filter_map(|annotation| annotation.child_by_field_name("name"))
                .any(|name| {
                    name.utf8_text(source)
                        .is_ok_and(|name| !COMPILE_HINTS.contains(&name))
                })
        })
}

fn start_line(node: Node<'_>) -> u32 {
    line(node.start_position().row)
}

fn end_line(node: Node<'_>) -> u32 {
    line(node.end_position().row)
}
