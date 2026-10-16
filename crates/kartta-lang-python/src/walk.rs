use kartta_lang::tree::{children, named_children, preorder, signature, text};
use kartta_lang::{
    CallRef, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use crate::banner::is_generated_source;

pub(crate) const IMPORT_FORM: u32 = 0;
pub(crate) const FUTURE_FORM: u32 = 1;

const IDENTIFIER: &str = "identifier";
const MODULE: &str = "module";
const EXPRESSION_STATEMENT: &str = "expression_statement";
const CLASS_DEFINITION: &str = "class_definition";
const FUNCTION_DEFINITION: &str = "function_definition";

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter {
        source,
        scopes: Vec::new(),
        flags: SymbolFlags::empty().with_generated(is_generated_source(source)),
        raw: RawExtraction::default(),
    };

    preorder(tree, |node| {
        if node.is_named() {
            emitter.handle(node);
        }
        true
    });

    emitter.raw
}

#[derive(Clone, Copy)]
struct Scope {
    end: usize,
    id: SymbolId,
}

struct Emitter<'a> {
    source: &'a [u8],
    scopes: Vec<Scope>,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl Emitter<'_> {
    fn handle(&mut self, node: Node<'_>) {
        let start = node.start_byte();
        while self.scopes.last().is_some_and(|scope| scope.end <= start) {
            self.scopes.pop();
        }
        let caller = self.scopes.last().map_or(NO_CALLER_ID, |scope| scope.id);

        match node.kind() {
            FUNCTION_DEFINITION => {
                if let Some(id) = self.function(node) {
                    self.scopes.push(Scope {
                        end: node.end_byte(),
                        id,
                    });
                }
            }
            CLASS_DEFINITION => self.class(node),
            "call" => self.call(node, caller),
            "assignment" => self.assignment(node),
            "import_statement" => self.import(node),
            "import_from_statement" => self.import_from(node),
            "future_import_statement" => self.future_import(node),
            _ => {}
        }
    }

    // constraint: a definition inside a function body, a class and its members included, is local to that body
    fn push_symbol(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.flags = symbol.flags.union(self.flags);
        let id = SymbolId(u32::try_from(self.raw.symbols.len()).unwrap_or(u32::MAX));
        if !self.scopes.is_empty() {
            self.raw.local_symbols.push(id);
        }
        self.raw.symbols.push(symbol);
        id
    }

    // constraint: only a free `def main` directly at module level is the entry point, the one a script runs under
    // `if __name__ == "__main__":`; a method or a nested `main` is not
    fn function(&mut self, node: Node<'_>) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let owner = enclosing_classes(node, self.source);
        let entry = name == "main" && owner.is_none() && is_module_level(node);

        let kind = if owner.is_some() {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        let qualified_name = owner.as_ref().map(|owner| format!("{owner}.{name}"));
        let mut symbol = symbol(name, qualified_name, kind, decorated_lines(node));
        symbol.owner = owner;
        symbol.signature = signature(node, self.source);
        symbol.flags = symbol.flags.with_entry(entry);
        Some(self.push_symbol(symbol))
    }

    fn class(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        let qualified_name =
            enclosing_classes(node, self.source).map(|chain| format!("{chain}.{name}"));

        let mut symbol = symbol(
            name,
            qualified_name,
            SymbolKind::Class,
            decorated_lines(node),
        );
        symbol.parents = superclasses(node, self.source);
        self.push_symbol(symbol);
    }

    // constraint: only a bare identifier on the left is a symbol, so `a = b = 1` defines `a` alone and `a, b = 1, 2`
    // defines nothing; at module level it is a variable, and directly in a class body it is a field only when
    // annotated, the surface a dataclass or a pydantic model declares, because an unannotated one is commonly
    // implementation state
    fn assignment(&mut self, node: Node<'_>) {
        let left = node
            .child_by_field_name("left")
            .filter(|left| left.kind() == IDENTIFIER);
        let (Some(left), Some(parent)) = (left, statement_parent(node)) else {
            return;
        };
        if parent.kind() == MODULE {
            let symbol = symbol(
                text(left, self.source),
                None,
                SymbolKind::Variable,
                lines(node),
            );
            self.push_symbol(symbol);
            return;
        }

        let in_class_body = parent.kind() == "block"
            && parent
                .parent()
                .is_some_and(|class| class.kind() == CLASS_DEFINITION);
        if !in_class_body || node.child_by_field_name("type").is_none() {
            return;
        }
        let Some(owner) = enclosing_classes(node, self.source) else {
            return;
        };
        let name = text(left, self.source);
        let qualified_name = format!("{owner}.{name}");
        let mut symbol = symbol(name, Some(qualified_name), SymbolKind::Field, lines(node));
        symbol.owner = Some(owner);
        self.push_symbol(symbol);
    }

    fn call(&mut self, node: Node<'_>, caller_id: SymbolId) {
        if self.raw.symbols.is_empty() {
            return;
        }
        let Some((callee, receiver)) = callee(node, self.source) else {
            return;
        };

        self.raw.calls.push(CallRef {
            caller_id,
            callee,
            receiver,
            line: line(node.start_position().row),
        });
    }

    fn import(&mut self, node: Node<'_>) {
        for child in named_children(node).filter_map(|at| node.named_child(at)) {
            let Some(module) = imported_name(child, self.source) else {
                continue;
            };
            self.raw.directives.push(RawDirective::new(
                IMPORT_FORM,
                module,
                line(node.start_position().row),
            ));
        }
    }

    // constraint: a wildcard import leaves the names empty, which the resolver reads as the module alone, as it reads
    // a from-import that names nothing
    fn import_from(&mut self, node: Node<'_>) {
        let Some(module) = node.child_by_field_name("module_name") else {
            return;
        };
        let mut cursor = node.walk();
        let names = node
            .children_by_field_name("name", &mut cursor)
            .filter_map(|name| imported_name(name, self.source))
            .collect();

        self.raw.directives.push(RawDirective {
            form: IMPORT_FORM,
            spec: text(module, self.source),
            names,
            line: line(node.start_position().row),
        });
    }

    // constraint: a future feature cannot be aliased, so its names are read as written
    fn future_import(&mut self, node: Node<'_>) {
        let mut cursor = node.walk();
        let names = node
            .children_by_field_name("name", &mut cursor)
            .map(|name| text(name, self.source))
            .collect();

        self.raw.directives.push(RawDirective {
            form: FUTURE_FORM,
            spec: "__future__".to_owned(),
            names,
            line: line(node.start_position().row),
        });
    }
}

// constraint: `Symbol::new` copies its name into the qualified name, so a symbol qualified by its classes starts
// unnamed, which allocates nothing, and then takes both names
fn symbol(
    name: String,
    qualified_name: Option<String>,
    kind: SymbolKind,
    [start, end]: [u32; 2],
) -> Symbol {
    let mut symbol = match qualified_name {
        Some(qualified_name) => {
            let mut symbol = Symbol::new(String::new(), kind, start, end);
            symbol.name = name;
            symbol.qualified_name = qualified_name;
            symbol
        }
        None => Symbol::new(name, kind, start, end),
    };
    symbol.exported = is_exported(&symbol.name);
    symbol
}

fn line(row: usize) -> u32 {
    u32::try_from(row).map_or(u32::MAX, |row| row.saturating_add(1))
}

fn lines(node: Node<'_>) -> [u32; 2] {
    [
        line(node.start_position().row),
        line(node.end_position().row),
    ]
}

// constraint: a decorated definition starts at its first decorator, which a reader sees as part of it
fn decorated_lines(node: Node<'_>) -> [u32; 2] {
    [
        line(decorated(node).start_position().row),
        line(node.end_position().row),
    ]
}

fn imported_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "dotted_name" => Some(text(node, source)),
        "aliased_import" => node
            .child_by_field_name("name")
            .map(|name| text(name, source)),
        _ => None,
    }
}

// constraint: the grammar wraps an assignment in an `expression_statement`, so the statement's parent is what the
// assignment sits in
fn statement_parent(node: Node<'_>) -> Option<Node<'_>> {
    node.parent()
        .filter(|parent| parent.kind() == EXPRESSION_STATEMENT)
        .and_then(|statement| statement.parent())
}

fn decorated(node: Node<'_>) -> Node<'_> {
    node.parent()
        .filter(|parent| parent.kind() == "decorated_definition")
        .unwrap_or(node)
}

fn is_exported(name: &str) -> bool {
    !name.starts_with('_')
}

fn is_module_level(node: Node<'_>) -> bool {
    decorated(node)
        .parent()
        .is_some_and(|parent| parent.kind() == MODULE)
}

// constraint: the chain stops at the nearest function, so a definition nested in a function is never a member of a
// class around that function
fn enclosing_classes(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut names = Vec::new();
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            CLASS_DEFINITION => names.push(text(parent.child_by_field_name("name")?, source)),
            FUNCTION_DEFINITION => break,
            _ => {}
        }
        ancestor = parent.parent();
    }
    if names.is_empty() {
        return None;
    }

    names.reverse();
    Some(names.join("."))
}

fn superclasses(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(arguments) = node.child_by_field_name("superclasses") else {
        return Vec::new();
    };

    children(arguments)
        .filter_map(|at| arguments.child(at))
        .filter(|child| matches!(child.kind(), IDENTIFIER | "attribute"))
        .map(|child| text(child, source))
        .collect()
}

// constraint: a member call on anything but a name records the empty receiver, a call on a value of no known type
fn callee(node: Node<'_>, source: &[u8]) -> Option<(String, Option<String>)> {
    let function = node.child_by_field_name("function")?;

    match function.kind() {
        IDENTIFIER => Some((text(function, source), None)),
        "attribute" => {
            let receiver = function.child_by_field_name("object").map(|object| {
                if object.kind() == IDENTIFIER {
                    text(object, source)
                } else {
                    String::new()
                }
            });
            let attribute = function.child_by_field_name("attribute")?;
            Some((text(attribute, source), receiver))
        }
        _ => None,
    }
}
