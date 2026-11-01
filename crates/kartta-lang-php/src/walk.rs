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

pub(crate) const CONSTRUCTOR: &str = "__construct";

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
        fields: HashMap::new(),
        field_calls: Vec::new(),
        declared_fields: Vec::new(),
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

    emitter.name_field_receivers();
    emitter.name_field_types();
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

type Field<'s> = (SymbolId, &'s [u8]);

enum FieldClass {
    Settled(Option<String>),
    Written(String),
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
    fields: HashMap<Field<'s>, FieldClass>,
    field_calls: Vec<(usize, Field<'s>)>,
    declared_fields: Vec<(SymbolId, SymbolId)>,
    // constraint: every `namespace` declaration replaces the namespace and `namespace { }` clears it; PHP admits no
    // declaration outside the braces of a file that uses the braced form, so a namespace needs no end
    namespace: String,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl<'s> Emitter<'s> {
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
                self.hold_parameters(node);
                self.function(node);
            }
            "method_declaration" => {
                self.open_frame(node);
                self.hold_parameters(node);
                self.method(node);
            }
            "anonymous_function" => self.closure(node),
            "arrow_function" => self.arrow_function(node),
            "class_declaration" => self.type_declaration(node, SymbolKind::Class),
            "interface_declaration" => self.type_declaration(node, SymbolKind::Interface),
            "trait_declaration" => self.type_declaration(node, SymbolKind::Trait),
            "enum_declaration" => self.type_declaration(node, SymbolKind::Enum),
            "property_declaration" => {
                self.declare_fields(node);
                self.members(node, "property_element", SymbolKind::Field);
            }
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
            "binary_expression" => self.narrow(node),
            "function_call_expression" => {
                let (callee, receiver) = call_target(node, self.source);
                self.callable_argument(&callee, node);
                self.call(callee, receiver, node);
            }
            "member_call_expression" | "nullsafe_member_call_expression" => {
                let (callee, receiver) = call_target(node, self.source);
                let receiver = self.called_class(node).or(receiver);
                let receiver = self.field_call(&callee, node).or(receiver);
                self.call(callee, receiver, node);
            }
            "scoped_call_expression" => {
                let (callee, receiver) = call_target(node, self.source);
                let receiver = self.class_from_the_root(node).or(receiver);
                let receiver = self.called_class(node).or(receiver);
                let receiver = self.field_call(&callee, node).or(receiver);
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
            "class_constant_access_expression" => match self.class_constant_scope(target) {
                Some(scope) if scope_keyword(scope.as_bytes()).is_some() => Receiver::Own,
                Some(scope) => Receiver::Path(scope),
                None => return,
            },
            _ => return,
        };
        self.value_use(name.to_owned(), receiver, method);
    }

    // constraint: `X::class` names the class `X`, the last name of the class an alias imports, which owns a member as
    // `X` does, the last segment of a qualified name, or a relative scope, `self`, `static` or `parent`
    fn class_constant_scope(&self, access: Node<'_>) -> Option<String> {
        let mut children = named_child_nodes(access);
        let (Some(scope), Some(constant), None) =
            (children.next(), children.next(), children.next())
        else {
            return None;
        };
        if &self.source[constant.byte_range()] != b"class" {
            return None;
        }
        match scope.kind() {
            "name" => Some(self.aliased_class(scope).map_or_else(
                || text(scope, self.source),
                |class| last_name(&class).to_owned(),
            )),
            "relative_scope" => Some(text(scope, self.source)),
            "qualified_name" | "relative_name" => last_segment(scope, self.source),
            _ => None,
        }
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

    // constraint: a name a `use ... as` binds in this namespace names the class from the root, which the graph reads
    // exactly, since an outside class's last name or its alias can be a class of the repository
    fn aliased_class(&self, name: Node<'_>) -> Option<String> {
        if name.kind() != "name" {
            return None;
        }
        let import = self.import(&self.source[name.byte_range()])?;
        (!import.names.is_empty()).then(|| format!("\\{}", imported_class(import)))
    }

    // constraint: a qualified scope names the class from the root as an alias does, so a namesake of its last name
    // never answers it; a method of the class's own last name goes by that last name, because the graph reads a call of
    // that name on the class from the root as a `new`
    fn class_from_the_root(&self, call: Node<'_>) -> Option<String> {
        let scope = call.child_by_field_name("scope")?;
        let class = match scope.kind() {
            "qualified_name" | "relative_name" => format!("\\{}", self.class_name(scope)),
            _ => self.aliased_class(scope)?,
        };
        let method = call.child_by_field_name("name")?;
        let last = last_name(&class);
        Some(if last.as_bytes() == &self.source[method.byte_range()] {
            last.to_owned()
        } else {
            class
        })
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
        let target = assignment_target(creation, self.source);
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
        if let Some(field) = target.and_then(|target| self.own_field(target)) {
            self.write_field(field, Some(name.clone()));
        }
        self.types.push(Scope {
            end: class.end_byte(),
            id,
        });
        let assignment = target.and_then(|target| target.parent());
        if let Some(assignment) = assignment.filter(|write| write.kind() == "assignment_expression")
        {
            self.hold(assignment, name.clone());
        }
        name
    }

    fn hold_created(&mut self, assignment: Node<'_>) {
        let created = (assignment.child_by_field_name("right"))
            .map(assigned_value)
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

    fn narrow(&mut self, test: Node<'_>) {
        if !operator_is(test, b"instanceof", self.source) {
            return;
        }
        let subject = (test.child_by_field_name("left"))
            .and_then(|left| self.narrowed_subject(inside_parentheses(left)));
        let class =
            (test.child_by_field_name("right")).filter(|right| NAMES.contains(&right.kind()));
        let (Some(subject), Some(class)) = (subject, class) else {
            return;
        };
        let class = self.receiver_class(class);
        for span in narrowed_spans(test, self.source) {
            self.holders.push(Holder {
                variable: subject.clone(),
                class: class.clone(),
                from: span.start,
                until: span.end,
            });
        }
    }

    // constraint: a property's holder is its name, which no variable's spelling equals, so `$this->c` and `$this?->c`
    // read as one; `$this` stays the class around the call, whose own members a test's interface does not hold
    fn narrowed_subject(&self, subject: Node<'_>) -> Option<Range<usize>> {
        if subject.kind() != "variable_name" {
            self.own_field(subject)?;
            let name = subject.child_by_field_name("name")?.byte_range();
            return Some(name.start + usize::from(self.source[name.start] == b'$')..name.end);
        }
        (self.source[subject.byte_range()] != *b"$this").then(|| subject.byte_range())
    }

    fn open_frame(&mut self, function: Node<'_>) {
        self.frames.push(Frame {
            end: function.end_byte(),
            holders: self.holders.len(),
        });
    }

    // constraint: a function checks each argument against its type on entry, through a reference too, and a variadic
    // parameter's type bounds each element of its array (`ZEND_RECV`, `zend_check_type`, `Zend/zend_execute.c`)
    fn hold_parameters(&mut self, function: Node<'_>) {
        let parameters = function.child_by_field_name("parameters");
        for parameter in parameters.iter().flat_map(|list| named_child_nodes(*list)) {
            let class = parameter
                .child_by_field_name("type")
                .filter(|_| parameter.kind() != "variadic_parameter")
                .and_then(|declared| self.declared_class(declared));
            if let (Some(variable), Some(class)) = (parameter_variable(parameter), class) {
                self.holders.push(whole_frame(variable.byte_range(), class));
            }
        }
    }

    // constraint: a type settles a class where one name is all a call can run on: a member call on null never runs, and
    // a value of a union can be any member's (`zend_check_type_slow`, `Zend/zend_execute.c`), so the class is the one
    // name, spelled as after `new` (`zend_compile_single_typename`, `Zend/zend_compile.c`)
    fn declared_class(&self, declared: Node<'_>) -> Option<String> {
        match declared.kind() {
            "named_type" => declared
                .named_child(0)
                .map(|name| self.receiver_class(name)),
            "optional_type" => self.declared_class(declared.named_child(0)?),
            "union_type" => {
                let mut members = named_child_nodes(declared)
                    .filter(|member| self.source[member.byte_range()] != *b"null");
                let (Some(class), None) = (members.next(), members.next()) else {
                    return None;
                };
                self.declared_class(class)
            }
            _ => None,
        }
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
        self.hold_parameters(closure);
    }

    // constraint: an arrow function copies each variable of the function around it as it is created, but for its
    // parameters (`find_implicit_binds`, `Zend/zend_compile.c`)
    fn arrow_function(&mut self, arrow: Node<'_>) {
        let start = arrow.start_byte();
        let source = self.source;
        let mut bound: Vec<Holder> = (self.frame_holders().iter())
            .filter(|holder| holder.from <= start && start < holder.until)
            .filter(|holder| source[holder.variable.start] == b'$')
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
        self.hold_parameters(arrow);
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
            "variable_name" => self.forget(&self.source[written.byte_range()], until),
            "member_access_expression" | "scoped_property_access_expression" => {
                self.field_written(written);
                if let Some((_, name)) = self.own_field(written) {
                    self.forget(name, until);
                }
            }
            "list_literal" | "pair" | "by_ref" => {
                for element in named_child_nodes(written) {
                    self.forget_written(element, until);
                }
            }
            _ => {}
        }
    }

    fn forget(&mut self, variable: &[u8], until: usize) {
        let source = self.source;
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

    // constraint: a call on a `new` or on a variable that holds a class is a call on that class, so its receiver is the
    // class's name, as a static call's is; `::` on an object fetches the object's class (`ZEND_FETCH_CLASS`,
    // `Zend/zend_vm_def.h`)
    fn called_class(&self, call: Node<'_>) -> Option<String> {
        self.assigned_class(called_value(call)?)
    }

    // constraint: a name in braces is an expression (`simple_variable`, `Zend/zend_language_parser.y`), so it is no
    // property: `self::${X}` holds a constant in a `dynamic_variable_name`, and `in_braces` reads `$this->{X}`
    fn own_field(&self, access: Node<'_>) -> Option<Field<'s>> {
        let open = *self.types.last()?;
        let name = match access.kind() {
            "member_access_expression" | "nullsafe_member_access_expression" => {
                let object = access.child_by_field_name("object")?;
                if &self.source[object.byte_range()] != b"$this" {
                    return None;
                }
                access
                    .child_by_field_name("name")
                    .filter(|name| name.kind() == "name" && !in_braces(access))?
            }
            "scoped_property_access_expression" => {
                if !self.is_own_class(access.child_by_field_name("scope")?, open) {
                    return None;
                }
                let variable = access
                    .child_by_field_name("name")
                    .filter(|variable| variable.kind() == "variable_name")?;
                named_child_nodes(variable).find(|child| child.kind() == "name")?
            }
            _ => return None,
        };
        Some((open.id, &self.source[name.byte_range()]))
    }

    // constraint: `self`, `static` and the class's own name fetch a static property through the class's own table
    // (`zend_std_get_static_property_with_info`, `Zend/zend_object_handlers.c`); `parent` skips a redeclaration the
    // class's body makes, so its property is not the one the body declares
    fn is_own_class(&self, scope: Node<'_>, open: Scope) -> bool {
        match scope.kind() {
            "relative_scope" => !self.source[scope.byte_range()].eq_ignore_ascii_case(b"parent"),
            "name" | "qualified_name" | "relative_name" => self
                .class_name(scope)
                .eq_ignore_ascii_case(&self.raw.symbols[open.id.0 as usize].qualified_name),
            _ => false,
        }
    }

    // constraint: a test's class stands ahead of the property's own, whichever the file settles once it
    fn field_call(&mut self, callee: &str, call: Node<'_>) -> Option<String> {
        let field = self.own_field(called_value(call)?)?;
        if let Some(class) = self.held_class(field.1, call.start_byte()) {
            return Some(class.to_owned());
        }
        if !callee.is_empty() {
            self.field_calls.push((self.raw.calls.len(), field));
        }
        None
    }

    // constraint: a method reads a property that a later member declares or writes as often as an earlier one, so the
    // receivers are read once the whole file has been walked
    fn name_field_receivers(&mut self) {
        for (call, field) in &self.field_calls {
            let held = match self.fields.get(field) {
                Some(FieldClass::Settled(Some(class)) | FieldClass::Written(class)) => {
                    class.clone()
                }
                None if !self.raw.symbols[field.0.0 as usize].parents.is_empty() => {
                    format!("->{}", String::from_utf8_lossy(field.1))
                }
                _ => continue,
            };
            self.raw.calls[*call].receiver = Some(held);
        }
    }

    // constraint: a child's call reads the class from another file, so `self` and `parent` become the classes they name
    // where the property is declared; `self` in a trait is each using class, and an anonymous class is named in its file
    // alone; `new static` makes the class of the `$this` it runs on, which a read on `$this` calls too, so it stays
    fn name_field_types(&mut self) {
        for &(owner, field) in &self.declared_fields {
            let symbol = &self.raw.symbols[field.0 as usize];
            let (Some(FieldClass::Settled(Some(class)) | FieldClass::Written(class)), declaring) = (
                self.fields.get(&(owner, symbol.name.as_bytes())),
                &self.raw.symbols[owner.0 as usize],
            ) else {
                continue;
            };
            let rooted = match class.as_str() {
                kept if kept.starts_with('\\') || kept == "static" => kept.to_owned(),
                _ if declaring.kind == SymbolKind::Trait => continue,
                "self" => format!("\\{}", declaring.qualified_name),
                "parent" => match declaring.parents.first() {
                    Some(parent) => format!("\\{parent}"),
                    None => continue,
                },
                _ => continue,
            };
            self.raw.symbols[field.0 as usize].value_types = vec![rooted];
        }
    }

    fn declare_fields(&mut self, declaration: Node<'_>) {
        let elements =
            named_child_nodes(declaration).filter(|element| element.kind() == "property_element");
        for element in elements {
            let valued = element
                .child_by_field_name("default_value")
                .is_some_and(|value| value.kind() != "null");
            if let Some(variable) = element.child_by_field_name("name") {
                self.declare_field(declaration, variable, valued);
            }
        }
    }

    // constraint: a declared type settles a property whatever a write says, since PHP checks every write against it
    fn declare_field(&mut self, declaration: Node<'_>, variable: Node<'_>, valued: bool) {
        let name = named_child_nodes(variable).find(|child| child.kind() == "name");
        let (Some(name), Some(open)) = (name, self.types.last()) else {
            return;
        };
        let field = (open.id, &self.source[name.byte_range()]);
        if let Some(declared) = declaration.child_by_field_name("type") {
            let class = self.property_class(declared);
            self.fields.insert(field, FieldClass::Settled(class));
        } else if valued {
            self.write_field(field, None);
        }
    }

    // constraint: a call on anything but an object throws (`zend_invalid_method_call`, `Zend/zend_execute.c`), so a
    // type's class is its one alternative that names a class; `iterable` compiles to `Traversable|array`
    // (`zend_compile_single_typename`, `Zend/zend_compile.c`), so it makes a second beside a class and alone gives none
    fn property_class(&self, declared: Node<'_>) -> Option<String> {
        match declared.kind() {
            "named_type" => declared
                .named_child(0)
                .map(|name| self.receiver_class(name)),
            "optional_type" => self.property_class(declared.named_child(0)?),
            "union_type" => {
                let mut classes = named_child_nodes(declared)
                    .filter(|alternative| names_a_class(*alternative, self.source));
                let (Some(class), None) = (classes.next(), classes.next()) else {
                    return None;
                };
                self.property_class(class)
            }
            _ => None,
        }
    }

    // constraint: every write the class's own body makes reads as one class or none, whichever order the walk meets
    // them in, so a second class settles the property on none; a declaration replaces what the writes before it said
    fn write_field(&mut self, field: Field<'s>, class: Option<String>) {
        let Some(held) = self.fields.get_mut(&field) else {
            let first = class.map_or(FieldClass::Settled(None), FieldClass::Written);
            self.fields.insert(field, first);
            return;
        };
        if matches!(held, FieldClass::Written(written) if class.as_ref() != Some(&*written)) {
            *held = FieldClass::Settled(None);
        }
    }

    // constraint: `null` and `unset` clear a property and say nothing of what it holds; `??=` writes its value over
    // `null` alone, and every other compound assignment stores its operator's result, a string from `.=`, where only a
    // class with a `do_operation` handler, which an extension alone sets, takes the rest (`zend_binop_error`,
    // `Zend/zend_operators.c`); an anonymous class has its name once the walk reaches it; any other write that names no
    // class, a list or `foreach` target or either side of `=&`, leaves the property holding what the file cannot name
    fn field_written(&mut self, written: Node<'_>) {
        let (Some(field), Some(write)) = (self.own_field(written), written.parent()) else {
            return;
        };
        let value = match write.kind() {
            "unset_statement" => return,
            _ if stores_its_right_side(write, self.source) => write.child_by_field_name("right"),
            _ => None,
        };
        if value.is_some_and(creates_an_anonymous_class) {
            return;
        }
        let value = value.map(assigned_value);
        if value.is_some_and(|value| value.kind() == "null") {
            return;
        }
        let class = value.and_then(|value| self.assigned_class(value));
        self.write_field(field, class);
    }

    fn assigned_class(&self, value: Node<'_>) -> Option<String> {
        match value.kind() {
            "object_creation_expression" => named_child_nodes(value)
                .find(|child| NAMES.contains(&child.kind()))
                .map(|created| self.receiver_class(created)),
            "variable_name" => {
                let variable = &self.source[value.byte_range()];
                self.held_class(variable, value.start_byte())
                    .map(str::to_owned)
            }
            _ => None,
        }
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
        if let Some(variable) = parameter_variable(node) {
            self.declare_field(node, variable, true);
        }
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
        let id = self.push(symbol);
        if let (SymbolKind::Field, Some(open)) = (member.kind, self.types.last()) {
            self.declared_fields.push((open.id, id));
        }
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
            .map(inside_parentheses)
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

    // constraint: `new self`, `new static` and `new parent` name no class; the grammar parses `static` as a plain name;
    // any other `new` names the class PHP compiles the name to and runs the constructor that class's table holds, or
    // none, so a construction is a call of each on the class from the root (`zend_compile_new`, `Zend/zend_compile.c`)
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
        let class = format!("\\{}", self.class_name(created));
        self.call(last_name(&class).to_owned(), Some(class.clone()), node);
        self.call(CONSTRUCTOR.to_owned(), Some(class), node);
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
// name, `self`, `parent`, `static` or a variable; a chained or computed one is the empty receiver, as a qualified scope
// is here, whose class the emitter reads through the file's imports. A qualified function name reduces to its last
// segment. PHP reads a keyword in any case, and the graph's lists hold it in lower case. A constant in braces is a
// name PHP computes, which the call records as it does `$this->$name()`, under a `$` no symbol's name holds. A name in
// parentheses is a constant's value (`constant`, `Zend/zend_language_parser.y`), so only a variable reads past them.
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
        "scoped_call_expression" => ("scope", &["name", "relative_scope", "variable_name"]),
        _ => return (String::new(), None),
    };

    let Some(name) = node.child_by_field_name("name") else {
        return (String::new(), None);
    };
    let receiver = node
        .child_by_field_name(receiver)
        .map(|receiver| match assigned_value(receiver) {
            variable if variable.kind() == "variable_name" => variable,
            _ => receiver,
        })
        .map(|receiver| match receiver.kind() {
            kind if !leaves.contains(&kind) => String::new(),
            "relative_scope" => {
                let mut keyword = text(receiver, source);
                keyword.make_ascii_lowercase();
                keyword
            }
            _ => text(receiver, source),
        });

    if name.kind() == "name" && in_braces(node) {
        return (format!("${}", text(name, source)), receiver);
    }
    (text(name, source), receiver)
}

// constraint: braces around a member's name hold an expression (`property_name` and `member_name`,
// `Zend/zend_language_parser.y`), and the grammar writes a constant there as the `name` a spelled member has; the
// member is the constant's value, which the walk does not hold. The braces are the access's own tokens
fn in_braces(access: Node<'_>) -> bool {
    child_nodes(access).any(|child| child.kind() == "{")
}

// constraint: a variable a function starts with, a parameter or what a closure binds, holds its class through the
// function, whose frame ends it
const fn whole_frame(variable: Range<usize>, class: String) -> Holder {
    Holder {
        variable,
        class,
        from: 0,
        until: usize::MAX,
    }
}

fn operator_is(expression: Node<'_>, operator: &[u8], source: &[u8]) -> bool {
    (expression.child_by_field_name("operator"))
        .is_some_and(|written| source[written.byte_range()].eq_ignore_ascii_case(operator))
}

// constraint: `instanceof` is true of an object of the class or below it alone, and false of a class not loaded
// (`ZEND_INSTANCEOF`, `Zend/zend_vm_def.h`), so its class holds on the paths only its truth reaches; `outcome` is the
// value of the expression walked up to from which the test's truth follows, and `!` turns it over
fn narrowed_spans(test: Node<'_>, source: &[u8]) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut outcome = true;
    let mut child = test;
    while let Some(parent) = child.parent() {
        match parent.kind() {
            "parenthesized_expression" => {}
            "unary_op_expression" if operator_is(parent, b"!", source) => outcome = !outcome,
            "binary_expression" if settles_both_operands(parent, outcome, source) => {
                spans.push(child.end_byte()..parent.end_byte());
            }
            "conditional_expression" if parent.child_by_field_name("condition") == Some(child) => {
                let path = if outcome { "body" } else { "alternative" };
                spans.extend(
                    parent
                        .child_by_field_name(path)
                        .map(|path| path.byte_range()),
                );
                break;
            }
            "if_statement" | "else_if_clause" => {
                if_spans(parent, outcome, source, &mut spans);
                break;
            }
            "while_statement" => {
                spans.extend(
                    (parent.child_by_field_name("body"))
                        .filter(|_| outcome)
                        .map(|body| body.byte_range()),
                );
                break;
            }
            "match_condition_list" => {
                spans.extend(match_arm_span(parent, outcome, source));
                break;
            }
            _ => break,
        }
        child = parent;
    }
    spans
}

// constraint: `&&` is true only with both operands true and `||` false only with both false, and each runs its right
// operand on that value of its left alone (`zend_compile_short_circuiting`, `Zend/zend_compile.c`)
fn settles_both_operands(binary: Node<'_>, outcome: bool, source: &[u8]) -> bool {
    let operators: [&[u8]; 2] = if outcome {
        [b"&&", b"and"]
    } else {
        [b"||", b"or"]
    };
    operators
        .iter()
        .any(|operator| operator_is(binary, operator, source))
}

// constraint: a body that leaves sends every path that goes on past the `if` through the other outcome, and an `if`'s
// first test alone is on every such path, which a later `elseif` is not
fn if_spans(branch: Node<'_>, outcome: bool, source: &[u8], spans: &mut Vec<Range<usize>>) {
    let (Some(condition), Some(body)) = (
        branch.child_by_field_name("condition"),
        branch.child_by_field_name("body"),
    ) else {
        return;
    };
    let first = branch.kind() == "if_statement";
    let Some(statement) = (if first { Some(branch) } else { branch.parent() }) else {
        return;
    };
    if !outcome {
        let until = if first && leaves(body, source) {
            branch_end(statement)
        } else {
            statement.end_byte()
        };
        spans.push(body.end_byte()..until);
        return;
    }
    spans.push(condition.end_byte()..body.end_byte());
    let sole_else = statement
        .child_by_field_name("alternative")
        .filter(|alternative| alternative.kind() == "else_clause");
    let else_body = sole_else.and_then(|sole_else| sole_else.child_by_field_name("body"));
    if else_body.is_some_and(|body| leaves(body, source)) {
        spans.push(statement.end_byte()..branch_end(statement));
    }
}

// constraint: control never reaches the end of a body whose last statement leaves it; PHP reads `die` as the token of
// `exit`, in any case (`Zend/zend_language_scanner.l`), and the grammar reads it as a name or a call of one
fn leaves(body: Node<'_>, source: &[u8]) -> bool {
    let last = match body.kind() {
        "compound_statement" | "colon_block" => {
            named_child_nodes(body).rfind(|statement| statement.kind() != "comment")
        }
        _ => Some(body),
    };
    last.is_some_and(|statement| match statement.kind() {
        "return_statement" | "break_statement" | "continue_statement" | "exit_statement" => true,
        "expression_statement" => statement.named_child(0).is_some_and(|expression| {
            let called = expression.child_by_field_name("function");
            expression.kind() == "throw_expression"
                || source[called.unwrap_or(expression).byte_range()].eq_ignore_ascii_case(b"die")
        }),
        _ => false,
    })
}

// constraint: `match (true)` runs the first arm whose condition is identical to `true`, so a true test settles the
// result of an arm it alone guards (`zend_compile_match`, `Zend/zend_compile.c`)
fn match_arm_span(conditions: Node<'_>, outcome: bool, source: &[u8]) -> Option<Range<usize>> {
    let arm = conditions.parent()?;
    let subject = arm
        .parent()?
        .parent()?
        .child_by_field_name("condition")?
        .named_child(0)?;
    let alone = conditions.named_child_count() == 1;
    (outcome && alone && source[subject.byte_range()].eq_ignore_ascii_case(b"true"))
        .then(|| arm.child_by_field_name("return_expression"))
        .flatten()
        .map(|result| result.byte_range())
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

// constraint: an assignment's value is the value assigned, whatever its target does with it: each `ZEND_ASSIGN*` handler
// yields it (`Zend/zend_vm_def.h`), past `__set` and a `set` hook too (`zend_std_write_property`), and a list target
// yields the right side (`zend_compile_list_assign`); a `string` target turns an object to text, on which no call runs
fn assigned_value(value: Node<'_>) -> Node<'_> {
    let mut assigned = inside_parentheses(value);
    while assigned.kind() == "assignment_expression"
        && let Some(right) = assigned.child_by_field_name("right")
    {
        assigned = inside_parentheses(right);
    }
    assigned
}

fn called_value(call: Node<'_>) -> Option<Node<'_>> {
    call.child_by_field_name("object")
        .or_else(|| call.child_by_field_name("scope"))
        .map(assigned_value)
}

// constraint: `'(' expr ')'` yields the expression itself and compiles to no node of its own
// (`Zend/zend_language_parser.y`), where the grammar keeps a node that also holds any comment written in the parentheses
fn inside_parentheses(value: Node<'_>) -> Node<'_> {
    let mut inner = value;
    while inner.kind() == "parenthesized_expression"
        && let Some(expression) = named_child_nodes(inner).find(|child| child.kind() != "comment")
    {
        inner = expression;
    }
    inner
}

fn assignment_target<'t>(value: Node<'t>, source: &[u8]) -> Option<Node<'t>> {
    let mut value = value;
    let mut parent = value.parent()?;
    while parent.kind() == "parenthesized_expression" {
        value = parent;
        parent = value.parent()?;
    }
    stores_its_right_side(parent, source)
        .then_some(parent)
        .filter(|assignment| assignment.child_by_field_name("right") == Some(value))?
        .child_by_field_name("left")
}

// constraint: `??=` compiles to `ZEND_COALESCE` around the assignment of `=` (`zend_compile_assign_coalesce`,
// `Zend/zend_compile.c`), so what it stores is its right side, where another compound assignment stores its result
fn stores_its_right_side(write: Node<'_>, source: &[u8]) -> bool {
    write.kind() == "assignment_expression"
        || write.kind() == "augmented_assignment_expression" && operator_is(write, b"??=", source)
}

// constraint: a value is assigned to a name where the assignment's target is a variable, `$job`, or a property of any
// class, `$this->job` or `self::$job`; a static property's `$$job` or `${job}` is computed (`simple_variable`,
// `Zend/zend_language_parser.y`), so it gives none
fn target_name(target: Node<'_>, source: &[u8]) -> Option<String> {
    match target.kind() {
        "variable_name" => variable_identifier(target, source),
        "member_access_expression" => target
            .child_by_field_name("name")
            .filter(|name| name.kind() == "name" && !in_braces(target))
            .map(|name| text(name, source)),
        "scoped_property_access_expression" => {
            target_name(target.child_by_field_name("name")?, source)
        }
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

// constraint: the grammar writes a promoted parameter's name as a `variable_name` or a `by_ref` around one
fn parameter_variable(parameter: Node<'_>) -> Option<Node<'_>> {
    let name = parameter.child_by_field_name("name")?;
    if name.kind() == "by_ref" {
        name.named_child(0)
    } else {
        Some(name)
    }
}

fn names_a_class(alternative: Node<'_>, source: &[u8]) -> bool {
    alternative.kind() != "primitive_type"
        || source[alternative.byte_range()].eq_ignore_ascii_case(b"iterable")
}

fn creates_an_anonymous_class(value: Node<'_>) -> bool {
    let value = inside_parentheses(value);
    value.kind() == "object_creation_expression"
        && named_child_nodes(value).any(|child| child.kind() == "anonymous_class")
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
