use std::collections::HashMap;
use std::ops::Range;

use kartta_lang::tree::{
    child_nodes, line, named_child_nodes, preorder, signature, signature_from, text,
};
use kartta_lang::{
    Access, CallRef, FILE_SCOPE, RawDirective, RawExtraction, Receiver, Symbol, SymbolFlags,
    SymbolId, SymbolKind, UseRef,
};
use tree_sitter::{Node, Tree};

use crate::banner::is_generated_source;
use crate::fold::{fold, marker_leads};

pub(crate) const INCLUDE_FORM: u32 = 0;
pub(crate) const USE_FORM: u32 = 1;
pub(crate) const FUNCTION_OR_CONST_USE_FORM: u32 = 2;

const NAMES: [&str; 3] = ["name", "qualified_name", "relative_name"];

// constraint: an anonymous class is named by the names that lead to it, joined by `·` as a Go anonymous struct is, in
// braces, which no PHP name holds, so the name stays apart from a class and a variable of those names: `{make·job}`
const JOIN: &str = "\u{b7}";

pub(crate) fn raw_walk(tree: &Tree, source: &[u8]) -> RawExtraction {
    let mut emitter = Emitter {
        source,
        scopes: Vec::new(),
        types: Vec::new(),
        anonymous_counts: HashMap::new(),
        holders: Vec::new(),
        frames: Vec::new(),
        imports: Vec::new(),
        namespace: String::new(),
        flags: SymbolFlags::empty().with_generated(is_generated_source(source)),
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

struct Member {
    owner: Option<String>,
    kind: SymbolKind,
    exported: bool,
}

// constraint: a variable holds its class from the end of the assignment, whose right side reads the value before it, up
// to the end of the branch that assigned it, where another path may have left another value, or the next write
struct Holder {
    variable: Range<usize>,
    class: String,
    from: usize,
    until: usize,
}

// constraint: a variable is the function's own, a closure's included, whether or not the closure opens a symbol, so a
// frame's holders are the ones past `holders`, and the frame's end takes them away
struct Frame {
    end: usize,
    holders: usize,
}

// constraint: a class `use` binds the name at `alias`, or the imported name's last segment, to that name, which the
// `use`'s directive holds; `use function` and `use const` bind their names in tables of their own
struct Import {
    alias: Range<usize>,
    directive: usize,
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
    anonymous_counts: HashMap<String, u32>,
    holders: Vec<Holder>,
    frames: Vec<Frame>,
    imports: Vec<Import>,
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
        while let Some(frame) = self.frames.pop_if(|frame| frame.end <= start) {
            self.holders.truncate(frame.holders);
        }

        match node.kind() {
            "namespace_definition" => {
                self.namespace = node
                    .child_by_field_name("name")
                    .map(|name| text(name, self.source))
                    .unwrap_or_default();
                self.imports.clear();
            }
            "function_definition" => {
                self.open_frame(node);
                self.function(node);
            }
            "method_declaration" => {
                self.open_frame(node);
                self.method(node);
            }
            "anonymous_function" => self.closure(node),
            "arrow_function" => self.arrow_function(node),
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
            "assignment_expression" => {
                self.forget_writes(node);
                self.hold_created(node);
                self.closure_binding(node);
            }
            "augmented_assignment_expression"
            | "reference_assignment_expression"
            | "foreach_statement"
            | "catch_clause"
            | "unset_statement" => self.forget_writes(node),
            "object_creation_expression" => self.object_creation(node),
            "function_call_expression" => {
                let (callee, receiver) = call_target(node, self.source);
                self.callable_argument(&callee, node);
                self.call(callee, receiver, node);
            }
            "member_call_expression" | "nullsafe_member_call_expression" => {
                let (callee, receiver) = call_target(node, self.source);
                let receiver = self.held_receiver(receiver, start);
                self.call(callee, receiver, node);
            }
            "scoped_call_expression" => {
                let (callee, receiver) = call_target(node, self.source);
                let receiver = self.aliased_class(node).or(receiver);
                let receiver = self.held_receiver(receiver, start);
                self.call(callee, receiver, node);
            }
            "array_creation_expression" => self.array_callable(node),
            _ => {}
        }
    }

    // constraint: a function that takes a callable takes it as a string naming a function, `'trim'`, or a static
    // method, `'Cache::flush'`, in the positional argument the table names; the table names global functions, so a
    // call spelled through a namespace, `App\array_map()`, calls a function of the repository instead
    fn callable_argument(&mut self, callee: &str, call: Node<'_>) {
        let Some(position) = callable_position(callee) else {
            return;
        };
        let global = call
            .child_by_field_name("function")
            .is_some_and(|function| {
                let spelled = &self.source[function.byte_range()];
                !spelled
                    .strip_prefix(b"\\")
                    .unwrap_or(spelled)
                    .contains(&b'\\')
            });
        if !global {
            return;
        }
        let Some(argument) = call
            .child_by_field_name("arguments")
            .and_then(|arguments| {
                named_child_nodes(arguments)
                    .filter(|argument| argument.kind() == "argument")
                    .nth(position)
            })
            .filter(|argument| argument.child_by_field_name("name").is_none())
            .and_then(|argument| argument.named_child(0))
        else {
            return;
        };
        let Some(callable) = string_text(argument, self.source) else {
            return;
        };
        let (receiver, name) = match callable.split_once("::") {
            Some((class, method)) if self.in_own_namespace(class) => {
                (Receiver::Path(last_name(class).to_owned()), method)
            }
            None if self.in_own_namespace(callable) => (Receiver::Bare, last_name(callable)),
            _ => return,
        };
        if is_identifier(name) {
            self.value_use(name.to_owned(), receiver, argument);
        }
    }

    // constraint: a callable string is fully qualified, so `'helper'` names the global `helper` and
    // `'App\Jobs\run'` the `run` of `App\Jobs`; it names one the file declares only where its namespace is the file's
    fn in_own_namespace(&self, spelled: &str) -> bool {
        let spelled = spelled.strip_prefix('\\').unwrap_or(spelled);
        spelled
            .rsplit_once('\\')
            .map_or("", |(namespace, _)| namespace)
            == self.namespace
    }

    // constraint: `[$this, 'm']`, `[self::class, 'm']`, `[static::class, 'm']` and `[parent::class, 'm']` name a
    // method of the class around them, and `[Cache::class, 'm']` one of `Cache`; `['Cache', 'm']` reads as any pair
    // of strings does, and names nothing
    fn array_callable(&mut self, array: Node<'_>) {
        let mut elements = named_child_nodes(array);
        let (Some(target), Some(method), None) = (
            elements.next().and_then(sole_value),
            elements.next().and_then(sole_value),
            elements.next(),
        ) else {
            return;
        };
        let Some(name) = string_text(method, self.source).filter(|name| is_identifier(name)) else {
            return;
        };
        let receiver = match target.kind() {
            "variable_name"
                if variable_identifier(target, self.source).as_deref() == Some("this") =>
            {
                Receiver::Own
            }
            "class_constant_access_expression" => match class_constant_scope(target, self.source) {
                Some(scope) if matches!(scope.as_str(), "self" | "static" | "parent") => {
                    Receiver::Own
                }
                Some(scope) => Receiver::Path(scope),
                None => return,
            },
            _ => return,
        };
        self.value_use(name.to_owned(), receiver, method);
    }

    // constraint: a use in a class body outside every method, a property's default, is made by the class, whose own
    // methods it names through the class, as a use at file scope keeps only its name
    fn value_use(&mut self, name: String, receiver: Receiver, node: Node<'_>) {
        let class = match (self.scopes.last(), self.types.last()) {
            (Some(function), Some(class)) => (class.end < function.end).then_some(class.id),
            (None, Some(class)) => Some(class.id),
            (_, None) => None,
        };
        let (site_of, receiver) = match class {
            Some(class) if receiver == Receiver::Own => {
                let class_name = self.raw.symbols[class.0 as usize].name.clone();
                (class, Receiver::Path(class_name))
            }
            Some(class) => (class, receiver),
            None => (
                self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id),
                receiver,
            ),
        };
        self.raw.uses.push(UseRef {
            site_of,
            name,
            receiver,
            access: Access::Read,
            line: line(node.start_position().row),
        });
    }

    fn push(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.flags = self.flags;
        self.raw.push_symbol(symbol)
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

    // constraint: PHP compiles a class name written from the root as it stands, a `namespace\X` under the namespace
    // alone, and any other name through the file's `use` map by its first segment, in any case, else under the
    // namespace (`zend_resolve_class_name`, `Zend/zend_compile.c`)
    fn class_name(&self, name: Node<'_>) -> String {
        let written = text(name, self.source);
        if name.kind() == "relative_name" {
            let local = written.split_once('\\').map_or("", |(_, local)| local);
            return self.qualify(local);
        }
        if let Some(rooted) = written.strip_prefix('\\') {
            return rooted.to_owned();
        }
        let (first, rest) = written
            .split_once('\\')
            .map_or((written.as_str(), None), |(first, rest)| {
                (first, Some(rest))
            });
        match (self.import(first.as_bytes()).map(imported_class), rest) {
            (Some(class), Some(rest)) => format!("{class}\\{rest}"),
            (Some(class), None) => class.to_owned(),
            (None, _) => self.qualify(&written),
        }
    }

    fn import(&self, alias: &[u8]) -> Option<&RawDirective> {
        let import = (self.imports.iter())
            .find(|import| self.source[import.alias.clone()].eq_ignore_ascii_case(alias))?;
        Some(&self.raw.directives[import.directive])
    }

    // constraint: a call on a name a `use ... as` binds in this namespace names the class from the root, which the
    // graph reads exactly, since an outside class's last name or its alias can be a class of the repository
    fn aliased_class(&self, call: Node<'_>) -> Option<String> {
        let scope = (call.child_by_field_name("scope")).filter(|scope| scope.kind() == "name")?;
        let import = self.import(&self.source[scope.byte_range()])?;
        (!import.names.is_empty()).then(|| format!("\\{}", imported_class(import)))
    }

    // constraint: a class takes its `extends` and `implements` names as parents, and an interface its `extends` names,
    // which the grammar writes in the same clause
    fn parents(&self, node: Node<'_>) -> Vec<String> {
        child_nodes(node)
            .filter(|clause| matches!(clause.kind(), "base_clause" | "class_interface_clause"))
            .flat_map(|clause| child_nodes(clause).filter(|name| NAMES.contains(&name.kind())))
            .map(|name| self.class_name(name))
            .collect()
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

    // constraint: a method of a type without a name, which only a declaration the grammar cannot read leaves, is
    // qualified by its name alone
    fn method(&mut self, node: Node<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, self.source);
        let owner = self.enclosing_type(node, Callables::Detach);
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
        symbol.parents = self.parents(node);
        symbol.exported = true;
        let id = self.push(symbol);
        self.types.push(Scope {
            end: node.end_byte(),
            id,
        });
    }

    // constraint: an anonymous class is a class the walk names, by the callable that creates it and by the variable
    // or the property it is assigned to, `{class}` where no name leads to it; a call on a variable that holds it has
    // that name as its receiver, which another file's class of the name would answer for a member this class inherits,
    // so the variable alone is not the name; one file's callables often create two of one name, so a name the file
    // has given already takes a count, `{make·2}`
    fn anonymous_class(&mut self, creation: Node<'_>, class: Node<'_>) -> String {
        let mut leading = String::new();
        if let Some(callable) = self.scopes.last() {
            leading.push_str(&self.raw.symbols[callable.id.0 as usize].name);
            leading.push_str(JOIN);
        }
        let target = assignment_target(creation);
        if let Some(name) = target.and_then(|target| target_name(target, self.source)) {
            leading.push_str(&name);
            leading.push_str(JOIN);
        }
        let leading = leading.strip_suffix(JOIN).unwrap_or("class");
        let name = if let Some(count) = self.anonymous_counts.get_mut(leading) {
            *count += 1;
            format!("{{{leading}{JOIN}{count}}}")
        } else {
            self.anonymous_counts.insert(leading.to_owned(), 1);
            format!("{{{leading}}}")
        };

        let mut symbol = declared(name.clone(), self.qualify(&name), SymbolKind::Class, class);
        symbol.parents = self.parents(class);
        let id = self.push(symbol);
        self.types.push(Scope {
            end: class.end_byte(),
            id,
        });
        if let Some(assignment) = target.and_then(|target| target.parent()) {
            self.hold(assignment, name.clone());
        }
        name
    }

    fn hold_created(&mut self, assignment: Node<'_>) {
        let created = (assignment.child_by_field_name("right"))
            .filter(|right| right.kind() == "object_creation_expression")
            .and_then(|new| named_child_nodes(new).find(|child| NAMES.contains(&child.kind())));
        if let Some(created) = created {
            let class = self.receiver_class(created);
            self.hold(assignment, class);
        }
    }

    fn hold(&mut self, assignment: Node<'_>, class: String) {
        let Some(target) = assignment
            .child_by_field_name("left")
            .filter(|left| left.kind() == "variable_name")
        else {
            return;
        };
        self.holders.push(Holder {
            variable: target.byte_range(),
            class,
            from: assignment.end_byte(),
            until: branch_end(assignment),
        });
    }

    fn open_frame(&mut self, function: Node<'_>) {
        self.frames.push(Frame {
            end: function.end_byte(),
            holders: self.holders.len(),
        });
    }

    fn frame_holders(&mut self) -> &mut [Holder] {
        let frame = self.frames.last().map_or(0, |frame| frame.holders);
        &mut self.holders[frame..]
    }

    // constraint: a closure's `use ($v)` copies the value as the closure is created, and a `use (&$v)` shares the
    // variable, which the closure can then write whenever it runs (`zend_compile_closure_uses`, `Zend/zend_compile.c`;
    // php-langspec `spec/10-expressions.md`, Anonymous Function Creation)
    fn closure(&mut self, closure: Node<'_>) {
        let start = closure.start_byte();
        let mut bound = Vec::new();
        let clause = named_child_nodes(closure)
            .find(|child| child.kind() == "anonymous_function_use_clause");
        for used in clause.iter().flat_map(|clause| named_child_nodes(*clause)) {
            if used.kind() == "by_ref" {
                self.forget_written(used, start);
            } else if let Some(class) = self.held_class(&self.source[used.byte_range()], start) {
                let class = class.to_owned();
                bound.push(whole_frame(used.byte_range(), class));
            }
        }
        self.open_frame(closure);
        self.holders.append(&mut bound);
    }

    // constraint: an arrow function copies each variable of the function around it as it is created, but for its
    // parameters (`find_implicit_binds`, `Zend/zend_compile.c`)
    fn arrow_function(&mut self, arrow: Node<'_>) {
        let start = arrow.start_byte();
        let mut bound: Vec<Holder> = (self.frame_holders().iter())
            .filter(|holder| holder.from <= start && start < holder.until)
            .map(|holder| whole_frame(holder.variable.clone(), holder.class.clone()))
            .collect();
        self.open_frame(arrow);
        self.holders.append(&mut bound);
        let parameters = arrow.child_by_field_name("parameters");
        for parameter in parameters.iter().flat_map(|list| named_child_nodes(*list)) {
            if let Some(name) = parameter.child_by_field_name("name") {
                self.forget_written(name, start);
            }
        }
    }

    // constraint: a variable an assignment writes keeps its value through the right side, a variable `=&` binds shares
    // its value with the other side, and `foreach` writes its value and key, which the grammar places after the subject
    fn forget_writes(&mut self, node: Node<'_>) {
        let end = node.end_byte();
        match node.kind() {
            "foreach_statement" => {
                if let Some(value) = named_child_nodes(node).nth(1) {
                    self.forget_written(value, value.end_byte());
                }
            }
            "catch_clause" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.forget_written(name, name.end_byte());
                }
            }
            "unset_statement" => {
                for variable in named_child_nodes(node) {
                    self.forget_written(variable, end);
                }
            }
            kind => {
                if let Some(target) = node.child_by_field_name("left") {
                    self.forget_written(target, end);
                }
                if let Some(aliased) = node
                    .child_by_field_name("right")
                    .filter(|_| kind == "reference_assignment_expression")
                {
                    self.forget_written(aliased, end);
                }
            }
        }
    }

    // constraint: a write to a variable ends what it held, and only a write to the variable itself does: `$v->p = 1`
    // and `$v['k'] = 1` leave it holding the object; a list assigns each variable it names, under its key or by
    // reference
    fn forget_written(&mut self, written: Node<'_>, until: usize) {
        match written.kind() {
            "variable_name" => self.forget(written, until),
            "list_literal" | "pair" | "by_ref" => {
                for element in named_child_nodes(written) {
                    self.forget_written(element, until);
                }
            }
            _ => {}
        }
    }

    fn forget(&mut self, variable: Node<'_>, until: usize) {
        let source = self.source;
        let variable = &source[variable.byte_range()];
        for holder in self.frame_holders() {
            if source[holder.variable.clone()] == *variable {
                holder.until = holder.until.min(until);
            }
        }
    }

    // constraint: the class a variable of the frame holds at a byte of the source; the holder's own span settles which,
    // as a write ends one where the next starts
    fn held_class(&self, variable: &[u8], at: usize) -> Option<&str> {
        let frame = self.frames.last().map_or(0, |frame| frame.holders);
        let holder = self.holders[frame..].iter().rfind(|holder| {
            holder.from <= at
                && at < holder.until
                && self.source[holder.variable.clone()] == *variable
        })?;
        Some(&holder.class)
    }

    // constraint: a call on a variable that holds a class is a call on that class, so its receiver is the class's name,
    // as a static call's is
    fn held_receiver(&self, receiver: Option<String>, at: usize) -> Option<String> {
        let held = receiver
            .as_deref()
            .and_then(|receiver| self.held_class(receiver.as_bytes(), at));
        held.map(str::to_owned).or(receiver)
    }

    // constraint: `private int $x, $y;` declares one member per element, each with the declaration's visibility
    fn members(&mut self, node: Node<'_>, element_kind: &str, kind: SymbolKind) {
        let member = Member {
            owner: self.enclosing_type(node, Callables::Detach),
            kind,
            exported: !is_private(node, self.source),
        };

        for element in named_child_nodes(node).filter(|element| element.kind() == element_kind) {
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
            owner: self.enclosing_type(node, Callables::Cross),
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
            owner: self.enclosing_type(node, Callables::Detach),
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

    fn trait_use(&mut self, node: Node<'_>) {
        let Some(&open) = self.types.last() else {
            return;
        };
        let names: Vec<String> = named_child_nodes(node)
            .filter(|child| NAMES.contains(&child.kind()))
            .map(|name| self.class_name(name))
            .collect();
        let Some(definition) = usize::try_from(open.id.0)
            .ok()
            .and_then(|at| self.raw.symbols.get_mut(at))
        else {
            return;
        };

        for name in names {
            if !definition.parents.contains(&name) {
                definition.parents.push(name);
            }
        }
    }

    // constraint: a grouped `use A\{B, C\D}` writes the shared prefix once, as the declaration's `namespace_name`
    fn namespace_use(&mut self, node: Node<'_>) {
        for child in named_child_nodes(node) {
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
        for clause in
            named_child_nodes(group).filter(|clause| clause.kind() == "namespace_use_clause")
        {
            self.use_clause(declaration, clause, &prefix);
        }
    }

    // constraint: `use function` and `use const` write their kind once for the whole declaration, or on each clause
    // of a mixed group, `use A\{B, function c}`
    fn use_clause(&mut self, declaration: Node<'_>, clause: Node<'_>, prefix: &str) {
        let Some(name) = named_child_nodes(clause).find(|child| NAMES.contains(&child.kind()))
        else {
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
        let written_alias = clause.child_by_field_name("alias");
        let alias = written_alias
            .or_else(|| named_child_nodes(name).rfind(|segment| segment.kind() == "name"))
            .unwrap_or(name)
            .byte_range();
        let name = text(name, self.source);
        let name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}\\{name}")
        };
        if name.is_empty() {
            return;
        }

        let mut directive = RawDirective::new(form, name, line(declaration.start_position().row));
        if form == USE_FORM {
            self.imports.push(Import {
                alias,
                directive: self.raw.directives.len(),
            });
            directive
                .names
                .extend(written_alias.map(|alias| text(alias, self.source)));
        }
        self.raw.directives.push(directive);
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
        if let Some(class) = named_child_nodes(node).find(|child| child.kind() == "anonymous_class")
        {
            let name = self.anonymous_class(node, class);
            self.call(name, None, node);
            return;
        }
        let Some(created) = named_child_nodes(node).find(|child| NAMES.contains(&child.kind()))
        else {
            return;
        };
        if scope_keyword(&self.source[created.byte_range()]).is_some() {
            return;
        }
        let class = if created.kind() == "name" {
            Some(text(created, self.source))
        } else {
            last_segment(created, self.source)
        };
        if let Some(class) = class {
            self.call(class, None, node);
        }
    }

    // constraint: `new self` and `new parent` create a class of the class around them and `new static` one of the class
    // called, which a receiver of the name reads alike; any other name is the class PHP compiles it to
    fn receiver_class(&self, created: Node<'_>) -> String {
        scope_keyword(&self.source[created.byte_range()])
            .map_or_else(|| format!("\\{}", self.class_name(created)), str::to_owned)
    }

    // constraint: a member's type is the class, interface, trait or enum around it, or the anonymous class, which
    // goes by the name the walk gave it as it opened; a function or closure between the member and the type detaches
    // it unless `callables` says to cross it
    fn enclosing_type(&self, node: Node<'_>, callables: Callables) -> Option<String> {
        let mut ancestor = node.parent();
        while let Some(current) = ancestor {
            match current.kind() {
                "class_declaration"
                | "interface_declaration"
                | "trait_declaration"
                | "enum_declaration" => {
                    return current
                        .child_by_field_name("name")
                        .map(|name| text(name, self.source))
                        .filter(|name| !name.is_empty());
                }
                "anonymous_class" => {
                    return self
                        .types
                        .iter()
                        .rfind(|open| open.end == current.end_byte())
                        .map(|open| self.raw.symbols[open.id.0 as usize].name.clone());
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

    fn call(&mut self, callee: String, receiver: Option<String>, node: Node<'_>) {
        if callee.is_empty() {
            return;
        }

        self.raw.calls.push(CallRef {
            site_of: self.scopes.last().map_or(FILE_SCOPE, |scope| scope.id),
            callee,
            receiver,
            line: line(node.start_position().row),
            invokes_a_macro: false,
            calls_a_method: false,
            arguments: 0,
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
// of a function or of a scope, reduces to its last segment, the bare name that owns a member. PHP reads a keyword in
// any case, and the graph's lists hold it in lower case.
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
            "relative_scope" => {
                let mut keyword = text(receiver, source);
                keyword.make_ascii_lowercase();
                keyword
            }
            _ => text(receiver, source),
        });

    (text(name, source), receiver)
}

// constraint: a variable a closure binds holds its class through the closure, whose frame ends it
const fn whole_frame(variable: Range<usize>, class: String) -> Holder {
    Holder {
        variable,
        class,
        from: 0,
        until: usize::MAX,
    }
}

// constraint: each child of these nodes may run or not, whichever way the walk reads it, so an assignment under one
// holds to the end of that child; a function's body runs apart from the code around it
fn branch_end(assignment: Node<'_>) -> usize {
    let mut child = assignment;
    while let Some(parent) = child.parent() {
        match parent.kind() {
            "if_statement"
            | "while_statement"
            | "do_statement"
            | "for_statement"
            | "foreach_statement"
            | "switch_block"
            | "try_statement"
            | "match_block"
            | "conditional_expression"
            | "binary_expression" => return child.end_byte(),
            "function_definition"
            | "method_declaration"
            | "anonymous_function"
            | "arrow_function" => break,
            _ => child = parent,
        }
    }
    usize::MAX
}

fn assignment_target(value: Node<'_>) -> Option<Node<'_>> {
    value
        .parent()
        .filter(|parent| parent.kind() == "assignment_expression")
        .filter(|assignment| assignment.child_by_field_name("right") == Some(value))?
        .child_by_field_name("left")
}

// constraint: a value is assigned to a name where the assignment's target is a variable, `$job`, or a property,
// `$this->job`
fn target_name(target: Node<'_>, source: &[u8]) -> Option<String> {
    match target.kind() {
        "variable_name" => variable_identifier(target, source),
        "member_access_expression" => target
            .child_by_field_name("name")
            .filter(|name| name.kind() == "name")
            .map(|name| text(name, source)),
        _ => None,
    }
}

// constraint: `private` is the one modifier that hides a member; `protected`, `public`, `var` and none export it
fn is_private(node: Node<'_>, source: &[u8]) -> bool {
    child_nodes(node).any(|child| {
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

// constraint: the builtins and the WordPress hook functions that take a callable, with the positional argument that
// holds it; an existence test, `function_exists` or `is_callable`, uses nothing
fn callable_position(callee: &str) -> Option<usize> {
    match callee {
        "array_map"
        | "call_user_func"
        | "call_user_func_array"
        | "forward_static_call"
        | "forward_static_call_array"
        | "ob_start"
        | "register_shutdown_function"
        | "set_error_handler"
        | "set_exception_handler"
        | "spl_autoload_register" => Some(0),
        "add_action"
        | "add_filter"
        | "add_shortcode"
        | "array_filter"
        | "array_reduce"
        | "array_walk"
        | "array_walk_recursive"
        | "iterator_apply"
        | "preg_replace_callback"
        | "register_activation_hook"
        | "register_deactivation_hook"
        | "remove_action"
        | "remove_filter"
        | "uasort"
        | "uksort"
        | "usort" => Some(1),
        _ => None,
    }
}

// constraint: a literal without interpolation, quoted either way
fn string_text<'s>(node: Node<'_>, source: &'s [u8]) -> Option<&'s str> {
    if !matches!(node.kind(), "string" | "encapsed_string") {
        return None;
    }
    let mut parts = named_child_nodes(node);
    let (Some(content), None) = (parts.next(), parts.next()) else {
        return None;
    };
    (content.kind() == "string_content")
        .then(|| std::str::from_utf8(&source[content.byte_range()]).ok())
        .flatten()
}

fn imported_class(import: &RawDirective) -> &str {
    import.spec.strip_prefix('\\').unwrap_or(&import.spec)
}

fn last_name(name: &str) -> &str {
    name.rsplit_once('\\').map_or(name, |(_, last)| last)
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|rest| rest == '_' || rest.is_ascii_alphanumeric())
}

// constraint: an array element with no key, no `&` and no spread is the value it holds
fn sole_value(element: Node<'_>) -> Option<Node<'_>> {
    if element.kind() != "array_element_initializer" {
        return None;
    }
    let mut children = named_child_nodes(element);
    let (Some(value), None) = (children.next(), children.next()) else {
        return None;
    };
    (!matches!(value.kind(), "by_ref" | "variadic_unpacking")).then_some(value)
}

// constraint: PHP reads `self`, `static` and `parent` in any case (`zend_get_class_fetch_type`, `Zend/zend_compile.c`)
fn scope_keyword(written: &[u8]) -> Option<&'static str> {
    ["self", "static", "parent"]
        .into_iter()
        .find(|keyword| written.eq_ignore_ascii_case(keyword.as_bytes()))
}

// constraint: `X::class` names the class `X`, the last segment of a qualified name, or a relative scope, `self`,
// `static` or `parent`
fn class_constant_scope(access: Node<'_>, source: &[u8]) -> Option<String> {
    let mut children = named_child_nodes(access);
    let (Some(scope), Some(constant), None) = (children.next(), children.next(), children.next())
    else {
        return None;
    };
    if &source[constant.byte_range()] != b"class" {
        return None;
    }
    match scope.kind() {
        "name" | "relative_scope" => Some(text(scope, source)),
        "qualified_name" | "relative_name" => last_segment(scope, source),
        _ => None,
    }
}

fn last_segment(name: Node<'_>, source: &[u8]) -> Option<String> {
    named_child_nodes(name)
        .rfind(|segment| segment.kind() == "name")
        .map(|segment| text(segment, source))
}

fn first_named_text(node: Node<'_>, kind: &str, source: &[u8]) -> Option<String> {
    named_child_nodes(node)
        .find(|child| child.kind() == kind)
        .map(|child| text(child, source))
}
