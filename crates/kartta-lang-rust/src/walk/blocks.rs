use std::ops::Range;

use kartta_lang::tree::{named_child_nodes, text};
use kartta_lang::{LocalCall, SymbolId, SymbolKind};
use tree_sitter::Node;

use super::Emitter;

// constraint: an item a function body declares is in scope in its whole block, a `macro_rules!` only after its
// definition and a `let` binding after its statement; an item is keyed by the inline module around it, because a
// module is a scope of its own
pub(super) struct BlockItem {
    scope: Range<usize>,
    module: Option<usize>,
    kind: BlockItemKind,
}

#[derive(Clone, Copy)]
enum BlockItemKind {
    Value(SymbolId),
    Macro(SymbolId),
    Module(usize),
    Import,
    Binding(SymbolId),
}

pub(super) struct BlockCall {
    index: usize,
    at: usize,
    module: Option<usize>,
    form: CallForm,
}

pub(super) enum CallForm {
    Plain,
    Macro,
    Path(Vec<PathSegment>),
}

pub(super) enum PathSegment {
    Name(String),
    SelfModule,
    SuperModule,
}

impl Emitter<'_> {
    pub(super) fn record_block_item(&mut self, node: Node<'_>, id: SymbolId) {
        let symbol = &self.raw.symbols[id.0 as usize];
        if self.functions.is_empty() || symbol.owner.is_some() {
            return;
        }
        let kind = match symbol.kind {
            SymbolKind::Function | SymbolKind::Constant | SymbolKind::Variable => {
                BlockItemKind::Value(id)
            }
            SymbolKind::Struct if names_a_value(node) => BlockItemKind::Value(id),
            SymbolKind::Macro => BlockItemKind::Macro(id),
            SymbolKind::Namespace => BlockItemKind::Module(node.end_byte()),
            _ => return,
        };
        let name = symbol.name.clone();
        let Some(block) = enclosing_block(node) else {
            return;
        };
        if let BlockItemKind::Module(module) = kind
            && let Some(enclosing) = self.modules.last()
        {
            self.module_parents.insert(module, enclosing.end);
        }
        let start = match kind {
            BlockItemKind::Macro(_) => node.end_byte(),
            _ => block.start_byte(),
        };
        self.push_block_item(name, start..block.end_byte(), kind);
    }

    fn push_block_item(&mut self, name: String, scope: Range<usize>, kind: BlockItemKind) {
        let module = self.modules.last().map(|module| module.end);
        self.block_items.entry(name).or_default().push(BlockItem {
            scope,
            module,
            kind,
        });
    }

    pub(super) fn use_declaration(&mut self, node: Node<'_>) {
        if self.functions.is_empty() {
            return;
        }
        let (Some(block), Some(argument)) = (node.parent(), node.child_by_field_name("argument"))
        else {
            return;
        };
        let mut names = Vec::new();
        use_names(argument, self.source, &mut names);
        for name in names {
            self.push_block_item(name, block.byte_range(), BlockItemKind::Import);
        }
    }

    pub(super) fn let_declaration(&mut self, node: Node<'_>) {
        let Some(function) = self.functions.last().map(|scope| scope.id) else {
            return;
        };
        let (Some(block), Some(pattern)) = (node.parent(), node.child_by_field_name("pattern"))
        else {
            return;
        };
        let mut names = Vec::new();
        pattern_names(pattern, self.source, &mut names);
        for name in names {
            self.push_block_item(
                name,
                node.end_byte()..block.end_byte(),
                BlockItemKind::Binding(function),
            );
        }
    }

    // constraint: only a function inside a function body sees an item of a block around it, so only its parameters can
    // shadow one, and they do for the calls of its own body
    pub(super) fn function_parameters(&mut self, node: Node<'_>, function: SymbolId) {
        if self.functions.is_empty() {
            return;
        }
        let (Some(parameters), Some(body)) = (
            node.child_by_field_name("parameters"),
            node.child_by_field_name("body"),
        ) else {
            return;
        };
        let mut names = Vec::new();
        for parameter in named_child_nodes(parameters) {
            if let Some(pattern) = parameter.child_by_field_name("pattern") {
                pattern_names(pattern, self.source, &mut names);
            }
        }
        for name in names {
            self.push_block_item(name, body.byte_range(), BlockItemKind::Binding(function));
        }
    }

    pub(super) fn record_block_call(&mut self, node: Node<'_>, form: CallForm) {
        if !self.functions.is_empty() {
            self.block_calls.push(BlockCall {
                index: self.raw.calls.len() - 1,
                at: node.start_byte(),
                module: self.modules.last().map(|module| module.end),
                form,
            });
        }
    }

    // constraint: a `macro_rules!` is in scope textually, so it is found across an inline module boundary too
    fn innermost_block_item(
        &self,
        name: &str,
        call: &BlockCall,
        wanted: impl Fn(BlockItemKind) -> bool,
    ) -> Option<BlockItemKind> {
        self.block_items
            .get(name)?
            .iter()
            .filter(|item| {
                wanted(item.kind)
                    && (item.module == call.module || matches!(item.kind, BlockItemKind::Macro(_)))
                    && item.scope.contains(&call.at)
            })
            .min_by_key(|item| item.scope.len())
            .map(|item| item.kind)
    }

    fn path_module(&self, segments: &[PathSegment], call: &BlockCall) -> Option<usize> {
        let (first, rest) = segments.split_first()?;
        let mut module = match first {
            PathSegment::Name(name) => match self.innermost_block_item(name, call, |kind| {
                matches!(kind, BlockItemKind::Module(_) | BlockItemKind::Import)
            })? {
                BlockItemKind::Module(module) => module,
                _ => return None,
            },
            PathSegment::SelfModule => call.module?,
            PathSegment::SuperModule => *self.module_parents.get(&call.module?)?,
        };
        for segment in rest {
            module = match segment {
                PathSegment::Name(name) => self.inner_module(name, module)?,
                PathSegment::SuperModule => *self.module_parents.get(&module)?,
                PathSegment::SelfModule => return None,
            };
        }
        Some(module)
    }

    fn inner_module(&self, name: &str, module: usize) -> Option<usize> {
        self.block_items
            .get(name)?
            .iter()
            .find_map(|item| match item.kind {
                BlockItemKind::Module(inner) if item.scope.end == module => Some(inner),
                _ => None,
            })
    }

    // constraint: a `let` binding of the name shadows an item only for calls in the function that binds it, and a
    // `use` of the name shadows it everywhere in its block; either leaves the call unbound
    fn block_item_of(&self, call: &BlockCall) -> Option<SymbolId> {
        let site = &self.raw.calls[call.index];
        let found = match &call.form {
            CallForm::Plain => {
                self.innermost_block_item(&site.callee, call, |kind| match kind {
                    BlockItemKind::Value(_) | BlockItemKind::Import => true,
                    BlockItemKind::Binding(function) => function == site.site_of,
                    _ => false,
                })?
            }
            CallForm::Macro => self.innermost_block_item(&site.callee, call, |kind| {
                matches!(kind, BlockItemKind::Macro(_) | BlockItemKind::Import)
            })?,
            CallForm::Path(segments) => {
                let module = self.path_module(segments, call)?;
                return self.block_items.get(&site.callee)?.iter().find_map(|item| {
                    match item.kind {
                        BlockItemKind::Value(id) if item.scope.end == module => Some(id),
                        _ => None,
                    }
                });
            }
        };
        match found {
            BlockItemKind::Value(id) | BlockItemKind::Macro(id) => Some(id),
            _ => None,
        }
    }

    // constraint: a call that names an item of a block around it is a local call of that item, which no other file
    // can reach; it leaves the calls in their order, and the block calls are recorded in the order of the calls
    pub(super) fn bind_block_item_calls(&mut self) {
        let bound: Vec<(usize, SymbolId)> = self
            .block_calls
            .iter()
            .filter_map(|call| Some((call.index, self.block_item_of(call)?)))
            .collect();
        if bound.is_empty() {
            return;
        }
        let mut bound = bound.into_iter().peekable();
        let mut index = 0;
        let local_calls = &mut self.raw.local_calls;
        self.raw.calls.retain(|call| {
            let at = index;
            index += 1;
            let Some((_, local)) = bound.next_if(|&(bound_at, _)| bound_at == at) else {
                return true;
            };
            local_calls.push(LocalCall {
                site_of: call.site_of,
                local,
                line: call.line,
            });
            false
        });
    }
}

// constraint: the block of an item in an `extern` block is the block around the `extern` block
fn enclosing_block(node: Node<'_>) -> Option<Node<'_>> {
    let parent = node.parent();
    match parent.and_then(|list| list.parent()) {
        Some(foreign) if foreign.kind() == "foreign_mod_item" => foreign.parent(),
        _ => parent,
    }
}

// constraint: a unit or tuple struct names a value, its constructor, and a struct with named fields does not
fn names_a_value(node: Node<'_>) -> bool {
    node.kind() == "struct_item"
        && node
            .child_by_field_name("body")
            .is_none_or(|body| body.kind() != "field_declaration_list")
}

fn use_names(node: Node<'_>, source: &[u8], names: &mut Vec<String>) {
    let field = |name| node.child_by_field_name(name);
    match node.kind() {
        "identifier" => names.push(text(node, source)),
        "scoped_identifier" => field("name")
            .into_iter()
            .for_each(|name| use_names(name, source, names)),
        "use_as_clause" => field("alias")
            .into_iter()
            .for_each(|alias| use_names(alias, source, names)),
        "scoped_use_list" => field("list")
            .into_iter()
            .for_each(|list| use_names(list, source, names)),
        "use_list" => named_child_nodes(node).for_each(|item| use_names(item, source, names)),
        _ => {}
    }
}

fn pattern_names(node: Node<'_>, source: &[u8], names: &mut Vec<String>) {
    pattern_bindings(node, &mut |name| names.push(text(name, source)));
}

// constraint: a pattern binds its identifiers, a shorthand field of `S { helper }` among them, and never the type a
// struct or tuple-struct pattern names
pub(super) fn pattern_bindings<'t>(node: Node<'t>, bind: &mut impl FnMut(Node<'t>)) {
    match node.kind() {
        "identifier" | "shorthand_field_identifier" => bind(node),
        "field_pattern" => {
            if let Some(bound) = node
                .child_by_field_name("pattern")
                .or_else(|| node.child_by_field_name("name"))
            {
                pattern_bindings(bound, bind);
            }
        }
        "mut_pattern"
        | "ref_pattern"
        | "reference_pattern"
        | "or_pattern"
        | "tuple_pattern"
        | "captured_pattern"
        | "slice_pattern"
        | "struct_pattern"
        | "tuple_struct_pattern" => {
            let named_type = node.child_by_field_name("type");
            named_child_nodes(node)
                .filter(|inner| Some(*inner) != named_type)
                .for_each(|inner| pattern_bindings(inner, bind));
        }
        _ => {}
    }
}

pub(super) fn path_segments(path: Node<'_>, source: &[u8]) -> Option<Vec<PathSegment>> {
    match path.kind() {
        "identifier" => Some(vec![PathSegment::Name(text(path, source))]),
        "self" => Some(vec![PathSegment::SelfModule]),
        "super" => Some(vec![PathSegment::SuperModule]),
        "scoped_identifier" => {
            let mut segments = path_segments(path.child_by_field_name("path")?, source)?;
            let name = path.child_by_field_name("name")?;
            segments.push(match name.kind() {
                "identifier" => PathSegment::Name(text(name, source)),
                "super" => PathSegment::SuperModule,
                _ => return None,
            });
            Some(segments)
        }
        _ => None,
    }
}
