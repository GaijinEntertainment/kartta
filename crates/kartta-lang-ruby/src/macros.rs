use kartta_lang::SymbolKind;
use kartta_lang::tree::text;
use tree_sitter::Node;

use crate::bindings::MethodKind;
use crate::owner::{EnclosingType, enclosing_type, in_singleton_class};
use crate::walk::{Emitter, declared, named, visibility_exports};

struct Accessors {
    owner: EnclosingType,
    kind: MethodKind,
    separator: &'static str,
    exported: bool,
}

impl Emitter<'_> {
    pub(crate) fn class_body_macro(&mut self, call: Node<'_>, callee: &str) {
        match callee {
            "include" | "extend" | "prepend" => self.mixin(call),
            "attr_accessor" | "attr_reader" | "attr_writer" => self.accessors(call, callee),
            "private" | "protected" | "public" => {
                self.visibility_arguments(call, method_kind(call).0, callee != "private");
            }
            "private_class_method" | "public_class_method" => {
                self.visibility_arguments(
                    call,
                    MethodKind::Singleton,
                    callee != "private_class_method",
                );
            }
            "alias_method" => self.alias_method(call),
            _ => {}
        }
    }

    // constraint: a mixin is a parent of the type whose body holds it, the innermost open type, which the walk has
    // already emitted; a mixin inside a method body belongs to no type
    fn mixin(&mut self, call: Node<'_>) {
        if enclosing_type(call, self.source).is_none() {
            return;
        }
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        let Some(definition) = self
            .open_types
            .last()
            .and_then(|open| usize::try_from(open.id.0).ok())
            .and_then(|at| self.raw.symbols.get_mut(at))
        else {
            return;
        };

        for module in named(arguments)
            .filter(|argument| matches!(argument.kind(), "constant" | "scope_resolution"))
        {
            let module = text(module, self.source);
            if !definition.parents.contains(&module) {
                definition.parents.push(module);
            }
        }
    }

    // constraint: `attr_reader :x` defines `Owner#x`, `attr_writer :x` defines `Owner#x=`, and `attr_accessor` both;
    // inside `class << self` they are class methods, `Owner.x`
    fn accessors(&mut self, call: Node<'_>, callee: &str) {
        let Some(owner) = enclosing_type(call, self.source) else {
            return;
        };
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        let (kind, separator) = method_kind(call);
        let accessors = Accessors {
            owner,
            kind,
            separator,
            exported: self.exported_by_default(),
        };
        let (reads, writes) = (callee != "attr_writer", callee != "attr_reader");

        for argument in named(arguments).filter(|argument| argument.kind() == "simple_symbol") {
            let name = text(argument, self.source)
                .trim_start_matches(':')
                .to_owned();
            if name.is_empty() {
                continue;
            }
            if reads {
                self.accessor(&accessors, name.clone(), argument);
            }
            if writes {
                self.accessor(&accessors, format!("{name}="), argument);
            }
        }
    }

    fn accessor(&mut self, accessors: &Accessors, name: String, argument: Node<'_>) {
        let qualified = accessors.owner.member(accessors.separator, &name);
        let mut symbol = declared(name, qualified.clone(), SymbolKind::Method, argument);
        symbol.owner = Some(accessors.owner.qualified.clone());
        symbol.exported = accessors.exported;
        let id = self.push(symbol);
        let name = self.name_key(accessors.kind, qualified);
        self.bindings.bind(name, id);
    }

    // constraint: only a bare modifier directly in the body of a class, a module or a `class << self` sets the default;
    // the same word in a method body is an expression
    pub(crate) fn visibility_statement(&mut self, identifier: Node<'_>) {
        let in_type_body = identifier
            .parent()
            .filter(|parent| parent.kind() == "body_statement")
            .and_then(|body| body.parent())
            .is_some_and(|definition| {
                matches!(definition.kind(), "class" | "module" | "singleton_class")
            });
        if !in_type_body {
            return;
        }

        if let Some(exported) = visibility_exports(&self.source[identifier.byte_range()])
            && let Some(visibility) = self.visibilities.last_mut()
        {
            visibility.exported = exported;
        }
    }

    // constraint: `private :x` sets the method the body has already declared, the instance method `Owner#x`, or the
    // class method `Owner.x` inside `class << self`; `private_class_method :x` sets the class method
    fn visibility_arguments(&mut self, call: Node<'_>, kind: MethodKind, exported: bool) {
        let Some(owner) = enclosing_type(call, self.source) else {
            return;
        };
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        let separator = match kind {
            MethodKind::Instance => "#",
            MethodKind::Singleton => ".",
        };

        for argument in named(arguments).filter(|argument| argument.kind() == "simple_symbol") {
            let name = text(argument, self.source);
            let name = self.name_key(kind, owner.member(separator, name.trim_start_matches(':')));
            if let Some(symbol) = self
                .bindings
                .declaration(&name)
                .and_then(|id| usize::try_from(id.0).ok())
                .and_then(|at| self.raw.symbols.get_mut(at))
            {
                symbol.exported = exported;
            }
        }
    }

    // constraint: the grammar's `name` field of `alias new old` is the new method and its `alias` field the original
    pub(crate) fn alias(&mut self, node: Node<'_>) {
        let Some(new) = node.child_by_field_name("name") else {
            return;
        };
        let original = node
            .child_by_field_name("alias")
            .map(|original| method_name(original, self.source));
        self.declare_alias(method_name(new, self.source), original, node);
    }

    fn alias_method(&mut self, call: Node<'_>) {
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        let source = self.source;
        let mut names = named(arguments)
            .filter(|argument| matches!(argument.kind(), "simple_symbol" | "string"))
            .map(|argument| method_name(argument, source));
        let Some(name) = names.next() else {
            return;
        };
        let original = names.next();
        self.declare_alias(name, original, call);
    }

    // constraint: an alias inside `class << self` is a class method, `Owner.name`; either takes the running visibility
    // of the body around it
    fn declare_alias(&mut self, name: String, original: Option<String>, node: Node<'_>) {
        if name.is_empty() {
            return;
        }
        let owner = enclosing_type(node, self.source);
        let (method_kind, separator) = method_kind(node);
        let qualify = |name: &str| {
            owner
                .as_ref()
                .map_or_else(|| name.to_owned(), |owner| owner.member(separator, name))
        };

        let qualified = qualify(&name);
        let body = original.and_then(|original| {
            self.bindings
                .body(&self.name_key(method_kind, qualify(&original)))
        });
        let kind = if owner.is_some() {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };

        let mut symbol = declared(name, qualified.clone(), kind, node);
        symbol.owner = owner.map(|owner| owner.qualified);
        symbol.exported = self.exported_by_default();
        let id = self.push(symbol);
        let name = self.name_key(method_kind, qualified);
        self.bindings.bind_alias(name, id, body);
    }
}

fn method_kind(node: Node<'_>) -> (MethodKind, &'static str) {
    if in_singleton_class(node) {
        (MethodKind::Singleton, ".")
    } else {
        (MethodKind::Instance, "#")
    }
}

fn method_name(node: Node<'_>, source: &[u8]) -> String {
    text(node, source)
        .trim_start_matches(':')
        .trim_matches(|quote| quote == '"' || quote == '\'')
        .to_owned()
}
