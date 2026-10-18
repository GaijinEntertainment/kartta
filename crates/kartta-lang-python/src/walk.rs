mod uses;

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use self::uses::NameUse;
use kartta_lang::tree::{children, named_children, preorder, signature, text};
use kartta_lang::{
    CallRef, LocalCall, NO_CALLER_ID, RawDirective, RawExtraction, Symbol, SymbolFlags, SymbolId,
    SymbolKind,
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
        frames: Vec::new(),
        receivers: Vec::new(),
        attributes: Vec::new(),
        members: HashSet::new(),
        module_names: HashMap::new(),
        module_uses: Vec::new(),
        flags: SymbolFlags::empty().with_generated(is_generated_source(source)),
        raw: RawExtraction::default(),
    };

    preorder(tree, |node| {
        if node.is_named() {
            emitter.handle(node);
        }
        true
    });
    emitter.close_frames(usize::MAX);
    emitter.record_module_uses();

    emitter.raw
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Function,
    Comprehension,
    Class,
}

#[derive(Clone, Copy)]
enum Binding {
    Module,
    Imported,
    Enclosing,
    Definition(SymbolId),
    Value,
}

impl Binding {
    // constraint: a `global` or `nonlocal` declaration holds for the whole of its scope, so it outranks every other
    // binding of the name there
    const fn strength(self) -> u8 {
        match self {
            Self::Module | Self::Imported => 3,
            Self::Enclosing => 2,
            Self::Definition(_) => 1,
            Self::Value => 0,
        }
    }
}

struct BareCall<'a> {
    caller_id: SymbolId,
    callee: &'a [u8],
    line: u32,
    at: usize,
}

// constraint: Python binds a name for the whole of the scope that binds it, so a call made before the binding, one in a
// nested function included, is decided only when the frame closes
struct Frame<'a> {
    end: usize,
    caller: SymbolId,
    kind: FrameKind,
    outside: Range<usize>,
    bindings: Vec<(&'a [u8], Binding)>,
    calls: Vec<BareCall<'a>>,
    uses: Vec<NameUse<'a>>,
}

impl Frame<'_> {
    fn holds(&self, at: usize) -> bool {
        !self.outside.contains(&at)
    }
}

// constraint: a method reaches its instance through its first parameter, whatever its name, and a function nested in it
// through the same name
struct OwnReceiver<'a> {
    end: usize,
    name: &'a [u8],
    owner: String,
    class: Range<usize>,
    frames: usize,
}

impl OwnReceiver<'_> {
    fn is_parameter(&self, bound: &[u8]) -> bool {
        std::ptr::eq(bound, self.name)
    }
}

struct Attribute {
    class: Range<usize>,
    owner: String,
    name: String,
    lines: [u32; 2],
}

struct Emitter<'a> {
    source: &'a [u8],
    frames: Vec<Frame<'a>>,
    receivers: Vec<OwnReceiver<'a>>,
    attributes: Vec<Attribute>,
    /// Each member a class declares, by where the class starts and the member's name, since two classes local to two
    /// functions can share a qualified name.
    members: HashSet<(usize, String)>,
    /// The names the module binds, each with what binds it.
    module_names: HashMap<&'a [u8], Binding>,
    module_uses: Vec<NameUse<'a>>,
    flags: SymbolFlags,
    raw: RawExtraction,
}

impl<'a> Emitter<'a> {
    fn handle(&mut self, node: Node<'_>) {
        self.close_frames(node.start_byte());
        let caller = self
            .frames
            .last()
            .map_or(NO_CALLER_ID, |frame| frame.caller);

        match node.kind() {
            FUNCTION_DEFINITION => {
                if let Some(id) = self.function(node) {
                    self.define(node, id);
                    self.open(node, id, FrameKind::Function);
                    self.open_receiver(node, id);
                }
            }
            CLASS_DEFINITION => {
                if let Some(id) = self.class(node) {
                    self.define(node, id);
                }
                self.open(node, caller, FrameKind::Class);
            }
            "lambda" => {
                self.open(node, caller, FrameKind::Function);
                self.shadow_receiver(node);
            }
            "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression" => {
                self.open(node, caller, FrameKind::Comprehension);
            }
            "parameters" | "lambda_parameters" | "splat_pattern" => self.bind(node, Binding::Value),
            "for_statement" | "for_in_clause" | "augmented_assignment" => {
                self.bind_field(node, "left");
            }
            "as_pattern" => self.bind_alias(node),
            "case_pattern" | "keyword_pattern" => self.bind_capture(node),
            "named_expression" => self.bind_outside_comprehensions(node),
            "global_statement" => self.bind(node, Binding::Module),
            "nonlocal_statement" => self.bind(node, Binding::Enclosing),
            "call" => self.call(node, caller),
            "attribute" => self.attribute_use(node, caller),
            IDENTIFIER => self.name_use(node, caller),
            "assignment" => {
                self.bind_field(node, "left");
                self.assignment(node);
            }
            "import_statement" => {
                self.bind_imported(node);
                self.import(node);
            }
            "import_from_statement" => {
                self.bind_imported(node);
                self.import_from(node);
            }
            "future_import_statement" => self.future_import(node),
            _ => {}
        }
    }

    // constraint: a comprehension evaluates its first iterable in the scope around it
    fn open(&mut self, node: Node<'_>, caller: SymbolId, kind: FrameKind) {
        let outside = if kind == FrameKind::Comprehension {
            named_children(node)
                .filter_map(|at| node.named_child(at))
                .find(|child| child.kind() == "for_in_clause")
                .and_then(|clause| clause.child_by_field_name("right"))
                .map_or(0..0, |iterable| iterable.byte_range())
        } else {
            0..0
        };
        self.frames.push(Frame {
            end: node.end_byte(),
            caller,
            kind,
            outside,
            bindings: Vec::new(),
            calls: Vec::new(),
            uses: Vec::new(),
        });
    }

    fn close_frames(&mut self, before: usize) {
        while self
            .receivers
            .pop_if(|receiver| receiver.end <= before)
            .is_some()
        {}
        while let Some(frame) = self.frames.pop_if(|frame| frame.end <= before) {
            self.close(frame);
        }
    }

    // constraint: a definition in a class body at module level is no local, so a call of it there stays a call
    // constraint: the graph binds a recorded call by its name, so a call of a value is not recorded, or it would bind
    // a definition the value only shares a name with
    fn close(
        &mut self,
        Frame {
            end,
            caller,
            kind,
            bindings,
            calls,
            uses,
            ..
        }: Frame<'a>,
    ) {
        if kind == FrameKind::Class {
            self.declare_attributes(end);
        }
        self.settle_uses(&bindings, uses);
        for call in calls {
            let binding = bindings
                .iter()
                .filter(|&&(name, _)| name == call.callee)
                .map(|&(_, binding)| binding)
                .max_by_key(|binding| binding.strength());
            match binding {
                Some(Binding::Definition(local)) if caller != NO_CALLER_ID => {
                    self.raw.local_calls.push(LocalCall {
                        caller_id: call.caller_id,
                        local,
                        line: call.line,
                    });
                }
                Some(Binding::Definition(_) | Binding::Module | Binding::Imported) => {
                    self.push_call(&call);
                }
                Some(Binding::Value) => {}
                Some(Binding::Enclosing) | None => match self.scope_frame(call.at) {
                    Some(outer) => outer.calls.push(call),
                    None => self.push_call(&call),
                },
            }
        }
    }

    // constraint: the names a class body binds reach neither its methods nor the function around the class
    fn scope_frame(&mut self, at: usize) -> Option<&mut Frame<'a>> {
        self.frames
            .iter_mut()
            .rev()
            .find(|frame| frame.kind != FrameKind::Class && frame.holds(at))
    }

    fn define(&mut self, node: Node<'_>, id: SymbolId) {
        if let Some(name) = node.child_by_field_name("name") {
            self.bind(name, Binding::Definition(id));
        }
    }

    fn bind_field(&mut self, node: Node<'_>, field: &str) {
        if let Some(target) = node.child_by_field_name(field) {
            self.bind(target, Binding::Value);
        }
    }

    // constraint: an assignment expression in a comprehension binds its name in the scope around the comprehension
    fn bind_outside_comprehensions(&mut self, node: Node<'_>) {
        let frame = self
            .frames
            .iter_mut()
            .rev()
            .find(|frame| frame.kind != FrameKind::Comprehension);
        if let (Some(frame), Some(name)) = (frame, node.child_by_field_name("name")) {
            frame
                .bindings
                .push((&self.source[name.byte_range()], Binding::Value));
        }
    }

    // constraint: the grammar spells the alias of a `with` or `except` target as a field, and that of a `case` pattern
    // as its last child
    fn bind_alias(&mut self, node: Node<'_>) {
        let alias = node.child_by_field_name("alias").or_else(|| {
            named_children(node)
                .last()
                .and_then(|at| node.named_child(at))
                .filter(|last| last.kind() == IDENTIFIER)
        });
        if let Some(alias) = alias {
            self.bind(alias, Binding::Value);
        }
    }

    // constraint: a `case` pattern captures a bare name and compares against a dotted one, and a keyword pattern's own
    // keyword is no capture
    fn bind_capture(&mut self, node: Node<'_>) {
        let captured = named_children(node)
            .filter_map(|at| node.named_child(at))
            .filter(|child| child.kind() == "dotted_name" && child.named_child_count() == 1)
            .filter_map(|name| name.named_child(0));
        for name in captured {
            self.bind(name, Binding::Value);
        }
    }

    fn bind_imported(&mut self, node: Node<'_>) {
        let mut cursor = node.walk();
        for imported in node.children_by_field_name("name", &mut cursor) {
            let bound = match imported.kind() {
                "aliased_import" => imported.child_by_field_name("alias"),
                "dotted_name" => imported.named_child(0),
                _ => None,
            };
            if let Some(bound) = bound {
                self.bind(bound, Binding::Imported);
            }
        }
    }

    // constraint: an attribute or a subscript target binds no name, and a default value or an annotation is no target
    fn bind(&mut self, target: Node<'_>, binding: Binding) {
        match target.kind() {
            IDENTIFIER => {
                let name = &self.source[target.byte_range()];
                if let Some(frame) = self.frames.last_mut() {
                    frame.bindings.push((name, binding));
                } else {
                    self.module_names.insert(name, binding);
                }
            }
            "default_parameter" | "typed_default_parameter" => {
                if let Some(name) = target.child_by_field_name("name") {
                    self.bind(name, binding);
                }
            }
            "parameters"
            | "lambda_parameters"
            | "typed_parameter"
            | "list_splat_pattern"
            | "dictionary_splat_pattern"
            | "splat_pattern"
            | "pattern_list"
            | "tuple_pattern"
            | "list_pattern"
            | "as_pattern_target"
            | "tuple"
            | "list"
            | "global_statement"
            | "nonlocal_statement" => {
                for child in named_children(target).filter_map(|at| target.named_child(at)) {
                    self.bind(child, binding);
                }
            }
            _ => {}
        }
    }

    // constraint: a definition inside a function body, a class and its members included, is local to that body
    fn push_symbol(&mut self, mut symbol: Symbol) -> SymbolId {
        symbol.flags = symbol.flags.union(self.flags);
        let id = SymbolId(u32::try_from(self.raw.symbols.len()).unwrap_or(u32::MAX));
        if self
            .frames
            .iter()
            .any(|frame| frame.kind == FrameKind::Function)
        {
            self.raw.local_symbols.push(id);
        }
        self.raw.symbols.push(symbol);
        id
    }

    // constraint: a free `def main` directly at module level is the one a script runs under
    // `if __name__ == "__main__":`, where a method or a nested `main` is not; the interpreter calls a special method,
    // such as `__init__` or `__enter__`, for an operator, a statement or an object it makes, and a property for an
    // attribute read
    fn function(&mut self, node: Node<'_>) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let owner = enclosing_classes(node, self.source);
        let entry = match owner {
            None => name == "main" && is_module_level(node),
            Some(_) => is_special(&name) || is_property(node, &name, self.source),
        };

        let kind = if owner.is_some() {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        };
        let qualified_name = owner.as_ref().map(|owner| format!("{owner}.{name}"));
        if let Some(class) = class_around(node) {
            self.members.insert((class.start_byte(), name.clone()));
        }
        let mut symbol = symbol(name, qualified_name, kind, decorated_lines(node));
        symbol.owner = owner;
        symbol.signature = signature(node, self.source);
        symbol.flags = symbol.flags.with_entry(entry);
        Some(self.push_symbol(symbol))
    }

    fn class(&mut self, node: Node<'_>) -> Option<SymbolId> {
        let name = text(node.child_by_field_name("name")?, self.source);
        let qualified_name =
            enclosing_classes(node, self.source).map(|chain| format!("{chain}.{name}"));
        if let Some(class) = class_around(node) {
            self.members.insert((class.start_byte(), name.clone()));
        }

        let mut symbol = symbol(
            name,
            qualified_name,
            SymbolKind::Class,
            decorated_lines(node),
        );
        symbol.parents = superclasses(node, self.source);
        Some(self.push_symbol(symbol))
    }

    // constraint: only a bare identifier on the left is a symbol, so `a = b = 1` defines `a` alone and `a, b = 1, 2`
    // defines nothing; at module level it is a variable, and directly in a class body a field
    fn assignment(&mut self, node: Node<'_>) {
        let left = node.child_by_field_name("left");
        if let Some(attribute) = left.filter(|left| left.kind() == "attribute") {
            self.attribute_assignment(attribute, node);
            return;
        }
        let left = left.filter(|left| left.kind() == IDENTIFIER);
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
        if !in_class_body {
            return;
        }
        let Some(owner) = enclosing_classes(node, self.source) else {
            return;
        };
        let name = text(left, self.source);
        let class_start = parent.parent().map_or(0, |class| class.start_byte());
        if !self.members.insert((class_start, name.clone())) {
            return;
        }
        let qualified_name = format!("{owner}.{name}");
        let mut symbol = symbol(name, Some(qualified_name), SymbolKind::Field, lines(node));
        symbol.owner = Some(owner);
        self.push_symbol(symbol);
    }

    fn open_receiver(&mut self, function: Node<'_>, id: SymbolId) {
        if let Some(receiver) = self.method_receiver(function, id) {
            self.receivers.push(receiver);
        } else {
            self.shadow_receiver(function);
        }
    }

    fn method_receiver(&self, function: Node<'_>, id: SymbolId) -> Option<OwnReceiver<'a>> {
        let owner = self.raw.symbols[id.0 as usize].owner.clone()?;
        let class = decorated(function)
            .parent()
            .and_then(|block| block.parent())
            .filter(|class| class.kind() == CLASS_DEFINITION)?;
        if is_static(function, self.source) {
            return None;
        }
        let name = function
            .child_by_field_name("parameters")?
            .named_child(0)
            .and_then(parameter_name)?;
        Some(OwnReceiver {
            end: function.end_byte(),
            name: &self.source[name.byte_range()],
            owner,
            class: class.byte_range(),
            frames: self.frames.len().saturating_sub(1),
        })
    }

    // constraint: a function or a lambda nested in a method reaches the method's receiver, unless a parameter of its
    // own takes the receiver's name
    fn shadow_receiver(&mut self, function: Node<'_>) {
        let Some(outer) = self.receivers.last() else {
            return;
        };
        let Some(parameters) = function.child_by_field_name("parameters") else {
            return;
        };
        let shadows = named_children(parameters)
            .filter_map(|at| parameters.named_child(at))
            .filter_map(parameter_name)
            .any(|name| &self.source[name.byte_range()] == outer.name);
        if shadows {
            self.receivers.push(OwnReceiver {
                end: function.end_byte(),
                name: &[],
                owner: String::new(),
                class: 0..0,
                frames: 0,
            });
        }
    }

    // constraint: the method, or a scope opened inside it, that binds the receiver's name again before the use hides
    // the receiver
    fn own_receiver(&self, object: Node<'_>) -> Option<&OwnReceiver<'a>> {
        let receiver = self.receivers.last()?;
        let name = &self.source[object.byte_range()];
        if object.kind() != IDENTIFIER || name != receiver.name {
            return None;
        }
        let rebound = self.frames[receiver.frames.min(self.frames.len())..]
            .iter()
            .flat_map(|frame| &frame.bindings)
            .any(|&(bound, binding)| {
                bound == name
                    && !receiver.is_parameter(bound)
                    && !matches!(binding, Binding::Enclosing)
            });
        (!rebound).then_some(receiver)
    }

    // constraint: an attribute an instance is given in any of its class's methods is a field of the class, declared
    // where it is first assigned, once the class body has declared its own members
    fn attribute_assignment(&mut self, attribute: Node<'_>, assignment: Node<'_>) {
        let (Some(object), Some(name)) = (
            attribute.child_by_field_name("object"),
            attribute.child_by_field_name("attribute"),
        ) else {
            return;
        };
        let Some(receiver) = self.own_receiver(object) else {
            return;
        };
        let attribute = Attribute {
            class: receiver.class.clone(),
            owner: receiver.owner.clone(),
            name: text(name, self.source),
            lines: lines(assignment),
        };
        self.attributes.push(attribute);
    }

    fn declare_attributes(&mut self, class_end: usize) {
        let declared: Vec<Attribute> = self
            .attributes
            .extract_if(.., |attribute| attribute.class.end == class_end)
            .collect();
        for Attribute {
            class,
            owner,
            name,
            lines,
        } in declared
        {
            if !self.members.insert((class.start, name.clone())) {
                continue;
            }
            let qualified_name = format!("{owner}.{name}");
            let mut symbol = symbol(name, Some(qualified_name), SymbolKind::Field, lines);
            symbol.owner = Some(owner);
            self.push_symbol(symbol);
        }
    }

    fn call(&mut self, node: Node<'_>, caller_id: SymbolId) {
        if self.raw.symbols.is_empty() {
            return;
        }
        let line = line(node.start_position().row);
        let at = node.start_byte();
        let source = self.source;
        if let Some(function) = node
            .child_by_field_name("function")
            .filter(|function| function.kind() == IDENTIFIER)
            && let Some(frame) = self.frames.iter_mut().rev().find(|frame| frame.holds(at))
        {
            frame.calls.push(BareCall {
                caller_id,
                callee: &source[function.byte_range()],
                line,
                at,
            });
            return;
        }
        let Some((callee, receiver)) = callee(node, self.source) else {
            return;
        };

        self.raw.calls.push(CallRef {
            caller_id,
            callee,
            receiver,
            line,
        });
    }

    fn push_call(&mut self, call: &BareCall<'_>) {
        self.raw.calls.push(CallRef {
            caller_id: call.caller_id,
            callee: String::from_utf8_lossy(call.callee).into_owned(),
            receiver: None,
            line: call.line,
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

fn is_special(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

// constraint: an accessor is written `@size.setter` over a `def size`, naming the property it extends
fn is_property(function: Node<'_>, name: &str, source: &[u8]) -> bool {
    let holder = decorated(function);
    named_children(holder)
        .filter_map(|i| holder.named_child(i))
        .filter(|decorator| decorator.kind() == "decorator")
        .filter_map(|decorator| decorator.named_child(0))
        .any(|written| {
            let written = text(written, source);
            let (object, last) = written.rsplit_once('.').unwrap_or(("", &written));
            matches!(last, "property" | "cached_property")
                || (matches!(last, "getter" | "setter" | "deleter") && object == name)
        })
}

fn is_static(function: Node<'_>, source: &[u8]) -> bool {
    let holder = decorated(function);
    named_children(holder)
        .filter_map(|i| holder.named_child(i))
        .filter(|decorator| decorator.kind() == "decorator")
        .filter_map(|decorator| decorator.named_child(0))
        .any(|written| {
            let written = text(written, source);
            written.rsplit('.').next() == Some("staticmethod")
        })
}

fn parameter_name(parameter: Node<'_>) -> Option<Node<'_>> {
    let name = match parameter.kind() {
        IDENTIFIER => Some(parameter),
        "default_parameter" | "typed_default_parameter" => parameter.child_by_field_name("name"),
        "typed_parameter" | "list_splat_pattern" | "dictionary_splat_pattern" => {
            parameter.named_child(0)
        }
        _ => None,
    };
    name.filter(|name| name.kind() == IDENTIFIER)
}

fn is_exported(name: &str) -> bool {
    !name.starts_with('_')
}

fn is_module_level(node: Node<'_>) -> bool {
    decorated(node)
        .parent()
        .is_some_and(|parent| parent.kind() == MODULE)
}

fn class_around(node: Node<'_>) -> Option<Node<'_>> {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            CLASS_DEFINITION => return Some(parent),
            FUNCTION_DEFINITION => return None,
            _ => ancestor = parent.parent(),
        }
    }
    None
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
