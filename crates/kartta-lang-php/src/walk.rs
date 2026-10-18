use kartta_lang::tree::{
    children, line, named_children, preorder, signature, signature_from, text,
};
use kartta_lang::{
    CallRef, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId, SymbolKind,
};
use tree_sitter::{Node, Tree};

use crate::banner::is_generated_source;
use crate::fold::{fold, marker_leads};

pub(crate) const INCLUDE_FORM: u32 = 0;
pub(crate) const USE_FORM: u32 = 1;
pub(crate) const FUNCTION_OR_CONST_USE_FORM: u32 = 2;

const NAMES: [&str; 3] = ["name", "qualified_name", "relative_name"];

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter {
        source,
        scopes: Vec::new(),
        types: Vec::new(),
        namespace: String::new(),
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

struct Member {
    owner: Option<String>,
    kind: SymbolKind,
    exported: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Callables {
    Detach,
    Cross,
}

struct Emitter<'s> {
    source: &'s [u8],
    scopes: Vec<Scope>,
    // constraint: a class-body `use Trait;` adds a parent to the type whose body is open, found by its id, so a member
    // that shares the type's name never takes the parent
    types: Vec<Scope>,
    // constraint: every `namespace` declaration replaces the namespace and `namespace { }` clears it; PHP admits no
    // declaration outside the braces of a file that uses the braced form, so a namespace needs no end
    namespace: String,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl Emitter<'_> {
    fn handle(&mut self, node: Node<'_>) {
        let start = node.start_byte();
        while self.scopes.pop_if(|scope| scope.end <= start).is_some() {}
        while self.types.pop_if(|scope| scope.end <= start).is_some() {}

        match node.kind() {
            "namespace_definition" => {
                self.namespace = node
                    .child_by_field_name("name")
                    .map(|name| text(name, self.source))
                    .unwrap_or_default();
            }
            "function_definition" => self.function(node),
            "method_declaration" => self.method(node),
            "class_declaration" => self.type_declaration(node, SymbolKind::Class),
            "interface_declaration" => self.type_declaration(node, SymbolKind::Interface),
            "trait_declaration" => self.type_declaration(node, SymbolKind::Trait),
            "enum_declaration" => self.type_declaration(node, SymbolKind::Enum),
            "property_declaration" => self.members(node, "property_element", SymbolKind::Field),
            "const_declaration" => self.members(node, "const_element", SymbolKind::Constant),
            "property_promotion_parameter" => self.promoted_property(node),
            "enum_case" => self.enum_case(node),
            "use_declaration" => self.trait_use(node),
            "namespace_use_declaration" => self.namespace_use(node),
            "include_expression"
            | "include_once_expression"
            | "require_expression"
            | "require_once_expression" => {
                self.include(node);
            }
            "assignment_expression" => self.closure_binding(node),
            "object_creation_expression" => self.object_creation(node),
            "function_call_expression"
            | "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression" => {
                let (callee, receiver) = call_target(node, self.source);
                self.call(callee, receiver, node);
            }
            _ => {}
        }
    }

    fn push(&mut self, mut symbol: Symbol) -> SymbolId {
        let id = SymbolId(u32::try_from(self.raw.symbols.len()).unwrap_or(u32::MAX));
        symbol.flags = self.flags;
        self.raw.symbols.push(symbol);
        id
    }

    fn open_scope(&mut self, symbol: Symbol, end: usize) {
        let id = self.push(symbol);
        self.scopes.push(Scope { end, id });
    }

    fn qualify(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_owned()
        } else {
            format!("{}\\{name}", self.namespace)
        }
    }

    fn qualify_member(&self, owner: &str, name: &str) -> String {
        if self.namespace.is_empty() {
            format!("{owner}::{name}")
        } else {
            format!("{}\\{owner}::{name}", self.namespace)
        }
    }

    fn function(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        let qualified = self.qualify(&name);

        let mut symbol = declared(name, qualified, SymbolKind::Function, node);
        symbol.exported = true;
        symbol.signature = signature(node, self.source);
        self.open_scope(symbol, node.end_byte());
    }

    // constraint: a method outside a named type, one of an anonymous class, is qualified by its name alone
    fn method(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        let owner = enclosing_type(node, self.source, Callables::Detach);
        let qualified = owner
            .as_deref()
            .map_or_else(|| name.clone(), |owner| self.qualify_member(owner, &name));

        let mut symbol = declared(name, qualified, SymbolKind::Method, node);
        symbol.owner = owner;
        symbol.exported = !is_private(node, self.source);
        symbol.signature = signature(node, self.source);
        self.open_scope(symbol, node.end_byte());
    }

    fn type_declaration(&mut self, node: Node<'_>, kind: SymbolKind) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        let qualified = self.qualify(&name);

        let mut symbol = declared(name, qualified, kind, node);
        symbol.parents = parents(node, self.source);
        symbol.exported = true;
        let id = self.push(symbol);
        self.types.push(Scope {
            end: node.end_byte(),
            id,
        });
    }

    // constraint: `private int $x, $y;` declares one member per element, each with the declaration's visibility
    fn members(&mut self, node: Node<'_>, element_kind: &str, kind: SymbolKind) {
        let member = Member {
            owner: enclosing_type(node, self.source, Callables::Detach),
            kind,
            exported: !is_private(node, self.source),
        };

        for element in named(node).filter(|element| element.kind() == element_kind) {
            let name = if kind == SymbolKind::Field {
                element
                    .child_by_field_name("name")
                    .and_then(|variable| variable_identifier(variable, self.source))
            } else {
                first_named_text(element, "name", self.source)
            };
            if let Some(name) = name {
                self.member(name, &member, element);
            }
        }
    }

    // constraint: a parameter promoted to a property belongs to the type whose constructor declares it, beyond the
    // constructor itself
    fn promoted_property(&mut self, node: Node<'_>) {
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| promoted_identifier(name, self.source))
        else {
            return;
        };
        let member = Member {
            owner: enclosing_type(node, self.source, Callables::Cross),
            kind: SymbolKind::Field,
            exported: !is_private(node, self.source),
        };
        self.member(name, &member, node);
    }

    fn enum_case(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let member = Member {
            owner: enclosing_type(node, self.source, Callables::Detach),
            kind: SymbolKind::EnumVariant,
            exported: true,
        };
        self.member(text(name, self.source), &member, node);
    }

    // constraint: a member outside any type, a `const` at file scope, is qualified by the namespace alone
    fn member(&mut self, name: String, member: &Member, node: Node<'_>) {
        let qualified = member.owner.as_deref().map_or_else(
            || self.qualify(&name),
            |owner| self.qualify_member(owner, &name),
        );

        let mut symbol = declared(name, qualified, member.kind, node);
        symbol.owner.clone_from(&member.owner);
        symbol.exported = member.exported;
        self.push(symbol);
    }

    // constraint: a `use` in the body of an anonymous class has no named type to take its parents, and the walk keeps
    // no scope for one, so the type the scopes name is that of an outer class
    fn trait_use(&mut self, node: Node<'_>) {
        if enclosing_type(node, self.source, Callables::Detach).is_none() {
            return;
        }
        let Some(&open) = self.types.last() else {
            return;
        };
        let Some(definition) = usize::try_from(open.id.0)
            .ok()
            .and_then(|at| self.raw.symbols.get_mut(at))
        else {
            return;
        };

        for name in named(node).filter(|child| NAMES.contains(&child.kind())) {
            let name = text(name, self.source);
            if !definition.parents.contains(&name) {
                definition.parents.push(name);
            }
        }
    }

    // constraint: a grouped `use A\{B, C\D}` writes the shared prefix once, as the declaration's `namespace_name`
    fn namespace_use(&mut self, node: Node<'_>) {
        for child in named(node) {
            match child.kind() {
                "namespace_use_clause" => self.use_clause(node, child, ""),
                "namespace_use_group" => self.use_group(node, child),
                _ => {}
            }
        }
    }

    fn use_group(&mut self, declaration: Node<'_>, group: Node<'_>) {
        let prefix =
            first_named_text(declaration, "namespace_name", self.source).unwrap_or_default();
        for clause in named(group).filter(|clause| clause.kind() == "namespace_use_clause") {
            self.use_clause(declaration, clause, &prefix);
        }
    }

    // constraint: `use function` and `use const` write their kind once for the whole declaration, or on each clause
    // of a mixed group, `use A\{B, function c}`
    fn use_clause(&mut self, declaration: Node<'_>, clause: Node<'_>, prefix: &str) {
        let Some(name) = named(clause).find(|child| NAMES.contains(&child.kind())) else {
            return;
        };
        let form = if [declaration, clause]
            .iter()
            .any(|node| node.child_by_field_name("type").is_some())
        {
            FUNCTION_OR_CONST_USE_FORM
        } else {
            USE_FORM
        };
        let name = text(name, self.source);
        let name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}\\{name}")
        };
        if name.is_empty() {
            return;
        }

        self.raw.directives.push(RawDirective::new(
            form,
            name,
            line(declaration.start_position().row),
        ));
    }

    fn include(&mut self, node: Node<'_>) {
        let Some(spec) = node.named_child(0).and_then(|path| fold(path, self.source)) else {
            return;
        };
        if !marker_leads(&spec) {
            return;
        }

        self.raw.directives.push(RawDirective::new(
            INCLUDE_FORM,
            spec,
            line(node.start_position().row),
        ));
    }

    // constraint: only a closure assigned to a plain variable is a callable, named by the variable and local to its
    // file; a closure passed as an argument opens no scope, so the calls in its body are calls of the enclosing
    // callable
    fn closure_binding(&mut self, node: Node<'_>) {
        let Some(variable) = node
            .child_by_field_name("left")
            .filter(|left| left.kind() == "variable_name")
        else {
            return;
        };
        let Some(closure) = node
            .child_by_field_name("right")
            .filter(|right| matches!(right.kind(), "anonymous_function" | "arrow_function"))
        else {
            return;
        };
        let Some(name) = variable_identifier(variable, self.source) else {
            return;
        };

        let mut symbol = Symbol::new(
            name,
            SymbolKind::Function,
            line(node.start_position().row),
            line(node.end_position().row),
        );
        symbol.signature = signature_from(node.start_byte(), closure, self.source);
        self.open_scope(symbol, closure.end_byte());
    }

    // constraint: `new self`, `new static` and `new parent` name no class; the grammar parses `static` as a plain name
    fn object_creation(&mut self, node: Node<'_>) {
        let class = named(node).find_map(|child| match child.kind() {
            "name" => {
                let name = text(child, self.source);
                (!matches!(name.as_str(), "self" | "static" | "parent")).then_some(name)
            }
            "qualified_name" | "relative_name" => last_segment(child, self.source),
            _ => None,
        });

        if let Some(class) = class {
            self.call(class, None, node);
        }
    }

    fn call(&mut self, callee: String, receiver: Option<String>, node: Node<'_>) {
        if callee.is_empty() || self.raw.symbols.is_empty() {
            return;
        }

        self.raw.calls.push(CallRef {
            caller_id: self.scopes.last().map_or(NO_CALLER_ID, |scope| scope.id),
            callee,
            receiver,
            line: line(node.start_position().row),
        });
    }
}

// constraint: `Symbol::new` copies its name into the qualified name, so the symbol starts unnamed, which allocates
// nothing, and then takes both names
fn declared(name: String, qualified: String, kind: SymbolKind, node: Node<'_>) -> Symbol {
    let mut symbol = Symbol::new(
        String::new(),
        kind,
        line(node.start_position().row),
        line(node.end_position().row),
    );
    symbol.name = name;
    symbol.qualified_name = qualified;
    symbol
}

// constraint: a member call names the receiver only by a variable and a scoped call only by a leaf scope, a class
// name, `self`, `parent`, `static` or a variable; a chained or computed one is the empty receiver. A qualified name,
// of a function or of a scope, reduces to its last segment, the bare name that owns a member.
fn call_target(node: Node<'_>, source: &[u8]) -> (String, Option<String>) {
    let (receiver, leaves): (&str, &[&str]) = match node.kind() {
        "function_call_expression" => {
            let callee =
                node.child_by_field_name("function")
                    .and_then(|function| match function.kind() {
                        "name" => Some(text(function, source)),
                        "qualified_name" | "relative_name" => last_segment(function, source),
                        _ => None,
                    });
            return (callee.unwrap_or_default(), None);
        }
        "member_call_expression" | "nullsafe_member_call_expression" => {
            ("object", &["variable_name"])
        }
        "scoped_call_expression" => (
            "scope",
            &[
                "name",
                "qualified_name",
                "relative_name",
                "relative_scope",
                "variable_name",
            ],
        ),
        _ => return (String::new(), None),
    };

    let Some(name) = node.child_by_field_name("name") else {
        return (String::new(), None);
    };
    let receiver = node
        .child_by_field_name(receiver)
        .map(|receiver| match receiver.kind() {
            kind if !leaves.contains(&kind) => String::new(),
            "qualified_name" | "relative_name" => {
                last_segment(receiver, source).unwrap_or_default()
            }
            _ => text(receiver, source),
        });

    (text(name, source), receiver)
}

// constraint: a member's type is the class, interface, trait or enum around it; an anonymous class is a type without
// a name, so its members have no owner, and a function or closure between the member and the type detaches it unless
// `callables` says to cross it
fn enclosing_type(node: Node<'_>, source: &[u8], callables: Callables) -> Option<String> {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        match current.kind() {
            "class_declaration"
            | "interface_declaration"
            | "trait_declaration"
            | "enum_declaration"
            | "anonymous_class" => {
                return current
                    .child_by_field_name("name")
                    .map(|name| text(name, source))
                    .filter(|name| !name.is_empty());
            }
            "function_definition"
            | "method_declaration"
            | "anonymous_function"
            | "arrow_function"
                if callables == Callables::Detach =>
            {
                return None;
            }
            _ => {}
        }
        ancestor = current.parent();
    }

    None
}

// constraint: a class takes its `extends` and `implements` names as parents, and an interface its `extends` names,
// which the grammar writes in the same clause
fn parents(node: Node<'_>, source: &[u8]) -> Vec<String> {
    all(node)
        .filter(|clause| matches!(clause.kind(), "base_clause" | "class_interface_clause"))
        .flat_map(|clause| all(clause).filter(|name| NAMES.contains(&name.kind())))
        .map(|name| text(name, source))
        .collect()
}

// constraint: `private` is the one modifier that hides a member; `protected`, `public`, `var` and none export it
fn is_private(node: Node<'_>, source: &[u8]) -> bool {
    all(node).any(|child| {
        child.kind() == "visibility_modifier" && &source[child.byte_range()] == b"private"
    })
}

fn variable_identifier(variable: Node<'_>, source: &[u8]) -> Option<String> {
    first_named_text(variable, "name", source)
}

// constraint: the grammar writes a promoted parameter's name as a `variable_name` or a `by_ref` around one
fn promoted_identifier(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "variable_name" => variable_identifier(node, source),
        "by_ref" => promoted_identifier(node.named_child(0)?, source),
        _ => None,
    }
}

fn last_segment(name: Node<'_>, source: &[u8]) -> Option<String> {
    named(name)
        .rfind(|segment| segment.kind() == "name")
        .map(|segment| text(segment, source))
}

fn first_named_text(node: Node<'_>, kind: &str, source: &[u8]) -> Option<String> {
    named(node)
        .find(|child| child.kind() == kind)
        .map(|child| text(child, source))
}

fn named(node: Node<'_>) -> impl DoubleEndedIterator<Item = Node<'_>> {
    named_children(node).filter_map(move |index| node.named_child(index))
}

fn all(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    children(node).filter_map(move |index| node.child(index))
}
