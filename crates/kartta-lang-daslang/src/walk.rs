use std::num::NonZeroU16;

use kartta_lang::tree::{child_nodes, line, preorder, signature, text};
use kartta_lang::{
    Access, Banner, CallRef, FILE_SCOPE, RawDirective, RawExtraction, Receiver, Symbol,
    SymbolFlags, SymbolId, SymbolKind, UseRef,
};
use tree_sitter::{Node, Tree, TreeCursor};

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

const SELF_MACROS: [&str; 2] = ["class_method", "explicit_const_class_method"];

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    if !opts_into_gen2(tree.root_node(), source) {
        return RawExtraction::default();
    }

    let mut emitter = Emitter::new(tree, source);
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

#[derive(Clone, Copy)]
struct DataMacro {
    first_data: usize,
    splices_run: bool,
}

struct DataSpan {
    start: usize,
    end: usize,
    splices_run: bool,
}

// constraint: `quote` and the `qmatch*` and `qmacro*` call macros of daScript's `daslib/ast_match` and
// `daslib/templates_boost` read the arguments from `first_data` on as syntax, and none of it runs where it is written;
// in a `qmacro*` argument a splice such as `$e(x)` is the exception, because its expression runs there
fn data_macro(macro_name: &str) -> Option<DataMacro> {
    let (first_data, splices_run) = match macro_name {
        "quote" => (0, false),
        "qmatch" | "qmatch_block" | "qmatch_function" => (1, false),
        "qmacro" | "qmacro_block" | "qmacro_block_to_array" | "qmacro_expr" | "qmacro_type" => {
            (0, true)
        }
        "qmacro_function" | "qmacro_variable" => (1, true),
        "qmacro_method" => (2, true),
        _ => return None,
    };

    Some(DataMacro {
        first_data,
        splices_run,
    })
}

struct Emitter<'a> {
    source: &'a [u8],
    scopes: Vec<Scope>,
    types: Vec<TypeScope>,
    data_spans: Vec<DataSpan>,
    piped: Vec<usize>,
    arguments: Option<NonZeroU16>,
    non_named_arguments: Option<NonZeroU16>,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn new(tree: &Tree, source: &'a [u8]) -> Self {
        let language = tree.language();
        Self {
            source,
            scopes: Vec::new(),
            types: Vec::new(),
            data_spans: Vec::new(),
            piped: Vec::new(),
            arguments: language.field_id_for_name("arguments"),
            non_named_arguments: language.field_id_for_name("non_named_arguments"),
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
        while self.data_spans.last().is_some_and(|data| data.end <= start) {
            self.data_spans.pop();
        }
        if self
            .data_spans
            .last()
            .is_some_and(|data| data.start <= start)
        {
            self.data(node);
            return;
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
            "pipe" => {
                self.piped_head(node);
                self.block_call(node);
                self.data_block(node);
            }
            "make_block" => self.anonymous_scope(node),
            "address" => self.address(node),
            _ => {}
        }
    }

    // constraint: `@@f` takes a function's address, the one form daslang passes a function as a value in, and
    // `@@mod::f` names it through its module; a macro splice `@@$c(...)` names nothing yet
    fn address(&mut self, node: Node<'_>) {
        let Some(target) = node
            .child_by_field_name("target")
            .filter(|target| target.kind() != "tag")
        else {
            return;
        };
        let (name, module) = callee_parts(target, self.source);
        self.push_use(name, module, node);
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
        symbol.most_arguments = parameters(node, self.source);
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
            self.data_arguments(node, callee);
            let (callee, receiver) = callee_parts(callee, self.source);
            let passed = self.passed(node);
            self.push_call((callee, receiver), node, passed);
        }
    }

    fn data_arguments(&mut self, call: Node<'_>, name: Node<'_>) {
        if call.kind() != "call" {
            return;
        }
        let Some(data_macro) = data_macro_named(name, self.source) else {
            return;
        };

        let mut cursor = call.walk();
        let mut data = named_arguments(&call, &mut cursor).skip(data_macro.first_data);
        if let Some(first) = data.next() {
            let end = data.last().unwrap_or(first).end_byte();
            self.data_spans.push(DataSpan {
                start: first.start_byte(),
                end,
                splices_run: data_macro.splices_run,
            });
        }
    }

    // constraint: `macro(args) <| data` and `macro(args) { data }` pass the piped block after the call's own arguments,
    // so it is data when the arguments before it reach `first_data`; `data |> macro(args)` passes it first and is not
    // read as data
    fn data_block(&mut self, pipe: Node<'_>) {
        let Some(head) = pipe
            .child_by_field_name("function_call")
            .filter(|head| head.kind() == "call")
        else {
            return;
        };
        let Some(data_macro) = head
            .child_by_field_name("name")
            .and_then(|name| data_macro_named(name, self.source))
        else {
            return;
        };
        let Some(block) = pipe
            .child_by_field_name("argument")
            .filter(|block| block.start_byte() >= head.end_byte())
        else {
            return;
        };

        if named_arguments(&head, &mut head.walk()).count() >= data_macro.first_data {
            self.data_spans.push(DataSpan {
                start: block.start_byte(),
                end: block.end_byte(),
                splices_run: data_macro.splices_run,
            });
        }
    }

    // constraint: what a data span calls, constructs or takes the address of is named by the code compiled from it, so
    // each name is a use
    fn data(&mut self, node: Node<'_>) {
        match node.kind() {
            "call" => self.data_name(node.child_by_field_name("name"), node),
            "named_call" if node.child_by_field_name("value").is_none() => {
                self.data_name(node.child_by_field_name("name"), node);
            }
            "make_struct" if has_child_token(node, "(") => {
                self.data_name(
                    node.child_by_field_name("make_type").and_then(nominal_type),
                    node,
                );
            }
            "new_expression" => self.data_name(
                node.child_by_field_name("type_expression")
                    .and_then(nominal_type),
                node,
            ),
            "address" => self.address(node),
            "tag" => self.splice(node),
            _ => {}
        }
    }

    fn data_name(&mut self, name: Option<Node<'_>>, node: Node<'_>) {
        if let Some(name) = name {
            let (name, module) = callee_parts(name, self.source);
            self.push_use(name, module, node);
        }
    }

    // constraint: a splice's expression is the one part of a data span that runs where the macro is written, so the
    // span is data again only from the end of that expression
    fn splice(&mut self, tag: Node<'_>) {
        let Some(run) = tag.child_by_field_name("subexpression") else {
            return;
        };
        if let Some(data) = self.data_spans.last_mut().filter(|data| data.splices_run) {
            data.start = run.end_byte();
        }
    }

    // constraint: `default<T>` is a make-expression too and constructs nothing a call names; only the form with its
    // own parentheses calls the type
    fn constructor(&mut self, node: Node<'_>) {
        if !has_child_token(node, "(") {
            return;
        }
        if let Some(type_name) = node.child_by_field_name("make_type").and_then(nominal_type) {
            self.push_call(callee_parts(type_name, self.source), node, 0);
        }
    }

    // constraint: a `new` over a make-expression, `new array<int>()` or `new { 1 => 2 }`, is an `ascend` the grammar
    // names no type for, and the new-expression holds its own arguments, so no `call` node records the site twice
    fn new_call(&mut self, node: Node<'_>) {
        let type_name = node
            .child_by_field_name("type_expression")
            .and_then(nominal_type);
        if let Some(type_name) = type_name {
            self.push_call(callee_parts(type_name, self.source), node, 0);
        }
    }

    fn member_call(&mut self, node: Node<'_>) {
        if let Some(method) = self.field_text(node, "name") {
            let receiver = node
                .child_by_field_name("value")
                .map(|object| member_receiver(object, self.source));
            let passed = self.passed(node).saturating_add(1);
            self.push_call((method, receiver), node, passed);
        }
    }

    // constraint: `a |> f(b)`, `f(a) <| b` and `f(a) { b }` pass the piped value to the call at the pipe's head, whose
    // node the walk reaches after the pipe's
    fn piped_head(&mut self, pipe: Node<'_>) {
        let head = pipe
            .child_by_field_name("function_call")
            .filter(|head| matches!(head.kind(), "call" | "named_call" | "invoke"));
        if let Some(head) = head {
            self.piped.push(head.id());
        }
    }

    // constraint: a parameter takes one argument wherever it is written, so a call's own list and the pipe at its head
    // count alike; a count past `u8::MAX` is cut, and a cut count never turns a definition away
    fn passed(&mut self, call: Node<'_>) -> u8 {
        let piped = self.piped.iter().position(|&head| head == call.id());
        if let Some(at) = piped {
            self.piped.swap_remove(at);
        }
        let mut written = 0_usize;
        let mut cursor = call.walk();
        let mut more = cursor.goto_first_child();
        while more {
            let field = cursor.field_id();
            written += usize::from(
                (field == self.arguments || field == self.non_named_arguments)
                    && cursor.node().is_named(),
            );
            more = cursor.goto_next_sibling();
        }

        u8::try_from(written + usize::from(piped.is_some())).unwrap_or(u8::MAX)
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
        self.push_call((callee, receiver), node, 2);
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
                id: FILE_SCOPE,
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

    fn push_call(
        &mut self,
        (callee, receiver): (String, Option<String>),
        node: Node<'_>,
        arguments: u8,
    ) {
        if callee.is_empty() {
            return;
        }

        let site_of = self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id);
        self.raw.calls.push(CallRef {
            site_of,
            callee,
            receiver,
            line: start_line(node),
            invokes_a_macro: false,
            calls_a_method: false,
            arguments,
        });
    }

    // constraint: `_::f` is the compiler's spelling of the scope that compiles the code, which a bare `f` names too
    fn push_use(&mut self, name: String, module: Option<String>, node: Node<'_>) {
        if name.is_empty() {
            return;
        }

        let site_of = self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id);
        self.raw.uses.push(UseRef {
            site_of,
            name,
            receiver: module
                .filter(|module| module != "_")
                .map_or(Receiver::Bare, Receiver::Qualifier),
            access: Access::Read,
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

// constraint: one parameter list entry declares every name it lists, `def f(a, b : int)`, and the `class_method` macros
// of `daslib/class_boost` give a `def static` the `self` it does not write
fn parameters(function: Node<'_>, source: &[u8]) -> u8 {
    let mut entries = function.walk();
    let names = function
        .children_by_field_name("arguments", &mut entries)
        .filter(Node::is_named)
        .map(|entry| {
            let mut cursor = entry.walk();
            entry.children_by_field_name("name", &mut cursor).count()
        })
        .sum::<usize>();
    let own_self = is_annotated(function, source, |name| SELF_MACROS.contains(&name));

    u8::try_from(names + usize::from(own_self)).unwrap_or(u8::MAX)
}

fn data_macro_named(name: Node<'_>, source: &[u8]) -> Option<DataMacro> {
    if name.kind() != "identifier" {
        return None;
    }

    name.utf8_text(source).ok().and_then(data_macro)
}

// constraint: the grammar labels the commas between arguments as `arguments` too
fn named_arguments<'tree, 'cursor>(
    call: &'cursor Node<'tree>,
    cursor: &'cursor mut TreeCursor<'tree>,
) -> impl Iterator<Item = Node<'tree>> + 'cursor {
    call.children_by_field_name("arguments", cursor)
        .filter(Node::is_named)
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
    is_annotated(function, source, |name| !COMPILE_HINTS.contains(&name))
}

fn is_annotated(function: Node<'_>, source: &[u8], is_it: impl Fn(&str) -> bool) -> bool {
    function
        .child_by_field_name("annotations")
        .is_some_and(|list| {
            child_nodes(list)
                .filter(|annotation| annotation.kind() == "annotation_declaration")
                .filter_map(|annotation| annotation.child_by_field_name("name"))
                .any(|name| name.utf8_text(source).is_ok_and(&is_it))
        })
}

fn start_line(node: Node<'_>) -> u32 {
    line(node.start_position().row)
}

fn end_line(node: Node<'_>) -> u32 {
    line(node.end_position().row)
}
