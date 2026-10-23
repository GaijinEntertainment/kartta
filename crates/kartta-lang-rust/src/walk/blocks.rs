use std::ops::Range;

use kartta_lang::tree::text;
use kartta_lang::{LocalCall, SymbolId, SymbolKind};
use tree_sitter::Node;

use super::Emitter;

// constraint: an item a function body declares is in scope in its whole block, and a `macro_rules!` only after its
// definition; an item is keyed by the inline module around it, because a
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

    // constraint: the names of a `use` are its directives from `first_directive` on, each in scope in the whole block
    pub(super) fn scope_use_to_block(&mut self, block: Range<usize>, first_directive: usize) {
        for at in first_directive..self.raw.directives.len() {
            let name = self.raw.directives[at].names[0].clone();
            self.push_block_item(name, block.clone(), BlockItemKind::Import);
        }
    }

    pub(super) fn in_use_scope(&self, name: &str, at: usize, module: Option<usize>) -> bool {
        self.block_items.get(name).is_some_and(|items| {
            items.iter().any(|item| {
                matches!(item.kind, BlockItemKind::Import)
                    && item.module == module
                    && item.scope.contains(&at)
            })
        })
    }

    pub(super) fn bare_block_calls(&self) -> impl Iterator<Item = (usize, usize, Option<usize>)> {
        (self.block_calls.iter())
            .filter(|call| matches!(call.form, CallForm::Plain | CallForm::Macro))
            .map(|call| (call.index, call.at, call.module))
    }

    pub(super) fn record_block_call(&mut self, node: Node<'_>, form: CallForm) {
        self.block_calls.push(BlockCall {
            index: self.raw.calls.len() - 1,
            at: node.start_byte(),
            module: self.modules.last().map(|module| module.end),
            form,
        });
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

    // constraint: a `use` of the name shadows an item everywhere in its block and leaves the call unbound
    fn block_item_of(&self, call: &BlockCall) -> Option<SymbolId> {
        let site = &self.raw.calls[call.index];
        let found = match &call.form {
            CallForm::Plain => self.innermost_block_item(&site.callee, call, |kind| {
                matches!(kind, BlockItemKind::Value(_) | BlockItemKind::Import)
            })?,
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
