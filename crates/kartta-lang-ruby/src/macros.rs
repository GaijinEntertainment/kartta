use std::collections::HashMap;
use std::ops::Range;

use kartta_lang::tree::{named_child_nodes, signature_from, text};
use kartta_lang::{FILE_SCOPE, LocalCall, RawExtraction, SymbolId, SymbolKind};
use tree_sitter::Node;

use crate::bindings::MethodKind;
use crate::owner::{enclosing_type, in_singleton_class};
use crate::walk::{Emitter, declared, visibility_exports};

/// A method `let` or `subject` defines, with the bytes of its example group.
pub(crate) struct GroupMethod<'s> {
    name: &'s [u8],
    id: SymbolId,
    group: Range<usize>,
}

struct Accessors {
    owner: String,
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
            "let" | "let!" | "subject" | "subject!" => self.example_group_method(call),
            _ => {}
        }
    }

    // constraint: `let(:x) { }`, `let!(:x) { }`, `subject(:x) { }` and `subject!(:x) { }` define the method `x` on the
    // anonymous class of the example group whose block holds them, a class no other file names, so `x` is local to the
    // file and the block is the method's body
    fn example_group_method(&mut self, call: Node<'_>) {
        if call.child_by_field_name("block").is_none() {
            return;
        }
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        let Some(symbol) = named_child_nodes(arguments)
            .next()
            .filter(|first| first.kind() == "simple_symbol")
        else {
            return;
        };
        let source = self.source;
        let spelled = &source[symbol.byte_range()];
        let name = text(symbol, source).trim_start_matches(':').to_owned();
        let mut definition = declared(name.clone(), name, SymbolKind::Function, call);
        definition.signature = signature_from(call.start_byte(), arguments, source);
        let id = self.push(definition);
        self.raw.local_symbols.push(id);
        self.open_method(call, id);
        self.group_methods.push(GroupMethod {
            name: spelled.strip_prefix(b":").unwrap_or(spelled),
            id,
            group: enclosing_group(call),
        });
    }

    // constraint: a method of an example group takes no argument and no block, so a call that passes one is a DSL's
    // method of the name, and a call at file scope keeps its name, which is all the graph keeps of it
    pub(crate) fn plain_call(&mut self, node: Node<'_>, site_of: SymbolId) {
        if site_of != FILE_SCOPE {
            self.plain_calls
                .push((self.raw.calls.len(), node.start_byte()));
        }
    }

    // constraint: a method of an example group is reached from the group and the groups nested in it, wherever in them
    // it is defined, so a call binds the innermost one around it, and of two in one group the later, which replaces the
    // earlier
    pub(crate) fn bind_group_methods(&mut self) {
        let mut named: HashMap<&[u8], Vec<&GroupMethod<'_>>> = HashMap::new();
        for method in &self.group_methods {
            named.entry(method.name).or_default().push(method);
        }
        if named.is_empty() {
            return;
        }
        let RawExtraction {
            calls, local_calls, ..
        } = &mut self.raw;
        let mut plain = self.plain_calls.iter().copied().peekable();
        let mut at = 0;
        calls.retain(|call| {
            let innermost = plain
                .next_if(|&(call, _)| call == at)
                .and_then(|(_, byte)| {
                    let methods = named.get(call.callee.as_bytes())?;
                    (methods.iter().rev())
                        .filter(|method| method.group.contains(&byte))
                        .min_by_key(|method| method.group.len())
                });
            at += 1;
            if let Some(method) = innermost {
                local_calls.push(LocalCall {
                    site_of: call.site_of,
                    local: method.id,
                    line: call.line,
                });
            }
            innermost.is_none()
        });
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

        for module in named_child_nodes(arguments)
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

        for argument in
            named_child_nodes(arguments).filter(|argument| argument.kind() == "simple_symbol")
        {
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
        let qualified = format!("{}{}{name}", accessors.owner, accessors.separator);
        let mut symbol = declared(name, qualified.clone(), SymbolKind::Method, argument);
        symbol.owner = Some(accessors.owner.clone());
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

        for argument in
            named_child_nodes(arguments).filter(|argument| argument.kind() == "simple_symbol")
        {
            let name = text(argument, self.source);
            let name = self.name_key(
                kind,
                format!("{owner}{separator}{}", name.trim_start_matches(':')),
            );
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
        let mut names = named_child_nodes(arguments)
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
            owner.as_ref().map_or_else(
                || name.to_owned(),
                |owner| format!("{owner}{separator}{name}"),
            )
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
        symbol.owner = owner;
        symbol.exported = self.exported_by_default();
        let id = self.push(symbol);
        let name = self.name_key(method_kind, qualified);
        self.bindings.bind_alias(name, id, body);
    }
}

/// The bytes of the example group that holds `call`: the block or the type around it, else the whole file.
fn enclosing_group(call: Node<'_>) -> Range<usize> {
    let mut group = call;
    while let Some(parent) = group.parent() {
        group = parent;
        if matches!(
            group.kind(),
            "block" | "do_block" | "class" | "module" | "singleton_class"
        ) {
            break;
        }
    }
    group.byte_range()
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
