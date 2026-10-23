use kartta_lang::tree::{line, named_child_nodes, preorder, signature, text};
use kartta_lang::{
    Access, Banner, CallRef, FILE_SCOPE, RawDirective, RawExtraction, Receiver, Symbol,
    SymbolFlags, SymbolId, SymbolKind, UseRef,
};
use tree_sitter::{Node, Tree};

use crate::bindings::{Bindings, MethodKind, NameKey};
use crate::locals::{Locals, Role, is_keyword, role};
use crate::owner::{constant_type, enclosing_type, in_singleton_class, type_qualification};

pub(crate) const REQUIRE_FORM: u32 = 0;
pub(crate) const REQUIRE_RELATIVE_FORM: u32 = 1;

const BANNER: Banner =
    Banner::new(&["@generated", "DO NOT EDIT", "This file is auto-generated"]).with_lines(&["#"]);

pub(crate) const IDENTIFIER: &str = "identifier";

/// The separator a class method's qualified name joins to its owner, `Shape.build`, which a receiver on the class ends
/// in.
pub(crate) const CLASS_MEMBER_SEPARATOR: &str = ".";

fn on_the_class(mut chain: String) -> String {
    chain.push_str(CLASS_MEMBER_SEPARATOR);
    chain
}

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter {
        source,
        scopes: Vec::new(),
        open_types: Vec::new(),
        visibilities: Vec::new(),
        flags: SymbolFlags::empty().with_generated(BANNER.opens(source)),
        bindings: Bindings::default(),
        locals: Locals::default(),
        raw: RawExtraction::default(),
    };

    preorder(tree.root_node(), |node| {
        if node.is_named() {
            emitter.handle(node);
        }
        true
    });

    let Emitter {
        bindings, mut raw, ..
    } = emitter;
    raw.callables = bindings.into_callables();
    raw
}

#[derive(Clone, Copy)]
pub(crate) struct Scope {
    end: usize,
    pub(crate) id: SymbolId,
}

// constraint: a bare `private`, `protected` or `public` sets whether the methods after it in the body of a class, a
// module or a `class << self` are exported
#[derive(Clone, Copy)]
pub(crate) struct Visibility {
    end: usize,
    pub(crate) exported: bool,
}

pub(crate) struct Emitter<'s> {
    pub(crate) source: &'s [u8],
    scopes: Vec<Scope>,
    pub(crate) open_types: Vec<Scope>,
    pub(crate) visibilities: Vec<Visibility>,
    flags: SymbolFlags,
    pub(crate) bindings: Bindings,
    locals: Locals<'s>,
    pub(crate) raw: RawExtraction,
}

impl Emitter<'_> {
    fn handle(&mut self, node: Node<'_>) {
        let start = node.start_byte();
        while self.scopes.last().is_some_and(|scope| scope.end <= start) {
            self.scopes.pop();
        }
        while self
            .open_types
            .last()
            .is_some_and(|scope| scope.end <= start)
        {
            self.open_types.pop();
        }
        while self
            .visibilities
            .last()
            .is_some_and(|visibility| visibility.end <= start)
        {
            self.visibilities.pop();
        }
        self.locals.leave(start);
        self.locals.enter(node, self.source);

        match node.kind() {
            // constraint: a `def` inside `class << self` defines a class method, `Owner.name`
            "method" if in_singleton_class(node) => self.scoped(node, Self::singleton_method),
            "method" => self.scoped(node, Self::method),
            "singleton_method" => self.scoped(node, Self::singleton_method),
            "class" | "module" => {
                self.type_definition(node);
                self.visibilities.push(Visibility {
                    end: node.end_byte(),
                    exported: true,
                });
            }
            "singleton_class" => self.visibilities.push(Visibility {
                end: node.end_byte(),
                exported: true,
            }),
            "assignment" => self.constant_assignment(node),
            "alias" => self.alias(node),
            IDENTIFIER => self.identifier(node),
            "call" => self.call(node),
            _ => {}
        }
    }

    fn identifier(&mut self, node: Node<'_>) {
        let source = self.source;
        let name = &source[node.byte_range()];
        match role(node) {
            Role::Binding => self.locals.bind(name),
            Role::Read if !self.locals.is_local(name) && !is_keyword(name) => {
                self.visibility_statement(node);
                let site_of = self.site_of();
                self.raw.calls.push(CallRef {
                    site_of,
                    callee: text(node, source),
                    receiver: self.implicit_receiver(site_of),
                    line: line(node.start_position().row),
                });
            }
            Role::Read | Role::Name => {}
        }
    }

    fn scoped<'t>(
        &mut self,
        node: Node<'t>,
        definition: fn(&mut Self, Node<'t>) -> Option<SymbolId>,
    ) {
        if let Some(id) = definition(self, node) {
            self.scopes.push(Scope {
                end: node.end_byte(),
                id,
            });
        }
    }

    pub(crate) fn push(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.flags = self.flags;
        self.raw.push_symbol(symbol)
    }

    pub(crate) fn name_key(&self, kind: MethodKind, qualified: String) -> NameKey {
        NameKey::new(self.scopes.last().map(|scope| scope.id), kind, qualified)
    }

    pub(crate) fn exported_by_default(&self) -> bool {
        self.visibilities
            .last()
            .is_none_or(|visibility| visibility.exported)
    }

    fn method(&mut self, node: Node<'_>) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let owner = enclosing_type(node, self.source);
        let (kind, qualified) = owner.as_ref().map_or_else(
            || (SymbolKind::Function, name.clone()),
            |owner| (SymbolKind::Method, format!("{owner}#{name}")),
        );

        let mut symbol = declared(name, qualified.clone(), kind, node);
        symbol.owner = owner;
        symbol.exported =
            wrapping_visibility(node, self.source).unwrap_or_else(|| self.exported_by_default());
        symbol.signature = signature(node, self.source).map(strip_endless_eq);
        let id = self.push(symbol);
        let name = self.name_key(MethodKind::Instance, qualified);
        self.bindings.bind(name, id);
        Some(id)
    }

    // constraint: `def self.x` is a method of the enclosing type, `def Foo.x` of the type `Foo` names, and `def obj.x`
    // of a runtime value, which has no owner. A `def` inside `class << self` takes the visibility of that body; a bare
    // `private` covers instance methods only, so a `def self.x` stays exported
    fn singleton_method(&mut self, node: Node<'_>) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let owner = match node.child_by_field_name("object") {
            Some(object) if object.kind() == "constant" => {
                Some(constant_type(node, text(object, self.source), self.source))
            }
            Some(object) if object.kind() != "self" => None,
            _ => enclosing_type(node, self.source),
        };
        let qualified = owner
            .as_ref()
            .map_or_else(|| name.clone(), |owner| format!("{owner}.{name}"));

        let mut symbol = declared(name, qualified.clone(), SymbolKind::Method, node);
        symbol.owner = owner;
        symbol.exported = node.kind() == "singleton_method"
            || wrapping_visibility(node, self.source).unwrap_or_else(|| self.exported_by_default());
        symbol.signature = signature(node, self.source).map(strip_endless_eq);
        let id = self.push(symbol);
        let name = self.name_key(MethodKind::Singleton, qualified);
        self.bindings.bind(name, id);
        Some(id)
    }

    fn type_definition(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        let (owner, qualified) = type_qualification(node, &name, self.source);
        let kind = if node.kind() == "class" {
            SymbolKind::Class
        } else {
            SymbolKind::Namespace
        };

        let mut symbol = declared(name, qualified, kind, node);
        symbol.owner = owner;
        symbol.exported = true;
        if let Some(superclass) = node.child_by_field_name("superclass") {
            let superclass = text(superclass, self.source);
            symbol
                .parents
                .push(superclass.trim().trim_start_matches('<').trim().to_owned());
        }
        let id = self.push(symbol);
        self.open_types.push(Scope {
            end: node.end_byte(),
            id,
        });
    }

    // constraint: only a constant on the left defines a symbol; a qualified one, `Foo::BAR = 5`, reopens another
    // type, and an identifier or a global is a variable
    fn constant_assignment(&mut self, node: Node<'_>) {
        let Some(left) = node
            .child_by_field_name("left")
            .filter(|left| left.kind() == "constant")
        else {
            return;
        };
        let name = text(left, self.source);
        let (owner, qualified) = type_qualification(node, &name, self.source);
        let kind = if node
            .child_by_field_name("right")
            .is_some_and(|right| builds_a_class(right, self.source))
        {
            SymbolKind::Class
        } else {
            SymbolKind::Constant
        };

        let mut symbol = declared(name, qualified, kind, node);
        symbol.owner = owner;
        symbol.exported = true;
        self.push(symbol);
    }

    fn call(&mut self, node: Node<'_>) {
        let Some(method) = node.child_by_field_name("method") else {
            return;
        };
        let callee = text(method, self.source);
        let site_of = self.site_of();
        let receiver_node = node.child_by_field_name("receiver");
        let receiver = match receiver_node {
            Some(receiver) => Some(match receiver.kind() {
                IDENTIFIER => text(receiver, self.source),
                "self" => self.type_of_self(site_of).unwrap_or_default(),
                "constant" | "scope_resolution" => CLASS_MEMBER_SEPARATOR.to_owned(),
                "call" if self.is_class_of_an_instance(receiver, site_of) => {
                    CLASS_MEMBER_SEPARATOR.to_owned()
                }
                _ => String::new(),
            }),
            None => self.implicit_receiver(site_of),
        };

        // constraint: a call with any receiver, `config.require 'x'`, is a method of its own and loads nothing
        if receiver_node.is_none()
            && let Some(form) = require_form(&callee)
            && let Some(module) = first_string_argument(node, self.source)
        {
            self.raw.directives.push(RawDirective::new(
                form,
                module,
                line(node.start_position().row),
            ));
        }

        if callee.is_empty() {
            return;
        }
        if receiver_node.is_none() {
            self.class_body_macro(node, &callee);
        }
        self.method_reference(&callee, node, site_of);
        self.raw.calls.push(CallRef {
            site_of,
            callee,
            receiver,
            line: line(node.start_position().row),
        });
    }

    // constraint: `method(:render)`, `send(:render)` and their kin name a method by a symbol, of the object's own type
    // where the receiver is none or `self` and of a class where it is a constant or a constant path; a receiver of any
    // other kind names a type the walk cannot tell, and `instance_method` and `respond_to?` most often only inspect. In
    // a class body outside every method, the class makes the use and names its own method through its name, as a use
    // at file scope keeps only the name it spells and would reach every namesake the file sees. A top-level `def` is a
    // private method of `Object`, which every object reaches, so at a site no class owns, file scope, a top-level
    // `def`, `def self.x` or `def obj.x`, the use is bare. `singleton_method` with no receiver or `self` names a method
    // of the object itself, which only a class body names, through its class
    fn method_reference(&mut self, callee: &str, call: Node<'_>, site_of: SymbolId) {
        if !matches!(
            callee,
            "method" | "public_method" | "singleton_method" | "send" | "public_send" | "__send__"
        ) {
            return;
        }
        let Some(symbol) = call
            .child_by_field_name("arguments")
            .and_then(|arguments| named_child_nodes(arguments).next())
            .filter(|argument| argument.kind() == "simple_symbol")
        else {
            return;
        };
        let receiver = match call.child_by_field_name("receiver") {
            None => Receiver::Own,
            Some(receiver) if receiver.kind() == "self" => Receiver::Own,
            Some(receiver) if matches!(receiver.kind(), "constant" | "scope_resolution") => {
                Receiver::Path(on_the_class(text(receiver, self.source)))
            }
            Some(_) => return,
        };
        let class = self.is_type_body(site_of).then_some(site_of);
        if callee == "singleton_method" && receiver == Receiver::Own && class.is_none() {
            return;
        }
        let (site_of, receiver) = match class {
            Some(class) if receiver == Receiver::Own => (
                class,
                Receiver::Path(on_the_class(self.qualified_name(class))),
            ),
            Some(class) => (class, receiver),
            None if receiver == Receiver::Own && !self.has_owner(site_of) => {
                (site_of, Receiver::Bare)
            }
            None => (site_of, receiver),
        };
        let spelled = &self.source[symbol.byte_range()];
        self.raw.uses.push(UseRef {
            site_of,
            name: String::from_utf8_lossy(spelled.strip_prefix(b":").unwrap_or(spelled))
                .into_owned(),
            receiver,
            access: Access::Read,
            line: line(symbol.start_position().row),
        });
    }

    // constraint: a call or a use outside every method is made by the class or the module whose body holds it, which
    // runs it with `self` as the type itself; outside every type it is made from file scope
    fn site_of(&self) -> SymbolId {
        let method = self.scopes.last();
        let body = self
            .open_types
            .last()
            .filter(|body| method.is_none_or(|method| body.end < method.end));
        body.or(method).map_or(FILE_SCOPE, |scope| scope.id)
    }

    fn is_type_body(&self, site_of: SymbolId) -> bool {
        site_of != FILE_SCOPE
            && self
                .open_types
                .last()
                .is_some_and(|body| body.id == site_of)
    }

    fn implicit_receiver(&self, site_of: SymbolId) -> Option<String> {
        self.is_type_body(site_of)
            .then(|| on_the_class(self.qualified_name(site_of)))
    }

    // constraint: `self` is the type itself in a type's body and in a class method, and an instance of the owner in an
    // instance method
    fn type_of_self(&self, site_of: SymbolId) -> Option<String> {
        if self.is_type_body(site_of) {
            return Some(on_the_class(self.qualified_name(site_of)));
        }
        let (owner, class_method) = self.owner_of_method(site_of)?;
        Some(if class_method {
            on_the_class(owner.to_owned())
        } else {
            owner.to_owned()
        })
    }

    /// The owner of the method `site_of` names, and whether it is a class method.
    fn owner_of_method(&self, site_of: SymbolId) -> Option<(&str, bool)> {
        let symbol = self.symbol(site_of)?;
        let owner = symbol.owner.as_deref()?;
        let class_method = symbol
            .qualified_name
            .strip_prefix(owner)
            .is_some_and(|member| member.starts_with(CLASS_MEMBER_SEPARATOR));
        Some((owner, class_method))
    }

    // constraint: `self.class` in an instance method is the owner's class; elsewhere `self` is a type, whose class is
    // `Class`
    fn is_class_of_an_instance(&self, receiver: Node<'_>, site_of: SymbolId) -> bool {
        receiver
            .child_by_field_name("arguments")
            .is_none_or(|arguments| named_child_nodes(arguments).next().is_none())
            && receiver
                .child_by_field_name("method")
                .is_some_and(|method| &self.source[method.byte_range()] == b"class")
            && receiver
                .child_by_field_name("receiver")
                .is_some_and(|of| of.kind() == "self")
            && !self.is_type_body(site_of)
            && self
                .owner_of_method(site_of)
                .is_some_and(|(_, class_method)| !class_method)
    }

    fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.raw.symbols.get(usize::try_from(id.0).ok()?)
    }

    fn qualified_name(&self, id: SymbolId) -> String {
        self.symbol(id)
            .map(|symbol| symbol.qualified_name.clone())
            .unwrap_or_default()
    }

    fn has_owner(&self, id: SymbolId) -> bool {
        self.symbol(id).is_some_and(|symbol| symbol.owner.is_some())
    }
}

pub(crate) fn declared(
    name: String,
    qualified: String,
    kind: SymbolKind,
    span: Node<'_>,
) -> Symbol {
    let mut symbol = Symbol::new(
        name,
        kind,
        line(span.start_position().row),
        line(span.end_position().row),
    );
    symbol.qualified_name = qualified;
    symbol
}

fn require_form(callee: &str) -> Option<u32> {
    match callee {
        "require" => Some(REQUIRE_FORM),
        "require_relative" => Some(REQUIRE_RELATIVE_FORM),
        _ => None,
    }
}

// constraint: only a string of plain text names a file; an interpolation builds the path at run time, and an escape
// spells it apart from its value, so a string holding either names nothing
fn first_string_argument(call: Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = call.child_by_field_name("arguments")?;
    let string = named_child_nodes(arguments).find(|argument| argument.kind() == "string")?;
    let mut parts = named_child_nodes(string);
    let content = parts
        .next()
        .filter(|content| content.kind() == "string_content")?;

    parts.next().is_none().then(|| text(content, source))
}

// constraint: an endless method, `def area = 3.14`, cuts to `def area =`; only a `=` after whitespace is that form's,
// since a setter's `=` is glued to its name
fn strip_endless_eq(signature: String) -> String {
    match signature.strip_suffix('=') {
        Some(head) if head.ends_with(char::is_whitespace) => head.trim_end().to_owned(),
        _ => signature,
    }
}

// constraint: `Struct.new(...)` and `Data.define(...)` bound to a constant build a class
fn builds_a_class(right: Node<'_>, source: &[u8]) -> bool {
    if right.kind() != "call" {
        return false;
    }
    let receiver = right
        .child_by_field_name("receiver")
        .filter(|receiver| receiver.kind() == "constant")
        .map(|receiver| &source[receiver.byte_range()]);
    let method = right
        .child_by_field_name("method")
        .map(|method| &source[method.byte_range()]);

    matches!(
        (receiver, method),
        (Some(b"Struct"), Some(b"new")) | (Some(b"Data"), Some(b"define"))
    )
}

// constraint: `private def x` sets the visibility of that one method over the running default of the body
fn wrapping_visibility(method: Node<'_>, source: &[u8]) -> Option<bool> {
    let call = method
        .parent()
        .filter(|parent| parent.kind() == "argument_list")?
        .parent()
        .filter(|parent| parent.kind() == "call")?;

    visibility_exports(&source[call.child_by_field_name("method")?.byte_range()])
}

pub(crate) const fn visibility_exports(modifier: &[u8]) -> Option<bool> {
    match modifier {
        b"private" => Some(false),
        b"protected" | b"public" => Some(true),
        _ => None,
    }
}
