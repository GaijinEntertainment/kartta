use std::collections::HashMap;
use std::ops::Range;

use kartta_lang::tree::{line, named_child_nodes, signature, text};
use kartta_lang::{
    Banner, FILE_SCOPE, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use self::attributes::{
    ImportedMember, attribute_marks_test, imported_member, marks_macro_export, marks_proc_macro,
    mod_body_marks_test, opens_test_scope, path_attribute,
};
use self::blocks::{BlockCall, BlockItem};
use self::syntax::{
    constructed_type, impl_type_name, is_attributable_item, is_exported, named_child_of_kind,
    symbol, type_base,
};

const BANNER: Banner = Banner::new(&["@generated", "DO NOT EDIT"])
    .with_lines(&["//"])
    .with_block("/*", "*/");

mod arguments;
mod attributes;
mod bindings;
mod blocks;
mod calls;
mod declared;
mod imports;
mod settle;
mod syntax;
mod values;

pub(crate) const MOD_FORM: u32 = 0;
pub(crate) const MOD_TEST_FORM: u32 = 1;
pub(crate) const USE_FORM: u32 = 2;
pub(crate) const GLOB_FORM: u32 = 3;

// constraint: a node is judged as a read by its parent's kind and its field there, which only the cursor holds, as
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
                emitter.settle_paths();
                emitter.keep_declared_receivers();
                emitter.record_imports();
                emitter.apply_pending_parents();
                emitter.bind_block_item_calls();
                emitter.bind_value_reads();
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

// constraint: a type scope is opened by an `impl` or a `trait`; a trait's members have no visibility of their own and
// take the trait's, so `exported_default` carries it down, and an impl leaves it false
struct TypeScope {
    end: usize,
    name: String,
    exported_default: bool,
    impl_owner: Option<usize>,
}

// constraint: the root module, the empty chain, has no parent
fn parent_module(chain: &str) -> Option<&str> {
    (!chain.is_empty()).then(|| chain.rsplit_once("::").map_or("", |(parent, _)| parent))
}

// constraint: a free function over the scopes alone, so a caller iterates it while it pushes into the impl owners
fn open_impls_over<'a>(types: &'a [TypeScope], name: &'a str) -> impl Iterator<Item = usize> + 'a {
    types
        .iter()
        .filter(move |scope| settle::is_under(name, &scope.name))
        .filter_map(|scope| scope.impl_owner)
}

struct ModuleScope {
    end: usize,
    chain: String,
}

// constraint: an impl's owner settles after the walk, once every declaration of the file is known: `tentative` is the
// written type path under the enclosing module chain, which names the type when the file declares one of that name,
// and `written` the path as written; what the walk recorded under `tentative` is listed here so that settling one
// impl renames nothing of another
#[derive(Default)]
struct ImplOwner {
    tentative: String,
    written: String,
    members: Vec<SymbolId>,
    receivers: Vec<usize>,
    paths: Vec<usize>,
    pairs: Vec<usize>,
}

struct Emitter<'a> {
    source: &'a [u8],
    functions: Vec<Scope>,
    types: Vec<TypeScope>,
    modules: Vec<ModuleScope>,
    impl_owners: Vec<ImplOwner>,
    receiver_paths: Vec<(usize, String)>,
    imports: Vec<imports::Import>,
    globs: Vec<usize>,
    written_paths: Vec<imports::WrittenPath>,
    pending_parents: Vec<(String, String)>,
    generated: bool,
    test_scopes: Vec<usize>,
    block_items: HashMap<String, Vec<BlockItem>>,
    block_calls: Vec<BlockCall>,
    module_parents: HashMap<usize, usize>,
    regions: Vec<bindings::Region<'a>>,
    pending_bindings: Vec<bindings::PendingBindings<'a>>,
    // constraint: the name and the type path of each parameter of a declared type of the functions open around the
    // walk, a stack each function region spans its part of
    parameter_types: Vec<(&'a [u8], Range<usize>)>,
    // constraint: each call recorded on a declared type, by its index, until the walk ends and the file's declarations
    // and `use`s tell whether the type may be one of the tree
    declared_receivers: Vec<(usize, declared::DeclaredReceiver)>,
    // constraint: the end of each `if` or `while` open around the walk, with the end of the branch it runs on a match
    branches: Vec<(usize, usize)>,
    read_scopes: Vec<String>,
    value_reads: Vec<values::ValueRead>,
    // constraint: the end of the block or module of each `macro_rules! matches` the walk has passed, the text its
    // scope runs to, so the furthest end bounds every one of them in scope
    matches_scope_end: usize,
    symbol_count: u32,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn new(source: &'a [u8]) -> Self {
        Self {
            source,
            functions: Vec::new(),
            types: Vec::new(),
            modules: Vec::new(),
            impl_owners: Vec::new(),
            receiver_paths: Vec::new(),
            imports: Vec::new(),
            globs: Vec::new(),
            written_paths: Vec::new(),
            pending_parents: Vec::new(),
            generated: BANNER.opens(source),
            test_scopes: Vec::new(),
            block_items: HashMap::new(),
            block_calls: Vec::new(),
            module_parents: HashMap::new(),
            regions: Vec::new(),
            pending_bindings: Vec::new(),
            parameter_types: Vec::new(),
            declared_receivers: Vec::new(),
            branches: Vec::new(),
            read_scopes: vec![String::new()],
            value_reads: Vec::new(),
            matches_scope_end: 0,
            symbol_count: 0,
            raw: RawExtraction::default(),
        }
    }
}

impl Emitter<'_> {
    fn handle(&mut self, node: Node<'_>, parent: Option<(&str, Option<&str>)>) {
        self.close_scopes(node.start_byte());
        self.open_test_scopes(node);
        self.track_bindings(node);
        let caller = self.functions.last().map_or(FILE_SCOPE, |scope| scope.id);

        match node.kind() {
            "function_item" => {
                let id = self.function(node);
                self.functions.push(Scope {
                    end: node.end_byte(),
                    id,
                });
            }
            "function_signature_item" => {
                self.function(node);
            }
            "impl_item" => self.implementation(node),
            "struct_item" | "union_item" => self.structure(node),
            "enum_item" => self.enumeration(node),
            "trait_item" => self.trait_definition(node),
            "mod_item" => self.module(node),
            "type_item" | "associated_type" => {
                self.item(node, SymbolKind::TypeAlias);
            }
            "macro_definition" => {
                if node
                    .child_by_field_name("name")
                    .is_some_and(|name| &self.source[name.byte_range()] == b"matches")
                {
                    let end = node.parent().map_or(usize::MAX, |scope| scope.end_byte());
                    self.matches_scope_end = self.matches_scope_end.max(end);
                }
                self.item(node, SymbolKind::Macro);
            }
            "const_item" => {
                self.item(node, SymbolKind::Constant);
            }
            "static_item" => {
                self.item(node, SymbolKind::Variable);
            }
            "use_declaration" => self.record_import(node),
            "call_expression" => self.call(node, caller),
            "macro_invocation" => {
                self.macro_call(node, caller);
                self.macro_arguments(node, caller);
            }
            "identifier" | "shorthand_field_identifier" => self.name(node, parent),
            "scoped_identifier" => self.scoped_read(node, parent),
            _ => {}
        }
    }

    // constraint: a node is inside a scope while it starts before the scope's end, which is exclusive
    fn close_scopes(&mut self, start: usize) {
        while self.functions.pop_if(|scope| scope.end <= start).is_some() {}
        while self.types.pop_if(|scope| scope.end <= start).is_some() {}
        while self.modules.pop_if(|scope| scope.end <= start).is_some() {}
        while self.test_scopes.pop_if(|end| *end <= start).is_some() {}
    }

    // constraint: a test gate opens before the item it gates is emitted, so the item's own symbol reads it; a
    // file-top `#![cfg(test)]` gates the whole file, and the inner gate of an inline module opens at the `mod`
    // item, because the walk reaches the inner attribute only after the module's symbol
    fn open_test_scopes(&mut self, node: Node<'_>) {
        let kind = node.kind();
        if is_attributable_item(kind) && opens_test_scope(node, self.source) {
            self.test_scopes.push(node.end_byte());
        }
        if kind == "inner_attribute_item"
            && let Some(file) = node
                .parent()
                .filter(|parent| parent.kind() == "source_file")
            && attribute_marks_test(node, self.source)
        {
            self.test_scopes.push(file.end_byte());
        }
        if kind == "mod_item" && mod_body_marks_test(node, self.source) {
            self.test_scopes.push(node.end_byte());
        }
    }

    const fn flags(&self) -> SymbolFlags {
        SymbolFlags::empty()
            .with_test(!self.test_scopes.is_empty())
            .with_generated(self.generated)
    }

    // constraint: a symbol under the chain of an open impl whose owner settles after the walk is listed on that
    // impl, unless it is a `macro_rules!`, which is under no chain
    fn push_symbol(&mut self, mut symbol: Symbol) -> SymbolId {
        let id = SymbolId(self.symbol_count);
        if symbol.kind != SymbolKind::Macro && !self.impl_owners.is_empty() {
            for at in open_impls_over(&self.types, &symbol.qualified_name) {
                self.impl_owners[at].members.push(id);
            }
        }
        if !self.functions.is_empty() {
            self.raw.local_symbols.push(id);
        }
        symbol.flags = self.flags();
        self.symbol_count += 1;
        self.raw.symbols.push(symbol);
        id
    }

    fn implementation(&mut self, node: Node<'_>) {
        let written = impl_type_name(node, self.source).or_else(|| {
            node.child_by_field_name("type")
                .filter(|_| !self.functions.is_empty())
                .map(|target| text(target, self.source))
        });
        let Some(written) = written else {
            return;
        };
        let (name, impl_owner) = self.impl_target(written);

        if let Some(trait_node) = node.child_by_field_name("trait") {
            self.queue_parent(&name, text(type_base(trait_node), self.source), impl_owner);
        }
        self.types.push(TypeScope {
            end: node.end_byte(),
            name,
            exported_default: false,
            impl_owner,
        });
    }

    fn impl_target(&mut self, written: String) -> (String, Option<usize>) {
        match self.module_path(&written) {
            Some(tentative) if tentative != written => {
                self.impl_owners.push(ImplOwner {
                    tentative: tentative.clone(),
                    written,
                    ..ImplOwner::default()
                });
                (tentative, Some(self.impl_owners.len() - 1))
            }
            _ => (written, None),
        }
    }

    // constraint: the impl may precede the type it names, so the pair waits for the end of the walk
    fn queue_parent(&mut self, type_name: &str, trait_name: String, impl_owner: Option<usize>) {
        if trait_name.is_empty() {
            return;
        }
        let pair = self.pending_parents.len();
        for at in open_impls_over(&self.types, type_name).chain(impl_owner) {
            self.impl_owners[at].pairs.push(pair);
        }
        self.pending_parents
            .push((type_name.to_owned(), trait_name));
    }

    // constraint: the scope whose chain qualifies an item is the innermost type scope, unless an inline module inside
    // it is nearer
    fn chain_scope(&self) -> Option<&TypeScope> {
        let scope = self.types.last()?;
        let module_inside = self
            .modules
            .last()
            .is_some_and(|module| module.end < scope.end);
        (!module_inside).then_some(scope)
    }

    // constraint: an item a function body declares inside a type scope is qualified under the type but is no member
    fn member_scope(&self) -> Option<&TypeScope> {
        let scope = self.chain_scope()?;
        let body_inside = self
            .functions
            .last()
            .is_some_and(|function| function.end < scope.end);
        (!body_inside).then_some(scope)
    }

    fn qualify(&self, name: &str) -> (Option<String>, String) {
        if let Some(scope) = self.chain_scope() {
            let owner = self.member_scope().map(|member| member.name.clone());
            return (owner, format!("{}::{name}", scope.name));
        }
        let qualified_name = self.modules.last().map_or_else(
            || name.to_owned(),
            |module| format!("{}::{name}", module.chain),
        );
        (None, qualified_name)
    }

    fn exported_by_scope(&self) -> bool {
        self.member_scope()
            .is_some_and(|scope| scope.exported_default)
    }

    // constraint: `self::` keeps the chain of the enclosing inline module and each `super::` drops one module of it;
    // a path that leaves the file's module tree, through `crate::`, a leading `::` or a `super::` past the root, or
    // that starts at `Self`, names nothing the file can tell
    fn module_path(&self, path: &str) -> Option<String> {
        let mut chain = self
            .modules
            .last()
            .map_or("", |module| module.chain.as_str());
        let mut segments = path.split("::").map(str::trim).peekable();
        if segments.next_if_eq(&"self").is_none() {
            while segments.next_if_eq(&"super").is_some() {
                chain = parent_module(chain)?;
            }
        }
        let mut named: Option<String> = None;
        for segment in segments {
            if matches!(segment, "" | "crate" | "Self" | "self" | "super") {
                return None;
            }
            let named = named.get_or_insert_with(|| chain.to_owned());
            if !named.is_empty() {
                named.push_str("::");
            }
            named.push_str(segment);
        }
        named
    }

    // constraint: every top-level `fn main` is an entry candidate, because binaries that pick their `main` by `cfg`
    // declare several; the resolver keeps them only in a file that is a crate's entry
    fn function(&mut self, node: Node<'_>) -> SymbolId {
        let name = node
            .child_by_field_name("name")
            .map_or_else(String::new, |name| text(name, self.source));
        let exported = is_exported(node, self.source) || self.exported_by_scope();
        let (owner, qualified_name) = self.imported_method_owner(node).map_or_else(
            || self.qualify(&name),
            |owner| {
                let qualified_name = format!("{owner}::{name}");
                (Some(owner), qualified_name)
            },
        );
        let kind = if owner.is_some() {
            SymbolKind::Method
        } else if marks_proc_macro(node, self.source) {
            SymbolKind::Macro
        } else {
            SymbolKind::Function
        };
        if name == "main"
            && node
                .parent()
                .is_some_and(|parent| parent.kind() == "source_file")
        {
            self.raw.entry_candidates.push(SymbolId(self.symbol_count));
        }

        let mut function = symbol(name, qualified_name, kind, node);
        function.owner = owner;
        function.exported = exported;
        function.signature = signature(node, self.source);
        let id = self.push_symbol(function);
        self.record_block_item(node, id);
        id
    }

    // constraint: wasm-bindgen makes a function of an `extern` block that its attribute marks a `method` a method of
    // the type its first parameter, `this`, refers to, one it marks a `constructor` a member of the type it returns,
    // `Ok` of a `catch` constructor's `Result`, and one it marks `static_method_of = T` a member of `T`
    fn imported_method_owner(&self, function: Node<'_>) -> Option<String> {
        if function.kind() != "function_signature_item" {
            return None;
        }
        let named = match imported_member(function, self.source)? {
            ImportedMember::Method => {
                let this = function
                    .child_by_field_name("parameters")?
                    .named_child(0)
                    .filter(|parameter| parameter.kind() == "parameter")?;
                type_base(this.child_by_field_name("type")?)
            }
            ImportedMember::Constructor => {
                constructed_type(function.child_by_field_name("return_type")?, self.source)
            }
            ImportedMember::StaticMethodOf(value) if value.kind() == "string_literal" => {
                named_child_of_kind(value, "string_content")?
            }
            ImportedMember::StaticMethodOf(value) => value,
        };
        if !matches!(
            named.kind(),
            "type_identifier" | "identifier" | "string_content"
        ) {
            return None;
        }
        self.module_path(&text(named, self.source))
    }

    // constraint: a `macro_rules!` keeps its bare name and has no owner, because it is in scope textually rather
    // than by path, and it has no visibility modifier: it is exported only by `#[macro_export]`, at its crate's root
    fn item(&mut self, node: Node<'_>, kind: SymbolKind) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let (owner, qualified_name, exported) = if kind == SymbolKind::Macro {
            (None, name.clone(), marks_macro_export(node, self.source))
        } else {
            let exported = is_exported(node, self.source) || self.exported_by_scope();
            let (owner, qualified_name) = self.qualify(&name);
            (owner, qualified_name, exported)
        };

        let mut item = symbol(name, qualified_name, kind, node);
        item.owner = owner;
        item.exported = exported;
        let id = self.push_symbol(item);
        self.record_block_item(node, id);
        Some(id)
    }

    fn qualified_name(&self, id: SymbolId) -> String {
        self.raw.symbols[id.0 as usize].qualified_name.clone()
    }

    // constraint: a union is struct-shaped and the shared kinds have no union, so it is a struct
    fn structure(&mut self, node: Node<'_>) {
        let Some(id) = self.item(node, SymbolKind::Struct) else {
            return;
        };
        let Some(body) = named_child_of_kind(node, "field_declaration_list") else {
            return;
        };

        let owner = self.qualified_name(id);
        let fields = named_child_nodes(body).filter(|field| field.kind() == "field_declaration");
        for field in fields {
            let exported = is_exported(field, self.source);
            self.member(&owner, field, (SymbolKind::Field, exported));
        }
    }

    // constraint: a variant has no visibility of its own and is as visible as its enum
    fn enumeration(&mut self, node: Node<'_>) {
        let exported = is_exported(node, self.source);
        let Some(id) = self.item(node, SymbolKind::Enum) else {
            return;
        };
        let Some(body) = named_child_of_kind(node, "enum_variant_list") else {
            return;
        };

        let owner = self.qualified_name(id);
        let variants = named_child_nodes(body).filter(|variant| variant.kind() == "enum_variant");
        for variant in variants {
            self.member(&owner, variant, (SymbolKind::EnumVariant, exported));
        }
    }

    // constraint: a field or a variant can carry a test gate of its own, which the walk reaches only after the item
    // around it emitted the member, so the member reads its attributes here
    fn member(&mut self, owner: &str, node: Node<'_>, (kind, exported): (SymbolKind, bool)) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        let qualified_name = format!("{owner}::{name}");
        let mut member = symbol(name, qualified_name, kind, node);
        member.owner = Some(owner.to_owned());
        member.exported = exported;
        let id = self.push_symbol(member);
        if opens_test_scope(node, self.source) {
            let flags = &mut self.raw.symbols[id.0 as usize].flags;
            *flags = flags.with_test(true);
        }
    }

    // constraint: a trait is an implementable contract, which the shared kinds call an interface
    fn trait_definition(&mut self, node: Node<'_>) {
        let exported = is_exported(node, self.source);
        if let Some(id) = self.item(node, SymbolKind::Interface) {
            self.types.push(TypeScope {
                end: node.end_byte(),
                name: self.qualified_name(id),
                exported_default: exported,
                impl_owner: None,
            });
        }
    }

    fn module(&mut self, node: Node<'_>) {
        let chain = self
            .item(node, SymbolKind::Namespace)
            .map(|id| self.qualified_name(id));
        let body = node.child_by_field_name("body");
        if let Some(chain) = chain
            && body.is_some()
        {
            self.modules.push(ModuleScope {
                end: node.end_byte(),
                chain,
            });
        }
        if body.is_none() {
            self.mod_directive(node);
        }
    }

    // constraint: a bodiless `mod NAME;` pulls in another file, which the resolver probes; its spec is `NAME` under the
    // inline modules around it, `inner/NAME`, its directive carries a `#[path]` override when one is written, and its
    // form says whether the declaration is test-gated, because the file it pulls in is extracted on its own and cannot
    // see the gate; an inline module's own `#[path]` is not read
    fn mod_directive(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        if name.is_empty() {
            return;
        }

        let form = if self.test_scopes.is_empty() {
            MOD_FORM
        } else {
            MOD_TEST_FORM
        };
        let spec = match self.modules.last() {
            Some(module) => format!("{}/{name}", module.chain.replace("::", "/")),
            None => name,
        };
        self.raw.directives.push(RawDirective {
            form,
            spec,
            names: path_attribute(node, self.source).into_iter().collect(),
            line: line(node.start_position().row),
        });
    }
}
