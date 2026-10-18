use kartta_lang::tree::{child_nodes, line, preorder, text};
use kartta_lang::{
    Banner, CallRef, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId,
    SymbolKind,
};
use tree_sitter::{Node, Tree};

use crate::class::class_name;

pub(crate) const CLASS_FORM: u32 = 0;

const BANNER: Banner = Banner::new(&["@generated", "DO NOT EDIT"]).with_lines(&["#"]);

const IDENTIFIER: &str = "identifier";
const CLASS_IDENTIFIER: &str = "class_identifier";
const NAMES: [&str; 2] = [CLASS_IDENTIFIER, IDENTIFIER];

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter {
        source,
        scopes: Vec::new(),
        flags: SymbolFlags::empty().with_generated(BANNER.opens(source)),
        raw: RawExtraction::default(),
    };

    preorder(tree.root_node(), |node| {
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

struct Emitter<'s> {
    source: &'s [u8],
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

        match node.kind() {
            "class_definition" => self.scoped(node, SymbolKind::Class),
            "defined_resource_type" | "function_declaration" => {
                self.scoped(node, SymbolKind::Function);
            }
            "type_declaration" => {
                self.definition(node, SymbolKind::TypeAlias);
            }
            "node_definition" => self.scoped(node, SymbolKind::Struct),
            "resource_declaration" => self.resource_declaration(node),
            "function_call" => self.function_call(node),
            "iterator_statement" => self.iterator_statement(node),
            "include_statement" | "require_statement" => self.class_ref_statement(node),
            IDENTIFIER => self.bare_contain(node),
            _ => {}
        }
    }

    fn scoped(&mut self, node: Node<'_>, kind: SymbolKind) {
        let Some(id) = self.definition(node, kind) else {
            return;
        };
        self.scopes.push(Scope {
            end: node.end_byte(),
            id,
        });
        if kind == SymbolKind::Class
            && let Some(inherits) = child_of_kind(node, &["class_inherits"])
            && let Some(parent) = child_of_kind(inherits, &NAMES)
        {
            let name = text(parent, self.source);
            if let Some(symbol) = self.raw.symbols.last_mut() {
                symbol.parents.push(name.clone());
            }
            self.push_class_ref(name, parent);
        }
    }

    fn definition(&mut self, node: Node<'_>, kind: SymbolKind) -> Option<SymbolId> {
        let names: &[&str] = if kind == SymbolKind::Struct {
            &["node_name"]
        } else {
            &NAMES
        };
        let name = text(child_of_kind(node, names)?, self.source);
        let mut symbol = Symbol::new(
            name,
            kind,
            line(node.start_position().row),
            line(node.end_position().row),
        );
        symbol.exported = true;
        symbol.flags = self.flags;

        Some(self.raw.push_symbol(symbol))
    }

    fn resource_declaration(&mut self, node: Node<'_>) {
        let Some(kind) = node.child_by_field_name("type") else {
            return;
        };
        let kind = text(kind, self.source);

        // constraint: a single-segment resource type is a built-in (`file`, `package`), which shadows autoloading, so
        // only a qualified one, an autoloaded defined type, names a manifest
        if kind.contains("::") {
            self.push_class_ref(kind, node);
        } else {
            self.push_call(kind, None, node);
        }
    }

    fn function_call(&mut self, node: Node<'_>) {
        if let Some(field) = child_of_kind(node, &["field_expression"]) {
            self.member_call(field, node);
            return;
        }

        let Some(name_node) = child_of_kind(node, &NAMES) else {
            return;
        };
        let name = text(name_node, self.source);
        let routes_classes = matches!(name.as_str(), "include" | "require" | "contain");
        if !(routes_classes && self.routed_arguments(node, name_node)) {
            self.push_call(name, None, node);
        }
    }

    fn routed_arguments(&mut self, node: Node<'_>, keyword: Node<'_>) -> bool {
        let mut routed = false;
        let after_keyword = child_nodes(node)
            .skip_while(|child| *child != keyword)
            .skip(1);
        for child in after_keyword {
            if let Some(name) = self.class_ref_name(child) {
                self.push_class_ref(name, child);
                routed = true;
            }
        }
        routed
    }

    fn class_ref_statement(&mut self, node: Node<'_>) {
        for child in child_nodes(node) {
            if let Some(name) = self.class_ref_name(child) {
                self.push_class_ref(name, child);
            }
        }
    }

    fn class_ref_name(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            IDENTIFIER | CLASS_IDENTIFIER => Some(text(node, self.source)),
            "string" => Some(
                text(node, self.source)
                    .trim_matches(|quote| quote == '\'' || quote == '"')
                    .to_owned(),
            ),
            _ => None,
        }
    }

    // constraint: `contain foo::bar` has no statement kind of its own; the keyword and the class name are loose nodes
    // of the block
    fn bare_contain(&mut self, node: Node<'_>) {
        if node.parent().is_some_and(|parent| parent.kind() == "block")
            && self.source.get(node.byte_range()) == Some(b"contain".as_slice())
            && let Some(next) = node.next_named_sibling()
            && let Some(name) = self.class_ref_name(next)
        {
            self.push_class_ref(name, next);
        }
    }

    fn iterator_statement(&mut self, node: Node<'_>) {
        if let Some(field) = child_of_kind(node, &["field_expression"]) {
            self.member_call(field, node);
        }
    }

    fn member_call(&mut self, field: Node<'_>, call: Node<'_>) {
        let Some(trailing) = child_nodes(field).rfind(|child| NAMES.contains(&child.kind())) else {
            return;
        };
        let receiver = field
            .named_child(0)
            .filter(|object| object.id() != trailing.id())
            .map(|object| text(object, self.source));
        self.push_call(text(trailing, self.source), receiver, call);
    }

    fn push_call(&mut self, callee: String, receiver: Option<String>, node: Node<'_>) {
        if callee.is_empty() {
            return;
        }
        let caller_id = self.scopes.last().map_or(NO_CALLER_ID, |scope| scope.id);
        self.raw.calls.push(CallRef {
            caller_id,
            callee,
            receiver,
            line: line(node.start_position().row),
        });
    }

    // constraint: a token that is no class name — an uppercase type reference such as `Class['foo']`, a `$var` — names
    // nothing, so it records neither a directive nor a call that could only bind wrongly
    fn push_class_ref(&mut self, name: String, node: Node<'_>) {
        if class_name(&name).is_none() {
            return;
        }
        self.raw.directives.push(RawDirective::new(
            CLASS_FORM,
            name.clone(),
            line(node.start_position().row),
        ));
        self.push_call(name, None, node);
    }
}

fn child_of_kind<'t>(node: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    child_nodes(node).find(|child| kinds.contains(&child.kind()))
}
